//! Рисунки из файлов рядом со сценой (этап 5.5): SVG и перетекающие фигуры Motif.
//!
//! В предмете сцены рисунок задаётся путём к файлу относительно файла сцены:
//!
//! ```ron
//! pictures: [(svg: "../assets/svg/illustration.svg", at: (0, 0), height: 0.3)],
//! morphs: [(motif: "../assets/motif/morph.reyvec.json", shape: "blob", scale: 0.5)],
//! ```
//!
//! Файл читается и режется один раз: [`crate::ShapeCache`] помнит готовый рисунок по
//! пути, размеру и времени изменения файла. Сохранили сцену, а рисунок не меняли — он
//! берётся готовым; поменяли рисунок — при следующем сохранении сцены он перечитается.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use reyvec_motif::{MorphAnimation, MorphFile, MorphShape, load_morph_file};
use reyvec_svg::{SvgDrawing, SvgOptions, load_svg};
use reyvec_vector::{ShapeId, ShapeSink};
use serde::Deserialize;

use crate::{Hex, MAX_COORD, P, check_point, check_size};

/// Рисунок SVG в предмете: файл, место центра, высота в единицах сцены, поворот.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PictureDesc {
    /// Путь к файлу `.svg` относительно файла сцены.
    pub svg: String,
    #[serde(default)]
    pub at: P,
    /// Высота рисунка (ширина — по его пропорциям). По ней выбирается точность нарезки.
    pub height: f64,
    #[serde(default)]
    pub angle: f64,
}

/// Перетекающая фигура из файла Motif (`*.reyvec.json`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MorphDesc {
    /// Путь к файлу Motif относительно файла сцены.
    pub motif: String,
    /// Имя фигуры в файле; можно не писать, если фигура одна.
    #[serde(default)]
    pub shape: Option<String>,
    #[serde(default)]
    pub at: P,
    #[serde(default = "crate::one_f64")]
    pub scale: f64,
    #[serde(default)]
    pub angle: f64,
    /// Анимация перетекания по имени; без имени — первая анимация фигуры (если есть).
    #[serde(default)]
    pub animation: Option<String>,
    /// Оттенок (умножается на цвет фигуры из Motif).
    #[serde(default)]
    pub color: Option<Hex>,
}

impl PictureDesc {
    pub(crate) fn check(&self) -> Result<(), String> {
        if self.svg.trim().is_empty() {
            return Err("svg: нужен путь к файлу .svg".into());
        }
        check_point("at", self.at)?;
        check_size("height", self.height)?;
        if !self.angle.is_finite() {
            return Err(format!("angle {} — нужно число", self.angle));
        }
        Ok(())
    }
}

impl MorphDesc {
    pub(crate) fn check(&self) -> Result<(), String> {
        if self.motif.trim().is_empty() {
            return Err("motif: нужен путь к файлу Motif (.reyvec.json)".into());
        }
        check_point("at", self.at)?;
        check_size("scale", self.scale)?;
        if !self.angle.is_finite() || self.angle.abs() > MAX_COORD * 360.0 {
            return Err(format!("angle {} — нужно число", self.angle));
        }
        Ok(())
    }
}

/// Файл рисунка: полный путь и ключ для [`crate::ShapeCache`] (путь, размер файла,
/// время изменения и то, что влияет на нарезку).
pub(crate) struct AssetFile {
    pub path: PathBuf,
    pub key: String,
}

impl AssetFile {
    pub fn find(dir: Option<&Path>, rel: &str, kind: &str, extra: &str) -> Result<Self, String> {
        let rel_path = Path::new(rel);
        let path = match dir {
            _ if rel_path.is_absolute() => rel_path.to_path_buf(),
            Some(dir) => dir.join(rel_path),
            None => {
                return Err(format!(
                    "«{rel}»: не знаю, где лежит файл сцены, путь не от чего считать"
                ));
            }
        };
        let meta = std::fs::metadata(&path)
            .map_err(|e| format!("файл «{}» не открыть: {e}", path.display()))?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let key = format!(
            "{kind}:{}:{}:{modified}:{extra}",
            path.display(),
            meta.len()
        );
        Ok(Self { path, key })
    }

    fn read(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.path)
            .map_err(|e| format!("файл «{}» не прочитать: {e}", self.path.display()))
    }
}

pub(crate) fn picture_file(dir: Option<&Path>, p: &PictureDesc) -> Result<AssetFile, String> {
    AssetFile::find(dir, &p.svg, "svg", &format!("{}", p.height))
}

pub(crate) fn motif_file(dir: Option<&Path>, m: &MorphDesc) -> Result<AssetFile, String> {
    AssetFile::find(dir, &m.motif, "motif", "")
}

/// Читает и режет рисунок SVG (в набор или в пачку для фона).
pub(crate) fn load_picture(
    file: &AssetFile,
    height: f64,
    sink: &mut dyn ShapeSink,
) -> Result<SvgDrawing, String> {
    let data = file.read()?;
    let drawing = load_svg(&data, sink, SvgOptions { height }).map_err(|e| e.to_string())?;
    for w in &drawing.warnings {
        tracing::warn!("SVG «{}»: {w}", file.path.display());
    }
    Ok(drawing)
}

/// Читает файл Motif и режет его фигуры.
pub(crate) fn load_motif(file: &AssetFile, sink: &mut dyn ShapeSink) -> Result<MorphFile, String> {
    let data = file.read()?;
    let motif = load_morph_file(&data, sink).map_err(|e| e.to_string())?;
    for w in &motif.warnings {
        tracing::warn!("Motif «{}»: {w}", file.path.display());
    }
    Ok(motif)
}

/// Как фигура Motif перетекает в предмете.
#[derive(Clone, Debug)]
pub struct MorphTrack {
    /// Форма без анимации.
    pub rest: f32,
    pub animation: Option<Arc<MorphAnimation>>,
}

impl MorphTrack {
    /// Форма в момент `t` секунд.
    pub fn at(&self, t: f64) -> f32 {
        self.animation
            .as_ref()
            .map_or(self.rest, |a| a.sample(t as f32))
    }
}

/// Фигура `m.shape` из файла и её перетекание.
pub(crate) fn pick_morph(file: &MorphFile, m: &MorphDesc) -> Result<(ShapeId, MorphTrack), String> {
    let names = || {
        file.shapes
            .iter()
            .map(|s| format!("«{}»", s.id))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let shape: &MorphShape = match &m.shape {
        Some(name) => file
            .shapes
            .iter()
            .find(|s| &s.id == name)
            .ok_or_else(|| format!("в «{}» нет фигуры «{name}», есть: {}", m.motif, names()))?,
        None if file.shapes.len() == 1 => &file.shapes[0],
        None => {
            return Err(format!(
                "в «{}» несколько фигур, напишите shape: одну из {}",
                m.motif,
                names()
            ));
        }
    };
    let animation = match &m.animation {
        Some(name) => Some(
            shape
                .animations
                .iter()
                .find(|a| &a.name == name)
                .ok_or_else(|| {
                    format!(
                        "у фигуры «{}» нет анимации «{name}», есть: {}",
                        shape.id,
                        shape
                            .animations
                            .iter()
                            .map(|a| format!("«{}»", a.name))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?
                .clone(),
        ),
        None => shape.animations.first().cloned(),
    };
    Ok((
        shape.shape,
        MorphTrack {
            rest: shape.morph,
            animation: animation.map(Arc::new),
        },
    ))
}
