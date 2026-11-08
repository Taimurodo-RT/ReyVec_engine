//! Сетка отсева для сцены «3D» (этап 7.6е): чтобы шаг луча не перебирал все объекты.
//!
//! Пространство вокруг конечных объектов делится на кубические клетки. У каждой клетки
//! свой список объектов, чей шар-граница ближе `margin` (размер клетки) к клетке: от
//! любой точки клетки объекты не из списка дальше `margin`. Шаг луча в точке берёт
//! клетку, считает только её список и не шагает дальше `margin` — так время шага
//! зависит от того, сколько объектов рядом, а не от их общего числа. Большие пустоты
//! перед предметами пролетает проход конусами (7.8), поэтому короткий шаг в сетке почти
//! не добавляет шагов (замер на процессоре: +2%).
//!
//! Бесконечные формы (плоскость пола) в сетку не попадают: они считаются всегда.
//! Снаружи сетки ответ — расстояние до коробки, в которой лежат все шары-границы.
//!
//! Сетка строится на процессоре каждый кадр (объекты двигаются): для сотен объектов это
//! десятки микросекунд. Шейдер (`include/space.slang`, `scene_near`) и зеркало здесь
//! ([`Grid::near`]) читают одно и то же; тесты сверяют зеркало с полным перебором.

use crate::shape3d::{ESTIMATE_MATERIAL, Program};
use glam::Vec3;

/// К этому числу клеток подбирается размер клетки.
const TARGET_CELLS: f32 = 2048.0;
/// Больше клеток не бывает (буфер кадра не резиновый).
pub const MAX_CELLS: usize = 32768;
/// Клетка не мельче 25 см: мельче — длинные списки соседей и лишняя память.
const MIN_CELL: f32 = 0.25;

/// Слов в заголовке сетки и в записи клетки (раскладку читает шейдер).
pub const GRID_HEADER_WORDS: usize = 20;
pub const CELL_WORDS: usize = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cell {
    /// Начало списка в [`Grid::items`] и его длина.
    pub start: u32,
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grid {
    /// Угол сетки (наименьшие координаты), размер клетки, число клеток по осям.
    pub origin: Vec3,
    pub cell: f32,
    pub dims: [u32; 3],
    /// Объект попадает в список клетки, если его шар ближе этого к клетке.
    pub margin: f32,
    /// Коробка, в которой лежат все шары-границы.
    pub inner_lo: Vec3,
    pub inner_hi: Vec3,
    pub cells: Vec<Cell>,
    pub items: Vec<u32>,
    /// Объекты без границы: считаются в каждой точке. Сначала обычные, в конце —
    /// плоскости, которые луч глаза находит формулой ([`crate::shape3d::OBJECT_PLANE`]).
    pub unbounded: Vec<u32>,
    /// Сколько первых из `unbounded` — не плоскости.
    pub unbounded_solid: usize,
}

impl Grid {
    /// Сетка для объектов `objects` (номера в списках — их индексы).
    pub fn build(objects: &[Program]) -> Self {
        let planes = crate::shape3d::plane_objects(objects);
        let (mut unbounded, flat): (Vec<u32>, Vec<u32>) = (0..objects.len() as u32)
            .filter(|&i| objects[i as usize].bounds.is_none())
            .partition(|i| !planes.contains(i));
        let unbounded_solid = unbounded.len();
        unbounded.extend(flat);
        let mut grid = Grid {
            unbounded,
            unbounded_solid,
            ..Grid::default()
        };
        let finite: Vec<(u32, Vec3, f32)> = objects
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.bounds.map(|(c, r)| (i as u32, c, r.max(0.0))))
            .collect();
        if finite.is_empty() {
            return grid;
        }
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for &(_, c, r) in &finite {
            lo = lo.min(c - r);
            hi = hi.max(c + r);
        }
        let extent = (hi - lo).max(Vec3::splat(MIN_CELL));
        // Клетка — по объёму сцены, но не крупнее радиуса обычного предмета: в большой и
        // густой сцене крупная клетка собирает длинные списки соседей, и каждый шаг
        // луча перебирает их все (на 200 предметах это и было узким местом).
        let mut sizes: Vec<f32> = finite.iter().map(|f| f.2).collect();
        let mid = sizes.len() / 2;
        let typical = *sizes.select_nth_unstable_by(mid, f32::total_cmp).1;
        let mut cell = ((extent.x * extent.y * extent.z) / TARGET_CELLS)
            .cbrt()
            .min(typical)
            .max(MIN_CELL);
        let dims = loop {
            // Сетка шире коробки на margin = cell с каждой стороны: у её края до любого
            // объекта не меньше клетки, и луч снаружи подходит к ней большими шагами.
            let d = ((extent + 2.0 * cell) / cell).ceil();
            let dims = [d.x as u32, d.y as u32, d.z as u32];
            if dims.iter().map(|&d| d as usize).product::<usize>() <= MAX_CELLS {
                break dims;
            }
            cell *= 1.25;
        };
        let margin = cell;
        grid.origin = lo - margin;
        grid.cell = cell;
        grid.dims = dims;
        grid.margin = margin;
        grid.inner_lo = lo;
        grid.inner_hi = hi;

        // Клетки, которых касается шар объекта, расширенный на margin. Два прохода:
        // сначала счёт (сколько объектов в каждой клетке), потом раскладка по местам —
        // без отдельного списка на клетку.
        let [nx, ny, nz] = dims.map(|d| d as usize);
        let top = Vec3::new(nx as f32 - 1.0, ny as f32 - 1.0, nz as f32 - 1.0);
        let touched = |c: Vec3, r: f32, f: &mut dyn FnMut(usize)| {
            let reach = r + margin;
            let a = ((c - reach - grid.origin) / cell)
                .floor()
                .clamp(Vec3::ZERO, top);
            let b = ((c + reach - grid.origin) / cell)
                .floor()
                .clamp(Vec3::ZERO, top);
            for z in a.z as usize..=b.z as usize {
                for y in a.y as usize..=b.y as usize {
                    for x in a.x as usize..=b.x as usize {
                        let lo = grid.origin + Vec3::new(x as f32, y as f32, z as f32) * cell;
                        if c.distance_squared(c.clamp(lo, lo + cell)) < reach * reach {
                            f((z * ny + y) * nx + x);
                        }
                    }
                }
            }
        };
        let mut counts = vec![0u32; nx * ny * nz];
        for &(_, c, r) in &finite {
            touched(c, r, &mut |k| counts[k] += 1);
        }
        let mut start = 0;
        grid.cells = counts
            .iter()
            .map(|&count| {
                let cell = Cell { start, count };
                start += count;
                cell
            })
            .collect();
        grid.items = vec![0; start as usize];
        let mut fill: Vec<u32> = grid.cells.iter().map(|c| c.start).collect();
        for &(i, c, r) in &finite {
            touched(c, r, &mut |k| {
                grid.items[fill[k] as usize] = i;
                fill[k] += 1;
            });
        }
        grid
    }

    /// Слов в раскладке для видеокарты ([`Grid::write`]).
    pub fn words(&self) -> usize {
        GRID_HEADER_WORDS + self.cells.len() * CELL_WORDS + self.items.len() + self.unbounded.len()
    }

    /// Раскладка для видеокарты, начиная со слова `at` буфера сцены:
    ///
    /// - заголовок, 20 слов: угол сетки (3 числа), размер клетки, клеток по осям
    ///   (3 целых), margin, коробка шаров (6 чисел), где клетки, где списки, где и
    ///   сколько бесконечных объектов, сколько из них не плоскости, 1 запасное;
    /// - клетки по 2 слова: начало списка (номер слова в буфере) и длина; клетка
    ///   (x, y, z) — номер (z · ny + y) · nx + x;
    /// - списки клеток подряд, потом список бесконечных объектов.
    pub fn write(&self, at: usize, out: &mut Vec<u32>) {
        let cells_at = at + GRID_HEADER_WORDS;
        let items_at = cells_at + self.cells.len() * CELL_WORDS;
        let unbounded_at = items_at + self.items.len();
        out.extend(self.origin.to_array().map(f32::to_bits));
        out.push(self.cell.to_bits());
        out.extend(self.dims);
        out.push(self.margin.to_bits());
        out.extend(self.inner_lo.to_array().map(f32::to_bits));
        out.extend(self.inner_hi.to_array().map(f32::to_bits));
        out.extend([
            cells_at as u32,
            items_at as u32,
            unbounded_at as u32,
            self.unbounded.len() as u32,
            self.unbounded_solid as u32,
            0,
        ]);
        for c in &self.cells {
            out.extend([items_at as u32 + c.start, c.count]);
        }
        out.extend(&self.items);
        out.extend(&self.unbounded);
    }

    /// Клетка точки `p` или `None`, если точка вне сетки.
    pub fn cell_at(&self, p: Vec3) -> Option<usize> {
        let q = ((p - self.origin) / self.cell).floor();
        let [nx, ny, nz] = self.dims.map(|d| d as f32);
        if q.x < 0.0 || q.y < 0.0 || q.z < 0.0 || q.x >= nx || q.y >= ny || q.z >= nz {
            return None;
        }
        Some(((q.z * ny + q.y) * nx + q.x) as usize)
    }

    /// Расстояние до ближайшего объекта и его материал — зеркало `scene_near` шейдера.
    /// Ответ-оценка (по шару-границе, по размеру клетки или по коробке) помечен материалом
    /// `None`: настоящее расстояние не меньше.
    pub fn near(&self, objects: &[Program], p: Vec3, near: f32) -> (f32, Option<u32>) {
        // Как в шейдере: сначала оценка по клетке (или по коробке снаружи сетки), потом
        // бесконечные объекты и список клетки одним проходом.
        let mut best = (1e9f32, None);
        let mut list: &[u32] = &[];
        if !self.cells.is_empty() {
            match self.cell_at(p) {
                Some(c) => {
                    let cell = self.cells[c];
                    list = &self.items[cell.start as usize..(cell.start + cell.count) as usize];
                    best = (self.margin, None);
                }
                None => {
                    best = ((p - p.clamp(self.inner_lo, self.inner_hi)).length(), None);
                }
            }
        }
        for &i in self.unbounded.iter().chain(list) {
            let o = &objects[i as usize];
            if let Some((c, r)) = o.bounds {
                let outside = (p - c).length() - r;
                if outside >= best.0 {
                    continue;
                }
                if outside > near {
                    best = (outside, None);
                    continue;
                }
            }
            let (d, m) = o.distance(p);
            if d < best.0 {
                // У поля слоя за полосой материал не известен: только оценка (7.18б).
                best = (d, (m != ESTIMATE_MATERIAL).then_some(m));
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape3d::{Shape3, compile};

    fn programs(text: &str) -> Vec<Program> {
        let scene = crate::SpaceScene::parse(text).unwrap();
        scene
            .frame(1.3)
            .shapes
            .iter()
            .map(|s| compile(s).unwrap())
            .collect()
    }

    fn exact(objects: &[Program], p: Vec3) -> f32 {
        objects
            .iter()
            .map(|o| o.distance(p).0)
            .fold(f32::INFINITY, f32::min)
    }

    fn check(objects: &[Program]) {
        let grid = Grid::build(objects);
        assert!(grid.cells.len() <= MAX_CELLS);
        let mut seed = 12345u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        let lo = grid.origin - 3.0;
        let size = grid.cell * Vec3::from(grid.dims.map(|d| d as f32)) + 6.0;
        for _ in 0..20000 {
            let p = lo + Vec3::new(rnd(), rnd(), rnd()) * size;
            let truth = exact(objects, p);
            let (d, m) = grid.near(objects, p, 1e9);
            // Оценка никогда не больше правды: шагать по ней безопасно.
            assert!(d <= truth + 1e-4, "{p}: сетка {d}, правда {truth}");
            // Вблизи формы ответ точный и с материалом.
            if truth < grid.margin.min(0.2) {
                assert!((d - truth).abs() < 1e-4, "{p}: сетка {d}, правда {truth}");
                assert!(m.is_some());
            }
        }
    }

    #[test]
    fn grid_never_overestimates_example_scene() {
        check(&programs(crate::space_file::EXAMPLE));
    }

    #[test]
    fn grid_never_overestimates_stress_scene() {
        check(&programs(include_str!("../../../scenes/3d/stress.ron")));
    }

    #[test]
    fn grid_lists_only_nearby_objects() {
        let objects = programs(include_str!("../../../scenes/3d/stress.ron"));
        let grid = Grid::build(&objects);
        let longest = grid.cells.iter().map(|c| c.count).max().unwrap();
        assert!(longest <= 16, "в клетке {longest} объектов");
        assert_eq!(grid.unbounded, vec![0], "пол считается всегда");
    }

    /// Поле слоя (7.18б) — бесконечный объект без плоскости: в начале `unbounded`,
    /// перед плоскостями; сетку оно не растягивает; за полосой его материал не известен.
    #[test]
    fn field_layer_is_unbounded_and_its_estimate_has_no_material() {
        use crate::layer_field::LayerField;
        use crate::profile2d::Profile;
        use crate::shape3d::FieldRef;
        let profile = Profile::new("M -1 -1 L 1 -1 L 1 1 L -1 1 Z").unwrap();
        let field = |band: f32| {
            compile(
                &Shape3::Field {
                    field: FieldRef(std::sync::Arc::new(LayerField::from_profile(
                        &profile, band,
                    ))),
                    depth: 0.4,
                    round: 0.0,
                }
                .paint(1),
            )
            .unwrap()
        };
        let plain = programs(include_str!("../../../scenes/3d/stress.ron"));
        let mut objects = plain.clone();
        objects.push(field(1.0));
        let grid = Grid::build(&objects);
        let last = (objects.len() - 1) as u32;
        assert_eq!(
            grid.unbounded,
            vec![last, 0],
            "поле слоя — перед плоскостями"
        );
        assert_eq!(grid.unbounded_solid, 1);
        assert_eq!(
            grid.dims,
            Grid::build(&plain).dims,
            "сетку поле не растягивает"
        );
        check(&objects);
        // Узкая полоса: далеко от квадрата поле знает только «не ближе полосы».
        let narrow = [field(0.05)];
        let g = Grid::build(&narrow);
        let (d, m) = g.near(&narrow, Vec3::new(1.5, 0.0, 0.0), 1e9);
        assert_eq!(m, None, "за полосой материала нет");
        assert!((d - 0.5).abs() < 1e-3, "{d}: по прямоугольнику фигуры");
        let (d, m) = g.near(&narrow, Vec3::ZERO, 1e9);
        assert_eq!(m, Some(1));
        assert!(d < 0.0, "{d}");
    }

    #[test]
    fn only_unbounded_objects_need_no_cells() {
        let floor = compile(&Shape3::Plane {
            normal: [0.0, 1.0, 0.0],
            offset: 0.0,
        })
        .unwrap();
        let grid = Grid::build(&[floor]);
        assert!(grid.cells.is_empty());
        assert_eq!(grid.unbounded, vec![0]);
    }

    /// Раскладка слов читается так же, как в шейдере (`scene_near` в
    /// `include/space.slang`), и даёт тот же ответ, что [`Grid::near`].
    #[test]
    fn packed_words_match_the_mirror() {
        let objects = programs(include_str!("../../../scenes/3d/stress.ron"));
        let grid = Grid::build(&objects);
        let mut w = vec![0u32; 100];
        grid.write(100, &mut w);
        assert_eq!(w.len(), 100 + grid.words());
        let f = |i: usize| f32::from_bits(w[i]);
        let g = 100;
        let from_words = |p: Vec3| -> (f32, Vec<u32>) {
            let origin = Vec3::new(f(g), f(g + 1), f(g + 2));
            let cell = f(g + 3);
            let dims = [w[g + 4], w[g + 5], w[g + 6]].map(|d| d as i64);
            let q = ((p - origin) / cell).floor();
            let q = [q.x as i64, q.y as i64, q.z as i64];
            let unbounded = w[g + 16] as usize;
            let mut list: Vec<u32> = w[unbounded..unbounded + w[g + 17] as usize].to_vec();
            let inside = (0..3).all(|k| q[k] >= 0 && q[k] < dims[k]);
            let best = if inside {
                let c =
                    w[g + 14] as usize + (((q[2] * dims[1] + q[1]) * dims[0] + q[0]) as usize) * 2;
                let (start, count) = (w[c] as usize, w[c + 1] as usize);
                list.extend(&w[start..start + count]);
                f(g + 7)
            } else {
                let lo = Vec3::new(f(g + 8), f(g + 9), f(g + 10));
                let hi = Vec3::new(f(g + 11), f(g + 12), f(g + 13));
                (p - p.clamp(lo, hi)).length()
            };
            (best, list)
        };
        for k in 0..2000 {
            let s = k as f32;
            let p = grid.origin - 2.0
                + Vec3::new((s * 0.37).fract(), (s * 0.61).fract(), (s * 0.83).fract())
                    * (grid.cell * Vec3::from(grid.dims.map(|d| d as f32)) + 4.0);
            let (start, list) = from_words(p);
            let mut best = start;
            for &i in &list {
                best = best.min(objects[i as usize].distance(p).0);
            }
            let mirror = grid.near(&objects, p, 1e9).0;
            assert!(
                (best - mirror).abs() < 1e-5,
                "{p}: слова {best}, зеркало {mirror}"
            );
        }
    }
}
