//! Модели из файлов glTF (этап 8.14): `.glb` (всё в одном файле) или `.gltf` с файлом
//! `.bin` рядом.
//!
//! Из файла берутся треугольники всех узлов сцены (с их сдвигами, поворотами и
//! масштабами) и цвет каждого материала (`baseColorFactor`). Текстуры, гладкие нормали,
//! анимация и кости пока не читаются: модель рисуется плоскими гранями цветом материала.
//!
//! Тень модели бросают невидимые заменители SDF ([`convex_proxies`]): модель делится на
//! связные куски, и каждый выпуклый кусок получает свой заменитель — пересечение
//! плоскостей его граней. У невыпуклых кусков тени пока нет: тень по настоящей форме
//! даст «кирпич поля» (задача 8.15).

use std::collections::HashMap;
use std::path::Path;

use glam::{Mat4, Vec3};

use crate::mesh3d::convex_proxy;
use crate::shape3d::Shape3;

/// Часть модели одного материала.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelPart {
    /// Имя материала в файле (или «материал N», если имени нет).
    pub material: String,
    /// Цвет материала (линейный, как в glTF).
    pub color: [f32; 3],
    /// Есть ли у материала текстура цвета (пока не рисуется).
    pub textured: bool,
    /// Треугольники в координатах модели.
    pub triangles: Vec<[Vec3; 3]>,
}

/// Модель из файла: части по материалам.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub parts: Vec<ModelPart>,
}

impl Model {
    /// Всего треугольников.
    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|p| p.triangles.len()).sum()
    }
}

/// Читает модель из файла. `scale` умножает все размеры.
pub fn load(path: &Path, scale: f32) -> Result<Model, String> {
    let data = std::fs::read(path).map_err(|e| format!("файл {}: {e}", path.display()))?;
    from_bytes(&data, path.parent(), scale).map_err(|e| format!("файл {}: {e}", path.display()))
}

/// Читает модель из содержимого файла; `dir` — откуда брать файлы `.bin` для `.gltf`.
pub fn from_bytes(data: &[u8], dir: Option<&Path>, scale: f32) -> Result<Model, String> {
    let gltf = gltf::Gltf::from_slice(data).map_err(|e| format!("не glTF: {e}"))?;
    let mut buffers = Vec::new();
    for buffer in gltf.document.buffers() {
        let bytes = match buffer.source() {
            gltf::buffer::Source::Bin => gltf
                .blob
                .clone()
                .ok_or("в файле нет двоичного раздела, на который он ссылается")?,
            gltf::buffer::Source::Uri(uri) if uri.starts_with("data:") => {
                return Err(
                    "данные вписаны в текст (data:), так движок не читает: сохрани модель как .glb"
                        .into(),
                );
            }
            gltf::buffer::Source::Uri(uri) => {
                let path = dir.unwrap_or(Path::new(".")).join(uri);
                std::fs::read(&path).map_err(|e| format!("файл {}: {e}", path.display()))?
            }
        };
        if bytes.len() < buffer.length() {
            return Err(format!(
                "буфер {} короче, чем записано: {} байт из {}",
                buffer.index(),
                bytes.len(),
                buffer.length()
            ));
        }
        buffers.push(bytes);
    }
    let scene = gltf
        .document
        .default_scene()
        .or_else(|| gltf.document.scenes().next())
        .ok_or("в файле нет сцены")?;
    // Части по номеру материала (None — материал по умолчанию), в порядке появления.
    let mut parts: Vec<(Option<usize>, ModelPart)> = Vec::new();
    let mut stack: Vec<(gltf::Node, Mat4)> = scene
        .nodes()
        .map(|n| (n, Mat4::from_scale(Vec3::splat(scale))))
        .collect();
    while let Some((node, parent)) = stack.pop() {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        stack.extend(node.children().map(|c| (c, world)));
        let Some(mesh) = node.mesh() else { continue };
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = primitive.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
            let Some(positions) = reader.read_positions() else {
                continue;
            };
            let points: Vec<Vec3> = positions
                .map(|p| world.transform_point3(Vec3::from(p)))
                .collect();
            let indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..points.len() as u32).collect(),
            };
            let material = primitive.material();
            let key = material.index();
            let at = match parts.iter().position(|(k, _)| *k == key) {
                Some(at) => at,
                None => {
                    let pbr = material.pbr_metallic_roughness();
                    let c = pbr.base_color_factor();
                    parts.push((
                        key,
                        ModelPart {
                            material: material.name().map_or_else(
                                || match key {
                                    Some(i) => format!("материал {}", i + 1),
                                    None => "без материала".into(),
                                },
                                str::to_owned,
                            ),
                            color: [c[0], c[1], c[2]],
                            textured: pbr.base_color_texture().is_some(),
                            triangles: Vec::new(),
                        },
                    ));
                    parts.len() - 1
                }
            };
            let triangles = &mut parts[at].1.triangles;
            for t in indices.chunks_exact(3) {
                let v = [t[0], t[1], t[2]].map(|i| points.get(i as usize).copied());
                let [Some(a), Some(b), Some(c)] = v else {
                    return Err(format!(
                        "модель «{}»: номер вершины за концом списка",
                        mesh.name().unwrap_or("без имени")
                    ));
                };
                triangles.push([a, b, c]);
            }
        }
    }
    parts.retain(|(_, p)| !p.triangles.is_empty());
    if parts.is_empty() {
        return Err("в файле нет треугольников".into());
    }
    Ok(Model {
        parts: parts.into_iter().map(|(_, p)| p).collect(),
    })
}

/// Связные куски модели: треугольники с общими вершинами (по месту) — один кусок.
pub fn pieces(triangles: &[[Vec3; 3]]) -> Vec<Vec<[Vec3; 3]>> {
    let key = |v: Vec3| (v * 1.0e4).round().as_ivec3().to_array();
    let mut parent: Vec<usize> = (0..triangles.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut first: HashMap<[i32; 3], usize> = HashMap::new();
    for (i, t) in triangles.iter().enumerate() {
        for v in t {
            let j = *first.entry(key(*v)).or_insert(i);
            let (a, b) = (root(&mut parent, i), root(&mut parent, j));
            if a != b {
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut groups: Vec<Vec<[Vec3; 3]>> = Vec::new();
    let mut group_of: HashMap<usize, usize> = HashMap::new();
    for (i, t) in triangles.iter().enumerate() {
        let r = root(&mut parent, i);
        let g = *group_of.entry(r).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(*t);
    }
    groups
}

/// Выпуклый ли кусок: все его вершины не снаружи плоскости ни одной его грани.
fn is_convex(triangles: &[[Vec3; 3]], center: Vec3) -> bool {
    let size = triangles
        .iter()
        .flatten()
        .fold(0.0f32, |m, v| m.max((*v - center).length()));
    let eps = 1.0e-3 * size.max(1.0e-6);
    triangles.iter().all(|t| {
        let mut n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
        if n == Vec3::ZERO {
            return true;
        }
        if n.dot(t[0] - center) < 0.0 {
            n = -n;
        }
        triangles.iter().flatten().all(|v| n.dot(*v - t[0]) <= eps)
    })
}

/// Больше заменителей на модель не бывает: каждый — объект в списке сцены.
pub const MAX_PROXIES: usize = 32;

/// Заменители для теней: по одному на выпуклый кусок (в координатах модели, до
/// [`MAX_PROXIES`]) и число кусков без тени (невыпуклых, плоских или лишних).
pub fn convex_proxies(triangles: &[[Vec3; 3]]) -> (Vec<Shape3>, usize) {
    let mut proxies = Vec::new();
    let mut skipped = 0;
    for piece in pieces(triangles) {
        let (lo, hi) = piece.iter().flatten().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(lo, hi), v| (lo.min(*v), hi.max(*v)),
        );
        let center = (lo + hi) * 0.5;
        let proxy = (proxies.len() < MAX_PROXIES && is_convex(&piece, center))
            .then(|| {
                let local: Vec<[Vec3; 3]> = piece.iter().map(|t| t.map(|v| v - center)).collect();
                convex_proxy(&local)
            })
            .flatten();
        match proxy {
            Some(p) => proxies.push(p.at(center.to_array())),
            None => skipped += 1,
        }
    }
    (proxies, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> Model {
        let path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scenes/3d/models/machine.glb"
        ));
        load(path, 1.0).unwrap()
    }

    #[test]
    fn machine_parts_by_material() {
        let m = machine();
        let names: Vec<&str> = m.parts.iter().map(|p| p.material.as_str()).collect();
        assert_eq!(names.len(), 4, "{names:?}");
        for name in ["станина", "корпус", "бак", "труба"] {
            assert!(names.contains(&name), "{names:?}");
        }
        // Станина 12 + дымоход 12 (без номеров вершин), корпус 12, два бака по 64,
        // труба 48.
        let count = |name: &str| {
            m.parts
                .iter()
                .find(|p| p.material == name)
                .unwrap()
                .triangles
                .len()
        };
        assert_eq!(count("станина"), 24);
        assert_eq!(count("корпус"), 12);
        assert_eq!(count("бак"), 128);
        assert_eq!(count("труба"), 48);
        let body = m.parts.iter().find(|p| p.material == "корпус").unwrap();
        assert!((body.color[0] - 0.80).abs() < 1e-6);
    }

    #[test]
    fn node_transforms_are_applied() {
        let m = machine();
        let bounds = |name: &str| {
            m.parts
                .iter()
                .find(|p| p.material == name)
                .unwrap()
                .triangles
                .iter()
                .flatten()
                .fold(
                    (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
                    |(lo, hi), v| (lo.min(*v), hi.max(*v)),
                )
        };
        // Корпус 0.9×0.8×0.7 сдвинут в (-0.25, 0.6, 0).
        let (lo, hi) = bounds("корпус");
        assert!((lo - Vec3::new(-0.7, 0.2, -0.35)).abs().max_element() < 1e-5);
        assert!((hi - Vec3::new(0.2, 1.0, 0.35)).abs().max_element() < 1e-5);
        // Труба (цилиндр высотой 0.62 вдоль Y) повёрнута на 90° вокруг Z: лежит вдоль X.
        let (lo, hi) = bounds("труба");
        assert!((hi.x - lo.x - 0.62).abs() < 1e-4, "{lo} {hi}");
        assert!((hi.y - lo.y - 0.12).abs() < 1e-4, "{lo} {hi}");
        // Масштаб всей модели.
        let big = load(
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scenes/3d/models/machine.glb"
            )),
            2.0,
        )
        .unwrap();
        let body = big.parts.iter().find(|p| p.material == "корпус").unwrap();
        let top = body
            .triangles
            .iter()
            .flatten()
            .map(|v| v.y)
            .fold(0.0, f32::max);
        assert!((top - 2.0).abs() < 1e-5);
    }

    #[test]
    fn every_machine_piece_casts_shadow() {
        let m = machine();
        let all: Vec<[Vec3; 3]> = m.parts.iter().flat_map(|p| p.triangles.clone()).collect();
        // Станина, корпус, два бака, труба, дымоход.
        assert_eq!(pieces(&all).len(), 6);
        let (proxies, skipped) = convex_proxies(&all);
        assert_eq!((proxies.len(), skipped), (6, 0));
        // Заменитель внутри куска: середина корпуса внутри, точка над ним — снаружи.
        let inside = |p: Vec3| proxies.iter().any(|s| s.distance(p) < 0.0);
        assert!(inside(Vec3::new(-0.25, 0.6, 0.0)));
        assert!(!inside(Vec3::new(-0.25, 1.05, 0.0)));
    }

    #[test]
    fn concave_piece_gets_no_shadow() {
        // Буква «Г» из двух коробок с общей гранью — невыпуклый кусок.
        let block = |at: Vec3, h: Vec3| {
            crate::mesh3d::Mesh3::Block {
                size: (h * 2.0).to_array(),
            }
            .triangles()
            .into_iter()
            .map(move |t| t.map(|v| v + at))
        };
        let mut tris: Vec<[Vec3; 3]> = block(Vec3::ZERO, Vec3::splat(0.5)).collect();
        tris.extend(block(Vec3::new(1.0, 0.0, 0.0), Vec3::splat(0.5)));
        tris.extend(block(Vec3::new(0.0, 1.0, 0.0), Vec3::splat(0.5)));
        assert_eq!(pieces(&tris).len(), 1);
        assert_eq!(convex_proxies(&tris), (Vec::new(), 1));
    }

    #[test]
    fn broken_files_say_what_is_wrong() {
        assert!(
            from_bytes(b"not a model", None, 1.0)
                .unwrap_err()
                .contains("не glTF")
        );
        let missing = load(Path::new("нет/такой.glb"), 1.0).unwrap_err();
        assert!(missing.contains("такой.glb"), "{missing}");
        let embedded = r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4,"uri":"data:application/octet-stream;base64,AAAAAA=="}]}"#;
        let e = from_bytes(embedded.as_bytes(), None, 1.0).unwrap_err();
        assert!(e.contains(".glb"), "{e}");
    }
}
