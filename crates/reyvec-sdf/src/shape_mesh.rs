//! Сетка из формы (8.20а, план «режим "треугольники" для 3D»): поверхность формы SDF
//! превращается в треугольники один раз, на процессоре, чтобы слабая видеокарта рисовала
//! её растеризацией, а не лучами.
//!
//! Способ — «двойной контур» (dual contouring): форма считается в узлах ровной объёмной
//! сетки; в каждой клетке, где знак расстояния меняется (поверхность проходит через
//! клетку), ставится одна вершина; на каждом ребре сетки со сменой знака четыре клетки
//! вокруг него дают четырёхугольник. Вершина клетки — точка, ближе всего лежащая ко всем
//! касательным плоскостям в местах, где поверхность пересекает рёбра клетки (QEF). Так
//! острые рёбра и углы коробок остаются острыми, а не срезаются, как у простого
//! усреднения. Нормаль вершины — наклон поля (градиент), а не грани: свет ложится так
//! же, как у лучей.
//!
//! Одинаковые формы в разных местах (станки завода) дают одну сетку: [`MeshCache`]
//! снимает с формы внешние сдвиги и повороты ([`split_rigid`]) и хранит сетки по
//! программе того, что осталось.
//!
//! Чего сетка не умеет: детали тоньше клетки могут пропасть, плавные слияния становятся
//! мелкими гранями. Размер клетки задаёт [`MeshSettings`].

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Mat3, Vec3};

use crate::shape3d::{CompileError, Program, Shape3, compile, rotation};

/// Как мелко строить сетку.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshSettings {
    /// Клетка не крупнее этого (метры).
    pub max_cell: f32,
    /// На длинную сторону формы не меньше стольких клеток (мелкие формы — мельче клетка).
    pub min_cells: u32,
    /// Узлов поля на форму не больше этого: очень большие формы получают клетку крупнее
    /// `max_cell`. Считается по объёму коробки, поэтому длинная тонкая стена остаётся с
    /// мелкой клеткой и не теряет толщину.
    pub max_samples: u32,
    /// Упрощение (8.20а2, [`crate::mesh_simplify`]): поверхность сетки не дальше этого от
    /// формы (метры; не больше четверти клетки). 0 — не упрощать.
    pub max_error: f32,
}

impl Default for MeshSettings {
    fn default() -> Self {
        Self {
            max_cell: 0.05,
            min_cells: 24,
            max_samples: 2_000_000,
            max_error: 0.004,
        }
    }
}

impl MeshSettings {
    /// Клетка для формы с коробкой размером `size`.
    pub fn cell(&self, size: Vec3) -> f32 {
        let longest = size.max_element();
        let wanted = (longest / self.min_cells.max(1) as f32).min(self.max_cell);
        // Узлов примерно (size / cell + 4) по каждой оси.
        let samples = |c: f32| {
            let n = size / c + 4.0;
            n.x * n.y * n.z
        };
        let mut cell = wanted.max(1e-4);
        while samples(cell) > self.max_samples as f32 {
            cell *= 1.25;
        }
        cell
    }
}

/// Сетка формы в её координатах.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapeMesh {
    pub positions: Vec<Vec3>,
    /// Единичная нормаль в каждой вершине (наружу).
    pub normals: Vec<Vec3>,
    /// Материал в каждой вершине: материал ближайшей части формы.
    pub materials: Vec<u32>,
    /// Тройки номеров вершин, обход против часовой стрелки снаружи.
    pub indices: Vec<u32>,
    /// Размер клетки, по которой строилась сетка (метры).
    pub cell: f32,
}

impl ShapeMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Наибольшее расстояние от сетки до поверхности формы: по вершинам и серединам
    /// треугольников (`program` — та же форма, в тех же координатах).
    pub fn max_error(&self, program: &Program) -> f32 {
        let d = |p: Vec3| program.distance(p).0.abs();
        let at_vertices = self.positions.iter().map(|&p| d(p)).fold(0.0, f32::max);
        self.indices
            .chunks_exact(3)
            .map(|t| {
                let [a, b, c] = [0, 1, 2].map(|k| self.positions[t[k] as usize]);
                d((a + b + c) / 3.0)
            })
            .fold(at_vertices, f32::max)
    }
}

/// Сетка формы `program` (в её координатах) или `None`, если форма бесконечна (пол,
/// поле уровня) или пуста.
pub fn mesh_program(program: &Program, settings: &MeshSettings) -> Option<ShapeMesh> {
    let (lo, hi) = program.bounds_box?;
    let size = hi - lo;
    if !size.is_finite() || size.max_element() <= 0.0 {
        return None;
    }
    let cell = settings.cell(size);
    // Поле лишь на клетку с лишним шире коробки: поверхность может лежать на самой её
    // границе, а знак нужен с обеих сторон.
    let origin = lo - Vec3::splat(1.5 * cell);
    let n = ((size + Vec3::splat(3.0 * cell)) / cell)
        .ceil()
        .as_uvec3()
        .max(glam::UVec3::splat(2))
        + 1;
    let grid = Grid {
        origin,
        cell,
        n: [n.x as usize, n.y as usize, n.z as usize],
    };
    let values = sample(program, &grid);
    let mut mesh = contour(program, &grid, &values);
    if settings.max_error > 0.0 {
        let tolerance = settings.max_error.min(0.25 * cell);
        crate::mesh_simplify::simplify(&mut mesh, program, tolerance);
        crate::mesh_simplify::split_normals(&mut mesh, program);
    }
    (!mesh.indices.is_empty()).then_some(mesh)
}

/// Ровная объёмная сетка узлов: `n` узлов по каждой оси от `origin` с шагом `cell`.
struct Grid {
    origin: Vec3,
    cell: f32,
    n: [usize; 3],
}

impl Grid {
    fn index(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.n[0] * (y + self.n[1] * z)
    }

    fn point(&self, x: usize, y: usize, z: usize) -> Vec3 {
        self.origin + Vec3::new(x as f32, y as f32, z as f32) * self.cell
    }
}

/// Расстояния во всех узлах: слои по Z делятся между потоками процессора.
fn sample(program: &Program, grid: &Grid) -> Vec<f32> {
    let [nx, ny, nz] = grid.n;
    let layer = nx * ny;
    let mut values = vec![0.0f32; layer * nz];
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 16);
    let per = nz.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (chunk, out) in values.chunks_mut(layer * per).enumerate() {
            scope.spawn(move || {
                for (k, v) in out.iter_mut().enumerate() {
                    let i = chunk * layer * per + k;
                    let (x, y, z) = (i % nx, (i / nx) % ny, i / layer);
                    *v = program.distance(grid.point(x, y, z)).0;
                }
            });
        }
    });
    values
}

/// Наклон поля в точке (не нормированный): четыре выборки по вершинам тетраэдра.
fn gradient(program: &Program, p: Vec3, h: f32) -> Vec3 {
    let k = [
        Vec3::new(1.0, -1.0, -1.0),
        Vec3::new(-1.0, -1.0, 1.0),
        Vec3::new(-1.0, 1.0, -1.0),
        Vec3::new(1.0, 1.0, 1.0),
    ];
    k.iter()
        .map(|&k| k * program.distance(p + k * h).0)
        .fold(Vec3::ZERO, |a, b| a + b)
}

fn normal_at(program: &Program, p: Vec3, h: f32) -> Vec3 {
    gradient(program, p, h).try_normalize().unwrap_or(Vec3::Y)
}

/// Насколько вершина держится за середину точек пересечения, когда плоскости не задают
/// её однозначно (плоская стенка, ребро): без этого точка уплывает вдоль плоскости.
const QEF_PULL: f32 = 0.05;

/// Углы клетки и её 12 рёбер (пары углов).
const CORNERS: [[usize; 3]; 8] = [
    [0, 0, 0],
    [1, 0, 0],
    [0, 1, 0],
    [1, 1, 0],
    [0, 0, 1],
    [1, 0, 1],
    [0, 1, 1],
    [1, 1, 1],
];
const EDGES: [[usize; 2]; 12] = [
    [0, 1],
    [2, 3],
    [4, 5],
    [6, 7],
    [0, 2],
    [1, 3],
    [4, 6],
    [5, 7],
    [0, 4],
    [1, 5],
    [2, 6],
    [3, 7],
];

fn contour(program: &Program, grid: &Grid, values: &[f32]) -> ShapeMesh {
    let [nx, ny, nz] = grid.n;
    let (cx, cy, cz) = (nx - 1, ny - 1, nz - 1);
    let cell_index = |x: usize, y: usize, z: usize| x + cx * (y + cy * z);
    let mut vertex_of = vec![u32::MAX; cx * cy * cz];
    let mut mesh = ShapeMesh {
        cell: grid.cell,
        ..ShapeMesh::default()
    };
    let h = grid.cell * 0.05;
    let inside = |v: f32| v < 0.0;

    // Вершины: по одной в клетке, через которую проходит поверхность.
    for z in 0..cz {
        for y in 0..cy {
            for x in 0..cx {
                let d = CORNERS.map(|[i, j, k]| values[grid.index(x + i, y + j, z + k)]);
                let mask = d.iter().filter(|&&v| inside(v)).count();
                if mask == 0 || mask == 8 {
                    continue;
                }
                let base = grid.point(x, y, z);
                let corner = |c: usize| base + Vec3::from(CORNERS[c].map(|v| v as f32)) * grid.cell;
                let mut ata = Mat3::ZERO;
                let mut atb = Vec3::ZERO;
                let mut mass = Vec3::ZERO;
                let mut count = 0.0;
                for [a, b] in EDGES {
                    if inside(d[a]) == inside(d[b]) {
                        continue;
                    }
                    let t = (d[a] / (d[a] - d[b])).clamp(0.0, 1.0);
                    let p = corner(a).lerp(corner(b), t);
                    let n = normal_at(program, p, h);
                    ata += Mat3::from_cols(n * n.x, n * n.y, n * n.z);
                    atb += n * n.dot(p);
                    mass += p;
                    count += 1.0;
                }
                let center = mass / count;
                let system = ata + Mat3::from_diagonal(Vec3::splat(QEF_PULL));
                let mut p = if system.determinant().abs() > 1e-12 {
                    system.inverse() * (atb + center * QEF_PULL)
                } else {
                    center
                };
                // Точка не выходит из своей клетки: иначе соседние грани заворачиваются.
                p = p.clamp(base, base + Vec3::splat(grid.cell));
                if !p.is_finite() {
                    p = center;
                }
                vertex_of[cell_index(x, y, z)] = mesh.positions.len() as u32;
                let (_, material) = program.distance(p);
                mesh.positions.push(p);
                mesh.normals.push(normal_at(program, p, h));
                mesh.materials.push(material);
            }
        }
    }

    // Четырёхугольники: на каждом ребре сетки со сменой знака — из вершин четырёх
    // клеток вокруг него.
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let here = values[grid.index(x, y, z)];
                for axis in 0..3 {
                    let step = [x, y, z];
                    let mut next = step;
                    next[axis] += 1;
                    if next[axis] >= grid.n[axis] {
                        continue;
                    }
                    let there = values[grid.index(next[0], next[1], next[2])];
                    if inside(here) == inside(there) {
                        continue;
                    }
                    let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
                    if step[b] == 0 || step[c] == 0 {
                        continue;
                    }
                    let around = [(0, 0), (1, 0), (1, 1), (0, 1)].map(|(i, j)| {
                        let mut cell = step;
                        cell[b] -= i;
                        cell[c] -= j;
                        if cell[0] >= cx || cell[1] >= cy || cell[2] >= cz {
                            return u32::MAX;
                        }
                        vertex_of[cell_index(cell[0], cell[1], cell[2])]
                    });
                    if around.contains(&u32::MAX) {
                        continue;
                    }
                    // Наружу смотрит сторона, где расстояние больше.
                    let mut outward = Vec3::ZERO;
                    outward[axis] = if inside(here) { 1.0 } else { -1.0 };
                    push_quad(&mut mesh, around, outward);
                }
            }
        }
    }
    mesh
}

/// Два треугольника из четырёх вершин по кругу, лицом к `outward`. Диагональ — та, при
/// которой оба треугольника лучше смотрят вдоль нормалей своих вершин: у острого ребра,
/// идущего наискосок к сетке, другая диагональ складывает четырёхугольник внутрь.
fn push_quad(mesh: &mut ShapeMesh, q: [u32; 4], outward: Vec3) {
    let p = q.map(|i| mesh.positions[i as usize]);
    let n = q.map(|i| mesh.normals[i as usize]);
    // Обход (0, 1, 2, 3) смотрит в сторону `outward` или против неё: сразу разворачиваем.
    let flip = (p[2] - p[0]).cross(p[3] - p[1]).dot(outward) < 0.0;
    let orient = |t: [usize; 3]| if flip { [t[0], t[2], t[1]] } else { t };
    let score = |t: [usize; 3]| {
        let face = (p[t[1]] - p[t[0]]).cross(p[t[2]] - p[t[0]]);
        let normal = n[t[0]] + n[t[1]] + n[t[2]];
        face.normalize_or_zero().dot(normal.normalize_or_zero())
    };
    let splits = [
        [orient([0, 1, 2]), orient([0, 2, 3])],
        [orient([0, 1, 3]), orient([1, 2, 3])],
    ];
    let quality = |s: &[[usize; 3]; 2]| score(s[0]).min(score(s[1]));
    let best = if quality(&splits[0]) >= quality(&splits[1]) {
        splits[0]
    } else {
        splits[1]
    };
    for t in best {
        let face = (p[t[1]] - p[t[0]]).cross(p[t[2]] - p[t[0]]);
        if face.length_squared() > 0.0 {
            mesh.indices.extend(t.map(|k| q[k]));
        }
    }
}

/// Сдвиг, поворот и увеличение формы как целого: точка формы `p` в мире — `at + rot · p · scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rigid {
    pub at: Vec3,
    pub rot: Mat3,
    pub scale: f32,
}

impl Rigid {
    pub const IDENTITY: Self = Self {
        at: Vec3::ZERO,
        rot: Mat3::IDENTITY,
        scale: 1.0,
    };

    pub fn apply(&self, p: Vec3) -> Vec3 {
        self.at + self.rot * (p * self.scale)
    }

    /// Сначала `inner`, потом `self`.
    fn then_inner(self, inner: Rigid) -> Self {
        Self {
            at: self.apply(inner.at),
            rot: self.rot * inner.rot,
            scale: self.scale * inner.scale,
        }
    }
}

/// Форма без внешних сдвигов: все `Move` сверху дерева (и под `Paint` тоже) сняты и
/// собраны в одно [`Rigid`]. Материал `Paint` остаётся у формы — он часть сетки.
pub fn split_rigid(shape: &Shape3) -> (Rigid, Shape3) {
    let mut rigid = Rigid::IDENTITY;
    let mut paint = None;
    let mut s = shape;
    loop {
        match s {
            Shape3::Move {
                at,
                turn,
                scale,
                shape,
            } => {
                rigid = rigid.then_inner(Rigid {
                    at: Vec3::from(*at),
                    rot: rotation(*turn),
                    scale: *scale,
                });
                s = shape;
            }
            // Внешний материал задаётся только самым внешним Paint: внутренний его
            // перекрывает, поэтому при спуске запоминается первый.
            Shape3::Paint { material, shape } if matches!(**shape, Shape3::Move { .. }) => {
                paint.get_or_insert(*material);
                s = shape;
            }
            _ => break,
        }
    }
    let local = match paint {
        Some(material) => s.clone().paint(material),
        None => s.clone(),
    };
    (rigid, local)
}

/// Сетки одинаковых форм: одна на все копии. Ключ — слова программы формы без внешних
/// сдвигов (одинаковые слова — одинаковая форма).
#[derive(Default)]
pub struct MeshCache {
    settings: MeshSettings,
    meshes: HashMap<Vec<u32>, Option<Arc<ShapeMesh>>>,
    /// Сколько раз сетка нашлась готовой.
    pub hits: usize,
}

/// Одна форма сцены в виде сетки: какая сетка и где она стоит.
#[derive(Clone, Debug)]
pub struct MeshInstance {
    pub mesh: Arc<ShapeMesh>,
    pub place: Rigid,
}

impl MeshCache {
    pub fn new(settings: MeshSettings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    /// Сколько разных сеток построено (без пустых и бесконечных форм).
    pub fn len(&self) -> usize {
        self.meshes.values().filter(|m| m.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Все треугольники разных сеток.
    pub fn triangle_count(&self) -> usize {
        self.meshes
            .values()
            .flatten()
            .map(|m| m.triangle_count())
            .sum()
    }

    /// Сетка формы и где она стоит; `None` — форма бесконечна или пуста.
    pub fn get(&mut self, shape: &Shape3) -> Result<Option<MeshInstance>, CompileError> {
        let (place, local) = split_rigid(shape);
        let program = compile(&local)?;
        if let Some(found) = self.meshes.get(&program.words) {
            self.hits += 1;
            return Ok(found.clone().map(|mesh| MeshInstance { mesh, place }));
        }
        let mesh = mesh_program(&program, &self.settings).map(Arc::new);
        self.meshes.insert(program.words, mesh.clone());
        Ok(mesh.map(|mesh| MeshInstance { mesh, place }))
    }
}

/// Сетки всех форм сцены: по одной записи на форму (`None` — без сетки).
#[derive(Debug, Default)]
pub struct SceneMeshes {
    pub instances: Vec<Option<MeshInstance>>,
    /// Разных сеток.
    pub unique: usize,
    /// Треугольников во всех разных сетках.
    pub triangles: usize,
}

/// Строит сетки для форм `shapes` (обычно [`crate::SpaceFrame::shapes`]). Формы, которые
/// не собираются в программу, остаются без сетки.
pub fn mesh_shapes(shapes: &[Shape3], settings: &MeshSettings) -> SceneMeshes {
    let mut cache = MeshCache::new(*settings);
    let instances = shapes.iter().map(|s| cache.get(s).ok().flatten()).collect();
    SceneMeshes {
        instances,
        unique: cache.len(),
        triangles: cache.triangle_count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn mesh(shape: &Shape3) -> (Program, ShapeMesh) {
        let program = compile(shape).unwrap();
        let mesh = mesh_program(&program, &MeshSettings::default()).expect("сетка");
        (program, mesh)
    }

    /// Каждое ребро сетки (по месту вершин) — у двух треугольников с встречным обходом: сетка замкнута и
    /// смотрит наружу одинаково.
    fn assert_closed(mesh: &ShapeMesh) {
        // Вершины на острых рёбрах поделены по нормалям: рёбра сравниваются по месту.
        let key = |i: u32| mesh.positions[i as usize].to_array().map(f32::to_bits);
        let mut edges: HashMap<([u32; 3], [u32; 3]), i32> = HashMap::new();
        for t in mesh.indices.chunks_exact(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let (a, b) = (key(a), key(b));
                *edges.entry((a.min(b), a.max(b))).or_default() += if a < b { 1 } else { -1 };
            }
        }
        let open = edges.values().filter(|&&v| v != 0).count();
        assert_eq!(open, 0, "незамкнутых или вывернутых рёбер {open}");
    }

    /// Треугольники смотрят туда же, куда нормали их вершин (почти все).
    fn assert_facing(mesh: &ShapeMesh) {
        let wrong = mesh
            .indices
            .chunks_exact(3)
            .filter(|t| {
                let [a, b, c] = [0, 1, 2].map(|k| mesh.positions[t[k] as usize]);
                let n: Vec3 = [0, 1, 2].map(|k| mesh.normals[t[k] as usize]).iter().sum();
                (b - a).cross(c - a).dot(n) < 0.0
            })
            .count();
        // На остром ребре наискосок к сетке нормаль вершины — нормаль одной из граней, и
        // крошечный треугольник вдоль ребра может смотреть «не туда» по ней, не будучи
        // вывернутым (замкнутость с обходом проверяет `assert_closed`). Таких — единицы.
        let total = mesh.indices.len() / 3;
        assert!(
            wrong * 500 <= total,
            "треугольников лицом внутрь {wrong} из {total}"
        );
    }

    fn check(shape: Shape3) -> ShapeMesh {
        let (program, mesh) = mesh(&shape);
        let error = mesh.max_error(&program);
        assert!(
            error <= 0.5 * mesh.cell,
            "ошибка {error} больше половины клетки {} у {shape:?}",
            mesh.cell
        );
        assert_facing(&mesh);
        mesh
    }

    #[test]
    fn simple_shapes_are_close_and_closed() {
        for shape in [
            Shape3::Sphere { radius: 0.3 },
            Shape3::Box {
                size: [0.5, 1.2, 0.4],
                round: 0.0,
            },
            Shape3::Box {
                size: [0.4, 0.4, 0.4],
                round: 0.02,
            },
            Shape3::Cylinder {
                height: 0.9,
                radius: 0.28,
                round: 0.03,
            },
            Shape3::Capsule {
                height: 3.6,
                radius: 0.12,
            },
            Shape3::Torus {
                radius: 0.5,
                thickness: 0.1,
            },
        ] {
            assert_closed(&check(shape));
        }
    }

    #[test]
    fn operations_and_turns_are_close() {
        let turned = Shape3::Box {
            size: [0.6, 0.3, 0.2],
            round: 0.0,
        }
        .placed([0.1, 0.2, -0.1], [20.0, 35.0, -15.0]);
        let cut = Shape3::Subtract {
            base: Box::new(Shape3::Box {
                size: [0.6, 0.6, 0.6],
                round: 0.0,
            }),
            cut: vec![Shape3::Cylinder {
                height: 1.0,
                radius: 0.2,
                round: 0.0,
            }],
            smooth: 0.0,
        };
        let blend = Shape3::Union {
            shapes: vec![
                Shape3::Sphere { radius: 0.25 },
                Shape3::Sphere { radius: 0.2 }.at([0.3, 0.0, 0.0]),
            ],
            smooth: 0.1,
        };
        for shape in [turned, cut, blend] {
            assert_closed(&check(shape));
        }
    }

    /// У коробки острые углы и рёбра: вершины встают точно в углы, а не срезают их.
    #[test]
    fn box_corners_stay_sharp() {
        let half = Vec3::new(0.25, 0.6, 0.2);
        let (_, mesh) = mesh(&Shape3::Box {
            size: (half * 2.0).to_array(),
            round: 0.0,
        });
        for sx in [-1.0, 1.0] {
            for sy in [-1.0, 1.0] {
                for sz in [-1.0, 1.0] {
                    let corner = half * Vec3::new(sx, sy, sz);
                    let nearest = mesh
                        .positions
                        .iter()
                        .map(|p| p.distance(corner))
                        .fold(f32::MAX, f32::min);
                    assert!(
                        nearest < 0.1 * mesh.cell,
                        "угол {corner} срезан: ближайшая вершина в {nearest}"
                    );
                }
            }
        }
    }

    /// Упрощение: у коробки без скруглений остаётся по паре треугольников на стенку, у
    /// цилиндра со скруглениями — немного; сетки завода в кадре — меньше миллиона.
    #[test]
    fn simplified_meshes_are_small() {
        let (_, plain) = mesh(&Shape3::Box {
            size: [0.5, 1.2, 0.4],
            round: 0.0,
        });
        assert!(
            plain.triangle_count() <= 24,
            "коробка: {}",
            plain.triangle_count()
        );
        let (_, barrel) = mesh(&Shape3::Cylinder {
            height: 0.9,
            radius: 0.28,
            round: 0.03,
        });
        assert!(
            barrel.triangle_count() <= 1500,
            "бочка: {}",
            barrel.triangle_count()
        );
    }

    /// Материал вершин — материал ближайшей части.
    #[test]
    fn vertices_carry_materials() {
        let shape = Shape3::Union {
            shapes: vec![
                Shape3::Box {
                    size: [1.0, 0.2, 0.4],
                    round: 0.0,
                }
                .paint(3),
                Shape3::Sphere { radius: 0.2 }.at([0.0, 0.3, 0.0]).paint(5),
            ],
            smooth: 0.0,
        };
        let (_, mesh) = mesh(&shape);
        assert!(mesh.materials.contains(&3) && mesh.materials.contains(&5));
        assert!(mesh.materials.iter().all(|&m| m == 3 || m == 5));
    }

    /// Пол и поле уровня бесконечны: сетки нет.
    #[test]
    fn infinite_shapes_have_no_mesh() {
        let floor = compile(&Shape3::Plane {
            normal: [0.0, 1.0, 0.0],
            offset: 0.0,
        })
        .unwrap();
        assert!(mesh_program(&floor, &MeshSettings::default()).is_none());
    }

    /// Одинаковые формы в разных местах — одна сетка, и поставленная на место она лежит на
    /// поверхности формы в мире.
    #[test]
    fn copies_share_one_mesh() {
        let part = || {
            Shape3::Cylinder {
                height: 0.9,
                radius: 0.28,
                round: 0.03,
            }
            .paint(2)
        };
        let shapes = vec![
            part().placed([1.0, 0.45, -2.0], [0.0, 30.0, 0.0]).paint(2),
            part().placed([-3.0, 0.45, 4.0], [10.0, 0.0, 0.0]).paint(2),
            part().at([0.0, 0.0, 0.0]).at([5.0, 1.0, 0.0]),
        ];
        let scene = mesh_shapes(&shapes, &MeshSettings::default());
        assert_eq!(scene.unique, 1);
        for (shape, instance) in shapes.iter().zip(&scene.instances) {
            let instance = instance.as_ref().expect("сетка");
            let world = compile(shape).unwrap();
            let error = instance
                .mesh
                .positions
                .iter()
                .map(|&p| world.distance(instance.place.apply(p)).0.abs())
                .fold(0.0, f32::max);
            assert!(error <= 0.5 * instance.mesh.cell, "ошибка на месте {error}");
        }
    }

    /// Все разные формы сцены завода превращаются в сетки рядом с формой. Сколько их и
    /// за сколько — в выводе теста (`--nocapture`).
    #[test]
    fn factory_shapes_mesh() {
        let text = include_str!("../../../scenes/3d/factory.ron");
        let scene = crate::SpaceScene::parse_in(
            text,
            Some(std::path::Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scenes/3d"
            ))),
        )
        .unwrap();
        let frame = scene.frame(0.0);
        let started = std::time::Instant::now();
        let meshes = mesh_shapes(&frame.shapes, &MeshSettings::default());
        let took = started.elapsed();
        let with = meshes.instances.iter().flatten().count();
        let drawn: usize = meshes
            .instances
            .iter()
            .flatten()
            .map(|i| i.mesh.triangle_count())
            .sum();
        eprintln!(
            "завод: форм {}, с сеткой {with}, разных сеток {}, треугольников {}, в кадре {drawn}, {:.0} мс",
            frame.shapes.len(),
            meshes.unique,
            meshes.triangles,
            took.as_secs_f64() * 1000.0
        );
        assert!(drawn < 1_000_000, "треугольников в кадре {drawn}");
        // Всё, кроме пола, получает сетку; станков, ящиков и конвейеров много одинаковых.
        assert_eq!(with + 1, frame.shapes.len());
        assert!(meshes.unique * 10 < with, "разных сеток {}", meshes.unique);
        for (shape, instance) in frame.shapes.iter().zip(&meshes.instances) {
            let Some(instance) = instance else { continue };
            let world = compile(shape).unwrap();
            let error = instance
                .mesh
                .positions
                .iter()
                .map(|&p| world.distance(instance.place.apply(p)).0.abs())
                .fold(0.0, f32::max);
            assert!(
                error <= 0.5 * instance.mesh.cell,
                "ошибка {error} у {shape:?}"
            );
        }
    }
}
