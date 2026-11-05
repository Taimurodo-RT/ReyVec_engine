//! Плоский контур для объёмных форм (этап 7.7): 2D-фигура, которую можно выдавить в
//! толщину ([`crate::Shape3::Extrude`]) или повернуть вокруг оси
//! ([`crate::Shape3::Revolve`]).
//!
//! Контур задаётся как в SVG (`M 0 0 L 1 0 Q 1 1 0 1 Z`), только в метрах и с Y вверх.
//! Один раз, при чтении сцены, контур превращается в сетку расстояний («поле фигуры»,
//! не больше 64×64 чисел): в каждой точке сетки — точное расстояние до края, внутри со
//! знаком минус. Видеокарта потом читает четыре соседних числа и смешивает их, а не
//! перебирает отрезки контура на каждом шаге луча. Цена — мелочь меньше клетки сетки
//! скругляется (у фигуры шириной 2 м клетка около 3 см).
//!
//! Раскладка слов поля: левый нижний угол сетки (x, y), шаг клетки, число точек по X в
//! младших 16 битах и по Y в старших, прямоугольник фигуры (левый нижний и правый
//! верхний углы, 4 числа), дальше значения по строкам снизу вверх. Точка
//! сетки `(i, j)` стоит в `угол + (i + ½, j + ½) · шаг`. Шейдер `include/sdf3d.slang`
//! (`sd2_field`) читает те же слова той же формулой, что [`field_distance`].

use std::sync::Arc;

use glam::Vec2;
use kurbo::{BezPath, PathEl, Shape};

/// Больше точек сетки по длинной стороне не бывает.
pub const FIELD_SIDE: u32 = 64;
/// Слов в заголовке поля.
pub const FIELD_HEADER_WORDS: usize = 8;
/// Пустых клеток вокруг фигуры с каждой стороны: снаружи сетки расстояние считается
/// от её края, и фигура не должна его касаться.
const MARGIN_CELLS: f32 = 2.0;

/// Контур и его запечённое поле. В файле сцены — строка с контуром.
#[derive(Clone)]
pub struct Profile {
    path: String,
    field: Arc<Field2>,
}

/// Запечённое поле фигуры.
struct Field2 {
    /// Заголовок и значения — ровно те слова, что уходят видеокарте.
    words: Vec<u32>,
    /// Прямоугольник самой фигуры (без запаса).
    lo: Vec2,
    hi: Vec2,
}

/// Почему контур не подошёл.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("контур не разобран: {0}")]
    Syntax(String),
    #[error("в контуре нет площади: нужна замкнутая фигура хотя бы из трёх точек")]
    Empty,
}

impl Profile {
    /// Разбирает контур и запекает его поле.
    pub fn new(path: &str) -> Result<Self, ProfileError> {
        let bez = BezPath::from_svg(path).map_err(|e| ProfileError::Syntax(e.to_string()))?;
        let contours = flatten(&bez);
        let points = contours.iter().flatten();
        let (lo, hi) = points.fold(
            (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)),
            |(lo, hi), &p| (lo.min(p), hi.max(p)),
        );
        let size = hi - lo;
        if contours.is_empty() || size.min_element() <= 0.0 {
            return Err(ProfileError::Empty);
        }
        Ok(Self {
            path: path.to_owned(),
            field: Arc::new(bake(&contours, lo, hi)),
        })
    }

    /// Контур, как он записан.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Слова поля для программы формы.
    pub fn words(&self) -> &[u32] {
        &self.field.words
    }

    /// Прямоугольник фигуры: левый нижний и правый верхний углы.
    pub fn bounds(&self) -> (Vec2, Vec2) {
        (self.field.lo, self.field.hi)
    }

    /// Расстояние до края фигуры по полю (как его считает видеокарта).
    pub fn distance(&self, p: Vec2) -> f32 {
        field_distance(&self.field.words, p)
    }
}

impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Profile({:?})", self.path)
    }
}

impl PartialEq for Profile {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

impl TryFrom<String> for Profile {
    type Error = ProfileError;
    fn try_from(path: String) -> Result<Self, Self::Error> {
        Self::new(&path)
    }
}

impl From<Profile> for String {
    fn from(p: Profile) -> Self {
        p.path
    }
}

impl serde::Serialize for Profile {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.path)
    }
}

impl<'de> serde::Deserialize<'de> for Profile {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let path = String::deserialize(d)?;
        Self::new(&path).map_err(serde::de::Error::custom)
    }
}

/// Контур → замкнутые ломаные (кривые разбиты на отрезки).
fn flatten(bez: &BezPath) -> Vec<Vec<Vec2>> {
    let size = bez.bounding_box().size();
    let tolerance = (size.width.max(size.height) * 1e-3).max(1e-6);
    let mut contours: Vec<Vec<Vec2>> = Vec::new();
    let mut current: Vec<Vec2> = Vec::new();
    let finish = |c: &mut Vec<Vec2>, all: &mut Vec<Vec<Vec2>>| {
        if c.len() >= 3 {
            all.push(std::mem::take(c));
        } else {
            c.clear();
        }
    };
    kurbo::flatten(bez, tolerance, |el| match el {
        PathEl::MoveTo(p) => {
            finish(&mut current, &mut contours);
            current.push(Vec2::new(p.x as f32, p.y as f32));
        }
        PathEl::LineTo(p) => current.push(Vec2::new(p.x as f32, p.y as f32)),
        PathEl::ClosePath => finish(&mut current, &mut contours),
        _ => {}
    });
    finish(&mut current, &mut contours);
    contours
}

/// Точное поле по ломаным: расстояние до ближайшего отрезка, знак — по числу обходов
/// (правило «ненулевое», как у заливки SVG).
fn bake(contours: &[Vec<Vec2>], lo: Vec2, hi: Vec2) -> Field2 {
    let size = hi - lo;
    let cell = size.max_element() / (FIELD_SIDE as f32 - 2.0 * MARGIN_CELLS);
    let count = |s: f32| ((s / cell).ceil() as u32 + 2 * MARGIN_CELLS as u32).min(FIELD_SIDE);
    let (nx, ny) = (count(size.x), count(size.y));
    // Сетка по середине фигуры.
    let corner = (lo + hi) * 0.5 - Vec2::new(nx as f32, ny as f32) * cell * 0.5;
    let mut words = vec![
        corner.x.to_bits(),
        corner.y.to_bits(),
        cell.to_bits(),
        nx | (ny << 16),
        lo.x.to_bits(),
        lo.y.to_bits(),
        hi.x.to_bits(),
        hi.y.to_bits(),
    ];
    for j in 0..ny {
        for i in 0..nx {
            let p = corner + (Vec2::new(i as f32, j as f32) + 0.5) * cell;
            words.push(exact(contours, p).to_bits());
        }
    }
    Field2 { words, lo, hi }
}

/// Расстояние со знаком от точки до фигуры из замкнутых ломаных.
fn exact(contours: &[Vec<Vec2>], p: Vec2) -> f32 {
    let mut best = f32::INFINITY;
    let mut winding = 0i32;
    for c in contours {
        for (k, &a) in c.iter().enumerate() {
            let b = c[(k + 1) % c.len()];
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-20)).clamp(0.0, 1.0);
            best = best.min((p - a - ab * t).length_squared());
            // Пересечение луча вправо от точки с отрезком.
            if (a.y <= p.y) != (b.y <= p.y) {
                let side = ab.perp_dot(p - a);
                if b.y > a.y && side > 0.0 {
                    winding += 1;
                } else if b.y <= a.y && side < 0.0 {
                    winding -= 1;
                }
            }
        }
    }
    let d = best.sqrt();
    if winding != 0 { -d } else { d }
}

/// Расстояние по запечённому полю (слова — с заголовком). Внутри сетки — смесь четырёх
/// соседних точек; снаружи — от ближайшей точки края сетки: фигура целиком внутри
/// сетки, поэтому настоящее расстояние не меньше `√(e² + v²)`, где `e` — до края,
/// `v` — значение на краю, и не меньше расстояния до прямоугольника фигуры.
pub fn field_distance(words: &[u32], p: Vec2) -> f32 {
    let f = |i: usize| f32::from_bits(words[i]);
    let corner = Vec2::new(f(0), f(1));
    let cell = f(2);
    let nx = words[3] & 0xffff;
    let ny = words[3] >> 16;
    let g = (p - corner) / cell - 0.5;
    let top = Vec2::new(nx as f32 - 1.0, ny as f32 - 1.0);
    let gc = g.clamp(Vec2::ZERO, top);
    let e = (g - gc).length() * cell;
    let i0 = gc.floor().min(top - 1.0);
    let t = gc - i0;
    let base = FIELD_HEADER_WORDS + i0.y as usize * nx as usize + i0.x as usize;
    let row = nx as usize;
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let v = lerp(
        lerp(f(base), f(base + 1), t.x),
        lerp(f(base + row), f(base + row + 1), t.x),
        t.y,
    );
    if e > 0.0 {
        let (lo, hi) = (Vec2::new(f(4), f(5)), Vec2::new(f(6), f(7)));
        let to_box = (lo - p).max(p - hi).max(Vec2::ZERO).length();
        (e * e + v.max(0.0) * v.max(0.0)).sqrt().max(to_box)
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAR: &str = "M 0 1 L 0.22 0.3 L 0.95 0.31 L 0.36 -0.12 L 0.59 -0.81 \
                        L 0 -0.38 L -0.59 -0.81 L -0.36 -0.12 L -0.95 0.31 L -0.22 0.3 Z";

    #[test]
    fn square_field_matches_exact_distance() {
        let p = Profile::new("M -1 -1 L 1 -1 L 1 1 L -1 1 Z").unwrap();
        for (q, want) in [
            (Vec2::ZERO, -1.0),
            (Vec2::new(0.5, 0.0), -0.5),
            (Vec2::new(1.5, 0.0), 0.5),
            (Vec2::new(5.0, 0.0), 4.0),
            (Vec2::new(4.0, 5.0), 5.0),
        ] {
            let d = p.distance(q);
            // Далеко за сеткой поле — оценка снизу (чуть меньше), в глубине фигуры
            // смешивание чуть сглаживает гребень посередине.
            assert!(
                d <= want + 0.02 && d > want * 0.97 - 0.02,
                "{q}: {d} вместо {want}"
            );
        }
    }

    /// Поле по сетке почти не врёт рядом с краем (где луч останавливается) и снаружи
    /// фигуры (где луч шагает) не завышает расстояние больше чем на полклетки: так бывает
    /// только у острых углов, где смешивание соседних точек их чуть срезает.
    #[test]
    fn star_field_is_close_and_not_over() {
        let p = Profile::new(STAR).unwrap();
        let contours = flatten(&BezPath::from_svg(STAR).unwrap());
        let cell = f32::from_bits(p.words()[2]);
        for j in 0..120 {
            for i in 0..120 {
                let q = Vec2::new(i as f32, j as f32) / 40.0 - 1.5;
                let (got, want) = (p.distance(q), exact(&contours, q));
                if want > 0.0 {
                    assert!(got <= want + 0.5 * cell, "{q}: {got} больше {want}");
                }
                if want.abs() < 2.0 * cell {
                    assert!((got - want).abs() < 0.75 * cell, "{q}: {got} вместо {want}");
                }
            }
        }
    }

    #[test]
    fn curves_holes_and_errors() {
        // Кольцо: внешний круг и дыра, обход дыры в ту же сторону — правило «ненулевое»
        // дало бы заливку, поэтому дыра обходится в обратную сторону.
        let ring = Profile::new(
            "M 1 0 A 1 1 0 1 1 -1 0 A 1 1 0 1 1 1 0 Z \
             M 0.5 0 A 0.5 0.5 0 1 0 -0.5 0 A 0.5 0.5 0 1 0 0.5 0 Z",
        )
        .unwrap();
        assert!(ring.distance(Vec2::ZERO) > 0.4, "дыра залита");
        assert!(ring.distance(Vec2::new(0.75, 0.0)) < -0.2);
        let (lo, hi) = ring.bounds();
        assert!((lo + Vec2::ONE).length() < 0.01 && (hi - Vec2::ONE).length() < 0.01);
        assert!(ring.words().len() <= FIELD_HEADER_WORDS + (FIELD_SIDE * FIELD_SIDE) as usize);
        assert!(matches!(
            Profile::new("M 0 0 L 1 0"),
            Err(ProfileError::Empty)
        ));
        assert!(matches!(
            Profile::new("M 0 0 X"),
            Err(ProfileError::Syntax(_))
        ));
    }
}
