//! Сцена «3D» (этап 7): предметы из файла сцены (`reyvec_sdf::SpaceScene`, 7.5). Каждый
//! кадр формы собираются заново (они двигаются), превращаются в программы для видеокарты
//! и едут в буфер кадра одним списком слов (`reyvec_sdf::shape3d::pack_scene`) вместе с
//! материалами и лампами (7.4).

use reyvec_sdf::SpaceFrame;
use reyvec_sdf::shape3d::{Material3, Program, Shape3, compile, pack_scene};

/// Сколько слов помещается в буфер сцены кадра (1 МБ).
pub(crate) const MAX_WORDS: usize = 256 * 1024;

/// Слов в заголовке раздела моделей: число вершин, 3 запасных, матрица «мир → экран»
/// (16 чисел, столбцами; пишет [`set_view_proj`]).
const MESH_HEADER_WORDS: usize = 20;
/// Слов на вершину: место (3 числа) и номер материала.
const VERTEX_WORDS: usize = 4;

/// Сцена кадра для видеокарты.
pub(crate) struct Packed {
    pub words: Vec<u32>,
    /// Вершин полигональных моделей (по 3 на треугольник; 0 — моделей нет).
    pub vertices: u32,
}

/// Список слов сцены для видеокарты: программы объектов (и невидимых заменителей
/// моделей, 7.6ж), материалы, лампы, сетка отсева (`culling: false` — шейдер её не
/// читает и перебирает все объекты) и треугольники моделей (слово 7 заголовка — где они).
pub(crate) fn pack(frame: &SpaceFrame, materials: &[Material3], culling: bool) -> Packed {
    let build = |shapes: &[Shape3], shadow_only: bool| -> Vec<Program> {
        shapes
            .iter()
            .filter_map(|s| match compile(s) {
                Ok(mut p) => {
                    p.shadow_only = shadow_only;
                    Some(p)
                }
                Err(e) => {
                    tracing::warn!("форма 3D пропущена: {e}");
                    None
                }
            })
            .collect()
    };
    let mut programs = build(&frame.shapes, false);
    programs.extend(build(&frame.proxies, true));
    let mut words = pack_scene(&programs, materials, &frame.lamps);
    let triangles: usize = frame.meshes.iter().map(|m| m.triangles.len()).sum();
    let mesh_words = MESH_HEADER_WORDS + triangles * 3 * VERTEX_WORDS;
    if words.len() + mesh_words > MAX_WORDS {
        tracing::warn!(
            "сцена 3D не помещается в буфер ({} слов из {MAX_WORDS}), рисую пустую",
            words.len() + mesh_words
        );
        let words = pack_scene(&[], materials, &frame.lamps);
        return Packed { words, vertices: 0 };
    }
    if !culling {
        words[6] = 0;
    }
    words[7] = words.len() as u32;
    words.extend([(triangles * 3) as u32, 0, 0, 0]);
    words.extend([0u32; 16]);
    for m in &frame.meshes {
        for t in &m.triangles {
            for v in t {
                words.extend(v.to_array().map(f32::to_bits));
                words.push(m.material);
            }
        }
    }
    Packed {
        words,
        vertices: (triangles * 3) as u32,
    }
}

/// Матрица камеры «мир → экран» в раздел моделей (вершинный шейдер «полигонов 3D»).
pub(crate) fn set_view_proj(words: &mut [u32], view_proj: glam::Mat4) {
    let at = words[7] as usize + 4;
    if let Some(slot) = words.get_mut(at..at + 16) {
        for (w, v) in slot.iter_mut().zip(view_proj.to_cols_array()) {
            *w = v.to_bits();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_scene_compiles_and_fits() {
        let scene = reyvec_sdf::SpaceScene::parse(reyvec_sdf::space_file::EXAMPLE).unwrap();
        let frame = scene.frame(1.0);
        let words = pack(&frame, &scene.materials, true).words;
        assert_eq!(words[0] as usize, frame.shapes.len(), "все формы собрались");
        assert!(words.len() < MAX_WORDS);
        // Материалы всех форм есть в списке.
        for shape in &frame.shapes {
            let program = compile(shape).unwrap();
            let (_, m) = program.distance(glam::Vec3::ZERO);
            assert!((m as usize) < scene.materials.len());
        }
    }

    #[test]
    fn stress_scene_compiles_and_fits() {
        // Сцена-нагрузка 7.6: 200 предметов, помещается в буфер.
        let text = include_str!("../../../scenes/3d/stress.ron");
        let scene = reyvec_sdf::SpaceScene::parse(text).unwrap();
        assert_eq!(scene.object_count(), 200);
        let frame = scene.frame(1.0);
        let words = pack(&frame, &scene.materials, true).words;
        assert_eq!(words[0] as usize, frame.shapes.len());
        assert!(words.len() < MAX_WORDS);
    }

    #[test]
    fn vector_scene_compiles_and_fits() {
        // 7.7: выдавленные и повёрнутые контуры, поле фигуры идёт в программе формы.
        let text = include_str!("../../../scenes/3d/vector.ron");
        let scene = reyvec_sdf::SpaceScene::parse(text).unwrap();
        let frame = scene.frame(0.0);
        let words = pack(&frame, &scene.materials, true).words;
        assert!(words.len() < MAX_WORDS);
        let inside = |object: usize, p: [f32; 3]| {
            let program = compile(&frame.shapes[object]).unwrap();
            program.distance(glam::Vec3::from(p)).0
        };
        // Звезда с дырой, сердце, ваза пустая внутри, бублик пустой в середине, облако
        // выпуклое вверх.
        assert!(inside(1, [-2.2, 1.6, 0.0]) < 0.0);
        assert!(inside(1, [-2.2, 1.1, 0.0]) > 0.05, "дыра в звезде");
        assert!(inside(2, [-0.6, 1.2, 0.8]) < -0.1);
        assert!(inside(3, [0.9, 0.6, 0.3]) > 0.0, "ваза пустая");
        assert!(inside(3, [0.9, 0.03, 0.3]) < 0.0, "дно вазы");
        assert!(inside(5, [-0.4, 0.15, -1.4]) > 0.2, "дырка бублика");
        assert!(inside(5, [0.05, 0.15, -1.4]) < 0.0);
        assert!(inside(7, [0.0, 2.75, -3.0]) < 0.0, "облако выгнуто вверх");
    }

    #[test]
    fn mixed_scene_packs_meshes_and_proxies() {
        // Опыт совмещения 7.6ж: модели и их невидимые заменители для теней.
        let text = include_str!("../../../scenes/3d/mixed.ron");
        let scene = reyvec_sdf::SpaceScene::parse(text).unwrap();
        assert_eq!(scene.mesh_count(), 7);
        let frame = scene.frame(1.0);
        assert_eq!(frame.proxies.len(), 7);
        let packed = pack(&frame, &scene.materials, true);
        let w = &packed.words;
        let triangles: usize = frame.meshes.iter().map(|m| m.triangles.len()).sum();
        assert_eq!(packed.vertices as usize, triangles * 3);
        let at = w[7] as usize;
        assert_eq!(w[at] as usize, triangles * 3);
        assert_eq!(
            w.len(),
            at + MESH_HEADER_WORDS + triangles * 3 * VERTEX_WORDS
        );
        // Заменители — последние объекты и помечены флагом.
        let (count, objects) = (w[0] as usize, w[1] as usize);
        let flag = |i: usize| w[objects + i * 8 + 2];
        // Пол — плоскость, луч глаза находит её формулой (7.6и).
        assert_eq!(flag(0), reyvec_sdf::shape3d::OBJECT_PLANE);
        // Капля — объединение списком: шейдер считает её без стека (7.6т).
        assert_eq!(flag(1), reyvec_sdf::shape3d::OBJECT_CHAIN);
        assert_eq!(
            flag(count - 1) & !reyvec_sdf::shape3d::OBJECT_CHAIN,
            reyvec_sdf::shape3d::OBJECT_SHADOW_ONLY,
            "заменитель"
        );
        // Матрица камеры встаёт в заголовок раздела моделей.
        let mut words = packed.words.clone();
        set_view_proj(&mut words, glam::Mat4::IDENTITY);
        assert_eq!(f32::from_bits(words[at + 4]), 1.0);
        assert_eq!(f32::from_bits(words[at + 9]), 1.0);
    }
}
