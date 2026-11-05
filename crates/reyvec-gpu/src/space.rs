//! Сцена «3D» (этап 7): предметы из файла сцены (`reyvec_sdf::SpaceScene`, 7.5). Каждый
//! кадр формы собираются заново (они двигаются), превращаются в программы для видеокарты
//! и едут в буфер кадра одним списком слов (`reyvec_sdf::shape3d::pack_scene`) вместе с
//! материалами и лампами (7.4).

use reyvec_sdf::shape3d::{Lamp3, Material3, Program, Shape3, compile, pack_scene};

/// Сколько слов помещается в буфер сцены кадра (256 КБ).
pub(crate) const MAX_WORDS: usize = 64 * 1024;

/// Список слов сцены для видеокарты: программы объектов, материалы и лампы.
pub(crate) fn pack(shapes: &[Shape3], materials: &[Material3], lamps: &[Lamp3]) -> Vec<u32> {
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
    let mut words = pack_scene(&programs, materials, lamps);
    if words.len() > MAX_WORDS {
        tracing::warn!(
            "сцена 3D не помещается в буфер ({} слов из {MAX_WORDS}), рисую пустую",
            words.len()
        );
        words = pack_scene(&[], materials, lamps);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_scene_compiles_and_fits() {
        let scene = reyvec_sdf::SpaceScene::parse(reyvec_sdf::space_file::EXAMPLE).unwrap();
        let frame = scene.frame(1.0);
        let words = pack(&frame.shapes, &scene.materials, &frame.lamps);
        assert_eq!(words[0] as usize, frame.shapes.len(), "все формы собрались");
        assert!(words.len() < MAX_WORDS);
        // Материалы всех форм есть в списке.
        for shape in &frame.shapes {
            let program = compile(shape).unwrap();
            let (_, m) = program.distance(glam::Vec3::ZERO);
            assert!((m as usize) < scene.materials.len());
        }
    }
}
