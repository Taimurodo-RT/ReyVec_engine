//! Сцена «3D» (этап 7): предметы из файла сцены (`reyvec_sdf::SpaceScene`, 7.5). Каждый
//! кадр формы собираются заново (они двигаются), превращаются в программы для видеокарты
//! и едут в буфер кадра одним списком слов (`reyvec_sdf::shape3d::pack_scene`) вместе с
//! материалами и лампами (7.4).

use std::sync::atomic::{AtomicBool, Ordering};

use glam::Vec3;
use reyvec_sdf::SpaceFrame;
use reyvec_sdf::grid3d::GridOptions;
use reyvec_sdf::shape3d::{
    MAX_MATERIALS, MAX_OBJECTS, Material3, Program, Shape3, compile, pack_scene, pack_scene_cached,
};

/// Сколько слов помещается в буфер сцены кадра (4 МБ: треугольники 2.5D-вида, 7.9б).
pub(crate) const MAX_WORDS: usize = 1024 * 1024;

/// Предупреждения о пределах уже записаны в журнал (пишутся один раз, пока предел
/// нарушен; панель показывает их каждый кадр, 7.18б).
static WARNED_OBJECTS: AtomicBool = AtomicBool::new(false);
static WARNED_WORDS: AtomicBool = AtomicBool::new(false);
static WARNED_MATERIALS: AtomicBool = AtomicBool::new(false);

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
/// ламп для карт теней (пишет [`set_shadow_maps`]), потом сетки из форм (8.20б, пишет
/// [`set_shape_meshes`]): адрес их буфера (2 слова), где в сцене их места и сколько
/// мест, потом карты теней солнца (8.20в, пишет [`set_sun_maps`]): сколько их (0 — нет) и
/// их матрицы ([`SUN_CASCADES`] по 16 чисел), потом где раздел коробок для RT-ядер
/// (опыт Р1, пишет [`set_ray_boxes`]; 0 — его нет) и запасное слово. Как в
/// `include/space.slang`.
const MESH_HEADER_WORDS: usize = MESH_PREV_VIEW_PROJ + 17;
/// Где в разделе моделей начало раздела коробок для RT-ядер.
const MESH_RAY: usize = 57 + 16 * SUN_CASCADES;
/// Где в разделе моделей начало раздела изменившихся квадратов «теней 3D» (опыт Р3,
/// пишет [`set_dirty_tiles`]; 0 — его нет, считаются все точки).
const MESH_DIRTY: usize = MESH_RAY + 1;
/// Где в разделе моделей матрица камеры прошлого кадра (ReSTIR DI, путь ReSTIR шаг 3), за
/// ней слово «прошлый кадр годится» (1 — запасы прошлого кадра можно брать).
const MESH_PREV_VIEW_PROJ: usize = MESH_DIRTY + 1;
/// Сторона квадрата «теней 3D» (точек половинного размера), который пересчитывается или
/// берётся из прошлого кадра целиком (опыт Р3). Как DIRTY_TILE в `raymarch_shadow.slang`.
pub(crate) const DIRTY_TILE: u32 = 8;
/// Где в разделе моделей запись сеток из форм.
const MESH_SHAPES: usize = 52;
/// Где в разделе моделей карты теней солнца: их число, потом матрицы.
const MESH_SUN: usize = 56;
/// Слов на место сетки из формы: столбцы матрицы (поворот с увеличением) и сдвиг.
pub(crate) const SHAPE_INSTANCE_WORDS: usize = 12;
/// Где в разделе моделей матрица камеры и матрицы ламп.
const MESH_VIEW_PROJ: usize = 4;
const MESH_SHADOW_PROJ: usize = 20;
/// Карт теней не больше (как SHADOW_LAMPS: тени есть только у двух первых ламп).
pub(crate) const MAX_SHADOW_MAPS: usize = 2;
/// Слово 11 заголовка сцены, бит 1: солнца нет (как SCENE_NO_SUN в `space.slang`).
const SCENE_NO_SUN: u32 = 2;
/// Слово 11, бит 3: карты ламп смотрят вниз (сцена «3D», режим «треугольники», 8.20в2;
/// [`lamp_down_view_proj`]), и внутри карты тень лампы — только по ней, без лучей. Как
/// SCENE_LAMP_MAPS_DOWN в `space.slang`.
const SCENE_LAMP_MAPS_DOWN: u32 = 8;
/// Слово 11, бит 4: затенение в углах только по полю, с ближней пробой (8.20г, режим
/// «треугольники»). Как SCENE_AO_FIELD_ONLY в `space.slang`.
const SCENE_AO_FIELD_ONLY: u32 = 16;
/// Слово 11, бит 5: карта солнца — кэш неподвижных предметов (опыт Р2, [`set_sun_static`]).
/// Как SCENE_SUN_STATIC в `space.slang`.
const SCENE_SUN_STATIC: u32 = 32;
/// Бит 2 слова 11: нормаль по всей сцене (как SCENE_FULL_NORMAL в `include/march.slang`).
#[cfg(test)]
const SCENE_FULL_NORMAL: u32 = 4;
/// Слов на вершину: место (3 числа), слово материала ([`vertex_word`]) и точка текстуры
/// (8.14б2: два числа половинной точности, [`uv_word`]; 0 — без текстуры).
const VERTEX_WORDS: usize = 5;

/// Точка текстуры вершины: u в младших 16 битах, v в старших, числами половинной
/// точности (как `f16tof32` в шейдере). Хватает и для повторов текстуры: до 2048 точек
/// текстуры дробная часть держится точнее, чем в 1/2 точки.
fn uv_word(uv: glam::Vec2) -> u32 {
    u32::from(f16_bits(uv.x)) | (u32::from(f16_bits(uv.y)) << 16)
}

/// Число половинной точности (IEEE 754 binary16) с округлением к ближайшему; слишком
/// большие — бесконечность, слишком малые — ноль.
fn f16_bits(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x7f_ffff;
    if exp == 0xff {
        // Бесконечность и не-число.
        return sign | 0x7c00 | if mantissa != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        // Денормальное: мантисса с неявной единицей, сдвинутая вправо.
        let m = mantissa | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let rounded = (m + half - 1 + ((m >> shift) & 1)) >> shift;
        return sign | rounded as u16;
    }
    let rounded = mantissa + 0xfff + ((mantissa >> 13) & 1);
    let (e, rounded) = if rounded & 0x80_0000 != 0 {
        (e + 1, 0)
    } else {
        (e, rounded)
    };
    if e >= 31 {
        return sign | 0x7c00;
    }
    sign | ((e as u16) << 10) | (rounded >> 13) as u16
}

/// Есть ли в буфере сцены `words` материал с текстурой (флаг MATERIAL_TEXTURED).
pub(crate) fn has_textures(words: &[u32]) -> bool {
    let (at, count) = (words[2] as usize, words[3] as usize);
    (0..count).any(|m| words[at + m * 8 + 3] & reyvec_sdf::shape3d::MATERIAL_TEXTURED != 0)
}

/// Ставит в записи материалов буфера сцены `words` (pack_scene) вместо номера текстуры
/// сцены (`Material3::texture`, слово 5) номер её картинки в общем списке текстур:
/// `slots[k]` — у текстуры `k + 1`. Текстура без картинки (не загрузилась) снимает флаг
/// MATERIAL_TEXTURED: материал рисуется своим цветом.
pub(crate) fn set_texture_slots(words: &mut [u32], slots: &[Option<u32>]) {
    let (at, count) = (words[2] as usize, words[3] as usize);
    for m in 0..count {
        let w = at + m * 8;
        let texture = words[w + 5] as usize;
        if texture == 0 {
            continue;
        }
        match slots.get(texture - 1).copied().flatten() {
            Some(slot) => words[w + 5] = slot,
            None => {
                words[w + 3] &= !reyvec_sdf::shape3d::MATERIAL_TEXTURED;
                words[w + 5] = 0;
            }
        }
    }
}

/// Слово материала вершины модели. Без нормали — просто номер материала (грань плоская,
/// нормаль шейдер берёт из наклона соседних точек). С нормалью (8.14б, гладкие грани) —
/// раскладка как у `pack_mesh` в `space.slang`: бит 31, 11 бит материала, по 10 бит на
/// две половины нормали (oct_encode).
fn vertex_word(material: u32, normal: Option<Vec3>) -> u32 {
    let Some(n) = normal else {
        return material;
    };
    let e = oct_encode(n).map(|v| ((v * 0.5 + 0.5).clamp(0.0, 1.0) * 1022.0).round() as u32);
    0x8000_0000 | (material.min(2047) << 20) | (e[0] << 10) | e[1]
}

/// Нормаль в два числа −1…1 (октаэдр), как `oct_encode` в `space.slang`.
fn oct_encode(n: Vec3) -> [f32; 2] {
    let n = n / (n.x.abs() + n.y.abs() + n.z.abs()).max(1e-20);
    if n.z >= 0.0 {
        [n.x, n.y]
    } else {
        let sign = |v: f32| if v >= 0.0 { 1.0 } else { -1.0 };
        [(1.0 - n.y.abs()) * sign(n.x), (1.0 - n.x.abs()) * sign(n.y)]
    }
}

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
    /// Коробки объектов для теней через RT-ядра (опыт Р1), по номеру объекта; `None` у
    /// объекта — теней не даёт или считается формулой (плоскость). `None` целиком — в
    /// сцене есть бесконечная форма с тенью (поле уровня), RT-ядрам её не отдать.
    pub ray_boxes: Option<Vec<Option<(glam::Vec3, glam::Vec3)>>>,
    /// Коробки движущихся объектов (опыт Р3); `None` — у движущегося нет границы.
    pub movers: Option<Vec<(glam::Vec3, glam::Vec3)>>,
    /// Ниже этой высоты поверхностей нет (опыт Р3): низ коробок объектов и полы.
    pub floor: f32,
}

/// Самая низкая поверхность сцены: низ коробок объектов (или шаров-границ) и
/// горизонтальные плоскости-полы. Бесконечные формы не учитываются.
pub(crate) fn lowest_point(programs: &[Program]) -> f32 {
    let mut low = f32::MAX;
    for p in programs {
        if let Some((n, c)) = p.plane() {
            if n.y > 0.99 {
                low = low.min(-c / n.y);
            }
            continue;
        }
        if let Some((a, _)) = p.bounds_box {
            low = low.min(a.y);
        } else if let Some((c, r)) = p.bounds {
            low = low.min(c.y - r);
        }
    }
    if low == f32::MAX { 0.0 } else { low }
}

/// Коробки движущихся объектов (не [`Program::still`]): плотная коробка или коробка
/// шара-границы. `None` — у одного из них нет границы (бесконечная форма).
pub(crate) fn mover_boxes(programs: &[Program]) -> Option<Vec<(glam::Vec3, glam::Vec3)>> {
    programs
        .iter()
        .filter(|p| !p.still)
        .map(|p| {
            let sphere = p
                .bounds
                .map(|(c, r)| (c - glam::Vec3::splat(r), c + glam::Vec3::splat(r)));
            p.bounds_box.or(sphere)
        })
        .collect()
}

/// Коробки объектов для теней через RT-ядра: у каждого объекта с тенью — его плотная
/// коробка (или коробка шара-границы); плоскости, которые шейдер считает формулой, и
/// светящиеся без теней — `None`. `None` целиком — есть бесконечная форма с тенью.
pub(crate) fn ray_boxes(programs: &[Program]) -> Option<Vec<Option<(glam::Vec3, glam::Vec3)>>> {
    let planes = reyvec_sdf::shape3d::plane_objects(programs);
    programs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if p.no_shadow || planes.contains(&(i as u32)) {
                return Some(None);
            }
            let sphere = p
                .bounds
                .map(|(c, r)| (c - glam::Vec3::splat(r), c + glam::Vec3::splat(r)));
            p.bounds_box.or(sphere).map(Some)
        })
        .collect()
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
        let n = *words.get(cells + i * reyvec_sdf::grid3d::CELL_WORDS + 1)? & 0xffff;
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
    pack_with(frame, materials, culling, true, GridOptions::default())
}

/// То же; `level_field: false` — поле уровня считается в общем переборе объектов, как до
/// 7.18д3 (для сравнения в тестах); `grid` — как строить сетку отсева (без
/// свободного расстояния — как до 7.18д4, по шарам — как до 8.8а; для сравнения на панели).
#[cfg(test)]
pub(crate) fn pack_with(
    frame: &SpaceFrame,
    materials: &[Material3],
    culling: bool,
    level_field: bool,
    grid: GridOptions,
) -> Packed {
    pack_cached(
        frame,
        materials,
        culling,
        level_field,
        grid,
        &mut PackCache::default(),
    )
}

/// Готовое из прошлых кадров (8.8б): вклад неподвижного в сетку отсева и слова вершин
/// частей моделей, которые не изменились.
#[derive(Default)]
pub(crate) struct PackCache {
    pub grid: reyvec_sdf::grid3d::GridCache,
    meshes: Vec<(reyvec_sdf::space_file::MeshFrame, Vec<u32>)>,
    programs: Vec<CachedProgram>,
    /// С какими границами частей (М.1) собраны запомненные программы.
    part_bounds: bool,
    /// Делились ли тяжёлые формы на детали ([`reyvec_sdf::shape3d::split_parts`]).
    split: bool,
}

/// То же, что [`pack_with`], с готовым из `cache`; слова те же.
pub(crate) fn pack_cached(
    frame: &SpaceFrame,
    materials: &[Material3],
    culling: bool,
    level_field: bool,
    grid: GridOptions,
    cache: &mut PackCache,
) -> Packed {
    // Переключили границы частей (М.1) — запомненные программы и сетка собраны по-старому.
    let part_bounds = reyvec_sdf::shape3d::part_bounds();
    let split = reyvec_sdf::shape3d::split_parts();
    if cache.part_bounds != part_bounds || cache.split != split {
        *cache = PackCache {
            part_bounds,
            split,
            ..PackCache::default()
        };
    }
    let mut programs = programs_cached(frame, Some(&mut cache.programs));
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
    // Буфер геометрии различает MAX_MATERIALS материалов (7.20б): у остальных цвет
    // последнего.
    warn_once(&WARNED_MATERIALS, materials.len() > MAX_MATERIALS, || {
        format!(
            "материалов сцены 3D больше предела: {} из {MAX_MATERIALS}, у лишних цвет последнего",
            materials.len()
        )
    });
    let mut words = pack_scene_cached(
        &programs,
        materials,
        &frame.lamps,
        level_field,
        grid,
        &mut cache.grid,
    );
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
            ray_boxes: None,
            movers: None,
            floor: 0.0,
        };
    }
    if !culling {
        words[6] = 0;
    }
    let grid = grid_stats(&words);
    words.reserve(mesh_words);
    words[7] = words.len() as u32;
    words.extend([(triangles * 3) as u32, 0, 0, 0]);
    words.extend([0u32; MESH_HEADER_WORDS - 4]);
    // Часть модели, такая же, как в прошлом кадре (неподвижный станок), берёт готовые
    // слова: упаковка нормалей и точек текстуры — самое дорогое здесь.
    cache.meshes.truncate(frame.meshes.len());
    for (n, m) in frame.meshes.iter().enumerate() {
        if let Some((source, packed)) = cache.meshes.get(n)
            && source == m
        {
            words.extend_from_slice(packed);
            continue;
        }
        let mut packed = Vec::with_capacity(m.triangles.len() * 3 * VERTEX_WORDS);
        for (i, t) in m.triangles.iter().enumerate() {
            let normals = m.normals.get(i);
            let uvs = m.uvs.get(i);
            for (k, v) in t.iter().enumerate() {
                packed.extend(v.to_array().map(f32::to_bits));
                packed.push(vertex_word(m.material, normals.map(|n| n[k])));
                packed.push(uvs.map_or(0, |u| uv_word(u[k])));
            }
        }
        words.extend_from_slice(&packed);
        let entry = (m.clone(), packed);
        if n < cache.meshes.len() {
            cache.meshes[n] = entry;
        } else {
            cache.meshes.push(entry);
        }
    }
    Packed {
        objects: words[0],
        words,
        vertices: (triangles * 3) as u32,
        grid,
        dropped,
        overflow: None,
        ray_boxes: ray_boxes(&programs),
        movers: mover_boxes(&programs),
        floor: lowest_point(&programs),
    }
}

/// Программы форм кадра: предметы, потом невидимые заменители моделей (7.6ж). Форма,
/// которую не вышло превратить в программу, пропускается с записью в журнал.
pub(crate) fn programs(frame: &SpaceFrame) -> Vec<Program> {
    programs_cached(frame, None)
}

/// Запомненные программы формы (8.8б): форма и что из неё вышло (программы её деталей,
/// [`reyvec_sdf::shape3d::compile_parts`]); `None` — не запоминается.
type CachedProgram = Option<(
    Shape3,
    Result<Vec<Program>, reyvec_sdf::shape3d::CompileError>,
)>;

/// То же; форма, такая же, как на том же месте в прошлом кадре, берёт готовые программы
/// из `cache` (8.8б: неподвижные предметы не превращаются в программу каждый кадр).
/// Тяжёлая модель становится несколькими предметами — по одному на деталь (сетка отсева
/// видит детали). Если с деталями предметов больше [`MAX_OBJECTS`], формы не делятся.
pub(crate) fn programs_cached(
    frame: &SpaceFrame,
    cache: Option<&mut Vec<CachedProgram>>,
) -> Vec<Program> {
    let split = reyvec_sdf::shape3d::split_parts();
    let programs = programs_with(frame, cache, split);
    if split && programs.len() > MAX_OBJECTS {
        return programs_with(frame, None, false);
    }
    programs
}

fn programs_with(
    frame: &SpaceFrame,
    mut cache: Option<&mut Vec<CachedProgram>>,
    split: bool,
) -> Vec<Program> {
    let total = frame.shapes.len() + frame.proxies.len() + frame.glows.len();
    if let Some(cache) = cache.as_mut() {
        cache.truncate(total);
    }
    let compile_one = |s: &Shape3| {
        if split {
            reyvec_sdf::shape3d::compile_parts(s)
        } else {
            compile(s).map(|p| vec![p])
        }
    };
    let mut at = 0;
    let mut compile_cached =
        |s: &Shape3| -> Result<Vec<Program>, reyvec_sdf::shape3d::CompileError> {
            let n = at;
            at += 1;
            let Some(cache) = cache.as_mut() else {
                return compile_one(s);
            };
            if let Some(Some((source, programs))) = cache.get(n)
                && source == s
            {
                return programs.clone();
            }
            let programs = compile_one(s);
            // Формы с полем слоя (и с ошибкой) не запоминаются: копия держала бы поле, и
            // уровень не смог бы перепечь его на месте (`Arc::get_mut`, «поле занято
            // прошлым кадром»).
            let holds_field = programs
                .as_ref()
                .map_or(true, |p| p.iter().any(|p| !p.fields.is_empty()));
            let entry = (!holds_field).then(|| (s.clone(), programs.clone()));
            if n < cache.len() {
                cache[n] = entry;
            } else {
                cache.push(entry);
            }
            programs
        };
    // Флаги неподвижности (8.8б): по одному на предмет, потом на заменитель.
    let still = |i: usize| frame.still.get(i).copied().unwrap_or(false);
    let mut build =
        |shapes: &[Shape3], first: Option<usize>, shadow_only: bool, no_shadow: bool| {
            let mut out = Vec::with_capacity(shapes.len());
            for (i, s) in shapes.iter().enumerate() {
                match compile_cached(s) {
                    Ok(parts) => {
                        for mut p in parts {
                            p.shadow_only = shadow_only;
                            p.no_shadow = no_shadow;
                            p.still = first.is_some_and(|f| still(f + i));
                            out.push(p);
                        }
                    }
                    Err(e) => tracing::warn!("форма 3D пропущена: {e}"),
                }
            }
            out
        };
    let mut programs = build(&frame.shapes, Some(0), false, false);
    programs.extend(build(&frame.proxies, Some(frame.shapes.len()), true, false));
    programs.extend(build(&frame.glows, None, false, true));
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

/// Буфер сеток из форм (8.20б) для видеокарты: слово 0 — где начинаются вершины, потом
/// номера вершин всех сеток подряд (по 3 на треугольник, номера общие для буфера), потом
/// вершины по 4 слова: место и слово материала с нормалью ([`vertex_word`]). Ответ —
/// слова и для каждой сетки её первый номер и число номеров (для `cmd_draw`).
pub(crate) fn shape_mesh_words(
    meshes: &[std::sync::Arc<reyvec_sdf::shape_mesh::ShapeMesh>],
) -> (Vec<u32>, Vec<(u32, u32)>) {
    let indices: usize = meshes.iter().map(|m| m.indices.len()).sum();
    let vertices: usize = meshes.iter().map(|m| m.positions.len()).sum();
    let mut words = Vec::with_capacity(1 + indices + vertices * 4);
    words.push((1 + indices) as u32);
    let mut ranges = Vec::with_capacity(meshes.len());
    let mut base = 0u32;
    for m in meshes {
        ranges.push(((words.len() - 1) as u32, m.indices.len() as u32));
        words.extend(m.indices.iter().map(|&i| base + i));
        base += m.positions.len() as u32;
    }
    for m in meshes {
        for ((p, n), &material) in m.positions.iter().zip(&m.normals).zip(&m.materials) {
            words.extend(p.to_array().map(f32::to_bits));
            words.push(vertex_word(material, Some(*n)));
        }
    }
    (words, ranges)
}

/// Место сетки из формы: столбцы `rot · scale` и сдвиг (12 чисел, как читает
/// `mesh3d.slang`).
pub(crate) fn shape_instance(place: &reyvec_sdf::shape_mesh::Rigid) -> [u32; SHAPE_INSTANCE_WORDS] {
    let m = place.rot * place.scale;
    let mut out = [0u32; SHAPE_INSTANCE_WORDS];
    for (k, v) in m
        .to_cols_array()
        .into_iter()
        .chain(place.at.to_array())
        .enumerate()
    {
        out[k] = v.to_bits();
    }
    out
}

/// Сетки из форм (8.20б) в раздел моделей: адрес их буфера и места (`instances`, по
/// [`SHAPE_INSTANCE_WORDS`] слов) в конец слов сцены. `false` — раздела нет или места в
/// буфере сцены не хватило: сеток в кадре не будет.
pub(crate) fn set_shape_meshes(words: &mut Vec<u32>, address: u64, instances: &[u32]) -> bool {
    let Some(at) = mesh_section(words) else {
        return false;
    };
    if words.len() + instances.len() > MAX_WORDS {
        return false;
    }
    let start = words.len() as u32;
    words.extend_from_slice(instances);
    words[at + MESH_SHAPES] = address as u32;
    words[at + MESH_SHAPES + 1] = (address >> 32) as u32;
    words[at + MESH_SHAPES + 2] = start;
    words[at + MESH_SHAPES + 3] = (instances.len() / SHAPE_INSTANCE_WORDS) as u32;
    true
}

/// Где раздел моделей; `None` — его нет (сцена не поместилась и рисуется пустой, слово 7
/// заголовка — 0): писать в «раздел» тогда значит портить заголовок сцены.
fn mesh_section(words: &[u32]) -> Option<usize> {
    let at = *words.get(7)? as usize;
    (at != 0).then_some(at)
}

/// Раздел коробок для теней через RT-ядра (опыт Р1) в конец слов сцены: адрес верхнего
/// дерева (2 слова), число коробок на уровень, запасное, потом коробки всех уровней
/// ([`crate::rt::box_words`]); его начало — в слово [`MESH_RAY`] раздела моделей.
/// Возвращает, где в словах начинаются коробки (для постройки деревьев), или `None`, если
/// раздела моделей нет или слова не помещаются в буфер (тогда тени идут по сетке).
pub(crate) fn set_ray_boxes(
    words: &mut Vec<u32>,
    tlas: u64,
    boxes: &[Option<(glam::Vec3, glam::Vec3)>],
) -> Option<usize> {
    let at = mesh_section(words)?;
    let body = crate::rt::box_words(boxes);
    // Коробки дерево читает по 8 байт: начало раздела — на чётном слове.
    let start = words.len().next_multiple_of(2);
    if start + 4 + body.len() > MAX_WORDS {
        return None;
    }
    words.resize(start, 0);
    words.extend([tlas as u32, (tlas >> 32) as u32, boxes.len() as u32, 0]);
    words.extend(body);
    words[at + MESH_RAY] = start as u32;
    Some(start + 4)
}

/// Матрица камеры «мир → экран» в раздел моделей (вершинный шейдер «полигонов 3D»).
pub(crate) fn set_view_proj(words: &mut [u32], view_proj: glam::Mat4) {
    if let Some(at) = mesh_section(words) {
        write_matrix(words, at + MESH_VIEW_PROJ, view_proj);
    }
}

/// Камера прошлого кадра для ReSTIR DI (`None` — запасов прошлого кадра нет).
pub(crate) fn set_prev_view_proj(words: &mut [u32], prev: Option<glam::Mat4>) {
    if let Some(at) = mesh_section(words) {
        write_matrix(
            words,
            at + MESH_PREV_VIEW_PROJ,
            prev.unwrap_or(glam::Mat4::IDENTITY),
        );
        if let Some(w) = words.get_mut(at + MESH_PREV_VIEW_PROJ + 16) {
            *w = u32::from(prev.is_some());
        }
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

/// Карт теней солнца (8.20в): ближняя подробная и дальняя грубая (cascaded shadow maps).
pub(crate) const SUN_CASCADES: usize = 2;
/// Границы карт солнца по расстоянию от камеры вдоль взгляда, метры: первая — до 16 м,
/// вторая — до 80 м, дальше тени лучами. Камера над заводом (22 м) видит пол в 25–80 м.
pub(crate) const SUN_SPLITS: [f32; SUN_CASCADES + 1] = [0.0, 16.0, 80.0];

/// Матрица карты солнца для куска взгляда камеры от `from` до `to` метров вдоль взгляда
/// (`aspect` — ширина / высота кадра) на карте в `side` точек: прямоугольная проекция
/// вдоль солнца на шар вокруг этого куска. Радиус шара — целые метры, середина сдвигается
/// целыми точками карты: при движении и повороте камеры размер точки карты не меняется, а
/// края теней не дрожат. Глубина обратная (ближе к солнцу — 1), как у карт ламп.
pub(crate) fn sun_view_proj(
    camera: &reyvec_sdf::Camera3D,
    aspect: f32,
    from: f32,
    to: f32,
    side: u32,
) -> glam::Mat4 {
    use glam::{Mat4, Vec3};
    let sun = reyvec_sdf::view3d::SUN;
    let up = if sun.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let view = Mat4::look_at_rh(Vec3::ZERO, -sun, up);
    let (f, r, u) = (camera.forward(), camera.right(), camera.up());
    let half = |d: f32| match camera.projection {
        reyvec_sdf::camera3d::Projection::Perspective { fov_y } => (fov_y * 0.5).tan() * d,
        reyvec_sdf::camera3d::Projection::Orthographic { height } => height * 0.5,
    };
    let mut corners = Vec::with_capacity(8);
    for d in [from.max(camera.near), to] {
        let (hy, hx) = (half(d), half(d) * aspect);
        for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            corners.push(camera.eye + f * d + r * (sx * hx) + u * (sy * hy));
        }
    }
    let center = corners.iter().copied().sum::<Vec3>() / corners.len() as f32;
    let radius = corners
        .iter()
        .map(|p| p.distance(center))
        .fold(0.0, f32::max)
        .ceil()
        .max(1.0);
    let texel = 2.0 * radius / side.max(1) as f32;
    let c = view.transform_point3(center);
    let (x, y) = ((c.x / texel).round() * texel, (c.y / texel).round() * texel);
    // Принимающие — в шаре, заслоняющие — ещё до 50 м выше к солнцу (по целым метрам:
    // глубина карты тоже не дрожит).
    let z = c.z.round();
    let (near, far) = (-z - radius - 50.0, -z + radius + 1.0);
    Mat4::orthographic_rh(x - radius, x + radius, y - radius, y + radius, far, near) * view
}

/// Карты теней солнца (8.20в, режим «треугольники» 3D): матрицы «мир → карта», по одной на
/// кусок взгляда (не больше [`SUN_CASCADES`]). Без раздела моделей не пишется ничего.
pub(crate) fn set_sun_maps(words: &mut [u32], maps: &[glam::Mat4]) {
    if let Some(at) = mesh_section(words) {
        let count = maps.len().min(SUN_CASCADES);
        words[at + MESH_SUN] = count as u32;
        for (k, m) in maps.iter().take(count).enumerate() {
            write_matrix(words, at + MESH_SUN + 1 + 16 * k, *m);
        }
    }
}

/// Угол карты тени лампы сцены «3D» (8.20в2), градусы: лампы висят под потолком и светят
/// вниз, широкий угол накрывает почти всё, что они освещают; выше лампы — тень лучами.
const LAMP_DOWN_FOV_DEG: f32 = 140.0;

/// Матрица «мир → карта тени» лампы сцены «3D» (8.20в2): лампа смотрит вниз (вдоль −Y,
/// «верх» карты — −Z), глубина обратная, как у карт ламп уровня. Оси вида: x — X, y — −Z,
/// z — Y (так `shadow3d.slang` поворачивает нормаль в карту).
pub(crate) fn lamp_down_view_proj(lamp: &reyvec_sdf::Lamp3) -> glam::Mat4 {
    use glam::{Mat4, Vec3};
    let at = Vec3::from(lamp.at);
    let view = Mat4::look_at_rh(at, at - Vec3::Y, -Vec3::Z);
    let far = lamp.range.max(1.0) + 1.0;
    let proj = Mat4::perspective_rh(LAMP_DOWN_FOV_DEG.to_radians(), 1.0, far, 0.05);
    proj * view
}

/// Карты ламп смотрят вниз (8.20в2, см. [`lamp_down_view_proj`]).
pub(crate) fn set_lamp_maps_down(words: &mut [u32]) {
    words[11] |= SCENE_LAMP_MAPS_DOWN;
}

/// Изменившиеся квадраты «теней 3D» (опыт Р3) в конец слов сцены: квадратов по ширине,
/// потом биты квадратов по строкам (1 — пересчитать). Шейдер пропускает точки квадратов
/// с 0: их значения уже в картинке из прошлого кадра. `false` — не влезло.
pub(crate) fn set_dirty_tiles(words: &mut Vec<u32>, across: u32, bits: &[u32]) -> bool {
    let Some(at) = mesh_section(words) else {
        return false;
    };
    if words.len() + 1 + bits.len() > MAX_WORDS {
        return false;
    }
    words[at + MESH_DIRTY] = words.len() as u32;
    words.push(across);
    words.extend_from_slice(bits);
    true
}

/// Какие квадраты «теней 3D» (по [`DIRTY_TILE`] точек половинного размера) могут
/// измениться, если камера стоит, а двигаются только объекты в коробках `movers`
/// (опыт Р3). Квадрат меняется, если в нём видна сама коробка с запасом на затенение в
/// углах, её тень от солнца (коробка, вытянутая от солнца до самого низа сцены `floor`, с
/// запасом на полутень) или тень от двух ламп с тенями (вытянутая от лампы до её
/// дальности). Лампы с тенями из `moved_lamps` (место и дальность) сдвинулись: меняется
/// всё в шаре их дальности. `full` — размер кадра лучей 3D в пикселях; результат — биты
/// квадратов по строкам и квадратов по ширине. `None` — меняется всё (коробка за спиной
/// камеры).
pub(crate) fn dirty_tiles(
    movers: &[(Vec3, Vec3)],
    lamps: &[reyvec_sdf::Lamp3],
    moved_lamps: &[(Vec3, f32)],
    view_proj: glam::Mat4,
    full: (u32, u32),
    floor: f32,
) -> Option<(u32, Vec<u32>)> {
    let half = (full.0.div_ceil(2), full.1.div_ceil(2));
    let across = half.0.div_ceil(DIRTY_TILE);
    let down = half.1.div_ceil(DIRTY_TILE);
    let mut bits = vec![0u32; (across * down).div_ceil(32) as usize];
    let sun = reyvec_sdf::view3d::SUN;
    // Затенение в углах дотягивается на 0,4 м (пробы ambient_occlusion), запас — до метра.
    const NEAR: f32 = 1.0;
    let corners = |a: Vec3, b: Vec3| {
        (0..8).map(move |k| {
            Vec3::new(
                if k & 1 == 0 { a.x } else { b.x },
                if k & 2 == 0 { a.y } else { b.y },
                if k & 4 == 0 { a.z } else { b.z },
            )
        })
    };
    // Области: коробка движущегося (с тенями) или шар дальности сдвинувшейся лампы.
    let spheres = moved_lamps.iter().map(|&(at, range)| {
        let r = Vec3::splat(range + NEAR);
        (corners(at - r, at + r).collect::<Vec<Vec3>>(), true)
    });
    let boxes = movers.iter().map(|&(a, b)| (vec![a, b], false));
    for (shape, sphere) in boxes.chain(spheres) {
        let mut points: Vec<Vec3> = Vec::new();
        if sphere {
            points = shape;
        } else {
            let (a, b) = (shape[0] - Vec3::splat(NEAR), shape[1] + Vec3::splat(NEAR));
            points.extend(corners(a, b));
            // Солнце: тень падает от солнца до низа сцены; полутень шире с расстоянием (k = 12).
            let reach = ((b.y - floor).max(0.0) / sun.y.max(0.05)).min(30.0);
            let soft = Vec3::splat(reach / 12.0 + 0.1);
            let (sa, sb) = (a - sun * reach - soft, b - sun * reach + soft);
            points.extend(corners(sa.min(sb), sa.max(sb)));
            for lamp in lamps.iter().take(reyvec_sdf::shape3d::SHADOW_LAMPS) {
                let at = Vec3::from(lamp.at);
                let range = lamp.range.max(0.0);
                // Дальше дальности лампа не светит: и тени от неё нет.
                if at.clamp(a, b).distance(at) > range {
                    continue;
                }
                for c in corners(a, b) {
                    let away = c - at;
                    let far = range.max(away.length());
                    let end = at + away.normalize_or_zero() * far;
                    // Полутень лампы (k = 10) — до десятой доли пути.
                    let pad = Vec3::splat(far / 10.0 + 0.1);
                    points.extend(corners(end - pad, end + pad));
                }
            }
        }
        let (mut lo, mut hi) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
        for p in points {
            let clip = view_proj * p.extend(1.0);
            if clip.w <= 0.01 {
                return None;
            }
            let ndc = glam::Vec2::new(clip.x, clip.y) / clip.w;
            let px = glam::Vec2::new(
                (ndc.x * 0.5 + 0.5) * full.0 as f32,
                (0.5 - ndc.y * 0.5) * full.1 as f32,
            ) * 0.5;
            lo = lo.min(px);
            hi = hi.max(px);
        }
        // Пиксель на запас: точка тени берёт пиксель кадра (2x, 2y).
        let tile = DIRTY_TILE as f32;
        let x0 = ((lo.x - 2.0) / tile).floor().max(0.0) as u32;
        let y0 = ((lo.y - 2.0) / tile).floor().max(0.0) as u32;
        let x1 = ((hi.x + 2.0) / tile).floor().min(across as f32 - 1.0);
        let y1 = ((hi.y + 2.0) / tile).floor().min(down as f32 - 1.0);
        if x1 < 0.0 || y1 < 0.0 {
            continue;
        }
        for y in y0..=y1 as u32 {
            for x in x0..=x1 as u32 {
                let i = y * across + x;
                bits[(i / 32) as usize] |= 1 << (i % 32);
            }
        }
    }
    Some((across, bits))
}

/// Карта солнца — кэш неподвижных предметов (опыт Р2): внутри неё тень солнца — меньшее из
/// карты и живого луча по движущимся предметам. Матрицу карты пишет [`set_sun_maps`].
pub(crate) fn set_sun_static(words: &mut [u32]) {
    words[11] |= SCENE_SUN_STATIC;
}

/// Неподвижное кадра (опыт Р2): неподвижные предметы и заменители моделей, без ламп,
/// сеток и светящихся. Из него печётся кэш солнца.
pub(crate) fn still_frame(frame: &SpaceFrame) -> SpaceFrame {
    let shapes: Vec<_> = still_shapes(frame, 0, &frame.shapes).cloned().collect();
    let proxies: Vec<_> = still_shapes(frame, frame.shapes.len(), &frame.proxies)
        .cloned()
        .collect();
    let count = shapes.len() + proxies.len();
    SpaceFrame {
        shapes,
        proxies,
        lamps: Vec::new(),
        meshes: Vec::new(),
        glows: Vec::new(),
        still: vec![true; count],
    }
}

/// Неподвижные из `shapes` (флаги кадра с номера `first`).
fn still_shapes<'a>(
    frame: &'a SpaceFrame,
    first: usize,
    shapes: &'a [Shape3],
) -> impl Iterator<Item = &'a Shape3> + 'a {
    shapes
        .iter()
        .enumerate()
        .filter(move |(i, _)| frame.still.get(first + i).copied().unwrap_or(false))
        .map(|(_, s)| s)
}

/// Неподвижное кадра то же, что `still` ([`still_frame`]), без копирования форм.
pub(crate) fn same_still(frame: &SpaceFrame, still: &SpaceFrame) -> bool {
    still_shapes(frame, 0, &frame.shapes).eq(still.shapes.iter())
        && still_shapes(frame, frame.shapes.len(), &frame.proxies).eq(still.proxies.iter())
}

/// Матрица кэша солнца (опыт Р2): прямоугольная проекция вдоль солнца на все предметы с
/// тенью из `programs` (плоскости — нет: их живой луч считает формулой), глубина обратная
/// (ближе к солнцу — 1), как у [`sun_view_proj`]. Квадрат карты накрывает их коробки с
/// запасом в метр. `None` — таких предметов нет или у одного нет границы.
pub(crate) fn sun_cache_view_proj(programs: &[Program]) -> Option<glam::Mat4> {
    use glam::Mat4;
    let sun = reyvec_sdf::view3d::SUN;
    let up = if sun.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let view = Mat4::look_at_rh(Vec3::ZERO, -sun, up);
    let boxes = ray_boxes(programs)?;
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (a, b) in boxes.into_iter().flatten() {
        for k in 0..8 {
            let corner = Vec3::new(
                if k & 1 == 0 { a.x } else { b.x },
                if k & 2 == 0 { a.y } else { b.y },
                if k & 4 == 0 { a.z } else { b.z },
            );
            let v = view.transform_point3(corner);
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    if lo.x > hi.x || !lo.is_finite() || !hi.is_finite() {
        return None;
    }
    let center = (lo + hi) * 0.5;
    let half = ((hi.x - lo.x).max(hi.y - lo.y) * 0.5 + 1.0).ceil();
    // Вид смотрит вдоль −Z: дальше от солнца — меньше z.
    let (near, far) = (-hi.z - 1.0, -lo.z + 1.0);
    Some(
        Mat4::orthographic_rh(
            center.x - half,
            center.x + half,
            center.y - half,
            center.y + half,
            far,
            near,
        ) * view,
    )
}

/// Все пробы затенения в углах — по полю, где оно есть (8.20г).
pub(crate) fn set_ao_field_only(words: &mut [u32]) {
    words[11] |= SCENE_AO_FIELD_ONLY;
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

    /// Карта лампы сцены «3D» смотрит вниз, оси вида — как поворачивает нормаль шейдер.
    #[test]
    fn lamp_down_view_axes_match_shader() {
        use glam::Vec3;
        let lamp = reyvec_sdf::Lamp3 {
            at: [1.0, 5.0, 2.0],
            color: [1.0; 3],
            power: 1.0,
            range: 10.0,
        };
        let m = lamp_down_view_proj(&lamp);
        // Прямо под лампой — середина карты, ближе к лампе — глубина больше.
        let below = m.project_point3(Vec3::new(1.0, 0.0, 2.0));
        assert!(below.x.abs() < 1e-4 && below.y.abs() < 1e-4, "{below}");
        let higher = m.project_point3(Vec3::new(1.0, 3.0, 2.0));
        assert!(higher.z > below.z);
        // Оси вида, как в shadow3d.slang: x — X, y — −Z.
        let east = m.project_point3(Vec3::new(2.0, 0.0, 2.0));
        let south = m.project_point3(Vec3::new(1.0, 0.0, 3.0));
        assert!(east.x > 0.0 && east.y.abs() < 1e-4, "{east}");
        assert!(south.y < 0.0 && south.x.abs() < 1e-4, "{south}");
    }

    /// Карты солнца: середина куска взгляда — внутри карты, точка ближе к солнцу — с большей
    /// глубиной (обратная глубина), сдвиг камеры меньше точки карты карту не двигает.
    #[test]
    fn sun_map_covers_view_and_snaps() {
        use glam::Vec3;
        let camera = reyvec_sdf::Camera3D::looking_at(Vec3::new(0.0, 22.0, 30.0), Vec3::ZERO);
        let aspect = 1.9;
        for k in 0..SUN_CASCADES {
            let (from, to) = (SUN_SPLITS[k], SUN_SPLITS[k + 1]);
            let m = sun_view_proj(&camera, aspect, from, to, 1024);
            // Середина куска взгляда — на карте, точка выше к солнцу — ближе (глубина
            // больше) и тоже в пределах глубины.
            let mid = camera.eye + camera.forward() * (from + to) * 0.5;
            let p = m.project_point3(mid);
            assert!(
                p.x.abs() < 0.9 && p.y.abs() < 0.9 && (0.0..=1.0).contains(&p.z),
                "{p}"
            );
            let above = m.project_point3(mid + reyvec_sdf::view3d::SUN * 20.0);
            assert!(above.z > p.z && above.z <= 1.0, "{above} {p}");
            // Сдвиг камеры меньше точки карты матрицу не меняет.
            let mut moved = camera;
            moved.eye += Vec3::X * 0.001;
            assert_eq!(m, sun_view_proj(&moved, aspect, from, to, 1024));
        }
    }

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
        set_sun_maps(&mut set, &[glam::Mat4::IDENTITY]);
        assert_eq!(set, empty, "без раздела моделей заголовок цел");
    }

    /// Тяжёлые модели делятся на детали-предметы, а все сцены 3D всё равно помещаются в
    /// предел предметов шейдера (иначе формы не делятся, и каждый кадр собирается дважды).
    #[test]
    fn scenes_split_into_parts_fit() {
        let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scenes/3d"));
        let scenes = [
            ("factory", include_str!("../../../scenes/3d/factory.ron")),
            ("forge", include_str!("../../../scenes/3d/forge.ron")),
            ("mirrors", include_str!("../../../scenes/3d/mirrors.ron")),
            ("mixed", include_str!("../../../scenes/3d/mixed.ron")),
            ("models", include_str!("../../../scenes/3d/models.ron")),
            ("space", include_str!("../../../scenes/3d/space.ron")),
            ("stress", include_str!("../../../scenes/3d/stress.ron")),
            ("vector", include_str!("../../../scenes/3d/vector.ron")),
            (
                "warehouse",
                include_str!("../../../scenes/3d/warehouse.ron"),
            ),
        ];
        for (name, text) in scenes {
            let scene = reyvec_sdf::SpaceScene::parse_in(text, Some(dir)).unwrap();
            let frame = scene.frame(1.0);
            let split = programs_with(&frame, None, true).len();
            let whole = programs_with(&frame, None, false).len();
            eprintln!("{name}: предметов {whole}, с деталями {split}");
            assert!(split <= MAX_OBJECTS, "{name}: {split} предметов");
            if name == "warehouse" {
                assert_eq!((whole, split), (9, 1 + 8 * 59));
            }
        }
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

    /// Кэш солнца (опыт Р2): в неподвижное идут только неподвижные предметы, сравнение
    /// видит сдвиг одного из них, а карта накрывает их все, ближе к солнцу — больше глубина.
    #[test]
    fn sun_cache_takes_still_shapes_and_covers_them() {
        let sphere = |x: f32| Shape3::Sphere { radius: 0.5 }.placed([x, 0.5, 0.0], [0.0; 3]);
        let frame = SpaceFrame {
            shapes: vec![sphere(-5.0), sphere(0.0), sphere(7.0)],
            lamps: Vec::new(),
            meshes: Vec::new(),
            proxies: Vec::new(),
            glows: Vec::new(),
            still: vec![true, false, true],
        };
        let still = still_frame(&frame);
        assert_eq!(still.shapes, vec![sphere(-5.0), sphere(7.0)]);
        assert!(same_still(&frame, &still));
        let mut moved = frame.clone();
        moved.shapes[1] = sphere(1.0);
        assert!(
            same_still(&moved, &still),
            "двигается движущееся — карта та же"
        );
        moved.shapes[2] = sphere(7.5);
        assert!(!same_still(&moved, &still));
        let words = pack_scene(&programs(&frame), &[], &[]);
        let flags = |i: usize| words[words[1] as usize + i * 8 + 2];
        let still_flag = reyvec_sdf::shape3d::OBJECT_STILL;
        assert_eq!(
            [flags(0), flags(1), flags(2)].map(|f| f & still_flag),
            [still_flag, 0, still_flag]
        );
        let m = sun_cache_view_proj(&programs(&still)).expect("карта");
        for x in [-5.0, 7.0] {
            let c = m * glam::Vec4::new(x, 0.5, 0.0, 1.0);
            assert!(
                c.x.abs() < 1.0 && c.y.abs() < 1.0 && (0.0..=1.0).contains(&c.z),
                "{c}"
            );
        }
        let sun = reyvec_sdf::view3d::SUN;
        let near = m * (Vec3::new(0.0, 0.5, 0.0) + sun * 0.4).extend(1.0);
        let far = m * (Vec3::new(0.0, 0.5, 0.0) - sun * 0.4).extend(1.0);
        assert!(near.z > far.z);
    }

    /// Тени из прошлого кадра (опыт Р3): ящик в кадре метит квадраты вокруг себя и своей
    /// тени, но не весь кадр; ящик за спиной камеры — весь кадр; лампа, сдвинувшаяся
    /// далеко за кадром, кадр не трогает.
    #[test]
    fn dirty_tiles_cover_mover_and_its_shadow() {
        let camera = reyvec_sdf::Camera3D::looking_at(Vec3::new(0.0, 10.0, 15.0), Vec3::ZERO);
        let view_proj = camera.view_proj(16.0 / 9.0);
        let full = (960, 540);
        let mover = (Vec3::new(-0.2, 0.0, -0.2), Vec3::new(0.2, 0.4, 0.2));
        let (across, bits) =
            dirty_tiles(&[mover], &[], &[], view_proj, full, 0.0).expect("в кадре");
        let ones: u32 = bits.iter().map(|w| w.count_ones()).sum();
        let total = across * 270u32.div_ceil(DIRTY_TILE);
        assert!(ones > 0 && ones * 4 < total, "{ones} из {total}");
        // Середина кадра (там ящик) помечена.
        let (x, y) = (240 / DIRTY_TILE, 135 / DIRTY_TILE);
        let i = y * across + x;
        assert_eq!((bits[(i / 32) as usize] >> (i % 32)) & 1, 1);
        let behind = (Vec3::new(-0.2, 9.8, 16.0), Vec3::new(0.2, 10.2, 16.5));
        assert!(dirty_tiles(&[behind], &[], &[], view_proj, full, 0.0).is_none());
        let far_lamp = [(Vec3::new(500.0, 4.0, 0.0), 5.0)];
        let (_, none) = dirty_tiles(&[], &[], &far_lamp, view_proj, full, 0.0).expect("в кадре");
        assert!(none.iter().all(|&w| w == 0));
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
        // Один треугольник — 15 слов; столько треугольников точно не поместится.
        let triangles = MAX_WORDS / (3 * VERTEX_WORDS) + 1;
        let t = [glam::Vec3::ZERO, glam::Vec3::X, glam::Vec3::Y];
        frame.meshes.push(reyvec_sdf::space_file::MeshFrame {
            triangles: vec![t; triangles],
            material: 0,
            ..Default::default()
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
        // Неподвижность (опыт Р2) здесь не проверяется.
        let flag = |i: usize| w[objects + i * 8 + 2] & !reyvec_sdf::shape3d::OBJECT_STILL;
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

    #[test]
    fn vertex_word_packs_normal_like_pack_mesh() {
        // Без нормали — просто номер материала (бит 31 пуст: шейдер берёт плоскую грань).
        assert_eq!(vertex_word(7, None), 7);
        // С нормалью: бит 31, материал в битах 20…30, половины нормали по 10 бит.
        let decode = |w: u32| {
            let e = [(w >> 10) & 1023, w & 1023].map(|v| v as f32 / 1022.0 * 2.0 - 1.0);
            let mut n = Vec3::new(e[0], e[1], 1.0 - e[0].abs() - e[1].abs());
            if n.z < 0.0 {
                let sign = |v: f32| if v >= 0.0 { 1.0 } else { -1.0 };
                let (x, y) = (n.x, n.y);
                n.x = (1.0 - y.abs()) * sign(x);
                n.y = (1.0 - x.abs()) * sign(y);
            }
            n.normalize()
        };
        for n in [
            Vec3::X,
            -Vec3::Y,
            -Vec3::Z,
            Vec3::new(0.3, -0.5, -0.8).normalize(),
            Vec3::new(-0.7, 0.1, 0.7).normalize(),
        ] {
            let w = vertex_word(5, Some(n));
            assert_eq!(w >> 31, 1);
            assert_eq!((w >> 20) & 2047, 5);
            assert!(decode(w).dot(n) > 0.9999, "{n} -> {}", decode(w));
        }
        // Номер материала больше 11 бит упирается в 2047, как у pack_mesh.
        assert_eq!((vertex_word(5000, Some(Vec3::Y)) >> 20) & 2047, 2047);
    }
}
