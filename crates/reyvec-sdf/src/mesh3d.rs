//! Полигональные модели в сцене «3D» (опыт совмещения, этап 7.6ж): треугольники рядом с
//! формами SDF в одном кадре.
//!
//! Модель рисуется растеризацией (проход «полигоны 3D») в тот же буфер геометрии, что и
//! лучи, поэтому свет, тени и затенение в углах для неё те же. Сама модель в поле
//! расстояний не участвует: тень от неё бросает невидимый «заменитель» — простая форма
//! SDF чуть внутри модели ([`Mesh3::shadow_proxy`]). Так делает Unreal: у полигональной
//! модели есть своё поле расстояний только для теней и затенения.
//!
//! Пока модели — простые выпуклые многогранники, собранные кодом (настоящие модели из
//! файлов glTF — этап 8). Грани плоские: нормаль пиксельный шейдер берёт из наклона
//! соседних точек, вершинам нормали не нужны.

use glam::Vec3;
use serde::Deserialize;

use crate::shape3d::Shape3;

/// Многогранник с плоскими гранями. Середина — в начале его координат.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub enum Mesh3 {
    /// Шар из треугольников: икосаэдр, грани которого `detail` раз поделены на 4
    /// (0 — 20 граней, 1 — 80, 2 — 320, больше 3 не бывает).
    Icosphere {
        radius: f32,
        #[serde(default)]
        detail: u32,
    },
    /// Коробка: ширина, высота, глубина.
    Block { size: [f32; 3] },
    /// Кристалл: две пирамиды основаниями друг к другу, `sides` граней по кругу.
    Crystal {
        radius: f32,
        height: f32,
        sides: u32,
    },
}

/// Больше граней на модель не бывает: модели собираются на процессоре каждый кадр.
pub const MAX_TRIANGLES: usize = 4096;
/// Заменитель с плоскостями граней — пока граней не больше этого, иначе шар внутри.
const MAX_PROXY_PLANES: usize = 32;

impl Mesh3 {
    /// Треугольники модели в её координатах (обход против часовой стрелки снаружи).
    pub fn triangles(&self) -> Vec<[Vec3; 3]> {
        match *self {
            Self::Icosphere { radius, detail } => icosphere(radius, detail.min(3)),
            Self::Block { size } => block(Vec3::from(size) * 0.5),
            Self::Crystal {
                radius,
                height,
                sides,
            } => crystal(radius, height * 0.5, sides.clamp(3, 24)),
        }
    }

    /// Невидимая форма SDF чуть внутри модели: бросает за неё тень и затеняет углы.
    /// Внутри, а не впритык, — чтобы луч тени с поверхности самой модели не начинался в
    /// заменителе и модель не затеняла сама себя. Пока плоскостей граней немного —
    /// пересечение полупространств (точная выпуклая форма), иначе шар внутри.
    pub fn shadow_proxy(&self) -> Shape3 {
        let triangles = self.triangles();
        let mut planes: Vec<(Vec3, f32)> = Vec::new();
        let mut outer = 0.0f32;
        for t in &triangles {
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
            if n == Vec3::ZERO {
                continue;
            }
            let d = n.dot(t[0]);
            if !planes
                .iter()
                .any(|(m, e)| m.dot(n) > 0.9999 && (e - d).abs() < 1e-4)
            {
                planes.push((n, d));
            }
            outer = t.iter().fold(outer, |o, v| o.max(v.length()));
        }
        let inner = planes.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
        let inset = 0.03 * outer;
        if planes.len() > MAX_PROXY_PLANES || !inner.is_finite() {
            return Shape3::Sphere {
                radius: (inner - inset).max(0.0),
            };
        }
        // Шар снаружи — граница для отсева (у плоскостей её нет), форму он не меняет.
        let mut shapes = vec![Shape3::Sphere { radius: outer }];
        shapes.extend(planes.iter().map(|&(n, d)| Shape3::Plane {
            normal: n.to_array(),
            offset: d - inset,
        }));
        Shape3::Intersect {
            shapes,
            smooth: 0.0,
        }
    }
}

fn icosphere(radius: f32, detail: u32) -> Vec<[Vec3; 3]> {
    let g = (1.0 + 5f32.sqrt()) * 0.5;
    let v = [
        Vec3::new(-1.0, g, 0.0),
        Vec3::new(1.0, g, 0.0),
        Vec3::new(-1.0, -g, 0.0),
        Vec3::new(1.0, -g, 0.0),
        Vec3::new(0.0, -1.0, g),
        Vec3::new(0.0, 1.0, g),
        Vec3::new(0.0, -1.0, -g),
        Vec3::new(0.0, 1.0, -g),
        Vec3::new(g, 0.0, -1.0),
        Vec3::new(g, 0.0, 1.0),
        Vec3::new(-g, 0.0, -1.0),
        Vec3::new(-g, 0.0, 1.0),
    ]
    .map(|p| p.normalize());
    const FACES: [[usize; 3]; 20] = [
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    let mut tris: Vec<[Vec3; 3]> = FACES.iter().map(|f| f.map(|i| v[i])).collect();
    for _ in 0..detail {
        tris = tris
            .iter()
            .flat_map(|&[a, b, c]| {
                let (ab, bc, ca) = (
                    (a + b).normalize(),
                    (b + c).normalize(),
                    (c + a).normalize(),
                );
                [[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]
            })
            .collect();
    }
    tris.iter().map(|t| t.map(|p| p * radius)).collect()
}

fn block(h: Vec3) -> Vec<[Vec3; 3]> {
    let mut out = Vec::new();
    // Каждая грань: ось, знак; два треугольника против часовой стрелки снаружи.
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let n = Vec3::AXES[axis] * sign;
            let u = Vec3::AXES[(axis + 1) % 3];
            let w = Vec3::AXES[(axis + 2) % 3];
            let (u, w) = if sign > 0.0 { (u, w) } else { (w, u) };
            let corner = |a: f32, b: f32| (n + u * a + w * b) * h;
            let q = [
                corner(-1.0, -1.0),
                corner(1.0, -1.0),
                corner(1.0, 1.0),
                corner(-1.0, 1.0),
            ];
            out.push([q[0], q[1], q[2]]);
            out.push([q[0], q[2], q[3]]);
        }
    }
    out
}

fn crystal(radius: f32, half: f32, sides: u32) -> Vec<[Vec3; 3]> {
    let (top, bottom) = (Vec3::Y * half, -Vec3::Y * half);
    let ring = |i: u32| {
        let a = std::f32::consts::TAU * i as f32 / sides as f32;
        Vec3::new(a.cos() * radius, 0.0, -a.sin() * radius)
    };
    let mut out = Vec::new();
    for i in 0..sides {
        let (a, b) = (ring(i), ring(i + 1));
        out.push([a, b, top]);
        out.push([b, a, bottom]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape3d::compile;

    fn kinds() -> Vec<Mesh3> {
        vec![
            Mesh3::Icosphere {
                radius: 0.8,
                detail: 0,
            },
            Mesh3::Icosphere {
                radius: 0.8,
                detail: 2,
            },
            Mesh3::Block {
                size: [1.0, 0.5, 2.0],
            },
            Mesh3::Crystal {
                radius: 0.4,
                height: 1.6,
                sides: 6,
            },
        ]
    }

    /// Грани смотрят наружу: нормаль по обходу направлена от середины.
    #[test]
    fn faces_point_outward() {
        for m in kinds() {
            for t in m.triangles() {
                let n = (t[1] - t[0]).cross(t[2] - t[0]);
                let mid = (t[0] + t[1] + t[2]) / 3.0;
                assert!(n.dot(mid) > 0.0, "{m:?}: грань внутрь");
            }
        }
    }

    /// Заменитель лежит внутри модели: вершины модели снаружи заменителя, а середина
    /// внутри. Иначе модель затеняла бы сама себя.
    #[test]
    fn proxy_is_inside_the_mesh() {
        for m in kinds() {
            let proxy = compile(&m.shadow_proxy()).unwrap();
            assert!(proxy.bounds.is_some(), "{m:?}: заменитель без границы");
            assert!(proxy.distance(Vec3::ZERO).0 < 0.0, "{m:?}");
            for t in m.triangles() {
                let mid = (t[0] + t[1] + t[2]) / 3.0;
                for p in [t[0], t[1], t[2], mid] {
                    assert!(proxy.distance(p).0 > 0.0, "{m:?}: {p} внутри заменителя");
                }
            }
        }
    }

    #[test]
    fn triangle_counts() {
        let counts: Vec<usize> = kinds().iter().map(|m| m.triangles().len()).collect();
        assert_eq!(counts, vec![20, 320, 12, 12]);
    }
}
