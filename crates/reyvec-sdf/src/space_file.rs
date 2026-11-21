//! Файл объёмной сцены (этап 7.5): материалы, лампы и предметы сцены «3D» в тексте RON.
//! Песочница следит за файлом (`scenes/3d/space.ron`): сохранил — сцена сразу обновилась,
//! ошибся — на панели строка с ошибкой, а в окне остаётся прошлая сцена.
//!
//! Формы предметов — те же деревья [`Shape3`] (решение 0018). Материалы называются
//! словами: предмет пишет `material: "слизь"`, а номер в списке подставляет движок.
//! Движение (покачивание, круг, вращение) записано данными и считается на каждый кадр
//! ([`SpaceFile::frame`]).

use std::path::Path;

use glam::{Vec2, Vec3};
use serde::Deserialize;

use crate::brick3d::{Brick, BrickRef};
use crate::mesh3d::{MAX_TRIANGLES, Mesh3};
use crate::shape3d::{
    Bob, Circle, Lamp3, MAX_MATERIALS, Material3, Shape3, compile, motion_offset, rotation,
};

/// Встроенный пример: тот же текст, что `scenes/3d/space.ron` в папке проекта.
pub const EXAMPLE: &str = include_str!("../../../scenes/3d/space.ron");

/// Цвет: `"#rrggbb"` (как в редакторах, переводится в линейный) или три числа 0…1
/// (уже линейные).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Color3 {
    Hex(String),
    Linear([f32; 3]),
}

impl Color3 {
    /// Линейный цвет или ошибка с понятным текстом.
    pub fn linear(&self) -> Result<[f32; 3], String> {
        match self {
            Self::Linear(c) => Ok(*c),
            Self::Hex(text) => {
                let hex = text.strip_prefix('#').unwrap_or(text);
                let byte = |i: usize| {
                    hex.get(i..i + 2)
                        .and_then(|h| u8::from_str_radix(h, 16).ok())
                };
                match (hex.len(), byte(0), byte(2), byte(4)) {
                    (6, Some(r), Some(g), Some(b)) => Ok([r, g, b].map(srgb_to_linear)),
                    _ => Err(format!("цвет «{text}» не похож на \"#rrggbb\"")),
                }
            }
        }
    }
}

fn srgb_to_linear(v: u8) -> f32 {
    let c = f32::from(v) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Материал в файле.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialEntry {
    pub name: String,
    pub color: Color3,
    #[serde(default)]
    pub glow: f32,
    #[serde(default)]
    pub grid: bool,
}

/// Лампа в файле. `bulb` — радиус светящегося шарика на месте лампы (0 — без шарика).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LampEntry {
    pub at: [f32; 3],
    pub color: Color3,
    pub power: f32,
    pub range: f32,
    #[serde(default)]
    pub bulb: f32,
    #[serde(default)]
    pub bob: Bob,
    #[serde(default)]
    pub circle: Circle,
}

/// Предмет: форма, её материал, место, поворот (градусы вокруг X, Y, Z) и движение.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectEntry {
    #[serde(default)]
    pub name: String,
    pub material: String,
    #[serde(default)]
    pub at: [f32; 3],
    #[serde(default)]
    pub turn: [f32; 3],
    pub shape: Shape3,
    #[serde(default)]
    pub bob: Bob,
    #[serde(default)]
    pub circle: Circle,
    #[serde(default)]
    pub spin: [f32; 3],
}

/// Файл объёмной сцены как он записан.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceFile {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub materials: Vec<MaterialEntry>,
    #[serde(default)]
    pub lamps: Vec<LampEntry>,
    #[serde(default)]
    pub objects: Vec<ObjectEntry>,
    /// Полигональные модели (7.6ж).
    #[serde(default)]
    pub meshes: Vec<MeshEntry>,
    /// Откуда камера смотрит при открытии файла и по R (без него — вид по умолчанию).
    #[serde(default)]
    pub camera: Option<CameraEntry>,
    /// Сетка отсева (7.6е): `false` выключает её для сравнения замеров.
    #[serde(default = "yes")]
    pub culling: bool,
    /// Отсев по взгляду (7.6к): в кадр идут только объекты, видимые камерой или
    /// бросающие тень в кадр. `false` выключает его для сравнения замеров.
    #[serde(default = "yes")]
    pub view_culling: bool,
    /// Лучи глаза вычислительным шейдером (7.6м, быстрее пиксельного на RTX 5080:
    /// реймарчинг 0.26 мс вместо 0.45). `false` — пиксельным, для сравнения.
    #[serde(default = "yes")]
    pub compute: bool,
}

fn yes() -> bool {
    true
}

/// Камера сцены в файле: где глаз и куда смотрит.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraEntry {
    pub eye: [f32; 3],
    pub look_at: [f32; 3],
}

/// Полигональная модель (7.6ж): многогранник, материал, место, поворот, движение.
/// `shadow: false` — без тени (заменителя SDF нет). У модели из файла (`Model`, 8.14)
/// материал можно не писать: тогда цвета берутся из материалов файла.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub material: String,
    #[serde(default)]
    pub at: [f32; 3],
    #[serde(default)]
    pub turn: [f32; 3],
    pub mesh: Mesh3,
    #[serde(default)]
    pub bob: Bob,
    #[serde(default)]
    pub circle: Circle,
    #[serde(default)]
    pub spin: [f32; 3],
    #[serde(default = "yes")]
    pub shadow: bool,
}

/// Модель в один момент: треугольники в мире и номер материала.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshFrame {
    pub triangles: Vec<[Vec3; 3]>,
    pub material: u32,
    /// Нормали вершин тех же треугольников (в мире, 8.14б). Пусто — грани плоские.
    pub normals: Vec<[Vec3; 3]>,
    /// Точки текстуры вершин тех же треугольников (8.14б2). Пусто — без текстуры.
    pub uvs: Vec<[Vec2; 3]>,
}

/// Модель сцены, готовая к кадрам: части по материалам (номер материала и треугольники в
/// координатах модели) и заменители для теней.
#[derive(Clone, Debug, PartialEq)]
struct SceneMesh {
    entry: MeshEntry,
    /// Части по материалам в координатах модели.
    parts: Vec<MeshFrame>,
    /// Заменители из плоскостей граней выпуклых кусков (7.6ж, 8.14) — для сравнения с
    /// кирпичом и для моделей, у которых кирпича нет.
    proxies: Vec<Shape3>,
    /// «Кирпич поля» модели (8.15): один заменитель на всю модель.
    brick: Option<Shape3>,
}

/// Сцена, готовая к кадрам: материалы по номерам, лампы и предметы с номерами материалов.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpaceScene {
    pub materials: Vec<Material3>,
    lamps: Vec<LampEntry>,
    /// Цвет каждой лампы (линейный).
    lamp_colors: Vec<[f32; 3]>,
    /// Номер материала шарика каждой лампы (если шарик есть).
    bulbs: Vec<Option<u32>>,
    objects: Vec<ObjectEntry>,
    object_materials: Vec<u32>,
    meshes: Vec<SceneMesh>,
    /// Заметки о чтении файла для журнала (например, у каких кусков моделей нет тени).
    pub notes: Vec<String>,
    /// Текстуры цвета моделей (8.14б2): на них ссылаются материалы (`Material3::texture`).
    pub textures: Vec<std::sync::Arc<crate::model3d::ModelTexture>>,
    /// Кирпичи поля моделей (8.15): видеокарта загружает их картинки и ставит номера
    /// ([`crate::brick3d::Brick::set_texture`]). Одна модель одного размера — один кирпич.
    pub bricks: Vec<BrickRef>,
    /// Тени моделей от плоскостей граней, как до 8.15 (для сравнения на панели); обычно
    /// (false) — от кирпичей поля.
    pub plane_shadows: bool,
    /// Камера из файла (если записана).
    pub camera: Option<CameraEntry>,
    /// Сетка отсева выключена в файле (`culling: false`).
    pub culling_off: bool,
    /// Отсев по взгляду выключен в файле (`view_culling: false`).
    pub view_culling_off: bool,
    /// Лучи глаза вычислительным шейдером (7.6м; `compute: false` — пиксельным).
    pub compute: bool,
}

/// Что сцена «3D» рисует в один момент.
#[derive(Clone, Debug, PartialEq)]
pub struct SpaceFrame {
    pub shapes: Vec<Shape3>,
    pub lamps: Vec<Lamp3>,
    /// Полигональные модели и их заменители для теней (7.6ж).
    pub meshes: Vec<MeshFrame>,
    pub proxies: Vec<Shape3>,
    /// Светящиеся предметы без теней (2.5D-вид, 7.9б): глаз их видит, тени и затенение
    /// в углах — нет.
    pub glows: Vec<Shape3>,
    /// Неподвижен ли предмет (8.8б): по одному на `shapes`, потом по одному на `proxies`.
    /// Пусто — не известно (всё считается движущимся). Неподвижное сетка отсева
    /// раскладывает один раз ([`crate::grid3d::GridCache`]).
    pub still: Vec<bool>,
}

/// Свечение шарика лампы: ярче 1, чтобы вокруг был ореол.
const BULB_GLOW: f32 = 4.0;

/// Двигается ли предмет с такими покачиванием, кругом и вращением.
fn moves(bob: &Bob, circle: &Circle, spin: &[f32; 3]) -> bool {
    bob.period != 0.0 && bob.by != [0.0; 3]
        || circle.period != 0.0 && circle.radius != [0.0; 2]
        || spin.iter().any(|&s| s != 0.0)
}

impl SpaceScene {
    /// Читает текст файла; пути к моделям glTF — от рабочей папки. Ошибка — строка для
    /// человека (где и что не так).
    pub fn parse(text: &str) -> Result<Self, String> {
        Self::parse_in(text, None)
    }

    /// Читает текст файла; пути к моделям glTF — от папки `dir` (папки файла сцены).
    pub fn parse_in(text: &str, dir: Option<&Path>) -> Result<Self, String> {
        let file: SpaceFile = ron::from_str(text).map_err(|e| e.to_string())?;
        if file.format != "reyvec-space" {
            return Err(format!(
                "format: \"{}\", а нужен \"reyvec-space\"",
                file.format
            ));
        }
        if file.version != 1 {
            return Err(format!(
                "version: {}, а движок знает только 1",
                file.version
            ));
        }
        let mut materials = Vec::new();
        for m in &file.materials {
            if file.materials.iter().filter(|o| o.name == m.name).count() > 1 {
                return Err(format!("материал «{}» записан дважды", m.name));
            }
            materials.push(Material3 {
                color: m
                    .color
                    .linear()
                    .map_err(|e| format!("материал «{}»: {e}", m.name))?,
                glow: m.glow,
                grid: m.grid,
                texture: 0,
            });
        }
        let mut object_materials = Vec::new();
        for (i, o) in file.objects.iter().enumerate() {
            let label = object_label(o, i);
            let Some(index) = file.materials.iter().position(|m| m.name == o.material) else {
                return Err(format!(
                    "{label}: нет материала «{}» (есть: {})",
                    o.material,
                    names(&file.materials)
                ));
            };
            object_materials.push(index as u32);
            compile(&o.shape).map_err(|e| format!("{label}: {e}"))?;
        }
        let mut meshes = Vec::new();
        let mut notes = Vec::new();
        let mut textures = Vec::new();
        let mut texture_keys: Vec<(std::path::PathBuf, usize)> = Vec::new();
        let mut bricks: Vec<BrickRef> = Vec::new();
        let mut brick_keys: Vec<String> = Vec::new();
        for (i, m) in file.meshes.iter().enumerate() {
            let label = if m.name.is_empty() {
                format!("модель {}", i + 1)
            } else {
                format!("модель «{}»", m.name)
            };
            let material = if m.material.is_empty() {
                None
            } else {
                let Some(index) = file.materials.iter().position(|e| e.name == m.material) else {
                    return Err(format!(
                        "{label}: нет материала «{}» (есть: {})",
                        m.material,
                        names(&file.materials)
                    ));
                };
                Some(index as u32)
            };
            // Ключ кирпича: та же модель того же размера — тот же кирпич.
            let brick_key = match &m.mesh {
                Mesh3::Model { file: path, scale } => {
                    let full = dir.map_or_else(|| Path::new(path).to_path_buf(), |d| d.join(path));
                    format!("{}|{}", full.display(), scale.to_bits())
                }
                mesh => format!("{mesh:?}"),
            };
            let (parts, proxies) = match &m.mesh {
                Mesh3::Model { file: path, scale } => {
                    let full = dir.map_or_else(|| Path::new(path).to_path_buf(), |d| d.join(path));
                    let model =
                        crate::model3d::load(&full, *scale).map_err(|e| format!("{label}: {e}"))?;
                    let mut parts = Vec::new();
                    let mut all = Vec::new();
                    for part in model.parts {
                        // Текстура: одна картинка одного файла — одна на сцену, сколько бы
                        // моделей её ни брали.
                        let texture = part.texture.map(|t| {
                            let t = &model.textures[t];
                            let key = (full.clone(), t.image);
                            match texture_keys.iter().position(|k| *k == key) {
                                Some(at) => at as u32 + 1,
                                None => {
                                    texture_keys.push(key);
                                    textures.push(t.clone());
                                    textures.len() as u32
                                }
                            }
                        });
                        let index = match material {
                            Some(index) => index,
                            None => {
                                materials.push(Material3 {
                                    color: part.color,
                                    glow: 0.0,
                                    grid: false,
                                    texture: texture.unwrap_or(0),
                                });
                                materials.len() as u32 - 1
                            }
                        };
                        if part.texture.is_some() && material.is_some() {
                            notes.push(format!(
                                "{label}: у материала «{}» есть текстура, но задан свой материал — \
                                 текстура не рисуется",
                                part.material
                            ));
                        }
                        all.extend_from_slice(&part.triangles);
                        parts.push(MeshFrame {
                            triangles: part.triangles,
                            material: index,
                            normals: part.normals,
                            uvs: if material.is_some() {
                                Vec::new()
                            } else {
                                part.uvs
                            },
                        });
                    }
                    let (proxies, skipped) = if m.shadow {
                        crate::model3d::convex_proxies(&all)
                    } else {
                        (Vec::new(), 0)
                    };
                    if skipped > 0 {
                        notes.push(format!(
                            "{label}: {skipped} кусков без тени (невыпуклые, плоские или больше {})",
                            crate::model3d::MAX_PROXIES
                        ));
                    }
                    (parts, proxies)
                }
                mesh => {
                    let Some(index) = material else {
                        return Err(format!("{label}: нужен материал (material: \"…\")"));
                    };
                    let proxies = if m.shadow {
                        vec![mesh.shadow_proxy()]
                    } else {
                        Vec::new()
                    };
                    (
                        vec![MeshFrame {
                            triangles: mesh.triangles(),
                            material: index,
                            ..MeshFrame::default()
                        }],
                        proxies,
                    )
                }
            };
            let count: usize = parts.iter().map(|p| p.triangles.len()).sum();
            if count > MAX_TRIANGLES {
                return Err(format!(
                    "{label}: {count} граней, больше {MAX_TRIANGLES} не бывает"
                ));
            }
            let brick = if m.shadow {
                let made = match brick_keys.iter().position(|k| *k == brick_key) {
                    Some(at) => Some(bricks[at].clone()),
                    None => {
                        let all: Vec<[Vec3; 3]> = parts
                            .iter()
                            .flat_map(|p| p.triangles.iter().copied())
                            .collect();
                        let (brick, open) = Brick::bake(&all);
                        if open > 0 {
                            notes.push(format!(
                                "{label}: {open} незамкнутых кусков без тени (у них нет «внутри»)"
                            ));
                        }
                        brick.map(|b| {
                            let b = BrickRef(std::sync::Arc::new(b));
                            brick_keys.push(brick_key);
                            bricks.push(b.clone());
                            b
                        })
                    }
                };
                made.map(|brick| Shape3::Brick { brick })
            } else {
                None
            };
            meshes.push(SceneMesh {
                entry: m.clone(),
                parts,
                proxies,
                brick,
            });
        }
        let mut lamp_colors = Vec::new();
        let mut bulbs = Vec::new();
        for (i, l) in file.lamps.iter().enumerate() {
            let color = l
                .color
                .linear()
                .map_err(|e| format!("лампа {}: {e}", i + 1))?;
            lamp_colors.push(color);
            bulbs.push((l.bulb > 0.0).then(|| {
                materials.push(Material3 {
                    color,
                    glow: BULB_GLOW,
                    grid: false,
                    texture: 0,
                });
                materials.len() as u32 - 1
            }));
        }
        if materials.len() > MAX_MATERIALS {
            return Err(format!(
                "материалов (с лампочками) {}, больше {MAX_MATERIALS} не бывает",
                materials.len()
            ));
        }
        Ok(Self {
            materials,
            lamps: file.lamps,
            lamp_colors,
            bulbs,
            objects: file.objects,
            object_materials,
            meshes,
            notes,
            textures,
            bricks,
            plane_shadows: false,
            camera: file.camera,
            culling_off: !file.culling,
            view_culling_off: !file.view_culling,
            compute: file.compute,
        })
    }

    /// Сколько предметов в сцене (без шариков ламп).
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    /// Камера при открытии сцены и по R: из файла или вид по умолчанию.
    pub fn home_camera(&self) -> crate::Camera3D {
        self.camera.map_or_else(crate::Camera3D::default, |c| {
            crate::Camera3D::looking_at(Vec3::from(c.eye), Vec3::from(c.look_at))
        })
    }

    /// Сколько предметов и моделей движется (покачивание, круг, вращение; 8.7).
    pub fn moving_count(&self) -> usize {
        self.objects
            .iter()
            .filter(|o| {
                moves(&o.bob, &o.circle, &o.spin) || matches!(o.shape, Shape3::Animate { .. })
            })
            .count()
            + self
                .meshes
                .iter()
                .filter(|m| moves(&m.entry.bob, &m.entry.circle, &m.entry.spin))
                .count()
    }

    /// Сколько полигональных моделей в сцене.
    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    /// Меняются ли формы кадра ([`SpaceFrame::shapes`]) во времени сами
    /// ([`Shape3::morphs`], 8.20е): по одному на форму, шарики ламп — нет. Сетки для
    /// режима «треугольники» строятся один раз, поэтому такие формы остаются лучам.
    pub fn morphing(&self) -> Vec<bool> {
        let mut out: Vec<bool> = self.objects.iter().map(|o| o.shape.morphs()).collect();
        out.extend(self.bulbs.iter().flatten().map(|_| false));
        out
    }

    /// Предметы и лампы в момент `time` (секунды).
    pub fn frame(&self, time: f32) -> SpaceFrame {
        let mut still: Vec<bool> = self
            .objects
            .iter()
            .map(|o| {
                !moves(&o.bob, &o.circle, &o.spin) && !matches!(o.shape, Shape3::Animate { .. })
            })
            .collect();
        let mut shapes: Vec<Shape3> = self
            .objects
            .iter()
            .zip(&self.object_materials)
            .map(|(o, &material)| {
                let at = Vec3::from(o.at) + motion_offset(&o.bob, &o.circle, time);
                let turn = [0, 1, 2].map(|i| o.turn[i] + o.spin[i] * time);
                o.shape
                    .at_time(time)
                    .placed(at.to_array(), turn)
                    .paint(material)
            })
            .collect();
        let mut lamps = Vec::new();
        for ((l, &color), bulb) in self.lamps.iter().zip(&self.lamp_colors).zip(&self.bulbs) {
            let at = (Vec3::from(l.at) + motion_offset(&l.bob, &l.circle, time)).to_array();
            lamps.push(Lamp3 {
                at,
                color,
                power: l.power,
                range: l.range,
            });
            if let Some(material) = bulb {
                shapes.push(Shape3::Sphere { radius: l.bulb }.at(at).paint(*material));
                still.push(!moves(&l.bob, &l.circle, &[0.0; 3]));
            }
        }
        let mut meshes = Vec::new();
        let mut proxies = Vec::new();
        let shapes_count = still.len();
        for mesh in &self.meshes {
            let m = &mesh.entry;
            let mesh_still = !moves(&m.bob, &m.circle, &m.spin);
            let at = Vec3::from(m.at) + motion_offset(&m.bob, &m.circle, time);
            let turn = [0, 1, 2].map(|i| m.turn[i] + m.spin[i] * time);
            let r = rotation(turn);
            for part in &mesh.parts {
                meshes.push(MeshFrame {
                    triangles: part
                        .triangles
                        .iter()
                        .map(|t| t.map(|v| at + r * v))
                        .collect(),
                    material: part.material,
                    normals: part.normals.iter().map(|t| t.map(|n| r * n)).collect(),
                    uvs: part.uvs.clone(),
                });
            }
            match &mesh.brick {
                Some(brick) if !self.plane_shadows => {
                    proxies.push(brick.clone().placed(at.to_array(), turn));
                }
                _ => proxies.extend(
                    mesh.proxies
                        .iter()
                        .map(|p| p.clone().placed(at.to_array(), turn)),
                ),
            }
            still.resize(shapes_count + proxies.len(), mesh_still);
        }
        SpaceFrame {
            shapes,
            lamps,
            meshes,
            proxies,
            glows: Vec::new(),
            still,
        }
    }
}

impl SpaceFrame {
    /// Формы с `hide[i] == true` глаз больше не видит (8.20б): их рисуют сетки, а тени и
    /// затенение в углах по-прежнему считаются по формам — они переходят в заменители
    /// (`proxies`, только тени). Флаги неподвижности переезжают вместе с ними.
    pub fn hide_from_eye(&mut self, hide: &[bool]) {
        if !hide.iter().any(|&h| h) {
            return;
        }
        let hidden = |i: usize| hide.get(i).copied().unwrap_or(false);
        let with_still = self.still.len() == self.shapes.len() + self.proxies.len();
        let (shapes, proxies) = (self.shapes.len(), self.proxies.len());
        let mut kept = Vec::with_capacity(shapes);
        let mut moved = Vec::new();
        let mut still_kept = Vec::new();
        let mut still_moved = Vec::new();
        for (i, shape) in std::mem::take(&mut self.shapes).into_iter().enumerate() {
            let still = with_still && self.still[i];
            if hidden(i) {
                moved.push(shape);
                still_moved.push(still);
            } else {
                kept.push(shape);
                still_kept.push(still);
            }
        }
        self.shapes = kept;
        self.proxies.extend(moved);
        if with_still {
            let old_proxies = self.still[shapes..shapes + proxies].to_vec();
            self.still = still_kept;
            self.still.extend(old_proxies);
            self.still.extend(still_moved);
        }
    }
}

fn object_label(o: &ObjectEntry, i: usize) -> String {
    if o.name.is_empty() {
        format!("предмет {}", i + 1)
    } else {
        format!("предмет «{}»", o.name)
    }
}

fn names(materials: &[MaterialEntry]) -> String {
    let list: Vec<_> = materials.iter().map(|m| format!("«{}»", m.name)).collect();
    if list.is_empty() {
        "ни одного".into()
    } else {
        list.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_reads_and_moves() {
        let scene = SpaceScene::parse(EXAMPLE).unwrap();
        assert!(scene.object_count() >= 7);
        let a = scene.frame(0.0);
        let b = scene.frame(1.0);
        assert_eq!(a.shapes.len(), b.shapes.len());
        assert_eq!(a.lamps.len(), 2);
        assert_ne!(a.lamps[0].at, b.lamps[0].at, "тёплая лампа летает по кругу");
        assert_eq!(a.lamps[1].at, b.lamps[1].at, "холодная висит");
        for s in &b.shapes {
            assert!(compile(s).is_ok());
        }
    }

    /// Опечатка в имени настройки — ошибка с её именем, а не тихий пропуск (так
    /// `compute: true` в старой сборке не выглядит включённым).
    #[test]
    fn unknown_keys_are_errors() {
        let text = EXAMPLE.replacen("version: 1,", "version: 1,\n    computе: true,", 1);
        let err = SpaceScene::parse(&text).unwrap_err();
        assert!(err.contains("computе"), "{err}");
        let text = EXAMPLE.replacen("version: 1,", "version: 1,\n    compute: false,", 1);
        assert!(!SpaceScene::parse(&text).unwrap().compute);
        // По умолчанию лучи вычислительным шейдером: обновление (update.bat) затирает
        // правки в файлах сцен, и строка `compute: true` пропадала.
        assert!(SpaceScene::parse(EXAMPLE).unwrap().compute);
    }

    /// Буфер геометрии различает MAX_MATERIALS материалов (7.20б): файл с большим числом —
    /// понятная ошибка, а не тихо слитые цвета.
    #[test]
    fn too_many_materials_is_an_error() {
        let list = |n: usize| {
            (0..n)
                .map(|i| format!(r##"(name: "м{i}", color: "#808080")"##))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let file = |n: usize| {
            format!(
                r##"(format: "reyvec-space", version: 1, materials: [{}])"##,
                list(n)
            )
        };
        assert!(SpaceScene::parse(&file(MAX_MATERIALS)).is_ok());
        let err = SpaceScene::parse(&file(MAX_MATERIALS + 1)).unwrap_err();
        assert!(err.contains(&MAX_MATERIALS.to_string()), "{err}");
    }

    #[test]
    fn hex_colors_are_srgb() {
        let c = Color3::Hex("#ff8000".into()).linear().unwrap();
        assert_eq!(c[0], 1.0);
        assert!((c[1] - 0.2158).abs() < 1e-3);
        assert_eq!(c[2], 0.0);
        assert!(Color3::Hex("#ff80".into()).linear().is_err());
        assert_eq!(
            Color3::Linear([0.1, 0.2, 0.3]).linear().unwrap(),
            [0.1, 0.2, 0.3]
        );
    }

    #[test]
    fn errors_say_what_is_wrong() {
        let bad = |text: &str| SpaceScene::parse(text).unwrap_err();
        let head = r##"(format: "reyvec-space", version: 1, materials: [(name: "камень", color: "#888888")], "##;
        let e = bad(&format!(
            r##"{head} objects: [(name: "шар", material: "глина", shape: Sphere(radius: 1.0))])"##
        ));
        assert!(
            e.contains("«шар»") && e.contains("«глина»") && e.contains("«камень»"),
            "{e}"
        );
        let e = bad(r##"(format: "reyvec-scene", version: 1)"##);
        assert!(e.contains("reyvec-space"), "{e}");
        let e = bad(&format!(
            r##"{head} objects: [(material: "камень", shape: Sphere(radius: ))])"##
        ));
        assert!(!e.is_empty());
    }

    #[test]
    fn animate_inside_a_shape_moves_only_that_part() {
        let s = Shape3::Union {
            smooth: 0.0,
            shapes: vec![
                Shape3::Sphere { radius: 0.5 },
                Shape3::Animate {
                    bob: Bob {
                        by: [2.0, 0.0, 0.0],
                        period: 4.0,
                    },
                    circle: Circle::default(),
                    spin: [0.0; 3],
                    shape: Box::new(Shape3::Sphere { radius: 0.5 }.at([0.0, 3.0, 0.0])),
                },
            ],
        };
        // Через четверть периода вторая сфера сдвинута на 2 по X.
        let t1 = s.at_time(1.0);
        assert!(t1.distance(Vec3::new(2.0, 3.0, 0.0)) < -0.49);
        assert!(t1.distance(Vec3::ZERO) < -0.49, "первая на месте");
        assert!(
            s.distance(Vec3::new(0.0, 3.0, 0.0)) < -0.49,
            "без времени — момент 0"
        );
    }

    /// Меняется во времени (8.20е) только форма, у которой часть движется внутри операции:
    /// в примере сцены «3D» — слизь. Движение целиком (`bob`, `spin`) — не меняется.
    #[test]
    fn only_shapes_with_moving_parts_morph() {
        let text = include_str!("../../../scenes/3d/space.ron");
        let scene = SpaceScene::parse_in(text, None).expect("сцена");
        let morphing = scene.morphing();
        let frame = scene.frame(0.0);
        assert_eq!(morphing.len(), frame.shapes.len());
        let names: Vec<&str> = scene
            .objects
            .iter()
            .zip(&morphing)
            .filter(|(_, m)| **m)
            .map(|(o, _)| o.name.as_str())
            .collect();
        assert_eq!(names, ["слизь"]);
        let lone = Shape3::Animate {
            bob: Bob {
                by: [1.0, 0.0, 0.0],
                period: 2.0,
            },
            circle: Circle::default(),
            spin: [0.0; 3],
            shape: Box::new(Shape3::Sphere { radius: 0.5 }),
        };
        assert!(!lone.morphs(), "движение целиком");
        let cut = Shape3::Subtract {
            base: Box::new(Shape3::Sphere { radius: 1.0 }),
            cut: vec![lone],
            smooth: 0.0,
        };
        assert!(cut.morphs(), "движется вырез");
    }

    #[test]
    fn models_scene_reads_gltf() {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scenes/3d"));
        let text = include_str!("../../../scenes/3d/models.ron");
        let scene = SpaceScene::parse_in(text, Some(dir)).unwrap();
        assert_eq!(scene.mesh_count(), 4);
        // 4 материала сцены, 4 — из файла станка (первая модель без material), 1 — доски
        // ящика, 2 лампочки.
        assert_eq!(scene.materials.len(), 11);
        // Текстура досок ящика (8.14б2) — одна на сцену, на неё ссылается материал досок.
        assert_eq!(scene.textures.len(), 1);
        let boards: Vec<_> = scene.materials.iter().filter(|m| m.texture != 0).collect();
        assert_eq!(boards.len(), 1);
        assert_eq!(boards[0].texture, 1);
        let orange = scene
            .materials
            .iter()
            .position(|m| m.color == [0.80, 0.30, 0.05]);
        assert!(orange.is_some(), "цвет корпуса из файла");
        let frame = scene.frame(0.0);
        // Станок — 4 части по материалам, малый станок — тоже 4 (все «золото»), ящик и
        // коробка — по 1.
        assert_eq!(frame.meshes.len(), 10);
        // У ящика точки текстуры, у остальных нет.
        assert_eq!(frame.meshes[8].uvs.len(), 12);
        assert!(frame.meshes[..8].iter().all(|m| m.uvs.is_empty()));
        let gold = 1;
        assert!(frame.meshes[4..8].iter().all(|m| m.material == gold));
        // Тени (8.15): по кирпичу поля на модель — станок, малый станок (другой размер —
        // свой кирпич), ящик и коробка.
        assert_eq!(frame.proxies.len(), 4);
        assert_eq!(scene.bricks.len(), 4);
        // Кирпич станка: середина корпуса в тени-заменителе, точка над корпусом — нет.
        let machine = &frame.proxies[0];
        let world = |v: Vec3| crate::shape3d::rotation([0.0, -20.0, 0.0]) * v;
        assert!(machine.distance(world(Vec3::new(-0.25, 0.6, 0.0))) < 0.0);
        assert!(machine.distance(world(Vec3::new(-0.25, 1.05, 0.0))) > 0.0);
        // Как до 8.15: 6 кусков станка, 6 малого, ящик и коробка.
        let mut planes = scene.clone();
        planes.plane_shadows = true;
        assert_eq!(planes.frame(0.0).proxies.len(), 14);
        // Малый станок вдвое меньше: верх дымохода на 1.5 × 0.5.
        let top = frame.meshes[4..8]
            .iter()
            .flat_map(|m| m.triangles.iter().flatten())
            .map(|v| v.y)
            .fold(0.0, f32::max);
        assert!((top - 0.75).abs() < 1e-4, "{top}");
        assert!(scene.notes.is_empty(), "{:?}", scene.notes);
    }

    #[test]
    fn factory_scene_has_thousand_objects() {
        // Сцена-нагрузка 8.7: больше 1000 предметов, 96 станков с одним общим кирпичом
        // поля, часть движется; в кадре все объекты помещаются в предел шейдера.
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scenes/3d"));
        let text = include_str!("../../../scenes/3d/factory.ron");
        let scene = SpaceScene::parse_in(text, Some(dir)).unwrap();
        assert!(scene.object_count() >= 1000, "{}", scene.object_count());
        assert_eq!(scene.mesh_count(), 96);
        assert_eq!(
            scene.bricks.len(),
            1,
            "одна модель одного размера — один кирпич"
        );
        assert!(scene.moving_count() > 300, "{}", scene.moving_count());
        assert!(scene.notes.is_empty(), "{:?}", scene.notes);
        let frame = scene.frame(1.0);
        assert_eq!(frame.proxies.len(), 96);
        let total = frame.shapes.len() + frame.proxies.len() + frame.glows.len();
        assert!(
            total > 1000 && total <= crate::shape3d::MAX_OBJECTS,
            "{total}"
        );
    }

    #[test]
    fn model_errors_say_what_is_wrong() {
        let scene = |meshes: &str| {
            format!(
                r#"(format: "reyvec-space", version: 1,
                    materials: [(name: "м", color: (1.0, 1.0, 1.0))], meshes: [{meshes}])"#
            )
        };
        let e = SpaceScene::parse(&scene(r#"(name: "к", mesh: Block(size: (1.0, 1.0, 1.0)))"#))
            .unwrap_err();
        assert!(e.contains("модель «к»") && e.contains("материал"), "{e}");
        let e = SpaceScene::parse_in(
            &scene(r#"(name: "нет", mesh: Model(file: "нет.glb"))"#),
            Some(Path::new("/нет/папки")),
        )
        .unwrap_err();
        assert!(e.contains("модель «нет»") && e.contains("нет.glb"), "{e}");
    }

    /// Спрятанные от глаза формы переходят в заменители вместе со своими флагами
    /// неподвижности; остальное — на месте и по порядку.
    #[test]
    fn hidden_shapes_become_proxies() {
        let ball = |r: f32| Shape3::Sphere { radius: r };
        let mut frame = SpaceFrame {
            shapes: vec![ball(1.0), ball(2.0), ball(3.0)],
            lamps: Vec::new(),
            meshes: Vec::new(),
            proxies: vec![ball(9.0)],
            glows: Vec::new(),
            still: vec![true, false, true, false],
        };
        frame.hide_from_eye(&[false, true, true]);
        assert_eq!(frame.shapes, vec![ball(1.0)]);
        assert_eq!(frame.proxies, vec![ball(9.0), ball(2.0), ball(3.0)]);
        assert_eq!(frame.still, vec![true, false, false, true]);
    }
}
