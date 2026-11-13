//! Файл объёмной сцены (этап 7.5): материалы, лампы и предметы сцены «3D» в тексте RON.
//! Песочница следит за файлом (`scenes/3d/space.ron`): сохранил — сцена сразу обновилась,
//! ошибся — на панели строка с ошибкой, а в окне остаётся прошлая сцена.
//!
//! Формы предметов — те же деревья [`Shape3`] (решение 0018). Материалы называются
//! словами: предмет пишет `material: "слизь"`, а номер в списке подставляет движок.
//! Движение (покачивание, круг, вращение) записано данными и считается на каждый кадр
//! ([`SpaceFile::frame`]).

use glam::Vec3;
use serde::Deserialize;

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
/// `shadow: false` — без тени (заменителя SDF нет).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshEntry {
    #[serde(default)]
    pub name: String,
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
#[derive(Clone, Debug, PartialEq)]
pub struct MeshFrame {
    pub triangles: Vec<[Vec3; 3]>,
    pub material: u32,
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
    meshes: Vec<(MeshEntry, u32, Vec<[Vec3; 3]>, Shape3)>,
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
}

/// Свечение шарика лампы: ярче 1, чтобы вокруг был ореол.
const BULB_GLOW: f32 = 4.0;

impl SpaceScene {
    /// Читает текст файла. Ошибка — строка для человека (где и что не так).
    pub fn parse(text: &str) -> Result<Self, String> {
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
        for (i, m) in file.meshes.iter().enumerate() {
            let label = if m.name.is_empty() {
                format!("модель {}", i + 1)
            } else {
                format!("модель «{}»", m.name)
            };
            let Some(index) = file.materials.iter().position(|e| e.name == m.material) else {
                return Err(format!(
                    "{label}: нет материала «{}» (есть: {})",
                    m.material,
                    names(&file.materials)
                ));
            };
            let triangles = m.mesh.triangles();
            if triangles.len() > MAX_TRIANGLES {
                return Err(format!(
                    "{label}: {} граней, больше {MAX_TRIANGLES} не бывает",
                    triangles.len()
                ));
            }
            let proxy = m.mesh.shadow_proxy();
            meshes.push((m.clone(), index as u32, triangles, proxy));
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

    /// Сколько полигональных моделей в сцене.
    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    /// Предметы и лампы в момент `time` (секунды).
    pub fn frame(&self, time: f32) -> SpaceFrame {
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
            }
        }
        let mut meshes = Vec::new();
        let mut proxies = Vec::new();
        for (m, material, triangles, proxy) in &self.meshes {
            let at = Vec3::from(m.at) + motion_offset(&m.bob, &m.circle, time);
            let turn = [0, 1, 2].map(|i| m.turn[i] + m.spin[i] * time);
            let r = rotation(turn);
            meshes.push(MeshFrame {
                triangles: triangles.iter().map(|t| t.map(|v| at + r * v)).collect(),
                material: *material,
            });
            if m.shadow {
                proxies.push(proxy.clone().placed(at.to_array(), turn));
            }
        }
        SpaceFrame {
            shapes,
            lamps,
            meshes,
            proxies,
            glows: Vec::new(),
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
}
