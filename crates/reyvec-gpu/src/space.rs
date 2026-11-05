//! Сцена «3D» (этап 7): объекты и материалы примера, собранные из форм `reyvec_sdf::Shape3`.
//! Каждый кадр формы собираются заново (они двигаются), превращаются в программы для
//! видеокарты и едут в буфер кадра одним списком слов (`reyvec_sdf::shape3d::pack_scene`).

use reyvec_sdf::shape3d::{Material3, Program, Shape3, compile, pack_scene};

/// Сколько слов помещается в буфер сцены кадра (256 КБ).
pub(crate) const MAX_WORDS: usize = 64 * 1024;

/// Материалы примера по номерам.
pub(crate) const MATERIALS: [Material3; 6] = [
    // 0: пол с сеткой.
    Material3 {
        color: [0.18, 0.19, 0.21],
        glow: 0.0,
        grid: true,
    },
    // 1: оранжевый шар.
    Material3 {
        color: [0.85, 0.35, 0.20],
        glow: 0.0,
        grid: false,
    },
    // 2: синяя коробка.
    Material3 {
        color: [0.25, 0.55, 0.85],
        glow: 0.0,
        grid: false,
    },
    // 3: золотой тор.
    Material3 {
        color: [0.90, 0.75, 0.25],
        glow: 0.0,
        grid: false,
    },
    // 4: зелёная слизь.
    Material3 {
        color: [0.30, 0.80, 0.35],
        glow: 0.0,
        grid: false,
    },
    // 5: светлый камень.
    Material3 {
        color: [0.70, 0.68, 0.64],
        glow: 0.0,
        grid: false,
    },
];

/// Объекты примера в момент `time`: пол, шар, коробка, тор (как в 7.1), капля слизи,
/// которая перетекает между двумя шарами, коробка с вырезанным шаром и колонна из
/// цилиндра с капсулами.
pub(crate) fn demo_shapes(time: f32) -> Vec<Shape3> {
    let bob = 0.15 * (time * 1.3).sin();
    // Капля бегает туда-сюда между двумя шарами слизи и плавно сливается с ними.
    let drop_x = 1.1 * (time * 0.9).sin();
    vec![
        Shape3::Plane {
            normal: [0.0, 1.0, 0.0],
            offset: 0.0,
        }
        .paint(0),
        Shape3::Sphere { radius: 1.0 }
            .at([0.0, 1.0 + bob, 0.0])
            .paint(1),
        Shape3::Box {
            size: [1.5, 1.5, 1.5],
            round: 0.08,
        }
        .at([3.0, 0.75, -0.5])
        .paint(2),
        // Тор стоит на ребре и медленно крутится вокруг вертикали.
        Shape3::Torus {
            radius: 0.75,
            thickness: 0.25,
        }
        .placed([-3.2, 1.0, 0.3], [90.0, (time * 0.5).to_degrees(), 0.0])
        .paint(3),
        Shape3::Union {
            smooth: 0.45,
            shapes: vec![
                Shape3::Sphere { radius: 0.55 }.at([-1.4, 0.55, 3.0]),
                Shape3::Sphere { radius: 0.55 }.at([1.4, 0.55, 3.0]),
                Shape3::Sphere { radius: 0.32 }.at([drop_x, 0.45, 3.0]),
                // Лужица, на которой они стоят.
                Shape3::Cylinder {
                    height: 0.06,
                    radius: 1.9,
                    round: 0.03,
                }
                .at([0.0, 0.03, 3.0]),
            ],
        }
        .paint(4),
        Shape3::Subtract {
            base: Box::new(Shape3::Box {
                size: [1.2, 1.2, 1.2],
                round: 0.04,
            }),
            cut: vec![Shape3::Sphere { radius: 0.78 }],
            smooth: 0.05,
        }
        .placed([3.4, 0.6, 2.6], [0.0, time.to_degrees() * 0.3, 0.0])
        .paint(2),
        Shape3::Union {
            smooth: 0.15,
            shapes: vec![
                Shape3::Cylinder {
                    height: 3.0,
                    radius: 0.3,
                    round: 0.02,
                }
                .at([0.0, 1.5, 0.0]),
                Shape3::Box {
                    size: [1.0, 0.2, 1.0],
                    round: 0.03,
                }
                .at([0.0, 0.1, 0.0]),
                Shape3::Box {
                    size: [0.9, 0.2, 0.9],
                    round: 0.03,
                }
                .at([0.0, 3.0, 0.0]),
            ],
        }
        .at([-1.5, 0.0, -3.0])
        .paint(5),
    ]
}

/// Список слов сцены для видеокарты: программы объектов и материалы.
pub(crate) fn pack(shapes: &[Shape3], materials: &[Material3]) -> Vec<u32> {
    let programs: Vec<Program> = shapes
        .iter()
        .filter_map(|s| match compile(s) {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!("форма 3D пропущена: {e}");
                None
            }
        })
        .collect();
    let mut words = pack_scene(&programs, materials);
    if words.len() > MAX_WORDS {
        tracing::warn!(
            "сцена 3D не помещается в буфер ({} слов из {MAX_WORDS}), рисую пустую",
            words.len()
        );
        words = pack_scene(&[], materials);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_scene_compiles_and_fits() {
        let shapes = demo_shapes(1.0);
        let words = pack(&shapes, &MATERIALS);
        assert_eq!(words[0] as usize, shapes.len(), "все формы собрались");
        assert!(words.len() < MAX_WORDS);
        // Материалы всех форм есть в списке.
        let program = compile(&shapes[4]).unwrap();
        let (_, m) = program.distance(glam::Vec3::new(-1.4, 0.55, 3.0));
        assert!((m as usize) < MATERIALS.len());
    }
}

