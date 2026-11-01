//! Чтение файлов перетекающих фигур от редактора Motif (`*.reyvec.json`, версия 1).
//!
//! Движок не подключает код Motif: он читает только этот файл. Формат описан в
//! `docs/motif-morph.md`, часть 6. [`load_morph_file`] разбирает файл, добавляет каждую
//! фигуру в [`ShapeLibrary`] как перетекающую (контуры сводятся и режутся один раз здесь)
//! и возвращает записанные в Motif анимации числа `morph`. Каждый кадр игре остаётся только
//! взять значение анимации ([`MorphAnimation::sample`]) и записать его в экземпляр.
//!
//! Координаты: y вверх, как в движке; пиксели Motif делятся на 100 (100 пикселей = 1
//! единица сцены).

use kurbo::{Affine, BezPath};
use reyvec_vector::{Color, FillRule, Gradient, Paint, ShapeId, ShapeLibrary, Spread};
use serde::Deserialize;

/// Сколько пикселей Motif в одной единице сцены.
pub const PIXELS_PER_UNIT: f64 = 100.0;

/// Почему файл не прочитать целиком. Ошибки отдельных фигур не останавливают чтение:
/// фигура пропускается, а причина попадает в [`MorphFile::warnings`].
#[derive(Debug, thiserror::Error)]
pub enum MotifError {
    #[error("файл не JSON или не того вида: {0}")]
    Json(#[from] serde_json::Error),
    #[error("это не файл перетекающих фигур («format» = «{0}», а нужно «reyvec-morph»)")]
    Format(String),
    #[error("файл версии {0}, а движок читает только версию 1: обновите движок")]
    Version(u32),
    #[error("ось y «{0}», а движок понимает только «up»")]
    Axis(String),
}

/// Прочитанный файл.
#[derive(Clone, Debug)]
pub struct MorphFile {
    /// Частота записанных значений `morph` (кадров в секунду).
    pub fps: f32,
    pub shapes: Vec<MorphShape>,
    /// Что пропущено или пока не учитывается (фигуры с ошибками, метки соответствия).
    pub warnings: Vec<String>,
}

/// Перетекающая фигура из файла, уже добавленная в набор фигур.
#[derive(Clone, Debug)]
pub struct MorphShape {
    /// Монтажный стол Motif, на котором стоит фигура.
    pub board: String,
    /// Имя фигуры в Motif.
    pub id: String,
    /// Фигура в [`ShapeLibrary`].
    pub shape: ShapeId,
    /// Значение `morph` без анимации.
    pub morph: f32,
    /// Положение на холсте Motif (уже в единицах сцены), если Motif его записал.
    pub transform: Option<Affine>,
    /// Размер фигуры (по всем ключевым контурам) в единицах сцены: ширина и высота.
    pub size: (f64, f64),
    pub animations: Vec<MorphAnimation>,
}

/// Анимация числа `morph`, записанная Motif покадрово.
#[derive(Clone, Debug)]
pub struct MorphAnimation {
    pub id: String,
    /// Название анимации в Motif.
    pub name: String,
    /// Длительность в секундах.
    pub duration: f32,
    /// Повторяется ли по кругу.
    pub looped: bool,
    /// Значение на каждый кадр: значение `i` относится ко времени `i / fps`.
    pub values: Vec<f32>,
    fps: f32,
}

impl MorphAnimation {
    /// Значение `morph` в момент `time` (секунды от начала анимации). Между записанными
    /// значениями смешивается по прямой; повторяющаяся анимация идёт по кругу, остальные
    /// после конца стоят на последнем значении.
    pub fn sample(&self, time: f32) -> f32 {
        let Some(&last) = self.values.last() else {
            return 0.0;
        };
        let duration = self.duration.max(0.0);
        let t = if self.looped && duration > 0.0 {
            time.rem_euclid(duration)
        } else {
            time.clamp(0.0, duration)
        };
        let x = t * self.fps;
        let i = x.floor();
        let k = i as usize;
        if k + 1 >= self.values.len() {
            return last;
        }
        let f = x - i;
        self.values[k] + (self.values[k + 1] - self.values[k]) * f
    }
}

/// Читает файл `*.reyvec.json` и добавляет его фигуры в `library`.
pub fn load_morph_file(data: &[u8], library: &mut ShapeLibrary) -> Result<MorphFile, MotifError> {
    let file: FileJson = serde_json::from_slice(data)?;
    if file.format != "reyvec-morph" {
        return Err(MotifError::Format(file.format));
    }
    if file.version != 1 {
        return Err(MotifError::Version(file.version));
    }
    if file.y != "up" {
        return Err(MotifError::Axis(file.y));
    }
    let mut warnings = Vec::new();
    let unit = match file.units.as_deref() {
        None | Some("px") => 1.0 / PIXELS_PER_UNIT,
        Some(other) => {
            warnings.push(format!(
                "единицы «{other}» не известны движку, считаем пикселями"
            ));
            1.0 / PIXELS_PER_UNIT
        }
    };
    let fps = if file.fps > 0.0 { file.fps } else { 60.0 };
    let scale = Affine::scale(unit);
    let mut shapes = Vec::new();
    for s in file.shapes {
        let name = format!("«{}» ({})", s.id, s.board);
        match shape(&s, scale, library) {
            Ok((shape, size)) => {
                if !s.marks.is_empty() {
                    warnings.push(format!(
                        "{name}: метки соответствия ({}) движок пока не учитывает, сопоставление автоматическое",
                        s.marks.len()
                    ));
                }
                shapes.push(MorphShape {
                    board: s.board,
                    id: s.id,
                    shape,
                    morph: s.morph,
                    transform: s
                        .transform
                        .map(|m| scale * Affine::new(m) * scale.inverse()),
                    size,
                    animations: s
                        .animations
                        .into_iter()
                        .map(|a| MorphAnimation {
                            id: a.id,
                            name: a.animation,
                            duration: a.duration,
                            looped: a.looped,
                            values: a.morph,
                            fps,
                        })
                        .collect(),
                });
            }
            Err(e) => warnings.push(format!("{name} пропущена: {e}")),
        }
    }
    Ok(MorphFile {
        fps,
        shapes,
        warnings,
    })
}

/// Добавляет одну фигуру: контуры в единицах сцены, закраска, перетекание.
fn shape(
    s: &ShapeJson,
    scale: Affine,
    library: &mut ShapeLibrary,
) -> Result<(ShapeId, (f64, f64)), String> {
    let mut contours = Vec::with_capacity(s.contours.len());
    for (k, d) in s.contours.iter().enumerate() {
        let mut path =
            BezPath::from_svg(d).map_err(|e| format!("контур №{k} не разобрать: {e}"))?;
        path.apply_affine(scale);
        contours.push(path);
    }
    let rule = match s.fill_rule.as_deref() {
        None | Some("nonzero") => FillRule::NonZero,
        Some("evenodd") => FillRule::EvenOdd,
        Some(other) => return Err(format!("правило заливки «{other}» не известно")),
    };
    let paint = match &s.gradient {
        Some(g) => Paint::Gradient(gradient(g, scale)?),
        None => {
            let color = match &s.color {
                Some(c) => hex(c).ok_or_else(|| format!("цвет «{c}» не разобрать"))?,
                None => Color::WHITE,
            };
            Paint::Solid(color.with_alpha(color.a * s.alpha.unwrap_or(1.0)))
        }
    };
    let bounds = contours
        .iter()
        .map(kurbo::Shape::bounding_box)
        .reduce(|a, b| a.union(b))
        .unwrap_or_default();
    let id = library
        .add_morph(&contours, rule, paint)
        .map_err(|e| e.to_string())?;
    Ok((id, (bounds.width(), bounds.height())))
}

fn gradient(g: &GradientJson, scale: Affine) -> Result<Gradient, String> {
    let point = |p: [f64; 2]| kurbo::Point::new(p[0], p[1]);
    let mut out = match g.kind.as_str() {
        "linear" => {
            let (Some(start), Some(end)) = (g.start, g.end) else {
                return Err("у линейного градиента нужны start и end".into());
            };
            Gradient::linear(point(start), point(end))
        }
        "radial" => {
            let (Some(center), Some(radius)) = (g.center, g.radius) else {
                return Err("у радиального градиента нужны center и radius".into());
            };
            let mut r = Gradient::radial(point(center), radius);
            if let Some(focus) = g.focus {
                r = r.with_focus(point(focus));
            }
            r
        }
        other => return Err(format!("вид градиента «{other}» не известен")),
    };
    for stop in &g.stops {
        let c = hex(&stop.color).ok_or_else(|| format!("цвет «{}» не разобрать", stop.color))?;
        out = out.stop(stop.at, c.with_alpha(c.a * stop.alpha.unwrap_or(1.0)));
    }
    out = out.with_spread(match g.spread.as_deref() {
        None | Some("pad") => Spread::Pad,
        Some("reflect") => Spread::Reflect,
        Some("repeat") => Spread::Repeat,
        Some(other) => return Err(format!("«{other}» за краями градиента не известно")),
    });
    // Градиент задан в пикселях Motif, как и контуры: переводим в единицы сцены.
    let own = g.transform.map_or(Affine::IDENTITY, Affine::new);
    Ok(out.with_transform(scale * own))
}

/// Цвет sRGB из `#rgb`, `#rrggbb` или `#rrggbbaa`.
fn hex(s: &str) -> Option<Color> {
    let h = s.strip_prefix('#')?;
    if !h.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    let (r, g, b, a) = match h.len() {
        3 => {
            let d = |i: usize| u8::from_str_radix(&h[i..=i], 16).ok().map(|v| v * 17);
            (d(0)?, d(1)?, d(2)?, 255)
        }
        6 => (byte(0)?, byte(2)?, byte(4)?, 255),
        8 => (byte(0)?, byte(2)?, byte(4)?, byte(6)?),
        _ => return None,
    };
    Some(Color::from_srgb8(r, g, b).with_alpha(f32::from(a) / 255.0))
}

#[derive(Deserialize)]
struct FileJson {
    format: String,
    version: u32,
    #[serde(default = "up")]
    y: String,
    #[serde(default)]
    units: Option<String>,
    #[serde(default)]
    fps: f32,
    #[serde(default)]
    shapes: Vec<ShapeJson>,
}

fn up() -> String {
    "up".into()
}

#[derive(Deserialize)]
struct ShapeJson {
    #[serde(default)]
    board: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    fill_rule: Option<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    alpha: Option<f32>,
    #[serde(default)]
    gradient: Option<GradientJson>,
    contours: Vec<String>,
    #[serde(default)]
    morph: f32,
    #[serde(default)]
    transform: Option<[f64; 6]>,
    #[serde(default)]
    marks: Vec<serde_json::Value>,
    #[serde(default)]
    animations: Vec<AnimationJson>,
}

#[derive(Deserialize)]
struct GradientJson {
    kind: String,
    start: Option<[f64; 2]>,
    end: Option<[f64; 2]>,
    center: Option<[f64; 2]>,
    radius: Option<f64>,
    focus: Option<[f64; 2]>,
    #[serde(default)]
    stops: Vec<StopJson>,
    spread: Option<String>,
    transform: Option<[f64; 6]>,
}

#[derive(Deserialize)]
struct StopJson {
    at: f32,
    color: String,
    alpha: Option<f32>,
}

#[derive(Deserialize)]
struct AnimationJson {
    #[serde(default)]
    id: String,
    #[serde(default)]
    animation: String,
    #[serde(default)]
    duration: f32,
    #[serde(default, rename = "loop")]
    looped: bool,
    #[serde(default)]
    morph: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &[u8] = include_bytes!("../../../assets/motif/morph.reyvec.json");

    fn library() -> ShapeLibrary {
        ShapeLibrary::new(0.001)
    }

    #[test]
    fn example_from_motif_loads() {
        let mut lib = library();
        let file = load_morph_file(EXAMPLE, &mut lib).expect("пример читается");
        let ids: Vec<&str> = file.shapes.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["blob", "play", "mouth"]);
        assert_eq!(file.fps, 60.0);
        // Круг → звезда → сердце: три ключевых контура (у фигуры хранятся два, кроме первого).
        assert_eq!(lib.morph_keyframes(file.shapes[0].shape).len(), 2);
        // По всем контурам: ширина у сердца (±75 пикселей), высота от низа звезды до её
        // верха (−60,68…75), в единицах сцены.
        let (w, h) = file.shapes[0].size;
        assert!(
            (w - 1.5).abs() < 1e-6 && (h - 1.356_763).abs() < 1e-6,
            "{w} {h}"
        );
        let a = &file.shapes[0].animations[0];
        assert_eq!(a.values.len(), 241);
        assert!(a.looped);
        // У рта есть метки: движок их пока не учитывает и предупреждает.
        assert_eq!(file.warnings.len(), 1, "{:?}", file.warnings);
        assert!(file.warnings[0].contains("mouth"));
    }

    #[test]
    fn sample_interpolates_loops_and_clamps() {
        let a = MorphAnimation {
            id: String::new(),
            name: String::new(),
            duration: 1.0,
            looped: true,
            values: vec![0.0, 1.0, 2.0],
            fps: 2.0,
        };
        assert_eq!(a.sample(0.0), 0.0);
        assert_eq!(a.sample(0.25), 0.5);
        assert_eq!(a.sample(0.75), 1.5);
        assert_eq!(a.sample(1.25), 0.5);
        let once = MorphAnimation { looped: false, ..a };
        assert_eq!(once.sample(5.0), 2.0);
        assert_eq!(once.sample(-1.0), 0.0);
    }

    #[test]
    fn errors_and_bad_shapes() {
        let mut lib = library();
        let err = |json: &str| load_morph_file(json.as_bytes(), &mut library()).unwrap_err();
        assert!(matches!(
            err(r#"{"format":"other","version":1}"#),
            MotifError::Format(_)
        ));
        assert!(matches!(
            err(r#"{"format":"reyvec-morph","version":2}"#),
            MotifError::Version(2)
        ));
        assert!(matches!(err("{"), MotifError::Json(_)));
        // Плохая фигура пропускается с предупреждением, хорошая читается; градиент и цвет.
        let file = load_morph_file(
            br##"{"format":"reyvec-morph","version":1,"y":"up","fps":30,"shapes":[
                {"id":"bad","contours":["M 0 0 L 10 0 L 0 10 Z"]},
                {"id":"bad2","contours":["M 0 0 L 10 0 L 0 10 Z","M x"]},
                {"id":"grad","fill_rule":"evenodd","contours":["M 0 0 L 100 0 L 0 100 Z","M 0 0 L 50 0 L 0 50 Z"],
                 "gradient":{"kind":"radial","center":[0,0],"radius":50,"focus":[10,0],
                   "stops":[{"at":0,"color":"#fff"},{"at":1,"color":"#00000080","alpha":0.5}],"spread":"reflect"}}
            ]}"##,
            &mut lib,
        )
        .unwrap();
        assert_eq!(file.shapes.len(), 1);
        assert_eq!(file.warnings.len(), 2, "{:?}", file.warnings);
        let shape = lib.shape(file.shapes[0].shape).unwrap();
        let Paint::Gradient(g) = &shape.paint else {
            panic!("ждали градиент")
        };
        assert_eq!(g.spread, Spread::Reflect);
        assert_eq!(g.transform, Affine::scale(0.01));
        assert!((g.stops[1].color.a - 0.251).abs() < 0.01);
        assert_eq!(file.shapes[0].size, (1.0, 1.0));
    }

    #[test]
    fn hex_colors() {
        assert_eq!(hex("#fff"), Some(Color::WHITE));
        assert_eq!(hex("#ffffff"), Some(Color::WHITE));
        assert_eq!(hex("#ffffff00").map(|c| c.a), Some(0.0));
        assert_eq!(hex("fff"), None);
        assert_eq!(hex("#ффф"), None);
    }
}
