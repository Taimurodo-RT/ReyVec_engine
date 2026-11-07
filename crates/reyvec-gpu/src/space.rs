//! Сцена «3D» (этап 7): предметы из файла сцены (`reyvec_sdf::SpaceScene`, 7.5). Каждый
//! кадр формы собираются заново (они двигаются), превращаются в программы для видеокарты
//! и едут в буфер кадра одним списком слов (`reyvec_sdf::shape3d::pack_scene`) вместе с
//! материалами и лампами (7.4).

use reyvec_sdf::SpaceFrame;
use reyvec_sdf::shape3d::{Material3, Program, Shape3, compile, pack_scene};

/// Сколько слов помещается в буфер сцены кадра (4 МБ: треугольники 2.5D-вида, 7.9б).
pub(crate) const MAX_WORDS: usize = 1024 * 1024;

/// Слов в заголовке раздела моделей: число вершин, число карт теней ламп, 2 запасных,
/// матрица «мир → экран» (16 чисел, столбцами; пишет [`set_view_proj`]) и матрицы двух
/// ламп для карт теней (пишет [`set_shadow_maps`]), как в `include/space.slang`.
const MESH_HEADER_WORDS: usize = 52;
/// Где в разделе моделей матрица камеры и матрицы ламп.
const MESH_VIEW_PROJ: usize = 4;
const MESH_SHADOW_PROJ: usize = 20;
/// Карт теней не больше (как SHADOW_LAMPS: тени есть только у двух первых ламп).
pub(crate) const MAX_SHADOW_MAPS: usize = 2;
/// Слово 11 заголовка сцены, бит 1: солнца нет (как SCENE_NO_SUN в `space.slang`).
const SCENE_NO_SUN: u32 = 2;
/// Слов на вершину: место (3 числа) и номер материала.
const VERTEX_WORDS: usize = 4;

/// Сцена кадра для видеокарты.
pub(crate) struct Packed {
    pub words: Vec<u32>,
    /// Вершин полигональных моделей (по 3 на треугольник; 0 — моделей нет).
    pub vertices: u32,
    /// Сетка отсева кадра (`None` — её нет).
    pub grid: Option<GridStats>,
}

/// Сетка отсева кадра для панели: размер клетки, число клеток по осям и длины списков
/// клеток (средняя и наибольшая). По ним видно, не растянул ли сетку один длинный
/// предмет: пол в 53,6 м давал в комнате клетку 2 м и списки по 30–50 объектов
/// (разбор 2026-11-07).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GridStats {
    pub cell: f32,
    pub dims: [u32; 3],
    pub mean_list: f32,
    pub max_list: u32,
}

/// Сетка отсева по словам сцены (раскладка — `Grid::write`); `None` — сетки нет.
pub(crate) fn grid_stats(words: &[u32]) -> Option<GridStats> {
    let g = *words.get(6)? as usize;
    if g == 0 {
        return None;
    }
    let cell = f32::from_bits(*words.get(g + 3)?);
    let dims = [*words.get(g + 4)?, *words.get(g + 5)?, *words.get(g + 6)?];
    let cells = *words.get(g + 14)? as usize;
    let count = dims.iter().map(|&d| d as usize).product::<usize>();
    let (mut sum, mut max) = (0u64, 0u32);
    for i in 0..count {
        let n = *words.get(cells + i * reyvec_sdf::grid3d::CELL_WORDS + 1)?;
        sum += u64::from(n);
        max = max.max(n);
    }
    Some(GridStats {
        cell,
        dims,
        mean_list: if count == 0 {
            0.0
        } else {
            sum as f32 / count as f32
        },
        max_list: max,
    })
}

/// Список слов сцены для видеокарты: программы объектов (и невидимых заменителей
/// моделей, 7.6ж), материалы, лампы, сетка отсева (`culling: false` — шейдер её не
/// читает и перебирает все объекты) и треугольники моделей (слово 7 заголовка — где они).
pub(crate) fn pack(frame: &SpaceFrame, materials: &[Material3], culling: bool) -> Packed {
    let programs = programs(frame);
    let mut words = pack_scene(&programs, materials, &frame.lamps);
    let triangles: usize = frame.meshes.iter().map(|m| m.triangles.len()).sum();
    let mesh_words = MESH_HEADER_WORDS + triangles * 3 * VERTEX_WORDS;
    if words.len() + mesh_words > MAX_WORDS {
        tracing::warn!(
            "сцена 3D не помещается в буфер ({} слов из {MAX_WORDS}), рисую пустую",
            words.len() + mesh_words
        );
        let words = pack_scene(&[], materials, &frame.lamps);
        return Packed {
            words,
            vertices: 0,
            grid: None,
        };
    }
    if !culling {
        words[6] = 0;
    }
    let grid = grid_stats(&words);
    words[7] = words.len() as u32;
    words.extend([(triangles * 3) as u32, 0, 0, 0]);
    words.extend([0u32; MESH_HEADER_WORDS - 4]);
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
        grid,
    }
}

/// Программы форм кадра: предметы, потом невидимые заменители моделей (7.6ж). Форма,
/// которую не вышло превратить в программу, пропускается с записью в журнал.
fn programs(frame: &SpaceFrame) -> Vec<Program> {
    let build = |shapes: &[Shape3], shadow_only: bool, no_shadow: bool| -> Vec<Program> {
        shapes
            .iter()
            .filter_map(|s| match compile(s) {
                Ok(mut p) => {
                    p.shadow_only = shadow_only;
                    p.no_shadow = no_shadow;
                    Some(p)
                }
                Err(e) => {
                    tracing::warn!("форма 3D пропущена: {e}");
                    None
                }
            })
            .collect()
    };
    let mut programs = build(&frame.shapes, false, false);
    programs.extend(build(&frame.proxies, true, false));
    programs.extend(build(&frame.glows, false, true));
    programs
}

/// Формы для запечённого поля (7.10б): все предметы и заменители кадра, до отсева по
/// взгляду. Светящихся без теней в поле нет: поле читают только тени и углы.
pub(crate) fn field_programs(frame: &SpaceFrame) -> Vec<Program> {
    programs(frame)
        .into_iter()
        .filter(|p| !p.no_shadow)
        .collect()
}

/// Слов в разделе запечённого поля: адрес буфера поля (2 слова), точек по стороне слоя,
/// 1 запасное, углы слоёв (по 3 целых).
const FIELD_SECTION_WORDS: usize = 4 + 3 * reyvec_sdf::field3d::FIELD_LEVELS;

/// Раздел запечённого поля (7.10) в конец сцены кадра; слово 10 заголовка — где он.
/// Раздел начинается с чётного слова: адрес буфера шейдер читает 8-байтным указателем.
/// `None` — поля в этом кадре нет (шейдер считает формы как раньше). `ao_from_field` —
/// дальние пробы затенения в углах берут число поля (7.10г, бит 0 слова 11).
pub(crate) fn set_field(
    words: &mut Vec<u32>,
    field: Option<(u64, &reyvec_sdf::field3d::FieldWindow)>,
    ao_from_field: bool,
) {
    let Some((address, window)) = field else {
        return;
    };
    words[11] = u32::from(ao_from_field);
    if words.len() % 2 == 1 {
        words.push(0);
    }
    if words.len() + FIELD_SECTION_WORDS > MAX_WORDS {
        return;
    }
    words[10] = words.len() as u32;
    words.extend([
        address as u32,
        (address >> 32) as u32,
        window.size as u32,
        0,
    ]);
    for c in window.corners {
        words.extend(c.to_array().map(|v| v as u32));
    }
}

/// Матрица камеры «мир → экран» в раздел моделей (вершинный шейдер «полигонов 3D»).
pub(crate) fn set_view_proj(words: &mut [u32], view_proj: glam::Mat4) {
    write_matrix(words, words[7] as usize + MESH_VIEW_PROJ, view_proj);
}

/// Матрицы ламп для карт теней (7.9б) в раздел моделей; их число — слово 1 раздела.
pub(crate) fn set_shadow_maps(words: &mut [u32], maps: &[glam::Mat4]) {
    let at = words[7] as usize;
    let count = maps.len().min(MAX_SHADOW_MAPS);
    if let Some(w) = words.get_mut(at + 1) {
        *w = count as u32;
    }
    for (k, m) in maps.iter().take(count).enumerate() {
        write_matrix(words, at + MESH_SHADOW_PROJ + 16 * k, *m);
    }
}

/// Солнца нет (2.5D-вид: уровень в помещении).
pub(crate) fn set_no_sun(words: &mut [u32]) {
    words[11] |= SCENE_NO_SUN;
}

fn write_matrix(words: &mut [u32], at: usize, m: glam::Mat4) {
    if let Some(slot) = words.get_mut(at..at + 16) {
        for (w, v) in slot.iter_mut().zip(m.to_cols_array()) {
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
    fn grid_stats_match_the_grid() {
        let text = include_str!("../../../scenes/3d/stress.ron");
        let scene = reyvec_sdf::SpaceScene::parse(text).unwrap();
        let frame = scene.frame(1.0);
        let grid = reyvec_sdf::grid3d::Grid::build(&programs(&frame));
        let stats = pack(&frame, &scene.materials, true).grid.unwrap();
        assert_eq!(stats.cell, grid.cell);
        assert_eq!(stats.dims, grid.dims);
        assert_eq!(
            stats.max_list,
            grid.cells.iter().map(|c| c.count).max().unwrap_or(0)
        );
        let mean = grid.cells.iter().map(|c| c.count as f32).sum::<f32>() / grid.cells.len() as f32;
        assert!((stats.mean_list - mean).abs() < 1e-3);
        assert!(pack(&frame, &scene.materials, false).grid.is_none());
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
