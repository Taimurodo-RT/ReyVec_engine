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

use crate::shape3d::{Bob, Circle, Lamp3, Material3, Shape3, compile, motion_offset};

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
pub struct SpaceFile {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub materials: Vec<MaterialEntry>,
    #[serde(default)]
    pub lamps: Vec<LampEntry>,
    #[serde(default)]
    pub objects: Vec<ObjectEntry>,
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
}

/// Что сцена «3D» рисует в один момент.
#[derive(Clone, Debug, PartialEq)]
pub struct SpaceFrame {
    pub shapes: Vec<Shape3>,
    pub lamps: Vec<Lamp3>,
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
        Ok(Self {
            materials,
            lamps: file.lamps,
            lamp_colors,
            bulbs,
            objects: file.objects,
            object_materials,
        })
    }

    /// Сколько предметов в сцене (без шариков ламп).
    pub fn object_count(&self) -> usize {
        self.objects.len()
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
        SpaceFrame { shapes, lamps }
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
