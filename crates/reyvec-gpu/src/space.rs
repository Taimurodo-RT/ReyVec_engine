//! Сцена «3D» (этап 7): предметы из файла сцены (`reyvec_sdf::SpaceScene`, 7.5). Каждый
//! кадр формы собираются заново (они двигаются), превращаются в программы для видеокарты
//! и едут в буфер кадра одним списком слов (`reyvec_sdf::shape3d::pack_scene`) вместе с
//! материалами и лампами (7.4).

use std::sync::atomic::{AtomicBool, Ordering};

use reyvec_sdf::SpaceFrame;
use reyvec_sdf::shape3d::{
    MAX_OBJECTS, Material3, Program, Shape3, compile, pack_scene, pack_scene_with,
};

/// Сколько слов помещается в буфер сцены кадра (4 МБ: треугольники 2.5D-вида, 7.9б).
pub(crate) const MAX_WORDS: usize = 1024 * 1024;

/// Предупреждения о пределах уже записаны в журнал (пишутся один раз, пока предел
/// нарушен; панель показывает их каждый кадр, 7.18б).
static WARNED_OBJECTS: AtomicBool = AtomicBool::new(false);
static WARNED_WORDS: AtomicBool = AtomicBool::new(false);

/// Пишет предупреждение один раз на нарушение предела: `over` — предел нарушен сейчас.
fn warn_once(flag: &AtomicBool, over: bool, message: impl FnOnce() -> String) {
    if over {
        if !flag.swap(true, Ordering::Relaxed) {
            tracing::warn!("{}", message());
        }
    } else {
        flag.store(false, Ordering::Relaxed);
    }
}

/// Слов в заголовке раздела моделей: число вершин, число карт теней ламп, адрес буфера
/// счётчиков лучей (2 слова, пишет [`set_counters`]; 0 — не считать), матрица «мир → экран» (16 чисел, столбцами; пишет [`set_view_proj`]) и матрицы двух
/// ламп для карт теней (пишет [`set_shadow_maps`]), как в `include/space.slang`.
const MESH_HEADER_WORDS: usize = 52;
/// Где в разделе моделей матрица камеры и матрицы ламп.
const MESH_VIEW_PROJ: usize = 4;
const MESH_SHADOW_PROJ: usize = 20;
/// Карт теней не больше (как SHADOW_LAMPS: тени есть только у двух первых ламп).
pub(crate) const MAX_SHADOW_MAPS: usize = 2;
/// Слово 11 заголовка сцены, бит 1: солнца нет (как SCENE_NO_SUN в `space.slang`).
const SCENE_NO_SUN: u32 = 2;
/// Бит 2 слова 11: нормаль по всей сцене (как SCENE_FULL_NORMAL в `include/march.slang`).
#[cfg(test)]
const SCENE_FULL_NORMAL: u32 = 4;
/// Слов на вершину: место (3 числа) и номер материала.
const VERTEX_WORDS: usize = 4;

/// Сцена кадра для видеокарты.
pub(crate) struct Packed {
    pub words: Vec<u32>,
    /// Вершин полигональных моделей (по 3 на треугольник; 0 — моделей нет).
    pub vertices: u32,
    /// Сетка отсева кадра (`None` — её нет).
    pub grid: Option<GridStats>,
    /// Объектов в списке (слово 0 заголовка; после обрезки до [`MAX_OBJECTS`]).
    pub objects: u32,
    /// Сколько объектов не вошло в список: шейдер дальше [`MAX_OBJECTS`] не читает.
    pub dropped: usize,
    /// Сцена не поместилась в буфер: сколько слов ей было нужно (рисуется пустая).
    pub overflow: Option<usize>,
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
#[cfg(test)]
pub(crate) fn pack(frame: &SpaceFrame, materials: &[Material3], culling: bool) -> Packed {
    pack_with(frame, materials, culling, true)
}

/// То же; `level_field: false` — поле уровня считается в общем переборе объектов, как до
/// 7.18д3 (для сравнения в тестах).
pub(crate) fn pack_with(
    frame: &SpaceFrame,
    materials: &[Material3],
    culling: bool,
    level_field: bool,
) -> Packed {
    let mut programs = programs(frame);
    // Шейдер читает не больше MAX_OBJECTS объектов и молча пропускает остальные:
    // обрезаем список здесь, чтобы зеркало на процессоре (сетка отсева) видело то же.
    let dropped = programs.len().saturating_sub(MAX_OBJECTS);
    warn_once(&WARNED_OBJECTS, dropped > 0, || {
        format!(
            "объектов сцены 3D больше предела: {} из {MAX_OBJECTS}, {dropped} не рисуются",
            programs.len()
        )
    });
    programs.truncate(MAX_OBJECTS);
    let mut words = pack_scene_with(&programs, materials, &frame.lamps, level_field);
    let triangles: usize = frame.meshes.iter().map(|m| m.triangles.len()).sum();
    let mesh_words = MESH_HEADER_WORDS + triangles * 3 * VERTEX_WORDS;
    let needed = words.len() + mesh_words;
    warn_once(&WARNED_WORDS, needed > MAX_WORDS, || {
        format!("сцена 3D не помещается в буфер ({needed} слов из {MAX_WORDS}), рисую пустую")
    });
    if needed > MAX_WORDS {
        let words = pack_scene(&[], materials, &frame.lamps);
        return Packed {
            words,
            vertices: 0,
            grid: None,
            objects: 0,
            dropped,
            overflow: Some(needed),
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
        objects: words[0],
        words,
        vertices: (triangles * 3) as u32,
        grid,
        dropped,
        overflow: None,
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

/// Где раздел моделей; `None` — его нет (сцена не поместилась и рисуется пустой, слово 7
/// заголовка — 0): писать в «раздел» тогда значит портить заголовок сцены.
fn mesh_section(words: &[u32]) -> Option<usize> {
    let at = *words.get(7)? as usize;
    (at != 0).then_some(at)
}

/// Матрица камеры «мир → экран» в раздел моделей (вершинный шейдер «полигонов 3D»).
pub(crate) fn set_view_proj(words: &mut [u32], view_proj: glam::Mat4) {
    if let Some(at) = mesh_section(words) {
        write_matrix(words, at + MESH_VIEW_PROJ, view_proj);
    }
}

/// Адрес буфера счётчиков лучей (7.18д2) в слова 2 и 3 раздела моделей: туда шейдеры
/// `*_counted` складывают счётчики (`counters_flush` в `include/space.slang`). `false` —
/// раздела нет (сцена не поместилась), шейдеры считать не будут.
pub(crate) fn set_counters(words: &mut [u32], address: u64) -> bool {
    if let Some(at) = mesh_section(words)
        && let Some(slot) = words.get_mut(at + 2..at + 4)
    {
        slot[0] = address as u32;
        slot[1] = (address >> 32) as u32;
        return true;
    }
    false
}

/// Матрицы ламп для карт теней (7.9б) в раздел моделей; их число — слово 1 раздела.
pub(crate) fn set_shadow_maps(words: &mut [u32], maps: &[glam::Mat4]) {
    let Some(at) = mesh_section(words) else {
        return;
    };
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

/// Нормаль всегда по всей сцене, без счёта одного ближайшего объекта (7.18д5; только для
/// сравнения в тестах).
#[cfg(test)]
pub(crate) fn set_full_normal(words: &mut [u32]) {
    words[11] |= SCENE_FULL_NORMAL;
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

    /// Адрес счётчиков ложится в слова 2 и 3 раздела моделей и больше ничего не трогает; у
    /// сцены, которая не поместилась (раздела нет), не пишется ничего — ни он, ни матрицы.
    #[test]
    fn counters_address_goes_to_mesh_header() {
        let scene = reyvec_sdf::SpaceScene::parse(reyvec_sdf::space_file::EXAMPLE).unwrap();
        let words = pack(&scene.frame(1.0), &scene.materials, true).words;
        let at = words[7] as usize;
        assert!(at > 0);
        let mut set = words.clone();
        assert!(set_counters(&mut set, 0x1234_5678_9abc_def0));
        assert_eq!(set[at + 2], 0x9abc_def0);
        assert_eq!(set[at + 3], 0x1234_5678);
        set[at + 2] = words[at + 2];
        set[at + 3] = words[at + 3];
        assert_eq!(set, words, "другие слова те же");

        let empty = pack_scene(&[], &scene.materials, &[]);
        assert_eq!(empty[7], 0);
        let mut set = empty.clone();
        assert!(!set_counters(&mut set, 0x1234_5678_9abc_def0));
        set_view_proj(&mut set, glam::Mat4::IDENTITY);
        set_shadow_maps(&mut set, &[glam::Mat4::IDENTITY]);
        assert_eq!(set, empty, "без раздела моделей заголовок цел");
    }

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
        let packed = pack(&frame, &scene.materials, true);
        assert_eq!(packed.words[0] as usize, frame.shapes.len());
        assert!(packed.words.len() < MAX_WORDS);
        // Счётчики для панели (7.18б): объекты — предметы и шарики ламп.
        assert_eq!(packed.objects as usize, frame.shapes.len());
        assert_eq!(packed.dropped, 0);
        assert_eq!(packed.overflow, None);
    }

    /// Объектов больше предела шейдера: список обрезается до 1024 на процессоре, и панель
    /// узнаёт, сколько не рисуется (7.18б). Раньше шейдер резал список молча.
    #[test]
    fn too_many_objects_are_dropped_with_a_count() {
        let scene = reyvec_sdf::SpaceScene::parse(reyvec_sdf::space_file::EXAMPLE).unwrap();
        let mut frame = scene.frame(1.0);
        frame.shapes.clear();
        for i in 0..(MAX_OBJECTS + 5) {
            frame
                .shapes
                .push(Shape3::Sphere { radius: 0.1 }.placed([i as f32 * 0.3, 0.0, 0.0], [0.0; 3]));
        }
        let packed = pack(&frame, &scene.materials, true);
        assert_eq!(packed.objects as usize, MAX_OBJECTS);
        assert_eq!(packed.words[0] as usize, MAX_OBJECTS);
        assert_eq!(packed.dropped, 5);
        assert_eq!(packed.overflow, None);
    }

    /// Сцена не влезает в буфер: панель узнаёт, сколько слов было нужно; раньше об этом
    /// говорила только строка в журнале.
    #[test]
    fn overflow_is_reported() {
        let scene = reyvec_sdf::SpaceScene::parse(reyvec_sdf::space_file::EXAMPLE).unwrap();
        let mut frame = scene.frame(1.0);
        // Один треугольник — 12 слов; столько треугольников точно не поместится.
        let triangles = MAX_WORDS / (3 * VERTEX_WORDS) + 1;
        let t = [glam::Vec3::ZERO, glam::Vec3::X, glam::Vec3::Y];
        frame.meshes.push(reyvec_sdf::space_file::MeshFrame {
            triangles: vec![t; triangles],
            material: 0,
        });
        let packed = pack(&frame, &scene.materials, true);
        assert!(packed.overflow.is_some_and(|w| w > MAX_WORDS));
        assert_eq!(packed.objects, 0);
        assert_eq!(packed.vertices, 0);
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
