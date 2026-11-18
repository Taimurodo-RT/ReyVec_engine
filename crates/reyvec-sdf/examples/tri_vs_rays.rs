// Scratch: CPU render of shapes by SDF vs by meshes (not committed).
use glam::Vec3;
use reyvec_sdf::shape_mesh::{MeshSettings, mesh_shapes};
use reyvec_sdf::shape3d::compile;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let text = std::fs::read_to_string(&args[1]).unwrap();
    let scene =
        reyvec_sdf::SpaceScene::parse_in(&text, Some(std::path::Path::new(&args[2]))).unwrap();
    let frame = scene.frame(2.0);
    let cam = reyvec_sdf::Camera3D::looking_at(Vec3::new(4.9, 0.9, 3.3), Vec3::new(3.4, 0.6, 2.6));
    let (w, h) = (640usize, 360usize);
    let aspect = w as f32 / h as f32;
    let env = |k: &str, d: f32| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let settings = MeshSettings {
        max_cell: env("CELL", 0.05),
        max_error: env("ERR", 0.004),
        ..MeshSettings::default()
    };
    let meshes = mesh_shapes(&frame.shapes, &settings);
    let progs: Vec<_> = frame.shapes.iter().map(|s| compile(s).ok()).collect();
    // World-space triangles with normals.
    let mut tris: Vec<([Vec3; 3], [Vec3; 3])> = Vec::new();
    let mut used = vec![false; frame.shapes.len()];
    for (i, inst) in meshes.instances.iter().enumerate() {
        let Some(inst) = inst else { continue };
        used[i] = true;
        let m = &inst.mesh;
        for t in m.indices.chunks_exact(3) {
            let p = [0, 1, 2].map(|k| inst.place.apply(m.positions[t[k] as usize]));
            let n = [0, 1, 2].map(|k| (inst.place.rot * m.normals[t[k] as usize]).normalize());
            tris.push((p, n));
        }
        eprintln!("shape {i}: cell {:.4} tris {}", m.cell, m.triangle_count());
    }
    let light = Vec3::new(0.4, 0.8, 0.3).normalize();
    let shade = |n: Vec3| (0.25 + 0.75 * n.dot(light).max(0.0)).min(1.0);
    let mut a = vec![0u8; w * h];
    let mut b = vec![0u8; w * h];
    let sdf = |p: Vec3| {
        progs
            .iter()
            .zip(&used)
            .filter(|(_, u)| **u)
            .filter_map(|(p2, _)| p2.as_ref())
            .map(|pr| pr.distance(p).0)
            .fold(f32::MAX, f32::min)
    };
    for y in 0..h {
        for x in 0..w {
            let s = [
                (x as f32 + 0.5) / w as f32 * 2.0 - 1.0,
                1.0 - (y as f32 + 0.5) / h as f32 * 2.0,
            ];
            let (o, d) = cam.ray(s, aspect);
            // SDF
            let mut t = 0.0;
            for _ in 0..300 {
                let dist = sdf(o + d * t);
                if dist < 0.0002 {
                    let p = o + d * t;
                    let e = 0.0005;
                    let n = Vec3::new(
                        sdf(p + Vec3::X * e) - sdf(p - Vec3::X * e),
                        sdf(p + Vec3::Y * e) - sdf(p - Vec3::Y * e),
                        sdf(p + Vec3::Z * e) - sdf(p - Vec3::Z * e),
                    )
                    .normalize();
                    a[y * w + x] = (shade(n) * 255.0) as u8;
                    break;
                }
                t += dist;
                if t > 30.0 {
                    break;
                }
            }
            // Mesh
            let mut best = f32::MAX;
            let mut nn = Vec3::ZERO;
            for (p, n) in &tris {
                let e1 = p[1] - p[0];
                let e2 = p[2] - p[0];
                let q = d.cross(e2);
                let det = e1.dot(q);
                if det.abs() < 1e-12 {
                    continue;
                }
                let inv = 1.0 / det;
                let s0 = o - p[0];
                let u = s0.dot(q) * inv;
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let r = s0.cross(e1);
                let v = d.dot(r) * inv;
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let tt = e2.dot(r) * inv;
                if tt > 0.0 && tt < best {
                    best = tt;
                    nn = (n[0] * (1.0 - u - v) + n[1] * u + n[2] * v).normalize();
                }
            }
            if best < f32::MAX {
                if std::env::var("NSDF").is_ok() {
                    let p = o + d * best;
                    let e = 0.001;
                    let g = Vec3::new(
                        sdf(p + Vec3::X * e) - sdf(p - Vec3::X * e),
                        sdf(p + Vec3::Y * e) - sdf(p - Vec3::Y * e),
                        sdf(p + Vec3::Z * e) - sdf(p - Vec3::Z * e),
                    )
                    .normalize();
                    if sdf(p).abs() < 0.006 && g.dot(nn) > 0.5 {
                        nn = g;
                    }
                }
                b[y * w + x] = (shade(nn) * 255.0) as u8;
            }
        }
    }
    for (name, img) in [("sdf", &a), ("mesh", &b)] {
        let mut out = format!("P5 {w} {h} 255\n").into_bytes();
        out.extend_from_slice(img);
        std::fs::write(format!("{}/{name}.pgm", args[3]), out).unwrap();
    }
}
