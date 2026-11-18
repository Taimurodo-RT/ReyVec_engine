//! Упрощение сетки из формы (8.20а2): ровная сетка двойного контура режет плоскую стенку
//! коробки на столько же треугольников, сколько и кривую. На заводе это 5,6 млн
//! треугольников в кадре — слабой видеокарте много. Здесь лишние вершины убираются: вершина
//! «сливается» с соседней (берёт её место), если от этого поверхность сетки не отходит от
//! формы дальше допуска. Проверка — по самой форме (расстояние SDF в точках новых
//! треугольников), а не по сетке, поэтому плоские стенки сжимаются до пары треугольников,
//! а скругления и цилиндры сохраняют столько, сколько нужно для допуска.
//!
//! После упрощения нормали считаются заново по углам треугольников ([`split_normals`]):
//! на остром ребре у вершины две нормали (по одной на каждую стенку), на плавной
//! поверхности — одна. Иначе большой треугольник стенки тянул бы нормаль соседней стенки
//! через всю свою площадь.

use glam::Vec3;

use crate::shape_mesh::ShapeMesh;
use crate::shape3d::Program;

/// Новый треугольник не поворачивается сильнее этого против прежнего (косинус угла).
const MAX_TURN_COS: f32 = 0.2;
/// Треугольник смотрит туда же, куда наклон поля в его середине (косинус угла).
const MIN_FACING_COS: f32 = 0.5;

/// Убирает вершины, пока поверхность остаётся ближе `tolerance` к форме `program`.
/// Сетка остаётся замкнутой; материалы не смешиваются (сливаются только вершины одного
/// материала).
pub fn simplify(mesh: &mut ShapeMesh, program: &Program, tolerance: f32) {
    let mut tris: Vec<[u32; 3]> = mesh
        .indices
        .chunks_exact(3)
        .map(|t| [t[0], t[1], t[2]])
        .collect();
    let mut alive = vec![true; tris.len()];
    let mut around: Vec<Vec<u32>> = vec![Vec::new(); mesh.positions.len()];
    for (i, t) in tris.iter().enumerate() {
        for &v in t {
            around[v as usize].push(i as u32);
        }
    }
    let h = mesh.cell * 0.05;
    let close = |p: Vec3| program.distance(p).0.abs() <= tolerance;
    let pos = &mesh.positions;
    let materials = &mesh.materials;

    // Кратчайшие рёбра — первыми: сетка грубеет равномерно, у вершин не вырастают огромные
    // веера треугольников (с ними каждая проверка стоила бы дорого).
    let mut queue = std::collections::BinaryHeap::new();
    let push = |queue: &mut std::collections::BinaryHeap<Candidate>, u: u32, v: u32| {
        if materials[u as usize] == materials[v as usize] {
            let len = pos[u as usize].distance_squared(pos[v as usize]);
            queue.push(Candidate {
                len,
                from: u,
                to: v,
            });
            queue.push(Candidate {
                len,
                from: v,
                to: u,
            });
        }
    };
    for t in &tris {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            if a < b {
                push(&mut queue, a, b);
            }
        }
    }
    while let Some(Candidate { from: u, to: v, .. }) = queue.pop() {
        around[u as usize].retain(|&t| alive[t as usize]);
        around[v as usize].retain(|&t| alive[t as usize]);
        // Два треугольника на ребре u–v (если ребра уже нет — запись устарела); их третьи
        // вершины — единственные общие соседи u и v (иначе слияние склеит сетку саму с
        // собой).
        let shared: Vec<u32> = around[u as usize]
            .iter()
            .copied()
            .filter(|&t| tris[t as usize].contains(&v))
            .collect();
        if shared.len() != 2 {
            continue;
        }
        let ring_u = ring(&tris, &around[u as usize], u);
        let ring_v = ring(&tris, &around[v as usize], v);
        let common = ring_u
            .iter()
            .filter(|n| ring_v.binary_search(n).is_ok())
            .count();
        if common != 2 {
            continue;
        }
        let pv = pos[v as usize];
        let ok = around[u as usize]
            .iter()
            .filter(|t| !shared.contains(t))
            .all(|&t| {
                let old = tris[t as usize].map(|k| pos[k as usize]);
                let new = tris[t as usize].map(|k| if k == u { pv } else { pos[k as usize] });
                let (n_old, n_new) = (face(old), face(new));
                if n_new.length_squared() <= 1e-12 * mesh.cell.powi(4) {
                    return false;
                }
                let (n_old, n_new) = (n_old.normalize_or_zero(), n_new.normalize());
                if n_new.dot(n_old) < MAX_TURN_COS {
                    return false;
                }
                let [a, b, c] = new;
                let center = (a + b + c) / 3.0;
                let slope = gradient(program, center, h).normalize_or_zero();
                if n_new.dot(slope) < MIN_FACING_COS {
                    return false;
                }
                [
                    center,
                    (a + b) * 0.5,
                    (b + c) * 0.5,
                    (c + a) * 0.5,
                    (a * 2.0 + b + c) * 0.25,
                    (a + b * 2.0 + c) * 0.25,
                    (a + b + c * 2.0) * 0.25,
                ]
                .into_iter()
                .all(close)
            });
        if !ok {
            continue;
        }
        for &t in &shared {
            alive[t as usize] = false;
        }
        let moved: Vec<u32> = around[u as usize]
            .iter()
            .copied()
            .filter(|t| !shared.contains(t))
            .collect();
        for &t in &moved {
            for k in &mut tris[t as usize] {
                if *k == u {
                    *k = v;
                }
            }
        }
        around[v as usize].extend(moved);
        around[u as usize].clear();
        for w in ring_u {
            if w != v && ring_v.binary_search(&w).is_err() {
                push(&mut queue, v, w);
            }
        }
    }

    mesh.indices = tris
        .iter()
        .zip(&alive)
        .filter(|(_, a)| **a)
        .flat_map(|(t, _)| *t)
        .collect();
    compact(mesh);
}

/// Возможное слияние: вершина `from` переходит на место `to`. В очереди короткие рёбра
/// выше длинных.
struct Candidate {
    len: f32,
    from: u32,
    to: u32,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for Candidate {}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .len
            .total_cmp(&self.len)
            .then((other.from, other.to).cmp(&(self.from, self.to)))
    }
}

/// Соседи вершины `u` по её треугольникам `around`.
fn ring(tris: &[[u32; 3]], around: &[u32], u: u32) -> Vec<u32> {
    let mut out: Vec<u32> = around
        .iter()
        .flat_map(|&t| tris[t as usize])
        .filter(|&k| k != u)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Ненормированная нормаль треугольника (по обходу).
fn face([a, b, c]: [Vec3; 3]) -> Vec3 {
    (b - a).cross(c - a)
}

/// Наклон поля (как в shape_mesh): четыре выборки по вершинам тетраэдра.
fn gradient(program: &Program, p: Vec3, h: f32) -> Vec3 {
    [
        Vec3::new(1.0, -1.0, -1.0),
        Vec3::new(-1.0, -1.0, 1.0),
        Vec3::new(-1.0, 1.0, -1.0),
        Vec3::new(1.0, 1.0, 1.0),
    ]
    .iter()
    .map(|&k| k * program.distance(p + k * h).0)
    .fold(Vec3::ZERO, |a, b| a + b)
}

/// Убирает вершины без треугольников и перенумеровывает остальные.
fn compact(mesh: &mut ShapeMesh) {
    let mut map = vec![u32::MAX; mesh.positions.len()];
    let mut next = 0u32;
    for &i in &mesh.indices {
        if map[i as usize] == u32::MAX {
            map[i as usize] = next;
            next += 1;
        }
    }
    let mut positions = vec![Vec3::ZERO; next as usize];
    let mut normals = vec![Vec3::ZERO; next as usize];
    let mut materials = vec![0; next as usize];
    for (old, &new) in map.iter().enumerate() {
        if new != u32::MAX {
            positions[new as usize] = mesh.positions[old];
            normals[new as usize] = mesh.normals[old];
            materials[new as usize] = mesh.materials[old];
        }
    }
    for i in &mut mesh.indices {
        *i = map[*i as usize];
    }
    mesh.positions = positions;
    mesh.normals = normals;
    mesh.materials = materials;
}

/// Две нормали у одной вершины считаются одной, если угол между ними меньше этого
/// (косинус).
const SAME_NORMAL_COS: f32 = 0.95;

/// Нормали по углам треугольников: наклон поля чуть внутри треугольника от угла. Если у
/// вершины углы смотрят по-разному (острое ребро), вершина делится на несколько, по одной
/// на каждую сторону.
pub fn split_normals(mesh: &mut ShapeMesh, program: &Program) {
    let h = mesh.cell * 0.05;
    let count = mesh.positions.len();
    // У каждой старой вершины — её копии: (нормаль, новый номер).
    let mut copies: Vec<Vec<(Vec3, u32)>> = vec![Vec::new(); count];
    let mut positions = Vec::with_capacity(count);
    let mut normals = Vec::with_capacity(count);
    let mut materials = Vec::with_capacity(count);
    let mut indices = Vec::with_capacity(mesh.indices.len());
    for t in mesh.indices.chunks_exact(3) {
        let p = [0, 1, 2].map(|k| mesh.positions[t[k] as usize]);
        let center = (p[0] + p[1] + p[2]) / 3.0;
        let flat = face(p).normalize_or_zero();
        for k in 0..3 {
            let v = t[k] as usize;
            // Точка на десятую пути от угла к середине — по ту же сторону ребра, что и
            // треугольник; нормаль не уводится далеко от грани.
            let at = p[k].lerp(center, 0.1);
            let mut n = gradient(program, at, h).normalize_or_zero();
            if n.dot(flat) < 0.0 || n == Vec3::ZERO {
                n = flat;
            }
            let found = copies[v]
                .iter()
                .find(|(m, _)| m.dot(n) >= SAME_NORMAL_COS)
                .map(|&(_, i)| i);
            let index = found.unwrap_or_else(|| {
                let i = positions.len() as u32;
                positions.push(mesh.positions[v]);
                normals.push(n);
                materials.push(mesh.materials[v]);
                copies[v].push((n, i));
                i
            });
            indices.push(index);
        }
    }
    mesh.positions = positions;
    mesh.normals = normals;
    mesh.materials = materials;
    mesh.indices = indices;
}
