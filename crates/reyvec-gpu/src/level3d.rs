//! 2.5D-вид уровня (этап 7.9): тот же уровень из файла сцены (этап 6), только
//! нарисованный реймарчингом в объёме. Игра не меняется: логика, столкновения и герой те
//! же, меняется лишь картинка — каждый кадр снимок мира превращается в объёмную сцену.
//!
//! Как плоское становится объёмным:
//! - **твёрдое** (столкновения уровня: пол, стены, полки, тайлы) — бруски глубиной
//!   [`SOLID_BACK`]…[`SOLID_FRONT`] по Z. По ним герой и ходит, поэтому в объёме видно,
//!   на что он встаёт;
//! - **фигуры** векторного слоя (герой, предметы, рисунок тайлов) — выдавленные пластины
//!   толщиной [`PLATE`] у переднего края брусков. Порядок рисования сохраняется: фигура,
//!   накрывающая уже поставленные, на [`LAYER_STEP`] ближе к камере самой передней из них,
//!   как в 2D она рисуется поверх; не перекрывающиеся делят одну лицевую грань (7.18а);
//! - **лампы** — лампы объёмной сцены перед уровнем; первые две (ближние к камере)
//!   отбрасывают тени;
//! - **SDF-объекты** (капли слизи, лампочки) — капсулы, светящиеся остаются светящимися;
//! - сзади **стена**, снизу **земля**.
//!
//! Плоская сцена в своих единицах (герой ростом 0,28), а свет и тени объёма настроены на
//! метры (затенение в углах видит на метр вокруг). Поэтому уровень увеличен в
//! [`LEVEL_SCALE`] раз: герой около 1,1 м.
//!
//! Форма фигуры (её поле расстояний) готовится один раз на фигуру библиотеки и потом
//! только переставляется: сдвиг, поворот, размер и отражение героя, бегущего влево.
//!
//! Два способа рисовать (7.9б, [`LevelRender`]):
//! - **SDF** — всё выше формами реймарчинга: точные мягкие тени и затенение в углах, но
//!   каждый шаг луча перебирает плитки рядом, а в комнате их десятки (встроенной видеокарте
//!   не по силам);
//! - **треугольники** — плитки и бруски становятся призмами из треугольников (лицевая
//!   сторона и бока; задняя не видна ни камере, ни лампам перед уровнем), их видеокарта
//!   рисует растеризацией, а тени от них — картами теней двух ближних ламп. Формами SDF
//!   остаются стена, земля и SDF-объекты (слизь, огоньки);
//! - **поле** (7.18в, `docs/decisions/0020-level-field.md`) — всё неподвижное (плиты без
//!   метки «движется» и неподвижное твёрдое) запечено в одно поле слоя: одна форма вместо
//!   сотни, тени и затенение в углах те же. Подвижное (герой, лифт, стрелки, монеты) —
//!   формами, как в SDF. Пока поле печётся в потоке загрузки, кадр рисуется как в SDF.

use std::collections::HashMap;
use std::sync::Arc;

use std::sync::atomic::{AtomicBool, Ordering};

use glam::{Vec2, Vec3};
use reyvec_sdf::layer_bake::{BakeStats, Flat2, LayerBake, Placed2};
use reyvec_sdf::layer_field::SecondLayer;
use reyvec_sdf::profile2d::Profile;
use reyvec_sdf::shape3d::{FieldRef, Shape3};
use reyvec_sdf::space_file::{MeshFrame, SpaceFrame};
use reyvec_sdf::{Camera3D, Lamp3, Material3};
use reyvec_vector::{
    Affine, BezPath, Color, Paint, PathEl, SceneItem, ShapeId, ShapeLibrary, kurbo,
};

/// Во сколько раз уровень больше в объёме, чем на плоскости.
pub(crate) const LEVEL_SCALE: f32 = 4.0;
/// Передний и задний края твёрдого, метры по Z.
pub(crate) const SOLID_FRONT: f32 = 1.0;
pub(crate) const SOLID_BACK: f32 = -0.6;
/// Толщина пластины фигуры, метры.
const PLATE: f32 = 0.3;
/// Насколько фигура ближе к камере, чем самая передняя из накрытых ею, метры.
const LAYER_STEP: f32 = 0.002;
/// Ширина полосы, по которым разложены поставленные плиты (единицы 2D, 4 м в объёме).
const LAYER_STRIP: f64 = 1.0;
/// Плита шире стольких полос — «широкая»: её проверяют всегда, а не по полосам.
const LAYER_WIDE: i64 = 64;
/// Касание краями — не перекрытие: соседние плитки ряда делят лицевую грань (единицы 2D).
const LAYER_TOUCH: f64 = 1e-4;
/// Самая мелкая клетка поля фигуры в единицах плоской сцены (4 мм в объёме).
const PROFILE_CELL: f32 = 0.001;
/// Самая мелкая клетка поля, если фигура мелкая: поле не больше нужного.
const MIN_PROFILE_CELLS: f32 = 24.0;
/// Наклон камеры вниз, градусы: видно верх брусков, но вид остаётся «сбоку».
const CAMERA_PITCH_DEG: f32 = 12.0;
/// Ламп в кадре не больше (как в сцене «3D» из файла: свет перебирается на каждую точку).
const MAX_LAMPS: usize = 8;
/// Где по Z стоят лампы, метры: перед уровнем, чтобы светили на лицевые стороны.
const LAMP_Z: f32 = SOLID_FRONT + 0.8;
/// Светящийся объект ближе этого к лампе (единицы 2D) — её огонёк.
const LAMP_NEAR: f32 = 0.05;
/// Больше форм в кэше не держим: после смены сцены старые фигуры не нужны.
const MAX_CACHED: usize = 8192;

/// Цвета фона «лаборатории» (каменная стена, тёмная земля) и твёрдого (тёсаный камень),
/// линейные.
const WALL: [f32; 3] = [0.10, 0.075, 0.06];
const GROUND: [f32; 3] = [0.035, 0.028, 0.022];
const SOLID: [f32; 3] = [0.10, 0.085, 0.07];

/// Форма фигуры в её собственных координатах, без толщины.
#[derive(Clone, Debug)]
enum Flat {
    /// Прямоугольник по осям: середина и размер (самый частый случай — без поля).
    Rect { center: [f32; 2], size: [f32; 2] },
    /// Любой контур: запечённое поле.
    Profile(Profile),
}

/// Как рисуется 2.5D-вид уровня (7.9б).
///
/// `Default` — SDF: эталон для снимков и тестов. В окне способ выбирается по видеокарте
/// ([`LevelRender::for_adapter`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LevelRender {
    /// Всё формами SDF: точные мягкие тени и затенение в углах. С 7.18д — только для
    /// сравнения.
    #[default]
    Sdf,
    /// Плитки и бруски — треугольники с картами теней, SDF только у мягких предметов.
    Triangles,
    /// Неподвижное запечено в одно поле слоя (7.18в), подвижное — формами SDF.
    Field,
}

impl LevelRender {
    /// Все способы по порядку (для панели): сначала основные, SDF — последним.
    pub const ALL: [Self; 3] = [Self::Field, Self::Triangles, Self::Sdf];

    /// Название для панели и журнала.
    pub fn title(self) -> &'static str {
        match self {
            Self::Sdf => "SDF (сравнение)",
            Self::Triangles => "быстро (треугольники)",
            Self::Field => "честные тени (поле)",
        }
    }

    /// По имени из командной строки: `sdf`, `field` (`поле`), `triangles`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "sdf" | "SDF" => Some(Self::Sdf),
            "field" | "поле" => Some(Self::Field),
            "triangles" | "треугольники" => Some(Self::Triangles),
            _ => None,
        }
    }

    /// По умолчанию для видеокарты (7.18д): на отдельной — поле, на встроенной —
    /// треугольники, на остальных (программная, виртуальная) — SDF, как в тестах.
    pub fn for_adapter(kind: crate::adapter::AdapterKind) -> Self {
        match kind {
            crate::adapter::AdapterKind::Discrete => Self::Field,
            crate::adapter::AdapterKind::Integrated => Self::Triangles,
            _ => Self::Sdf,
        }
    }
}

/// Треугольники фигуры в её координатах: лицевая сторона (по три точки) и контуры (для
/// боков призмы).
#[derive(Debug, Default)]
struct FlatMesh {
    triangles: Vec<[f32; 2]>,
    outlines: Vec<Vec<[f32; 2]>>,
}

/// 2.5D-вид: формы фигур, готовые к перестановке.
#[derive(Default)]
pub(crate) struct Level3d {
    flats: HashMap<ShapeId, Option<Flat>>,
    meshes: HashMap<ShapeId, Option<Arc<FlatMesh>>>,
    /// Прямоугольник фигуры в её координатах (для слоёв плит); общий для обоих способов
    /// рисовать, чтобы слои совпадали.
    bounds: HashMap<ShapeId, Option<kurbo::Rect>>,
    /// Поле уровня (7.18в).
    field: FieldState,
}

/// Полоса поля уровня `B`, метры (0020): дальше от неподвижного поле только оценка.
const FIELD_BAND: f32 = 1.0;
/// Шаги сетки поля уровня по порядку: 1 см, а если уровень не влезает — 2 см.
const FIELD_CELLS: [f32; 2] = [0.01, 0.02];
/// Сторона картинки поля не больше (0020), даже если видеокарта может больше.
pub(crate) const FIELD_MAX_SIDE: u32 = 8192;
/// Новая или изменившаяся неподвижная фигура (без метки «движется») входит в поле, когда
/// простоит столько кадров: иначе то, что меняется каждый кадр, перепекалось бы каждый
/// кадр. До того она рисуется формой.
const FIELD_STABLE_FRAMES: u32 = 30;

/// Сколько отказанных наборов помнит поле уровня.
const FIELD_FAILED_KEEP: usize = 8;
/// Возвраты в поле — не больше стольких точек сетки за кадр (0020, 7.18г): кусок вокруг
/// фигуры в 1 м при полосе 1 м — около 90 000 точек, доли миллисекунды на процессоре.
const FIELD_ADD_POINTS: usize = 50_000;
/// Ушедшее из поля перепекается в тот же кадр, если кусков вместе не больше стольких
/// точек (около 10 × 10 м при клетке 1 см); больше — поле выключается до полного
/// запекания в фоне, кадр — как в SDF.
const FIELD_REMOVE_POINTS: usize = 1_000_000;

/// Поле уровня на панели (7.18в).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum LevelFieldStatus {
    /// Способ рисовать — не «поле».
    #[default]
    Off,
    /// Поле печётся (или ждёт, пока неподвижное перестанет меняться): кадр — как в SDF.
    Baking { reason: &'static str },
    /// Поле готово и рисуется.
    Ready {
        /// Точек сетки по X и по Y и её шаг, метры.
        size: (u32, u32),
        cell: f32,
        /// Плит и брусков в поле; плит, нарисованных формами (подвижные).
        plates: usize,
        solids: usize,
        shapes: usize,
        /// Сколько пеклось в фоне, мс, и сколько занимают картинки, МБ.
        bake_ms: f64,
        megabytes: f64,
        /// Последнее перепекание на месте (7.18г): кусков, точек и мс на процессоре.
        patched: Option<FieldPatched>,
        /// Поле целиком печётся заново в фоне (новая фигура за краем сетки); пока —
        /// старое поле и эта фигура формой.
        rebaking: bool,
    },
    /// Поля нет: кадр — как в SDF.
    Failed(&'static str),
}

/// Перепекание поля на месте (7.18г) для панели.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FieldPatched {
    pub pieces: usize,
    pub points: usize,
    pub ms: f64,
}

/// Кусок поля уровня, перепечённый в кадре: его надо загрузить на видеокарту в обе
/// картинки поля запроса `ticket` до проходов 3D этого кадра. `area` — `[i0, j0, w, h]`
/// в точках сетки (строки снизу вверх).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldPatch {
    pub ticket: u64,
    pub area: [u32; 4],
}

/// Прямоугольник фигуры в метрах уровня (левый нижний и правый верхний углы).
type Rect2 = (Vec2, Vec2);

/// Поле уровня: готовое, в работе и запрос на запекание.
#[derive(Default)]
struct FieldState {
    baked: Option<Baked>,
    pending: Option<Pending>,
    request: Option<(u64, LayerBake)>,
    next_ticket: u64,
    /// Неподвижное, которого нет в поле: сколько кадров подряд оно стоит на месте.
    fresh: HashMap<u64, u32>,
    /// Сколько раз неподвижная плита выходила из поля, оказавшись над подвижным или над
    /// формой (нижняя плита изменилась): каждый раз ждать возврата вдвое дольше
    /// (подвижное, качающееся у края большой плиты, или мигающая без пометки нижняя плита
    /// не должны перепекать её по кругу).
    bounces: HashMap<u64, u32>,
    /// Набор неподвижного (подпись), для которого поля не будет, и почему.
    /// Последние наборы (до [`FIELD_FAILED_KEEP`]), для которых поля не будет: набор,
    /// который то появляется, то пропадает (подвижное заезжает под плиту), не просится
    /// снова при каждом возврате.
    failed: Vec<(u64, &'static str)>,
    /// Куски, перепечённые в кадре и ещё не отданные видеокарте.
    patches: Vec<FieldPatch>,
    /// Сторона картинки поля не больше (0 — [`FIELD_MAX_SIDE`]).
    max_side: u32,
    status: LevelFieldStatus,
}

/// Готовое поле: какие плиты и бруски в нём (ключ — экземпляр, значение — его
/// прямоугольник) и с какими материалами.
struct Baked {
    ticket: u64,
    field: FieldRef,
    materials: Materials,
    plates: HashMap<u64, Rect2>,
    solids: HashMap<u64, Rect2>,
    /// Плиты поля по порядку рисования: от него зависит, чей цвет там, где они
    /// перекрываются.
    order: Vec<u64>,
    bake_ms: f64,
    patched: Option<FieldPatched>,
}

/// Поле в работе.
struct Pending {
    ticket: u64,
    materials: Materials,
    plates: HashMap<u64, Rect2>,
    solids: HashMap<u64, Rect2>,
    order: Vec<u64>,
    /// Подпись набора, из которого печётся поле (для [`Level3d::field_failed`]).
    signature: u64,
    cancel: Arc<AtomicBool>,
    reason: &'static str,
}

/// Форма кадра 2.5D-вида в режиме «поле»: подвижная, неподвижный брусок или
/// неподвижная плита (с ключом экземпляра).
#[derive(Clone, Copy)]
enum Still {
    Moving,
    Solid(u64),
    Plate(u64),
}

/// Неподвижное кадра для поля: ключ экземпляра и фигура на месте.
#[derive(Clone)]
struct StillShape {
    key: u64,
    placed: Placed2,
    rect: Rect2,
    /// Прямоугольник плиты в мире для правила слоёв (как у [`overlaps`]); у брусков пуст.
    layer: kurbo::Rect,
}

/// Запоминает отказанный набор (последние [`FIELD_FAILED_KEEP`]).
fn remember_failed(list: &mut Vec<(u64, &'static str)>, set: u64, reason: &'static str) {
    list.retain(|(s, _)| *s != set);
    list.push((set, reason));
    if list.len() > FIELD_FAILED_KEEP {
        list.remove(0);
    }
}

/// Перекрытия плит поля, поменявшихся местами в порядке рисования: `now` и `was` — одни и
/// те же плиты по новому и прежнему порядку. Там цвет — другой плиты, эти прямоугольники
/// надо перепечь. Перекрытие — без допуска [`LAYER_TOUCH`]: цвет меняется и на волоске.
fn reordered(now: &[u64], was: &[u64], rects: &HashMap<u64, Rect2>) -> Vec<Rect2> {
    let was_at: HashMap<u64, usize> = was.iter().enumerate().map(|(i, k)| (*k, i)).collect();
    let items: Vec<(Rect2, usize)> = now
        .iter()
        .filter_map(|k| Some((*rects.get(k)?, *was_at.get(k)?)))
        .collect();
    // Плиты на самой длинной возрастающей цепочке прежних номеров сохранили взаимный
    // порядок: в каждой поменявшейся паре хотя бы одна — вне её («сдвинутая»). Так правка,
    // переставившая одну плиту, стоит n проверок, а не n².
    let moved = off_longest_run(&items.iter().map(|it| it.1).collect::<Vec<_>>());
    let mut out = Vec::new();
    for (i, &(ra, ia)) in items.iter().enumerate() {
        if !moved[i] {
            continue;
        }
        for (j, &(rb, ib)) in items.iter().enumerate() {
            // Пара двух сдвинутых — один раз.
            if j == i || (moved[j] && j < i) {
                continue;
            }
            let swapped = if i < j { ia > ib } else { ib > ia };
            let (lo, hi) = (ra.0.max(rb.0), ra.1.min(rb.1));
            if swapped && lo.x < hi.x && lo.y < hi.y {
                out.push((lo, hi));
            }
        }
    }
    out
}

/// Номера вне одной самой длинной возрастающей цепочки `values` (`true` — вне).
fn off_longest_run(values: &[usize]) -> Vec<bool> {
    // Терпеливая сортировка: tails[l] — номер последнего элемента лучшей цепочки длины l+1.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; values.len()];
    for (i, &v) in values.iter().enumerate() {
        let l = tails.partition_point(|&t| values[t] < v);
        if l > 0 {
            prev[i] = tails[l - 1];
        }
        if l == tails.len() {
            tails.push(i);
        } else {
            tails[l] = i;
        }
    }
    let mut off = vec![true; values.len()];
    let mut at = tails.last().copied().unwrap_or(usize::MAX);
    while at != usize::MAX {
        off[at] = false;
        at = prev[at];
    }
    off
}

/// Пары таблицы по порядку ключей (чтобы порядок работы не зависел от хеша).
fn sorted(map: &HashMap<u64, Rect2>) -> Vec<(u64, Rect2)> {
    let mut list: Vec<(u64, Rect2)> = map.iter().map(|(k, r)| (*k, *r)).collect();
    list.sort_by_key(|(k, _)| *k);
    list
}

/// Ключи экземпляров кадра: одинаковые экземпляры (та же фигура на том же месте) различаются
/// номером повтора, так что у каждого свой ключ.
#[derive(Default)]
struct Keys(HashMap<u64, u64>);

impl Keys {
    fn key(&mut self, base: u64) -> u64 {
        let n = self.0.entry(base).or_insert(0);
        let mut key = Signature::new();
        key.add(base);
        key.add(*n);
        *n += 1;
        key.0
    }
}

/// Подпись набора неподвижного (FNV-1a): одинаковая у одинаковых наборов в одном
/// процессе.
struct Signature(u64);

impl Signature {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn add(&mut self, word: u64) {
        for b in word.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// Слои плит по перекрытию (7.18а): плита встаёт на [`LAYER_STEP`] перед самой передней
/// из уже поставленных, чьи прямоугольники в мире её пересекают; никого не накрывающая —
/// у переднего края брусков. Так герой и предметы стоят вплотную к мебели, а не на
/// полметра впереди всего, что нарисовано раньше. Поставленные плиты разложены по полосам
/// вдоль X, чтобы не перебирать все на каждую.
#[derive(Default)]
struct Layers {
    /// Прямоугольник в мире (единицы 2D) и слой каждой поставленной плиты.
    placed: Vec<(kurbo::Rect, u32)>,
    /// Номера плит по полосам.
    strips: HashMap<i64, Vec<u32>>,
    /// Номера широких плит (во многих полосах сразу).
    wide: Vec<u32>,
}

impl Layers {
    /// Ставит плиту с прямоугольником `rect` в мире и возвращает её слой. Плита без
    /// площади (нулевой размер, NaN) ничего не рисует: слой 0, не ставится.
    fn place(&mut self, rect: kurbo::Rect) -> u32 {
        let rect = rect.abs();
        if !(rect.width() > LAYER_TOUCH && rect.height() > LAYER_TOUCH) {
            return 0;
        }
        // Широкая ли — считается в f64 до перевода в номера полос: у огромного
        // прямоугольника номера упираются в край i64, и их разность бы переполнилась.
        let narrow = rect.width() / LAYER_STRIP < LAYER_WIDE as f64;
        let strips = narrow.then(|| {
            let lo = (rect.x0 / LAYER_STRIP).floor() as i64;
            let hi = (rect.x1 / LAYER_STRIP).floor() as i64;
            lo..=hi
        });
        let layer = match &strips {
            // Узкая плита: соперники — плиты её полос и широкие.
            Some(strips) => {
                let in_strips = strips
                    .clone()
                    .filter_map(|strip| self.strips.get(&strip))
                    .flatten();
                self.top(rect, in_strips.chain(&self.wide).copied())
            }
            // Широкая: соперники — все поставленные.
            None => self.top(rect, 0..self.placed.len() as u32),
        };
        let index = self.placed.len() as u32;
        self.placed.push((rect, layer));
        match strips {
            Some(strips) => {
                for strip in strips {
                    self.strips.entry(strip).or_default().push(index);
                }
            }
            None => self.wide.push(index),
        }
        layer
    }

    /// Слой перед самой передней из плит `indices`, пересекающих `rect` (0, если никто).
    fn top(&self, rect: kurbo::Rect, indices: impl Iterator<Item = u32>) -> u32 {
        indices
            .map(|i| self.placed[i as usize])
            .filter(|&(other, _)| overlaps(rect, other))
            .map(|(_, below)| below + 1)
            .max()
            .unwrap_or(0)
    }
}

/// Пересекаются ли прямоугольники (касание краями не в счёт).
fn overlaps(a: kurbo::Rect, b: kurbo::Rect) -> bool {
    a.x0 < b.x1 - LAYER_TOUCH
        && b.x0 < a.x1 - LAYER_TOUCH
        && a.y0 < b.y1 - LAYER_TOUCH
        && b.y0 < a.y1 - LAYER_TOUCH
}

/// Передняя сторона плиты слоя `layer`, метры по Z.
fn plate_front(layer: u32) -> f32 {
    SOLID_FRONT + PLATE * 0.5 + layer as f32 * LAYER_STEP
}

/// Прямоугольник плиты в мире (единицы 2D) для слоёв: по точному преобразованию (так её
/// рисуют треугольники) и по тому, как её ставит [`placement`] (формы SDF: у неравного
/// размера — по среднему), объединение. Один прямоугольник на оба способа рисовать, и он
/// накрывает плиту в каждом из них.
fn world_rect(bounds: kurbo::Rect, transform: Affine) -> kurbo::Rect {
    let ([x, y], angle, scale, mirror) = placement(transform);
    let scale = f64::from(scale);
    let placed = Affine::translate((f64::from(x), f64::from(y)))
        * Affine::rotate(f64::from(angle).to_radians())
        * Affine::scale_non_uniform(if mirror { -scale } else { scale }, scale);
    transform
        .transform_rect_bbox(bounds)
        .union(placed.transform_rect_bbox(bounds))
}

/// Треугольники кадра по материалам (в мире).
#[derive(Default)]
struct Prisms {
    by_material: HashMap<u32, Vec<[Vec3; 3]>>,
}

impl Prisms {
    /// Призма: лицевая сторона из `front` (треугольники на плоскости, по три точки) на
    /// глубине `z_front`, бока вдоль контуров `outlines` до `z_back`. Точки уже в мире
    /// (x, y).
    fn add(
        &mut self,
        material: u32,
        front: impl IntoIterator<Item = [[f32; 2]; 3]>,
        outlines: impl IntoIterator<Item = Vec<[f32; 2]>>,
        z_front: f32,
        z_back: f32,
    ) {
        let list = self.by_material.entry(material).or_default();
        let at = |p: [f32; 2], z: f32| Vec3::new(p[0], p[1], z);
        for [a, b, c] in front {
            list.push([at(a, z_front), at(b, z_front), at(c, z_front)]);
        }
        for outline in outlines {
            let n = outline.len();
            if n < 2 {
                continue;
            }
            for i in 0..n {
                let (a, b) = (outline[i], outline[(i + 1) % n]);
                if a == b {
                    continue;
                }
                list.push([at(a, z_front), at(b, z_front), at(b, z_back)]);
                list.push([at(a, z_front), at(b, z_back), at(a, z_back)]);
            }
        }
    }

    fn into_meshes(self) -> Vec<MeshFrame> {
        let mut out: Vec<MeshFrame> = self
            .by_material
            .into_iter()
            .map(|(material, triangles)| MeshFrame {
                triangles,
                material,
            })
            .collect();
        // Порядок не важен для картинки, но одинаковый от кадра к кадру.
        out.sort_by_key(|m| m.material);
        out
    }
}

/// Объёмная сцена одного кадра и её материалы.
pub(crate) struct LevelFrame {
    pub frame: SpaceFrame,
    pub materials: Vec<Material3>,
}

/// Материалы кадра: одинаковые цвета — один материал.
#[derive(Clone, Default)]
struct Materials {
    list: Vec<Material3>,
    index: HashMap<([u16; 3], u32), u32>,
}

impl Materials {
    fn get(&mut self, color: [f32; 3], glow: f32) -> u32 {
        let key = (
            color.map(|c| (c.clamp(0.0, 4.0) * 4096.0) as u16),
            glow.to_bits(),
        );
        *self.index.entry(key).or_insert_with(|| {
            self.list.push(Material3 {
                color,
                glow,
                grid: false,
            });
            self.list.len() as u32 - 1
        })
    }
}

impl Level3d {
    /// Забывает формы фигур и поле уровня (сцена сменилась).
    pub fn clear(&mut self) {
        self.clear_cache();
        if let Some(p) = &self.field.pending {
            p.cancel.store(true, Ordering::Relaxed);
        }
        let max_side = self.field.max_side;
        self.field = FieldState {
            max_side,
            next_ticket: self.field.next_ticket,
            ..FieldState::default()
        };
    }

    /// Забывает готовые формы фигур (поле уровня остаётся: оно не зависит от них).
    fn clear_cache(&mut self) {
        self.flats.clear();
        self.meshes.clear();
        self.bounds.clear();
    }

    /// Сторона картинки поля уровня не больше `max_side` точек (предел видеокарты).
    pub fn set_field_limit(&mut self, max_side: u32) {
        self.field.max_side = max_side;
    }

    /// Поле уровня для панели.
    pub fn field_status(&self) -> LevelFieldStatus {
        self.field.status
    }

    /// Запрос на запекание поля уровня, если кадр его сделал: номер запроса и задание.
    /// Готовое поле возвращается через [`Level3d::install_field`] с тем же номером.
    pub fn take_field_request(&mut self) -> Option<(u64, LayerBake)> {
        self.field.request.take()
    }

    /// Номер запроса, чьё поле сейчас готово (его картинки нельзя удалять).
    pub fn baked_ticket(&self) -> Option<u64> {
        self.field.baked.as_ref().map(|b| b.ticket)
    }

    /// Ждёт ли кадр поля по запросу `ticket` (иначе его не нужно грузить на видеокарту).
    pub fn field_pending(&self, ticket: u64) -> bool {
        self.field
            .pending
            .as_ref()
            .is_some_and(|p| p.ticket == ticket)
    }

    /// Куски поля, перепечённые в кадрах после прошлого вызова (их надо загрузить на
    /// видеокарту до проходов 3D этого кадра).
    pub fn take_field_patches(&mut self) -> Vec<FieldPatch> {
        std::mem::take(&mut self.field.patches)
    }

    /// Готовое поле уровня на процессоре (то же, что в картинках запроса
    /// [`Level3d::baked_ticket`]).
    pub fn baked_field(&self) -> Option<&reyvec_sdf::layer_field::LayerField> {
        self.field.baked.as_ref().map(|b| &*b.field.0)
    }

    /// Сцену поправили в файле: прежние выходы плит из поля (нижнюю плиту правили) — не
    /// мигание, ожидание возврата снова обычное.
    pub fn scene_edited(&mut self) {
        self.field.bounces.clear();
    }

    /// Картинки поля разошлись с полем на процессоре (кусок не загрузился): поле
    /// выключается и печётся заново целиком.
    pub fn drop_field(&mut self) {
        self.field.baked = None;
        self.field.patches.clear();
        self.field.failed.clear();
    }

    /// Запечённое поле по запросу `ticket` (картинки уже на видеокарте). `false` — запрос
    /// устарел, поле не нужно (его картинки можно удалять).
    pub fn install_field(
        &mut self,
        ticket: u64,
        field: FieldRef,
        stats: BakeStats,
        ms: f64,
    ) -> bool {
        let Some(p) = self.field.pending.take_if(|p| p.ticket == ticket) else {
            return false;
        };
        tracing::info!(
            "Поле уровня готово: {}×{} по {} см, {} плит и {} брусков, {:.0} мс в фоне, \
             в среднем {:.1} фигуры на плитку",
            field.0.size().0,
            field.0.size().1,
            field.0.cell() * 100.0,
            p.plates.len(),
            p.solids.len(),
            ms,
            stats.per_tile
        );
        self.field.baked = Some(Baked {
            ticket,
            field,
            materials: p.materials,
            plates: p.plates,
            solids: p.solids,
            order: p.order,
            bake_ms: ms,
            patched: None,
        });
        // Куски старого поля видеокарте больше не нужны: картинки у нового свои.
        self.field.patches.retain(|patch| patch.ticket == ticket);
        self.field.failed.clear();
        true
    }

    /// Запрос `ticket` не дал поля: отменён (тогда кадр попросит снова) или видеокарта
    /// не дала картинки (`reason`, тогда для этого набора поля не будет).
    pub fn field_failed(&mut self, ticket: u64, reason: Option<&'static str>) {
        if let Some(p) = self.field.pending.take_if(|p| p.ticket == ticket)
            && let Some(reason) = reason
        {
            tracing::warn!("Поле уровня не готово: {reason}");
            remember_failed(&mut self.field.failed, p.signature, reason);
        }
    }

    /// Объёмная сцена из снимка мира `scene`; формы фигур берутся из `library`.
    /// `render` — формами SDF, плитки и бруски треугольниками или неподвижное полем.
    pub fn frame(
        &mut self,
        scene: &reyvec_scene::Scene,
        library: &ShapeLibrary,
        render: LevelRender,
    ) -> LevelFrame {
        if self.flats.len() > MAX_CACHED
            || self.meshes.len() > MAX_CACHED
            || self.bounds.len() > MAX_CACHED
        {
            self.clear_cache();
        }
        let triangles = render == LevelRender::Triangles;
        let field_mode = render == LevelRender::Field;
        let mut prisms = Prisms::default();
        // Материалы готового поля — первыми: номера в его картинке остаются верными, новые
        // цвета (подвижное, новые фигуры) идут следом.
        let mut materials = match &self.field.baked {
            Some(b) if field_mode => b.materials.clone(),
            _ => Materials::default(),
        };
        let mut shapes = Vec::new();
        // Форма неподвижного (в поле её не будет, если поле готово и она в нём).
        let mut still: Vec<Still> = Vec::new();
        let mut keys = Keys::default();
        // Неподвижные плиты кадра по порядку рисования; плиты «поверх» подвижных в поле не
        // идут.
        let mut bake_plates: Vec<StillShape> = Vec::new();
        let mut bake_solids: Vec<StillShape> = Vec::new();
        // Неподвижные плиты, не накрывающие подвижное (с накрытыми новыми неподвижными
        // формами): их кадры покоя идут, и их берёт полное запекание.
        let mut full_plates: Vec<StillShape> = Vec::new();
        // Неподвижные плиты над подвижным или над формой (для счёта выходов из поля).
        let mut held: Vec<u64> = Vec::new();
        let s = LEVEL_SCALE;

        // Стена и земля.
        let wall = materials.get(WALL, 0.0);
        shapes.push(
            Shape3::Plane {
                normal: [0.0, 0.0, 1.0],
                offset: SOLID_BACK,
            }
            .paint(wall),
        );
        still.push(Still::Moving);
        let ground_y = scene
            .solids
            .iter()
            .map(|c| c.aabb.min[1] as f32)
            .fold(f32::INFINITY, f32::min);
        if ground_y.is_finite() {
            let ground = materials.get(GROUND, 0.0);
            shapes.push(
                Shape3::Plane {
                    normal: [0.0, 1.0, 0.0],
                    offset: ground_y * s,
                }
                .paint(ground),
            );
            still.push(Still::Moving);
        }

        // Твёрдое: бруски.
        let solid = materials.get(SOLID, 0.0);
        for (i, c) in scene.solids.iter().enumerate() {
            if triangles {
                if let Some(outline) = solid_outline(c) {
                    let outline: Vec<[f32; 2]> =
                        outline.iter().map(|p| [p[0] * s, p[1] * s]).collect();
                    let front = fan(&outline);
                    prisms.add(solid, front, [outline], SOLID_FRONT, SOLID_BACK);
                }
            } else {
                let moving = scene.solids_dynamic.get(i).copied().unwrap_or(true);
                let mut kind = Still::Moving;
                if field_mode
                    && !moving
                    && let Some(placed) = solid_placed(c)
                {
                    let mut base = Signature::new();
                    base.add(1);
                    for w in [
                        c.aabb.min[0],
                        c.aabb.min[1],
                        c.aabb.max[0],
                        c.aabb.max[1],
                        c.slope,
                    ] {
                        base.add(w.to_bits());
                    }
                    let key = keys.key(base.0);
                    let rect = placed.world_bounds();
                    bake_solids.push(StillShape {
                        key,
                        placed,
                        rect,
                        layer: kurbo::Rect::ZERO,
                    });
                    kind = Still::Solid(key);
                }
                for shape in solid_shapes(c) {
                    shapes.push(shape.paint(solid));
                    still.push(kind);
                }
            }
        }

        // Фигуры: пластины по порядку рисования, слой — по перекрытию с поставленными.
        let mut layers = Layers::default();
        let mut mask_depth = 0u32;
        let mut dynamic_depth = 0u32;
        // Плиты кадра в мире, которые рисуются формами: подвижные и неподвижные «поверх».
        // Неподвижная, нарисованная поверх одной из них, в поле не идёт (в поле она легла
        // бы под неё) и сама становится такой плитой: стопка над ней тоже остаётся формами.
        let mut front_rects = Vec::new();
        // Из них — подвижные и накрывающие подвижное (их не берёт и полное запекание).
        let mut moving_rects = Vec::new();
        let mut moving_plates = 0;
        for item in &scene.items {
            match item {
                SceneItem::BeginMask { .. } => mask_depth += 1,
                SceneItem::EndMask => mask_depth = mask_depth.saturating_sub(1),
                SceneItem::BeginDynamic => dynamic_depth += 1,
                SceneItem::EndDynamic => dynamic_depth = dynamic_depth.saturating_sub(1),
                SceneItem::Shape(instance) if mask_depth == 0 => {
                    let Some(color) = shape_color(library, instance.shape, instance.tint) else {
                        continue;
                    };
                    let Some(bounds) = self.bounds(library, instance.shape) else {
                        continue;
                    };
                    let in_world = world_rect(bounds, instance.transform);
                    if triangles {
                        let Some(mesh) = self.mesh(library, instance.shape) else {
                            continue;
                        };
                        let front = plate_front(layers.place(in_world));
                        let world = Affine::scale(f64::from(s)) * instance.transform;
                        let put = |p: [f32; 2]| {
                            let q = world * kurbo::Point::new(f64::from(p[0]), f64::from(p[1]));
                            [q.x as f32, q.y as f32]
                        };
                        let front_tris = mesh
                            .triangles
                            .chunks_exact(3)
                            .map(|t| [put(t[0]), put(t[1]), put(t[2])]);
                        let outlines = mesh
                            .outlines
                            .iter()
                            .map(|o| o.iter().map(|&p| put(p)).collect::<Vec<_>>());
                        let material = materials.get(color, 0.0);
                        prisms.add(material, front_tris, outlines, front, front - PLATE);
                        continue;
                    }
                    let Some(flat) = self.flat(library, instance.shape) else {
                        continue;
                    };
                    let front = plate_front(layers.place(in_world));
                    if let Some(shape) = plate(&flat, instance.transform, front) {
                        let material = materials.get(color, 0.0);
                        let fixed = field_mode && dynamic_depth == 0;
                        if fixed {
                            let mut base = Signature::new();
                            base.add(u64::from(instance.shape.0));
                            for c in instance.transform.as_coeffs() {
                                base.add(c.to_bits());
                            }
                            let t = instance.tint;
                            for c in [t.r, t.g, t.b, t.a] {
                                base.add(u64::from(c.to_bits()));
                            }
                            let key = keys.key(base.0);
                            let over = front_rects.iter().any(|&r| overlaps(in_world, r));
                            // Неподвижная, которой нет в готовом поле (новая, сдвинутая), до
                            // входа в поле рисуется формой на своём слое — плиты поля,
                            // нарисованные поверх неё, тоже уходят в формы, иначе легли бы
                            // под неё.
                            let form = self
                                .field
                                .baked
                                .as_ref()
                                .is_some_and(|b| !b.plates.contains_key(&key));
                            let over_moving = moving_rects.iter().any(|&r| overlaps(in_world, r));
                            if over || form {
                                front_rects.push(in_world.abs());
                            }
                            if over_moving || over {
                                held.push(key);
                            }
                            let placed = if over_moving {
                                None
                            } else {
                                plate_placed(&flat, instance.transform, material)
                            };
                            if let Some(placed) = placed {
                                let rect = placed.world_bounds();
                                let shape = StillShape {
                                    key,
                                    placed,
                                    rect,
                                    layer: in_world.abs(),
                                };
                                if !over {
                                    bake_plates.push(shape.clone());
                                }
                                full_plates.push(shape);
                            } else {
                                // Над подвижным или без формы для поля — форма всегда, как
                                // подвижная.
                                moving_rects.push(in_world.abs());
                            }
                            still.push(Still::Plate(key));
                        } else {
                            if field_mode {
                                front_rects.push(in_world.abs());
                                moving_rects.push(in_world.abs());
                                moving_plates += 1;
                            }
                            still.push(Still::Moving);
                        }
                        shapes.push(shape.paint(material));
                    }
                }
                _ => {}
            }
        }

        if field_mode {
            let ready = self.field_frame(
                &materials,
                bake_plates,
                full_plates,
                bake_solids,
                &held,
                solid,
            );
            if let (Some(field), Some(baked)) = (ready, &self.field.baked) {
                let mut forms = 0;
                let mut kept: Vec<Shape3> = shapes
                    .into_iter()
                    .zip(still)
                    .filter_map(|(shape, still)| {
                        let keep = match still {
                            Still::Moving => true,
                            Still::Solid(key) => !baked.solids.contains_key(&key),
                            Still::Plate(key) => !baked.plates.contains_key(&key),
                        };
                        forms += usize::from(keep && !matches!(still, Still::Moving));
                        keep.then_some(shape)
                    })
                    .collect();
                kept.push(field);
                shapes = kept;
                if let LevelFieldStatus::Ready { shapes: n, .. } = &mut self.field.status {
                    *n = moving_plates + forms;
                }
            }
        } else {
            self.field.status = LevelFieldStatus::Off;
        }

        // SDF-объекты: капсулы у переднего края. Светящийся объект у лампы (огонёк бра,
        // шар-светильник) — на глубине лампы, там же, где свет.
        let lights: Vec<[f32; 2]> = scene
            .lights
            .iter()
            .filter(|l| l.kind == reyvec_scene::LightKind::Local)
            .map(|l| [(l.a[0] + l.b[0]) * 0.5, (l.a[1] + l.b[1]) * 0.5])
            .collect();
        let mut glows = Vec::new();
        for o in &scene.objects {
            let at_lamp = o.glow
                && o.parts.iter().any(|p| {
                    let c = [(p.a[0] + p.b[0]) * 0.5, (p.a[1] + p.b[1]) * 0.5];
                    lights
                        .iter()
                        .any(|l| (l[0] - c[0]).hypot(l[1] - c[1]) < p.r + LAMP_NEAR)
                });
            let z = if at_lamp {
                LAMP_Z
            } else {
                SOLID_FRONT + PLATE * 0.5
            };
            if let Some((shape, color, glow)) = object_shape(o, z) {
                let shape = shape.paint(materials.get(color, glow));
                // Светящиеся теней не дают: иначе шар у лампы бросает от неё кольца
                // полутени на стену.
                if o.glow {
                    glows.push(shape);
                } else {
                    shapes.push(shape);
                }
            }
        }

        LevelFrame {
            frame: SpaceFrame {
                shapes,
                lamps: Vec::new(),
                meshes: prisms.into_meshes(),
                proxies: Vec::new(),
                glows,
            },
            materials: materials.list,
        }
    }

    /// Поле уровня в кадре: форма поля, если поле готово, иначе `None` (кадр рисуется
    /// как в SDF). `plates` и `solids` — неподвижное кадра, которое может быть в поле
    /// (плиты по порядку рисования). Готовое поле сверяется с ним каждый кадр (7.18г):
    /// ушедшее из кадра (убранное, сдвинутое, изменившееся, оказавшееся поверх подвижного)
    /// выходит из поля в тот же кадр — кусок вокруг него перепекается на месте; новое
    /// входит, простояв [`FIELD_STABLE_FRAMES`] кадров, очередью не больше
    /// [`FIELD_ADD_POINTS`] точек за кадр, а до того рисуется формой. Полное запекание в
    /// фоне — только когда поля нет, кусок слишком велик или новое за краем сетки.
    /// `full` — неподвижные плиты, не накрывающие подвижное: `plates` и плиты над новыми
    /// неподвижными формами. В поле этим вторым пока нельзя, но кадры покоя у них идут
    /// (стопка над новой плитой входит в поле сразу за ней), и полное запекание берёт их.
    fn field_frame(
        &mut self,
        materials: &Materials,
        plates: Vec<StillShape>,
        full: Vec<StillShape>,
        solids: Vec<StillShape>,
        held: &[u64],
        solid: u32,
    ) -> Option<Shape3> {
        let f = &mut self.field;
        // Выходы из поля помнятся, пока плита есть в кадре.
        let present: std::collections::HashSet<u64> = full
            .iter()
            .map(|s| s.key)
            .chain(held.iter().copied())
            .collect();
        f.bounces.retain(|k, _| present.contains(k));
        let bounces = f.bounces.clone();
        let need =
            // Первый выход — обычное ожидание, каждый следующий — вдвое дольше.
            move |k: &u64| {
                let n = bounces.get(k).copied().unwrap_or(0);
                FIELD_STABLE_FRAMES << n.saturating_sub(1).min(6)
            };
        // Сколько кадров стоит неподвижное, которого нет в поле.
        let in_field = |key: &u64, plate: bool| {
            f.baked.as_ref().is_some_and(|b| {
                if plate {
                    b.plates.contains_key(key)
                } else {
                    b.solids.contains_key(key)
                }
            })
        };
        let mut fresh = HashMap::new();
        for (key, plate) in full
            .iter()
            .map(|s| (s.key, true))
            .chain(solids.iter().map(|s| (s.key, false)))
        {
            if !in_field(&key, plate) {
                let n = f.fresh.get(&key).copied().unwrap_or(0);
                fresh.insert(key, n.saturating_add(1));
            }
        }
        f.fresh = fresh;
        // Что берёт полное запекание: без поля — всё неподвижное; при готовом поле (оно
        // печётся заново) — только простоявшее: в поле или [`FIELD_STABLE_FRAMES`] кадров
        // без изменений. Иначе фигура, меняющаяся каждый кадр без пометки «подвижное»,
        // уходила бы из каждого нового поля, и большая — снова запускала бы запекание.
        // После отказа (поля не будет) — тоже: иначе меняющаяся фигура меняла бы подпись
        // набора каждый кадр, и запекание просилось бы снова и снова. Плита, нарисованная
        // поверх непростоявшей, после запекания окажется над её формой и тут же уйдёт из
        // поля — её тоже не берём (цепочка — по тому же правилу слоёв, что в кадре).
        let (full, solids_full): (Vec<StillShape>, Vec<StillShape>) =
            if f.baked.is_none() && f.failed.is_empty() {
                (full, solids.clone())
            } else {
                let empty = HashMap::new();
                let (plates_in, solids_in) = f
                    .baked
                    .as_ref()
                    .map_or((&empty, &empty), |b| (&b.plates, &b.solids));
                let stable = |s: &StillShape, members: &HashMap<u64, Rect2>| {
                    members.contains_key(&s.key)
                        || f.fresh.get(&s.key).copied().unwrap_or(0) >= need(&s.key)
                };
                let mut front: Vec<kurbo::Rect> = Vec::new();
                let mut kept = Vec::new();
                for s in full {
                    if stable(&s, plates_in) && !front.iter().any(|&r| overlaps(s.layer, r)) {
                        kept.push(s);
                    } else {
                        front.push(s.layer);
                    }
                }
                let solids_kept = solids
                    .iter()
                    .filter(|s| stable(s, solids_in))
                    .cloned()
                    .collect();
                (kept, solids_kept)
            };
        let mut set = Signature::new();
        for s in full.iter().chain(&solids_full) {
            set.add(s.key);
        }

        let Some(b) = f.baked.as_mut() else {
            // Поля для этого набора не будет; после отказа набор — только простоявшее, и
            // пока его нет, просить нечего.
            if let Some(p) = &f.pending {
                f.status = LevelFieldStatus::Baking { reason: p.reason };
                return None;
            }
            let refused = f.failed.iter().find(|(failed, _)| *failed == set.0);
            let last = f
                .failed
                .last()
                .filter(|_| full.is_empty() && solids_full.is_empty());
            if let Some(&(_, reason)) = refused.or(last) {
                f.status = LevelFieldStatus::Failed(reason);
                return None;
            }
            let reason = "новый уровень";
            Self::request_full(f, materials, &full, &solids_full, solid, set.0, reason);
            return None;
        };

        // Сетка готового поля и кусок вокруг прямоугольника: фигура влияет на точки не
        // дальше полосы (и двух клеток запаса на смешивание соседей).
        let (corner, cell, (nx, ny)) = (b.field.0.corner(), b.field.0.cell(), b.field.0.size());
        let area_of = |(lo, hi): Rect2| -> ([u32; 4], bool) {
            let pad = Vec2::splat(FIELD_BAND + 2.0 * cell);
            let a = ((lo - pad - corner) / cell).floor();
            let z = ((hi + pad - corner) / cell).ceil();
            let inside = a.x >= 0.0 && a.y >= 0.0 && z.x <= nx as f32 && z.y <= ny as f32;
            let a = a.clamp(Vec2::ZERO, Vec2::new(nx as f32, ny as f32));
            let z = z.clamp(a, Vec2::new(nx as f32, ny as f32));
            (
                [
                    a.x as u32,
                    a.y as u32,
                    (z.x - a.x) as u32,
                    (z.y - a.y) as u32,
                ],
                inside,
            )
        };
        let points = |area: &[u32; 4]| area[2] as usize * area[3] as usize;

        // Ушедшее из кадра — из поля в этот же кадр.
        let now_plates: std::collections::HashSet<u64> = plates.iter().map(|s| s.key).collect();
        let now_solids: std::collections::HashSet<u64> = solids.iter().map(|s| s.key).collect();
        let gone_plates: Vec<u64> = sorted(&b.plates)
            .into_iter()
            .filter(|(k, _)| !now_plates.contains(k))
            .map(|(k, _)| k)
            .collect();
        let gone_solids: Vec<u64> = sorted(&b.solids)
            .into_iter()
            .filter(|(k, _)| !now_solids.contains(k))
            .map(|(k, _)| k)
            .collect();
        // Ушла, оказавшись над подвижным или над формой (нижняя изменилась), — выход
        // засчитан: следующий возврат ждёт дольше.
        for k in &gone_plates {
            if held.contains(k) {
                *f.bounces.entry(*k).or_default() += 1;
            }
        }
        let mut areas: Vec<[u32; 4]> = gone_plates
            .iter()
            .map(|k| b.plates[k])
            .chain(gone_solids.iter().map(|k| b.solids[k]))
            .map(|r| area_of(r).0)
            .collect();
        // Плиты поля поменялись местами в порядке рисования (правка файла): там, где они
        // перекрываются, цвет — другой плиты, этот кусок перепекается в новом порядке.
        let gone_set: std::collections::HashSet<u64> = gone_plates.iter().copied().collect();
        let now_order: Vec<u64> = plates
            .iter()
            .map(|s| s.key)
            .filter(|k| b.plates.contains_key(k) && !gone_set.contains(k))
            .collect();
        let was_order: Vec<u64> = b
            .order
            .iter()
            .copied()
            .filter(|k| !gone_set.contains(k))
            .collect();
        if now_order != was_order {
            areas.extend(
                reordered(&now_order, &was_order, &b.plates)
                    .into_iter()
                    .map(|r| area_of(r).0),
            );
        }
        areas.retain(|a| points(a) > 0);
        let removed: usize = areas.iter().map(points).sum();
        if removed > FIELD_REMOVE_POINTS {
            tracing::info!(
                "Поле уровня: из поля ушло {} фигур, кусок {removed} точек — запекание целиком",
                gone_plates.len() + gone_solids.len()
            );
            f.baked = None;
            f.patches.clear();
            let reason = "изменилось много неподвижного";
            if f.pending.is_none() {
                Self::request_full(f, materials, &full, &solids_full, solid, set.0, reason);
            } else {
                f.status = LevelFieldStatus::Baking { reason };
            }
            return None;
        }

        // Простоявшее новое — в поле, очередью по порядку рисования.
        let mut budget = FIELD_ADD_POINTS;
        let mut outside = false;
        let mut added_plates = Vec::new();
        let mut added_solids = Vec::new();
        for (s, plate) in plates
            .iter()
            .map(|s| (s, true))
            .chain(solids.iter().map(|s| (s, false)))
        {
            let members = if plate { &b.plates } else { &b.solids };
            if members.contains_key(&s.key)
                || f.fresh.get(&s.key).copied().unwrap_or(0) < need(&s.key)
            {
                continue;
            }
            let (area, inside) = area_of(s.rect);
            if !inside || points(&area) > FIELD_REMOVE_POINTS {
                // Сетка мала или кусок слишком велик для кадра: фигура остаётся формой до
                // полного запекания в фоне.
                outside = true;
                continue;
            }
            // Первый кусок кадра входит, даже если он больше очереди.
            if points(&area) > budget && budget < FIELD_ADD_POINTS {
                continue;
            }
            budget = budget.saturating_sub(points(&area));
            areas.push(area);
            if plate {
                added_plates.push(s.key);
            } else {
                added_solids.push(s.key);
            }
        }

        if !areas.is_empty() {
            if Arc::get_mut(&mut b.field.0).is_none() {
                // Поле ещё держит прошлый кадр: этот кадр — без поля, перепекание — в
                // следующем (старое поле показывать нельзя).
                f.status = LevelFieldStatus::Baking {
                    reason: "поле занято прошлым кадром",
                };
                return None;
            }
            // Состав поля после этого кадра: без ушедшего, с вошедшим.
            let member_plate = |k: &u64| {
                (b.plates.contains_key(k) && !gone_set.contains(k)) || added_plates.contains(k)
            };
            let member_solid = |k: &u64| {
                (b.solids.contains_key(k) && !gone_solids.contains(k)) || added_solids.contains(k)
            };
            let bake = Self::bake_of(
                plates
                    .iter()
                    .filter(|s| member_plate(&s.key))
                    .map(|s| s.placed.clone())
                    .collect(),
                solids
                    .iter()
                    .filter(|s| member_solid(&s.key))
                    .map(|s| s.placed.clone())
                    .collect(),
                solid,
            );
            let started = std::time::Instant::now();
            // Сначала все куски, потом правка: не испёкся хоть один — поле разошлось бы с
            // составом, поэтому оно выключается и печётся целиком.
            let pieces: Option<Vec<_>> = areas
                .iter()
                .map(|area| bake.run_area(corner, cell, *area).map(|a| (*area, a)))
                .collect();
            let Some(pieces) = pieces else {
                tracing::warn!("Поле уровня: кусок не перепёкся — запекание целиком");
                f.baked = None;
                f.patches.clear();
                let reason = "кусок поля не перепёкся";
                if f.pending.is_none() {
                    Self::request_full(f, materials, &full, &solids_full, solid, set.0, reason);
                } else {
                    f.status = LevelFieldStatus::Baking { reason };
                }
                return None;
            };
            for k in &gone_plates {
                b.plates.remove(k);
            }
            for k in &gone_solids {
                b.solids.remove(k);
            }
            for s in &plates {
                if added_plates.contains(&s.key) {
                    b.plates.insert(s.key, s.rect);
                }
            }
            for s in &solids {
                if added_solids.contains(&s.key) {
                    b.solids.insert(s.key, s.rect);
                }
            }
            let field = Arc::get_mut(&mut b.field.0).expect("поле проверено выше");
            let mut done = FieldPatched::default();
            for (area, baked) in &pieces {
                let area = *area;
                field.patch(area, &baked.texels, &baked.materials);
                done.pieces += 1;
                done.points += baked.stats.points;
                f.patches.push(FieldPatch {
                    ticket: b.ticket,
                    area,
                });
            }
            done.ms = started.elapsed().as_secs_f64() * 1000.0;
            tracing::debug!(
                "Поле уровня: перепечено на месте {} кусков, {} точек за {:.2} мс \
                 (ушло {}, вошло {})",
                done.pieces,
                done.points,
                done.ms,
                gone_plates.len() + gone_solids.len(),
                added_plates.len() + added_solids.len()
            );
            b.patched = Some(done);
            // Номера новых фигур — в таблице поля.
            b.materials = materials.clone();
        }
        if let Some(b) = f.baked.as_mut()
            && (now_order != was_order || !areas.is_empty())
        {
            b.order = plates
                .iter()
                .map(|s| s.key)
                .filter(|k| b.plates.contains_key(k))
                .collect();
        }
        let gave_up = f.failed.iter().any(|(failed, _)| *failed == set.0);
        if outside && f.pending.is_none() && !gave_up {
            let reason = "новое за краем поля";
            Self::request_full(f, materials, &full, &solids_full, solid, set.0, reason);
        }
        let b = f.baked.as_ref()?;
        let (size, cell) = (b.field.0.size(), b.field.0.cell());
        f.status = LevelFieldStatus::Ready {
            size,
            cell,
            plates: b.plates.len(),
            solids: b.solids.len(),
            shapes: 0,
            bake_ms: b.bake_ms,
            megabytes: f64::from(size.0) * f64::from(size.1) * 6.0 / 1e6,
            patched: b.patched,
            rebaking: f.pending.is_some(),
        };
        let front = plate_front(0);
        Some(
            Shape3::Field {
                field: b.field.clone(),
                depth: PLATE,
                round: 0.0,
            }
            .at([0.0, 0.0, front - PLATE * 0.5])
            .paint(solid),
        )
    }

    /// Задание на запекание поля уровня из плит `first` и брусков `second` (сетка — по
    /// ним, клетка — по [`Level3d::field_frame`] позже).
    fn bake_of(first: Vec<Placed2>, second: Vec<Placed2>, solid: u32) -> LayerBake {
        LayerBake {
            cell: FIELD_CELLS[0],
            band: FIELD_BAND,
            first,
            second,
            second_layer: Some(SecondLayer {
                shift: (SOLID_FRONT + SOLID_BACK) * 0.5 - (plate_front(0) - PLATE * 0.5),
                half: (SOLID_FRONT - SOLID_BACK) * 0.5,
                material: solid,
            }),
            threads: std::thread::available_parallelism()
                .map_or(1, |n| n.get().saturating_sub(1).max(1)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Запрос на полное запекание неподвижного кадра (прежний запрос отменяется).
    fn request_full(
        f: &mut FieldState,
        materials: &Materials,
        plates: &[StillShape],
        solids: &[StillShape],
        solid: u32,
        set: u64,
        reason: &'static str,
    ) {
        if let Some(p) = f.pending.take() {
            p.cancel.store(true, Ordering::Relaxed);
        }
        let mut bake = Self::bake_of(
            plates.iter().map(|s| s.placed.clone()).collect(),
            solids.iter().map(|s| s.placed.clone()).collect(),
            solid,
        );
        let max_side = if f.max_side == 0 {
            FIELD_MAX_SIDE
        } else {
            f.max_side.min(FIELD_MAX_SIDE)
        };
        if bake.extent().is_none() {
            // Пустой набор не запоминается: следующий кадр с неподвижным просит сразу.
            if f.baked.is_none() {
                f.status = LevelFieldStatus::Failed("неподвижного нет");
            }
            return;
        }
        let failed = if !bake.fit(&FIELD_CELLS, max_side) {
            Some("уровень слишком большой для картинки поля")
        } else {
            None
        };
        if let Some(reason) = failed {
            // Для этого набора поля не будет: кадр не просит его снова. Готовое поле (если
            // есть) продолжает работать, новое за краем остаётся формой.
            remember_failed(&mut f.failed, set, reason);
            if f.baked.is_none() {
                f.status = LevelFieldStatus::Failed(reason);
            }
            return;
        }
        f.next_ticket += 1;
        let ticket = f.next_ticket;
        tracing::info!(
            "Поле уровня: запекание ({reason}), {} плит и {} брусков, шаг {} см",
            plates.len(),
            solids.len(),
            bake.cell * 100.0
        );
        f.pending = Some(Pending {
            ticket,
            materials: materials.clone(),
            plates: plates.iter().map(|s| (s.key, s.rect)).collect(),
            solids: solids.iter().map(|s| (s.key, s.rect)).collect(),
            order: plates.iter().map(|s| s.key).collect(),
            signature: set,
            cancel: bake.cancel.clone(),
            reason,
        });
        f.request = Some((ticket, bake));
        if f.baked.is_none() {
            f.status = LevelFieldStatus::Baking { reason };
        }
    }

    /// Треугольники фигуры `id` (готовятся при первой встрече).
    fn mesh(&mut self, library: &ShapeLibrary, id: ShapeId) -> Option<Arc<FlatMesh>> {
        self.meshes
            .entry(id)
            .or_insert_with(|| library.shape(id).and_then(mesh_of).map(Arc::new))
            .clone()
    }

    /// Форма фигуры `id` (готовится при первой встрече).
    fn flat(&mut self, library: &ShapeLibrary, id: ShapeId) -> Option<Flat> {
        self.flats
            .entry(id)
            .or_insert_with(|| library.shape(id).and_then(flat_of))
            .clone()
    }

    /// Прямоугольник фигуры `id` в её координатах (готовится при первой встрече).
    fn bounds(&mut self, library: &ShapeLibrary, id: ShapeId) -> Option<kurbo::Rect> {
        *self
            .bounds
            .entry(id)
            .or_insert_with(|| library.shape(id).and_then(bounds_of))
    }
}

/// Лампы уровня: ближние к `target` (середине вида) первыми — у первых двух тени.
pub(crate) fn lamps(scene: &reyvec_scene::Scene, target: [f32; 2]) -> Vec<Lamp3> {
    let s = LEVEL_SCALE;
    let mut lamps: Vec<(f32, Lamp3)> = scene
        .lights
        .iter()
        .filter(|l| l.kind == reyvec_scene::LightKind::Local)
        .filter_map(|l| {
            let at = [(l.a[0] + l.b[0]) * 0.5, (l.a[1] + l.b[1]) * 0.5];
            let strength = l.color.iter().copied().fold(0.0f32, f32::max);
            if strength <= 1e-4 {
                return None;
            }
            let distance = ((at[0] - target[0]).powi(2) + (at[1] - target[1]).powi(2)).sqrt();
            Some((
                distance,
                Lamp3 {
                    at: [at[0] * s, at[1] * s, LAMP_Z],
                    color: l.color.map(|c| c / strength),
                    // Яркость плоской лампы — у её места; в объёме лампа на LAMP_Z перед
                    // уровнем, и свет до стены идёт дальше.
                    power: strength * 4.0,
                    range: (l.reach * s * 1.5).max(1.0),
                },
            ))
        })
        .collect();
    lamps.sort_by(|a, b| a.0.total_cmp(&b.0));
    lamps.into_iter().take(MAX_LAMPS).map(|(_, l)| l).collect()
}

/// Угол карты тени лампы (7.9б), градусы: лампа перед уровнем видит его весь под этим
/// углом, кроме самых косых мест сбоку, куда её свет почти не доходит.
const SHADOW_MAP_FOV_DEG: f32 = 140.0;

/// Матрица «мир → карта тени» лампы (7.9б): лампа смотрит на уровень (вдоль −Z), глубина
/// обратная, как у камеры.
pub(crate) fn lamp_view_proj(lamp: &Lamp3) -> glam::Mat4 {
    let at = Vec3::from(lamp.at);
    let view = glam::Mat4::look_at_rh(at, at - Vec3::Z, Vec3::Y);
    let far = lamp.range.max(1.0) + 1.0;
    let proj = glam::Mat4::perspective_rh(SHADOW_MAP_FOV_DEG.to_radians(), 1.0, far, 0.05);
    proj * view
}

/// Камера 2.5D-вида: сбоку с лёгким наклоном сверху, смотрит на середину 2D-камеры
/// `center`; при увеличении `zoom` видна та же высота уровня, что в плоском виде.
/// `orbit` — облёт для осмотра: поворот вокруг середины влево-вправо и вверх-вниз,
/// градусы (0, 0 — обычный вид).
pub(crate) fn camera(center: [f64; 2], zoom: f64, orbit: [f32; 2]) -> Camera3D {
    let s = LEVEL_SCALE;
    let target = Vec3::new(center[0] as f32 * s, center[1] as f32 * s, SOLID_FRONT);
    // Плоский вид показывает по высоте 2 / zoom единиц.
    let half_height = s / zoom.max(1e-3) as f32;
    let distance = half_height / (reyvec_sdf::camera3d::DEFAULT_FOV_Y * 0.5).tan();
    let pitch = (CAMERA_PITCH_DEG + orbit[1]).to_radians();
    let yaw = orbit[0].to_radians();
    let eye = target
        + Vec3::new(
            pitch.cos() * yaw.sin(),
            pitch.sin(),
            pitch.cos() * yaw.cos(),
        ) * distance;
    Camera3D::looking_at(eye, target)
}

/// Контур бруска твёрдого на плоскости (единицы 2D), против часовой стрелки:
/// прямоугольник или у склона четырёхугольник со скошенным верхом.
fn solid_outline(c: &reyvec_scene::game::Collider) -> Option<Vec<[f32; 2]>> {
    let (lo, hi) = (c.aabb.min, c.aabb.max);
    if hi[0] <= lo[0] || hi[1] <= lo[1] {
        return None;
    }
    let drop = c.slope.abs().min(hi[1] - lo[1]);
    let (left_top, right_top) = if c.slope > 0.0 {
        (hi[1] - drop, hi[1])
    } else if c.slope < 0.0 {
        (hi[1], hi[1] - drop)
    } else {
        (hi[1], hi[1])
    };
    Some(vec![
        [lo[0] as f32, lo[1] as f32],
        [hi[0] as f32, lo[1] as f32],
        [hi[0] as f32, right_top as f32],
        [lo[0] as f32, left_top as f32],
    ])
}

/// Треугольники выпуклого многоугольника веером от первой точки.
fn fan(points: &[[f32; 2]]) -> Vec<[[f32; 2]; 3]> {
    (1..points.len().saturating_sub(1))
        .map(|i| [points[0], points[i], points[i + 1]])
        .collect()
}

/// Плита фигуры для поля уровня: то же место, что у [`plate`], в метрах на плоскости.
fn plate_placed(flat: &Flat, transform: Affine, material: u32) -> Option<Placed2> {
    let ([x, y], angle, scale, mirror) = placement(transform);
    if scale <= 1e-6 {
        return None;
    }
    let flat = match flat {
        Flat::Rect { center, size } => Flat2::Rect {
            center: Vec2::from(*center),
            half: Vec2::from(*size) * 0.5,
        },
        Flat::Profile(profile) => Flat2::Profile(profile.clone()),
    };
    Some(Placed2 {
        flat,
        at: Vec2::new(x, y) * LEVEL_SCALE,
        angle,
        scale: LEVEL_SCALE * scale,
        mirror,
        material: u16::try_from(material).unwrap_or(u16::MAX),
    })
}

/// Брусок твёрдого для поля уровня: тот же контур, что у [`solid_shapes`], в метрах.
fn solid_placed(c: &reyvec_scene::game::Collider) -> Option<Placed2> {
    let s = LEVEL_SCALE;
    let (lo, hi) = (c.aabb.min, c.aabb.max);
    let (w, h) = ((hi[0] - lo[0]) as f32, (hi[1] - lo[1]) as f32);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let flat = if c.slope == 0.0 {
        Flat2::Rect {
            center: Vec2::new((lo[0] + hi[0]) as f32, (lo[1] + hi[1]) as f32) * 0.5,
            half: Vec2::new(w, h) * 0.5,
        }
    } else {
        Flat2::Profile(slope_profile(c)?)
    };
    Some(Placed2 {
        flat,
        at: Vec2::ZERO,
        angle: 0.0,
        scale: s,
        mirror: false,
        material: 0,
    })
}

/// Поле склона: верх идёт от одного верхнего угла вниз на |slope| у другого края.
fn slope_profile(c: &reyvec_scene::game::Collider) -> Option<Profile> {
    let (lo, hi) = (c.aabb.min, c.aabb.max);
    let drop = c.slope.abs().min(hi[1] - lo[1]);
    let (left_top, right_top) = if c.slope > 0.0 {
        (hi[1] - drop, hi[1])
    } else {
        (hi[1], hi[1] - drop)
    };
    let mut path = BezPath::new();
    path.move_to((lo[0], lo[1]));
    path.line_to((hi[0], lo[1]));
    path.line_to((hi[0], right_top));
    path.line_to((lo[0], left_top));
    path.close_path();
    Profile::from_bez(&path, PROFILE_CELL * 10.0).ok()
}

/// Длиннее этого (в метрах) брусок твёрдого режется на куски встык. У длинного бруска
/// шар-граница накрывает всю комнату: сетка отсева считала его в каждой клетке, а её
/// границы растягивались по нему (пол в 53,6 м давал клетку 2 м вместо полуметра, и
/// каждый шаг луча перебирал 30–50 объектов). Снаружи расстояние до кусков то же, что
/// до целого бруска: картинка не меняется.
const SOLID_PIECE: f32 = 2.0;

/// Бруски твёрдого: коробка (длинная — кусками) или у склона выдавленный многоугольник.
fn solid_shapes(c: &reyvec_scene::game::Collider) -> Vec<Shape3> {
    let s = LEVEL_SCALE;
    let (lo, hi) = (c.aabb.min, c.aabb.max);
    let (w, h) = ((hi[0] - lo[0]) as f32, (hi[1] - lo[1]) as f32);
    if w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    let depth = SOLID_FRONT - SOLID_BACK;
    let z = (SOLID_FRONT + SOLID_BACK) * 0.5;
    if c.slope == 0.0 {
        let pieces = |len: f32| ((len * s / SOLID_PIECE).ceil() as u32).max(1);
        let (nx, ny) = (pieces(w), pieces(h));
        if nx == 1 && ny == 1 {
            return vec![
                Shape3::Box {
                    size: [w * s, h * s, depth],
                    round: 0.0,
                }
                .at([
                    (lo[0] + hi[0]) as f32 * 0.5 * s,
                    (lo[1] + hi[1]) as f32 * 0.5 * s,
                    z,
                ]),
            ];
        }
        let (pw, ph) = (w * s / nx as f32, h * s / ny as f32);
        let (x0, y0) = (lo[0] as f32 * s, lo[1] as f32 * s);
        let mut out = Vec::with_capacity((nx * ny) as usize);
        for j in 0..ny {
            for i in 0..nx {
                out.push(
                    Shape3::Box {
                        size: [pw, ph, depth],
                        round: 0.0,
                    }
                    .at([
                        x0 + (i as f32 + 0.5) * pw,
                        y0 + (j as f32 + 0.5) * ph,
                        z,
                    ]),
                );
            }
        }
        return out;
    }
    let Some(profile) = slope_profile(c) else {
        return Vec::new();
    };
    vec![Shape3::Move {
        at: [0.0, 0.0, z],
        turn: [0.0, 0.0, 0.0],
        scale: s,
        shape: Box::new(Shape3::Extrude {
            profile,
            depth: depth / s,
            round: 0.0,
        }),
    }]
}

/// Цвет фигуры (линейный) с оттенком; `None` — почти прозрачная, не рисуется.
fn shape_color(library: &ShapeLibrary, id: ShapeId, tint: Color) -> Option<[f32; 3]> {
    let shape = library.shape(id)?;
    let base = match &shape.paint {
        Paint::Solid(c) => *c,
        Paint::Gradient(g) => {
            // Градиент — его средний цвет.
            let n = g.stops.len().max(1) as f32;
            g.stops
                .iter()
                .fold(Color::linear(0.0, 0.0, 0.0, 0.0), |acc, s| {
                    Color::linear(
                        acc.r + s.color.r / n,
                        acc.g + s.color.g / n,
                        acc.b + s.color.b / n,
                        acc.a + s.color.a / n,
                    )
                })
        }
    };
    let alpha = base.a * tint.a;
    (alpha >= 0.05).then_some([base.r * tint.r, base.g * tint.g, base.b * tint.b])
}

/// Контур того, что фигура закрашивает: у заливки — её путь, у обводки — её полоса.
fn drawn_path(shape: &reyvec_vector::Shape) -> std::borrow::Cow<'_, BezPath> {
    match shape.style {
        reyvec_vector::Style::Fill(_) => std::borrow::Cow::Borrowed(&shape.path),
        reyvec_vector::Style::Stroke(stroke) => std::borrow::Cow::Owned(kurbo::stroke(
            shape.path.iter(),
            &kurbo::Stroke::new(stroke.width),
            &kurbo::StrokeOpts::default(),
            stroke.width * 0.05,
        )),
    }
}

/// Прямоугольник фигуры в её координатах; `None` — закрашивать нечего.
fn bounds_of(shape: &reyvec_vector::Shape) -> Option<kurbo::Rect> {
    let rect = kurbo::Shape::bounding_box(&*drawn_path(shape));
    (rect.is_finite() && rect.width() > 0.0 && rect.height() > 0.0).then_some(rect)
}

/// Форма фигуры без толщины: прямоугольник по осям или поле контура (обводка — поле
/// её полосы).
fn flat_of(shape: &reyvec_vector::Shape) -> Option<Flat> {
    if let reyvec_vector::Style::Fill(_) = shape.style
        && let Some(rect) = axis_rect(&shape.path)
    {
        return Some(rect);
    }
    let path = drawn_path(shape).into_owned();
    let size = kurbo::Shape::bounding_box(&path).size();
    let long = size.width.max(size.height) as f32;
    let cell = (long / MIN_PROFILE_CELLS).clamp(PROFILE_CELL, PROFILE_CELL * 10.0);
    Profile::from_bez(&path, cell).ok().map(Flat::Profile)
}

/// Треугольники фигуры для призмы: заливка контура (у обводки — её полосы) и контуры
/// ломаными. Точность — сотая доля размера фигуры.
fn mesh_of(shape: &reyvec_vector::Shape) -> Option<FlatMesh> {
    let path = drawn_path(shape).into_owned();
    let rule = match shape.style {
        reyvec_vector::Style::Fill(rule) => rule,
        reyvec_vector::Style::Stroke(_) => reyvec_vector::FillRule::NonZero,
    };
    let size = kurbo::Shape::bounding_box(&path).size();
    let long = size.width.max(size.height);
    if long.is_nan() || long <= 0.0 {
        return None;
    }
    let tolerance = long / 100.0;
    let fill = reyvec_vector::Shape {
        path: path.clone(),
        style: reyvec_vector::Style::Fill(rule),
        paint: shape.paint.clone(),
    };
    let tess = reyvec_vector::tessellate(&fill, tolerance).ok()?;
    let triangles = tess
        .indices
        .iter()
        .map(|&i| tess.vertices[i as usize])
        .collect();
    let mut outlines = Vec::new();
    let mut current: Vec<[f32; 2]> = Vec::new();
    kurbo::flatten(path.iter(), tolerance, |el| match el {
        PathEl::MoveTo(p) => {
            if current.len() > 1 {
                outlines.push(std::mem::take(&mut current));
            }
            current.clear();
            current.push([p.x as f32, p.y as f32]);
        }
        PathEl::LineTo(p) => current.push([p.x as f32, p.y as f32]),
        PathEl::ClosePath => {
            if current.len() > 1 {
                outlines.push(std::mem::take(&mut current));
            }
            current.clear();
        }
        _ => {}
    });
    if current.len() > 1 {
        outlines.push(current);
    }
    Some(FlatMesh {
        triangles,
        outlines,
    })
}

/// Прямоугольник по осям: четыре угла и замыкание.
fn axis_rect(path: &BezPath) -> Option<Flat> {
    let mut points = Vec::new();
    for el in path.elements() {
        match el {
            PathEl::MoveTo(p) if points.is_empty() => points.push(*p),
            PathEl::LineTo(p) => points.push(*p),
            PathEl::ClosePath => {}
            _ => return None,
        }
    }
    if points.len() == 5 && points[4].distance(points[0]) < 1e-9 {
        points.pop();
    }
    if points.len() != 4 {
        return None;
    }
    let axis =
        |a: kurbo::Point, b: kurbo::Point| (a.x - b.x).abs() < 1e-9 || (a.y - b.y).abs() < 1e-9;
    if !(0..4).all(|i| axis(points[i], points[(i + 1) % 4])) {
        return None;
    }
    let (mut lo, mut hi) = (points[0], points[0]);
    for p in &points {
        lo = kurbo::Point::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = kurbo::Point::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    let size = [(hi.x - lo.x) as f32, (hi.y - lo.y) as f32];
    (size[0] > 0.0 && size[1] > 0.0).then_some(Flat::Rect {
        center: [(lo.x + hi.x) as f32 * 0.5, (lo.y + hi.y) as f32 * 0.5],
        size,
    })
}

/// Где стоит фигура в объёме: сдвиг, поворот вокруг Z, размер и отражение по X из
/// преобразования `transform` (наклон и неравный размер — по среднему). Отражение — это
/// поворот на 180° вокруг вертикали: пластина симметрична по Z, поэтому он отражает её
/// по X, не меняя расстояний.
pub(crate) fn placement(transform: Affine) -> ([f32; 2], f32, f32, bool) {
    let [a, b, c, d, e, f] = transform.as_coeffs();
    let det = a * d - b * c;
    let mirror = det < 0.0;
    // Первый столбец (куда уходит X фигуры), у отражённой — с минусом.
    let (x0, y0) = if mirror { (-a, -b) } else { (a, b) };
    let angle = y0.atan2(x0);
    let scale = det.abs().sqrt();
    (
        [e as f32, f as f32],
        angle.to_degrees() as f32,
        scale as f32,
        mirror,
    )
}

/// Пластина фигуры толщиной [`PLATE`] с передней стороной на `front`.
fn plate(flat: &Flat, transform: Affine, front: f32) -> Option<Shape3> {
    let ([x, y], angle, scale, mirror) = placement(transform);
    if scale <= 1e-6 {
        return None;
    }
    let s = LEVEL_SCALE * scale;
    let depth = PLATE / s;
    let local = match flat {
        Flat::Rect { center, size } => Shape3::Box {
            size: [size[0], size[1], depth],
            round: 0.0,
        }
        .at([center[0], center[1], 0.0]),
        Flat::Profile(profile) => Shape3::Extrude {
            profile: profile.clone(),
            depth,
            round: 0.0,
        },
    };
    Some(Shape3::Move {
        at: [x * LEVEL_SCALE, y * LEVEL_SCALE, front - PLATE * 0.5],
        turn: [0.0, if mirror { 180.0 } else { 0.0 }, angle],
        scale: s,
        shape: Box::new(local),
    })
}

/// SDF-объект: плавное объединение капсул, цвет и свечение.
fn object_shape(o: &reyvec_scene::SceneObject, z: f32) -> Option<(Shape3, [f32; 3], f32)> {
    if o.parts.is_empty() || o.color[3] < 0.05 {
        return None;
    }
    let parts: Vec<Shape3> = o
        .parts
        .iter()
        .map(|p| {
            let (dx, dy) = (p.b[0] - p.a[0], p.b[1] - p.a[1]);
            let length = (dx * dx + dy * dy).sqrt();
            // Капсула вдоль Y: поворот так, чтобы Y смотрел от a к b.
            let angle = dx.atan2(dy).to_degrees();
            Shape3::Capsule {
                height: length,
                radius: p.r,
            }
            .placed(
                [(p.a[0] + p.b[0]) * 0.5, (p.a[1] + p.b[1]) * 0.5, 0.0],
                [0.0, 0.0, -angle],
            )
        })
        .collect();
    let shape = Shape3::Move {
        at: [0.0, 0.0, z],
        turn: [0.0, 0.0, 0.0],
        scale: LEVEL_SCALE,
        shape: Box::new(Shape3::Union {
            shapes: parts,
            smooth: o.k,
        }),
    };
    let rgb = [o.color[0], o.color[1], o.color[2]];
    let bright = rgb.iter().copied().fold(0.0f32, f32::max);
    if o.glow {
        let unit = rgb.map(|c| c / bright.max(1e-4));
        Some((shape, unit, bright.max(1.0) * 2.0))
    } else {
        Some((shape, rgb.map(|c| c.min(1.0)), 0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 7.18д: на отдельной видеокарте 2.5D-вид по умолчанию рисуется полем, на встроенной —
    /// треугольниками, на программной — SDF (как в тестах); на панели SDF последний.
    #[test]
    fn level_render_for_adapter() {
        use crate::adapter::AdapterKind;
        assert_eq!(
            LevelRender::for_adapter(AdapterKind::Discrete),
            LevelRender::Field
        );
        assert_eq!(
            LevelRender::for_adapter(AdapterKind::Integrated),
            LevelRender::Triangles
        );
        assert_eq!(LevelRender::for_adapter(AdapterKind::Cpu), LevelRender::Sdf);
        assert_eq!(LevelRender::default(), LevelRender::Sdf);
        assert_eq!(LevelRender::ALL[0], LevelRender::Field);
        assert_eq!(LevelRender::ALL[2], LevelRender::Sdf);
        for render in LevelRender::ALL {
            assert!(!render.title().is_empty());
        }
        for name in ["sdf", "field", "поле", "triangles"] {
            assert!(LevelRender::from_name(name).is_some(), "{name}");
        }
    }

    /// Отражённая и повёрнутая фигура в объёме стоит там же, где на плоскости.
    #[test]
    fn placement_matches_affine() {
        let flat = Flat::Rect {
            center: [0.1, 0.0],
            size: [0.2, 0.05],
        };
        for transform in [
            Affine::translate((1.0, 2.0)),
            Affine::translate((1.0, 2.0)) * Affine::rotate(0.7),
            Affine::translate((-0.5, 0.3)) * Affine::scale_non_uniform(-1.0, 1.0),
            Affine::translate((0.2, 0.1))
                * Affine::rotate(-0.4)
                * Affine::scale_non_uniform(-1.0, 1.0)
                * Affine::scale(1.5),
        ] {
            let shape = plate(&flat, transform, 1.0).unwrap();
            // Точка внутри фигуры (её правый край) и точка снаружи.
            let inside = transform * kurbo::Point::new(0.19, 0.0);
            let outside = transform * kurbo::Point::new(-0.05, 0.0);
            let at = |p: kurbo::Point| {
                Vec3::new(
                    p.x as f32 * LEVEL_SCALE,
                    p.y as f32 * LEVEL_SCALE,
                    1.0 - PLATE * 0.5,
                )
            };
            assert!(shape.distance(at(inside)) < 0.0, "{transform:?}");
            assert!(shape.distance(at(outside)) > 0.0, "{transform:?}");
        }
    }

    /// Плита встаёт перед самой передней из накрытых; не перекрывающиеся делят лицевую
    /// грань; касание краями — не перекрытие.
    #[test]
    fn layers_follow_overlap_not_draw_order() {
        let r = |x0: f64, y0: f64, x1: f64, y1: f64| kurbo::Rect::new(x0, y0, x1, y1);
        let mut layers = Layers::default();
        // Мебель: три плитки ряда вплотную друг к другу — один слой.
        assert_eq!(layers.place(r(0.0, 0.0, 1.0, 1.0)), 0);
        assert_eq!(layers.place(r(1.0, 0.0, 2.0, 1.0)), 0);
        assert_eq!(layers.place(r(2.0, 0.0, 3.0, 1.0)), 0);
        // Стопка: монета из двух кругов — второй над первым.
        assert_eq!(layers.place(r(0.2, 0.2, 0.4, 0.4)), 1);
        assert_eq!(layers.place(r(0.25, 0.25, 0.35, 0.35)), 2);
        // Предмет далеко от всего (другая полоса) — снова у переднего края.
        assert_eq!(layers.place(r(10.0, 0.0, 10.5, 0.5)), 0);
        // Герой у третьей плитки: накрывает её, но не монету — сразу перед плиткой.
        assert_eq!(layers.place(r(2.3, 0.5, 2.6, 1.3)), 1);
        // Широкая плита (фон на весь уровень) накрывает всё: перед монетой.
        assert_eq!(layers.place(r(-100.0, -1.0, 100.0, 2.0)), 3);
        // И её видят из любой полосы.
        assert_eq!(layers.place(r(50.0, 0.0, 51.0, 1.0)), 4);
        // Перевёрнутый прямоугольник (отражение) — тот же, что прямой.
        assert_eq!(layers.place(r(51.0, 1.0, 50.0, 0.0)), 5);
        // Огромный прямоугольник (номера полос упёрлись бы в край i64) — широкий, сразу.
        assert_eq!(layers.place(r(-5e18, 0.0, 5e18, 1.0)), 6);
        // Плита без площади ничего не рисует: слой 0 и не ставится.
        assert_eq!(layers.place(r(0.3, 0.3, 0.3, 0.3)), 0);
        assert_eq!(layers.place(r(0.3, 0.3, 0.3, 0.3 + 1e-5)), 0);
        assert_eq!(layers.place(r(f64::NAN, 0.0, 1.0, 1.0)), 0);
        assert_eq!(
            layers.place(r(0.25, 0.25, 0.35, 0.35)),
            7,
            "над огромной, не над точкой"
        );
    }

    /// Прямоугольник плиты в мире накрывает её и так, как рисуют формы SDF (размер по
    /// среднему), и так, как рисуют треугольники (точное преобразование).
    #[test]
    fn world_rect_covers_drawn_plate() {
        let bounds = kurbo::Rect::new(-0.1, -0.05, 0.3, 0.05);
        let flat = Flat::Rect {
            center: [0.1, 0.0],
            size: [0.4, 0.1],
        };
        for transform in [
            Affine::translate((1.0, 2.0)) * Affine::rotate(0.7),
            Affine::translate((1.0, 2.0)) * Affine::scale_non_uniform(2.0, 0.5),
            Affine::translate((-0.5, 0.3)) * Affine::scale_non_uniform(-1.0, 1.0),
            Affine::translate((0.2, 0.1))
                * Affine::rotate(-0.4)
                * Affine::scale_non_uniform(-1.0, 1.0)
                * Affine::scale_non_uniform(1.5, 0.6),
            Affine::translate((0.2, 0.1)) * Affine::rotate(2.5) * Affine::skew(0.4, 0.0),
        ] {
            let rect = world_rect(bounds, transform);
            // `Rect::contains` не считает точку на дальнем краю, поэтому с допуском.
            let within = |p: kurbo::Point| rect.inflate(1e-9, 1e-9).contains(p);
            let shape = plate(&flat, transform, SOLID_FRONT + PLATE * 0.5).unwrap();
            // Точки внутри нарисованной пластины — внутри прямоугольника.
            let z = SOLID_FRONT + PLATE * 0.5 - PLATE * 0.5;
            let span = rect.inflate(1.0, 1.0);
            let mut inside = 0;
            for i in 0..80 {
                for j in 0..80 {
                    let x = span.x0 + span.width() * (i as f64 + 0.5) / 80.0;
                    let y = span.y0 + span.height() * (j as f64 + 0.5) / 80.0;
                    let p = Vec3::new(x as f32 * LEVEL_SCALE, y as f32 * LEVEL_SCALE, z);
                    if shape.distance(p) < 0.0 {
                        inside += 1;
                        assert!(within(kurbo::Point::new(x, y)), "{transform:?}: {x} {y}");
                    }
                }
            }
            assert!(inside > 20, "{transform:?}: пластина не найдена");
            // Точные углы фигуры тоже внутри (так рисуют треугольники).
            for corner in [
                (bounds.x0, bounds.y0),
                (bounds.x1, bounds.y0),
                (bounds.x1, bounds.y1),
                (bounds.x0, bounds.y1),
            ] {
                let q = transform * kurbo::Point::new(corner.0, corner.1);
                assert!(within(q), "{transform:?}: угол {q:?}");
            }
        }
    }

    /// Слои из снимка мира: не перекрывающиеся плиты на одной грани, накрывающая — на
    /// шаг впереди, и одинаково для форм SDF и треугольников.
    #[test]
    fn frame_layers_match_between_sdf_and_triangles() {
        let mut library = ShapeLibrary::new(0.01);
        let square = kurbo::Shape::to_path(&kurbo::Rect::new(-0.5, -0.5, 0.5, 0.5), 0.1);
        // Пять фигур разного цвета (свой материал у каждой), на местах по порядку:
        // A, B далеко от A, C накрывает A, D накрывает C, но не A, E далеко от всех.
        let places = [(0.0, 0.0), (5.0, 0.0), (0.6, 0.0), (1.05, 0.0), (5.0, 2.0)];
        let want_layers = [0u32, 0, 1, 2, 0];
        let mut scene = reyvec_scene::Scene::default();
        for (i, (x, y)) in places.iter().enumerate() {
            let color = Color::linear(0.2 + 0.15 * i as f32, 0.5, 0.5, 1.0);
            let id = library
                .add(reyvec_vector::Shape::fill(
                    square.clone(),
                    Paint::Solid(color),
                ))
                .unwrap();
            scene
                .items
                .push(SceneItem::Shape(reyvec_vector::Instance::new(
                    id,
                    Affine::translate((*x, *y)),
                )));
        }
        let mut level = Level3d::default();
        let sdf = level.frame(&scene, &library, LevelRender::Sdf);
        let triangles = level.frame(&scene, &library, LevelRender::Triangles);
        let want: Vec<f32> = want_layers.iter().map(|&l| plate_front(l)).collect();
        // Формы SDF: стена, потом пять пластин по порядку.
        let fronts: Vec<f32> = sdf
            .frame
            .shapes
            .iter()
            .filter_map(|s| match s {
                Shape3::Paint { shape, .. } => match **shape {
                    Shape3::Move { at, .. } => Some(at[2] + PLATE * 0.5),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(fronts.len(), 5);
        for (got, want) in fronts.iter().zip(&want) {
            assert!((got - want).abs() < 1e-6, "SDF: {fronts:?} против {want:?}");
        }
        // Треугольники: призма каждого материала (по порядку фигур), передняя грань —
        // самая большая Z.
        assert_eq!(triangles.frame.meshes.len(), 5);
        for (mesh, want) in triangles.frame.meshes.iter().zip(&want) {
            let front = mesh
                .triangles
                .iter()
                .flatten()
                .map(|v| v.z)
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(
                (front - want).abs() < 1e-6,
                "треугольники: {front} против {want}"
            );
        }
    }

    /// Поле уровня (7.18в) в комнате из примера: первый кадр — как у SDF и с запросом на
    /// запекание; с готовым полем неподвижное — одной формой, подвижное — формами, а
    /// расстояния и цвета, которые видит камера, — те же, что у SDF.
    #[test]
    fn field_frame_matches_sdf() {
        use reyvec_sdf::shape3d::{Program, compile};
        let file = reyvec_scene::SceneFile::parse_in(
            reyvec_scene::EXAMPLE,
            Some(std::path::Path::new(reyvec_scene::EXAMPLE_DIR)),
        )
        .unwrap();
        let mut library = ShapeLibrary::new(0.001);
        let scene = file
            .build(&mut library, &mut reyvec_scene::ShapeCache::new())
            .unwrap();
        assert_ne!(scene.generation, 0);
        let mut level = Level3d::default();
        let sdf = level.frame(&scene, &library, LevelRender::Sdf);
        assert_eq!(level.field_status(), LevelFieldStatus::Off);
        let first = level.frame(&scene, &library, LevelRender::Field);
        assert_eq!(
            first.frame.shapes, sdf.frame.shapes,
            "пока поля нет — как SDF"
        );
        assert!(matches!(
            level.field_status(),
            LevelFieldStatus::Baking { .. }
        ));
        let (ticket, bake) = level.take_field_request().expect("запрос на запекание");
        // Тот же набор — второй запрос не нужен.
        level.frame(&scene, &library, LevelRender::Field);
        assert!(level.take_field_request().is_none());
        let (mut field, stats) = bake.run().expect("поле");
        field.set_texture(1);
        field.set_material_texture(2);
        assert!(level.install_field(ticket, FieldRef(Arc::new(field)), stats, 1.0));
        let made = level.frame(&scene, &library, LevelRender::Field);
        let LevelFieldStatus::Ready {
            plates,
            solids,
            shapes,
            ..
        } = level.field_status()
        else {
            panic!("поле не готово: {:?}", level.field_status());
        };
        assert!(
            plates > 100 && solids > 40 && shapes > 40,
            "{plates} {solids} {shapes}"
        );
        assert!(
            made.frame.shapes.len() + plates + solids < sdf.frame.shapes.len() + 10,
            "{} форм с полем, {} у SDF",
            made.frame.shapes.len(),
            sdf.frame.shapes.len()
        );
        // Номера материалов те же: таблица поля — начало таблицы кадра.
        assert!(made.materials.len() >= sdf.materials.len());
        let programs = |shapes: &[Shape3]| -> Vec<Program> {
            shapes.iter().map(|s| compile(s).unwrap()).collect()
        };
        let (a, b) = (programs(&sdf.frame.shapes), programs(&made.frame.shapes));
        let near = |list: &[Program], p: Vec3| {
            list.iter()
                .map(|q| q.distance(p))
                .fold(
                    (f32::INFINITY, 0),
                    |best, d| if d.0 < best.0 { d } else { best },
                )
        };
        // Точки — у фигур снимка (их места, плюс-минус 0,15 единицы 2D).
        let centers: Vec<[f64; 2]> = scene
            .items
            .iter()
            .filter_map(|i| match i {
                SceneItem::Shape(s) => Some(s.transform.as_coeffs()),
                _ => None,
            })
            .map(|c| [c[4], c[5]])
            .collect();
        let mut seed = 17u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % 100_000) as f32 / 100_000.0
        };
        let (mut same, mut hits, mut on_level) = (0, 0, 0);
        for _ in 0..400 {
            let c = centers[(rnd() * centers.len() as f32) as usize % centers.len()];
            let x = (c[0] as f32 + rnd() * 0.3 - 0.15) * LEVEL_SCALE;
            let y = (c[1] as f32 + rnd() * 0.3 - 0.15) * LEVEL_SCALE;
            let z = 0.5 + rnd();
            let p = Vec3::new(x, y, z);
            let (want, _) = near(&a, p);
            let (got, _) = near(&b, p);
            // Стопки неподвижных плит в поле — в одной плоскости (до ~1 см разницы),
            // клетка поля 1 см; дальше полосы поле — только оценка снизу. Внутри бруска
            // формы SDF — куски по 2 м, их расстояние внутри до шва, а у поля — до края.
            if want < -0.02 {
                assert!(got < 0.0, "{p}: поле {got}, SDF {want}");
            } else if want < 0.9 {
                assert!((got - want).abs() < 0.03, "{p}: поле {got}, SDF {want}");
            } else {
                assert!(got <= want + 0.03, "{p}: поле {got}, SDF {want}");
            }
            // Луч камеры спереди: тот же цвет там, куда он попал.
            let mut q = Vec3::new(x, y, 2.5);
            let hit = |list: &[Program], mut q: Vec3| {
                for _ in 0..200 {
                    let (d, m) = near(list, q);
                    if d < 1e-3 {
                        return Some((q.z, m));
                    }
                    q.z -= d.max(1e-3);
                }
                None
            };
            q.z = 2.5;
            if let (Some((za, ma)), Some((zb, mb))) = (hit(&a, q), hit(&b, q)) {
                hits += 1;
                // Не в стену (материал 0): в плиту или брусок.
                on_level += usize::from(ma != 0);
                if ma == mb && (za - zb).abs() < 0.03 {
                    same += 1;
                }
            }
        }
        assert!(
            hits > 300 && on_level > 150,
            "попаданий {hits}, в уровень {on_level}"
        );
        assert!(same * 100 >= hits * 97, "тот же цвет у {same} из {hits}");
        // Новый мир с тем же неподвижным (уровень загружен заново) — поле то же, без
        // запекания и без кусков.
        let other = reyvec_scene::Scene {
            generation: scene.generation + 1,
            ..scene.clone()
        };
        let again = level.frame(&other, &library, LevelRender::Field);
        assert_eq!(again.frame.shapes.len(), made.frame.shapes.len());
        assert!(level.take_field_request().is_none());
        assert!(level.take_field_patches().is_empty());
    }

    /// Подвижная плита, заехавшая под неподвижную, нарисованную после неё: поле, где эта
    /// неподвижная лежит в общем слое, становится неверным (подвижная оказалась бы над
    /// ней), поэтому кадр рисуется формами, пока не испечётся новое поле, где она и
    /// стопка над ней — формы. Уехала обратно — годится и новое поле.
    #[test]
    fn field_keeps_still_plates_over_moving_ones_as_forms() {
        let mut library = ShapeLibrary::new(0.01);
        let square = kurbo::Shape::to_path(&kurbo::Rect::new(-0.5, -0.5, 0.5, 0.5), 0.1);
        let ids: Vec<ShapeId> = (0..4)
            .map(|i| {
                let color = Color::linear(0.2 + 0.2 * i as f32, 0.5, 0.5, 1.0);
                library
                    .add(reyvec_vector::Shape::fill(
                        square.clone(),
                        Paint::Solid(color),
                    ))
                    .unwrap()
            })
            .collect();
        let put = |id: ShapeId, x: f64| {
            SceneItem::Shape(reyvec_vector::Instance::new(
                id,
                Affine::translate((x, 0.0)),
            ))
        };
        // Подвижная M, потом неподвижные B, C (накрывает B) и D (далеко от всех).
        let scene_with = |m: f64| reyvec_scene::Scene {
            generation: 1,
            items: vec![
                SceneItem::BeginDynamic,
                put(ids[0], m),
                SceneItem::EndDynamic,
                put(ids[1], 0.0),
                put(ids[2], 0.6),
                put(ids[3], 5.0),
            ],
            ..reyvec_scene::Scene::default()
        };
        let mut level = Level3d::default();
        let bake_and_install = |level: &mut Level3d| {
            let (ticket, bake) = level.take_field_request().expect("запрос на запекание");
            let (mut field, stats) = bake.run().expect("поле");
            field.set_texture(1);
            field.set_material_texture(2);
            assert!(level.install_field(ticket, FieldRef(Arc::new(field)), stats, 1.0));
        };
        let far = scene_with(-10.0);
        level.frame(&far, &library, LevelRender::Field);
        bake_and_install(&mut level);
        // Стена, M и поле.
        let made = level.frame(&far, &library, LevelRender::Field);
        assert_eq!(made.frame.shapes.len(), 3);
        // Кадр держит поле; пока он жив, поле нельзя перепечь на месте.
        drop(made);
        // M под B: поле неверно, всё формами (стена и четыре плиты), запекание ждёт покоя.
        let under = scene_with(0.0);
        let sdf = level.frame(&under, &library, LevelRender::Sdf);
        let fronts = |shapes: &[Shape3]| -> Vec<f32> {
            shapes
                .iter()
                .filter_map(|s| match s {
                    Shape3::Paint { shape, .. } => match **shape {
                        Shape3::Move { at, .. } => Some(at[2] + PLATE * 0.5),
                        _ => None,
                    },
                    _ => None,
                })
                .collect()
        };
        // B и C выходят из поля в этот же кадр (кусок вокруг B перепекается на месте),
        // полное запекание не нужно. Стена, M, B и C формами (C — над B, поэтому тоже), D —
        // в поле. Слои форм — как у SDF: M, над ней B, над B — C.
        let made = level.frame(&under, &library, LevelRender::Field);
        assert_eq!(made.frame.shapes.len(), 5);
        assert!(level.take_field_request().is_none());
        assert_eq!(level.take_field_patches().len(), 2, "куски вокруг B и C");
        let LevelFieldStatus::Ready {
            plates,
            shapes,
            patched,
            ..
        } = level.field_status()
        else {
            panic!("поле не готово: {:?}", level.field_status());
        };
        assert_eq!((plates, shapes), (1, 3));
        assert_eq!(patched.map(|p| p.pieces), Some(2));
        // Последняя форма — само поле, у лицевой грани неподвижного.
        let got = fronts(&made.frame.shapes);
        assert_eq!(got[..3], fronts(&sdf.frame.shapes)[..3]);
        assert_eq!(got[3], plate_front(0));
        drop(made);
        // M уехала: B и C — формами, пока не простоят FIELD_STABLE_FRAMES кадров, потом
        // возвращаются в поле на месте.
        for _ in 1..FIELD_STABLE_FRAMES {
            let made = level.frame(&far, &library, LevelRender::Field);
            assert_eq!(made.frame.shapes.len(), 5);
        }
        assert!(level.take_field_patches().is_empty());
        // Очередь — не больше FIELD_ADD_POINTS точек за кадр: кусок вокруг квадрата больше,
        // поэтому B и C входят по одному за кадр.
        let made = level.frame(&far, &library, LevelRender::Field);
        assert_eq!(made.frame.shapes.len(), 4);
        assert_eq!(level.take_field_patches().len(), 1);
        drop(made);
        let made = level.frame(&far, &library, LevelRender::Field);
        assert_eq!(made.frame.shapes.len(), 3);
        assert_eq!(level.take_field_patches().len(), 1);
        assert!(level.take_field_request().is_none());
        assert!(matches!(
            level.field_status(),
            LevelFieldStatus::Ready { plates: 3, .. }
        ));
    }

    /// Правка файла сцены на лету (7.18г): мир строится заново, а поле не печётся целиком —
    /// сдвинутые ящики вынимаются из него на месте и потом вписываются на новом месте.
    #[test]
    fn edited_scene_file_is_patched_in_place() {
        let dir = Some(std::path::Path::new(reyvec_scene::EXAMPLE_DIR));
        let mut library = ShapeLibrary::new(0.001);
        let mut cache = reyvec_scene::ShapeCache::new();
        let mut build = |text: &str| {
            reyvec_scene::SceneFile::parse_in(text, dir)
                .unwrap()
                .build(&mut library, &mut cache)
                .unwrap()
        };
        let before = build(reyvec_scene::EXAMPLE);
        // В файле сдвинули ящики под полкой и сохранили: мир строится заново из того же
        // кэша форм, у остальных предметов те же ключи.
        let moved = reyvec_scene::EXAMPLE.replacen("at: (1.95, -0.78)", "at: (2.05, -0.78)", 1);
        assert_ne!(moved, reyvec_scene::EXAMPLE);
        let after = build(&moved);
        assert_ne!(after.generation, before.generation);
        let mut level = Level3d::default();
        level.frame(&before, &library, LevelRender::Field);
        let (ticket, bake) = level.take_field_request().expect("запрос на запекание");
        let (mut field, stats) = bake.run().expect("поле");
        field.set_texture(1);
        field.set_material_texture(2);
        assert!(level.install_field(ticket, FieldRef(Arc::new(field)), stats, 1.0));
        drop(level.frame(&before, &library, LevelRender::Field));
        drop(level.frame(&after, &library, LevelRender::Field));
        assert!(
            level.take_field_request().is_none(),
            "правка файла — не полное запекание"
        );
        assert!(
            !level.take_field_patches().is_empty(),
            "ящики вынуты на месте"
        );
        assert!(matches!(
            level.field_status(),
            LevelFieldStatus::Ready {
                patched: Some(_),
                rebaking: false,
                ..
            }
        ));
        // Простояв FIELD_STABLE_FRAMES кадров (первый — кадр правки), ящики на новом месте
        // входят в поле.
        for _ in 2..FIELD_STABLE_FRAMES {
            drop(level.frame(&after, &library, LevelRender::Field));
        }
        assert!(level.take_field_patches().is_empty());
        drop(level.frame(&after, &library, LevelRender::Field));
        assert!(
            !level.take_field_patches().is_empty(),
            "ящики вписаны в поле"
        );
        assert!(level.take_field_request().is_none());
    }

    /// Три неподвижные плиты для проверок порядка: широкий пол, на нём «камень» и «трава»
    /// поверх камня. `tint` — оттенок камня, `swap` — трава рисуется раньше камня.
    fn stack_scene(
        library: &mut ShapeLibrary,
    ) -> impl Fn(Color, bool) -> reyvec_scene::Scene + use<> {
        let rect = |w: f64, h: f64| kurbo::Shape::to_path(&kurbo::Rect::new(0.0, 0.0, w, h), 0.1);
        let mut shape = |w: f64, h: f64, r: f32| {
            library
                .add(reyvec_vector::Shape::fill(
                    rect(w, h),
                    Paint::Solid(Color::linear(r, 0.4, 0.3, 1.0)),
                ))
                .unwrap()
        };
        let (floor, rock, grass) = (
            shape(4.0, 0.2, 0.2),
            shape(0.4, 0.4, 0.5),
            shape(0.4, 0.4, 0.8),
        );
        move |tint: Color, swap: bool| {
            let put = |id: ShapeId, x: f64, y: f64, tint: Color| {
                let mut instance = reyvec_vector::Instance::new(id, Affine::translate((x, y)));
                instance.tint = tint;
                SceneItem::Shape(instance)
            };
            let white = Color::linear(1.0, 1.0, 1.0, 1.0);
            let (a, b) = (put(rock, 1.0, 0.1, tint), put(grass, 1.2, 0.3, white));
            let mut items = vec![put(floor, 0.0, 0.0, white)];
            if swap {
                items.extend([b, a]);
            } else {
                items.extend([a, b]);
            }
            reyvec_scene::Scene {
                generation: 1,
                items,
                ..reyvec_scene::Scene::default()
            }
        }
    }

    /// Лицевые грани плит кадра (формы по порядку, поле — последним).
    fn plate_fronts(shapes: &[Shape3]) -> Vec<f32> {
        shapes
            .iter()
            .filter_map(|s| match s {
                Shape3::Paint { shape, .. } => match **shape {
                    Shape3::Move { at, .. } => Some(at[2] + PLATE * 0.5),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    /// Готовое поле `level` — то же, что полное запекание сцены `scene` (байт в байт).
    fn assert_field_is_full_bake(
        level: &Level3d,
        scene: &reyvec_scene::Scene,
        library: &ShapeLibrary,
    ) {
        let mut fresh = Level3d::default();
        drop(fresh.frame(scene, library, LevelRender::Field));
        let (_, bake) = fresh.take_field_request().unwrap();
        let (whole, _) = bake.run().unwrap();
        let field = level.baked_field().unwrap();
        assert_eq!(
            (whole.corner(), whole.cell()),
            (field.corner(), field.cell())
        );
        assert!(
            whole.bytes() == field.bytes(),
            "значения как у полного запекания"
        );
    }

    fn bake_level_field(level: &mut Level3d) {
        let (ticket, bake) = level.take_field_request().expect("запрос на запекание");
        let (mut field, stats) = bake.run().expect("поле");
        field.set_texture(1);
        field.set_material_texture(2);
        assert!(level.install_field(ticket, FieldRef(Arc::new(field)), stats, 1.0));
    }

    /// Неподвижная плита, которой нет в поле (камень перекрасили), рисуется формой на своём
    /// слое — и трава, нарисованная поверх неё, уходит из поля в формы над ней, как в SDF,
    /// а не остаётся в поле под камнем. Простояв, камень входит в поле, трава — сразу за ним.
    #[test]
    fn fresh_plate_keeps_later_overlapping_ones_as_forms() {
        let mut library = ShapeLibrary::new(0.01);
        let scene_with = stack_scene(&mut library);
        let white = Color::linear(1.0, 1.0, 1.0, 1.0);
        let mut level = Level3d::default();
        let before = scene_with(white, false);
        drop(level.frame(&before, &library, LevelRender::Field));
        bake_level_field(&mut level);
        let made = level.frame(&before, &library, LevelRender::Field);
        assert_eq!(made.frame.shapes.len(), 2, "стена и поле");
        drop(made);
        let red = Color::linear(1.0, 0.3, 0.3, 1.0);
        let after = scene_with(red, false);
        let sdf = plate_fronts(&level.frame(&after, &library, LevelRender::Sdf).frame.shapes);
        let made = level.frame(&after, &library, LevelRender::Field);
        // Стена, камень и трава формами, поле.
        assert_eq!(made.frame.shapes.len(), 4);
        let got = plate_fronts(&made.frame.shapes);
        assert_eq!(got[..2], sdf[1..3], "слои камня и травы — как в SDF");
        assert!(
            got[1] > got[0] && got[0] > got[2],
            "трава над камнем, камень над полем"
        );
        drop(made);
        assert_eq!(level.take_field_patches().len(), 2, "камень и трава вынуты");
        assert!(level.take_field_request().is_none());
        for _ in 2..FIELD_STABLE_FRAMES {
            assert_eq!(
                level
                    .frame(&after, &library, LevelRender::Field)
                    .frame
                    .shapes
                    .len(),
                4
            );
        }
        assert!(level.take_field_patches().is_empty());
        // Камень входит в поле; трава ещё над формой камня — форма.
        assert_eq!(
            level
                .frame(&after, &library, LevelRender::Field)
                .frame
                .shapes
                .len(),
            3
        );
        assert_eq!(level.take_field_patches().len(), 1);
        // Трава простояла столько же и входит сразу за ним.
        assert_eq!(
            level
                .frame(&after, &library, LevelRender::Field)
                .frame
                .shapes
                .len(),
            2
        );
        assert_eq!(level.take_field_patches().len(), 1);
        assert!(level.take_field_request().is_none());
        assert_field_is_full_bake(&level, &after, &library);
    }

    /// Плиты поля поменялись местами в порядке рисования (правка файла), ключи те же: в
    /// перекрытии цвет теперь другой плиты — этот кусок перепекается в тот же кадр.
    #[test]
    fn reordered_plates_are_rebaked_in_place() {
        let mut library = ShapeLibrary::new(0.01);
        let scene_with = stack_scene(&mut library);
        let white = Color::linear(1.0, 1.0, 1.0, 1.0);
        let mut level = Level3d::default();
        let before = scene_with(white, false);
        drop(level.frame(&before, &library, LevelRender::Field));
        bake_level_field(&mut level);
        drop(level.frame(&before, &library, LevelRender::Field));
        let swapped = scene_with(white, true);
        let made = level.frame(&swapped, &library, LevelRender::Field);
        assert_eq!(made.frame.shapes.len(), 2, "всё в поле");
        let materials = made.materials.clone();
        drop(made);
        assert_eq!(level.take_field_patches().len(), 1, "кусок перекрытия");
        assert!(level.take_field_request().is_none());
        assert_field_is_full_bake(&level, &swapped, &library);
        // Цвет в перекрытии — камня (теперь он сверху).
        let field = level.baked_field().unwrap();
        let p = Vec2::new(1.3, 0.4) * LEVEL_SCALE;
        let id = field.material_at(p).expect("материал");
        assert_eq!(materials[id as usize].color, [0.5, 0.4, 0.3]);
        // Порядок запомнен: следующий кадр ничего не перепекает.
        drop(level.frame(&swapped, &library, LevelRender::Field));
        assert!(level.take_field_patches().is_empty());
    }

    /// Смена порядка: проверяются только пары со сдвинутой плитой, и находятся все
    /// поменявшиеся перекрывающиеся пары.
    #[test]
    fn reordered_finds_swapped_overlaps_only() {
        assert_eq!(off_longest_run(&[0, 1, 2, 3]), [false; 4]);
        assert_eq!(off_longest_run(&[3, 0, 1, 2]), [true, false, false, false]);
        assert_eq!(off_longest_run(&[]), Vec::<bool>::new());
        // Пять плит в ряд, каждая перекрывает соседей; последнюю переставили в начало.
        let rect = |x: f32| (Vec2::new(x, 0.0), Vec2::new(x + 1.5, 1.0));
        let rects: HashMap<u64, Rect2> = (0..5u64).map(|k| (k, rect(k as f32))).collect();
        let was = [0, 1, 2, 3, 4];
        let now = [4, 0, 1, 2, 3];
        let got = reordered(&now, &was, &rects);
        // Плита 4 перекрывает только плиту 3: одно перекрытие.
        assert_eq!(got, vec![(Vec2::new(4.0, 0.0), Vec2::new(4.5, 1.0))]);
        assert!(reordered(&was, &was, &rects).is_empty());
    }

    /// Большая плита без пометки «подвижное», меняющаяся каждый кадр, и большая новая
    /// плита: поле не перепекается целиком по кругу и не печёт миллионы точек в кадре.
    #[test]
    fn large_changing_plates_do_not_loop_full_bakes() {
        let mut library = ShapeLibrary::new(0.01);
        let rect = |w: f64, h: f64| kurbo::Shape::to_path(&kurbo::Rect::new(0.0, 0.0, w, h), 0.1);
        let mut shape = |w: f64, h: f64| {
            library
                .add(reyvec_vector::Shape::fill(
                    rect(w, h),
                    Paint::Solid(Color::linear(0.5, 0.4, 0.3, 1.0)),
                ))
                .unwrap()
        };
        // Пол и столб задают сетку 16 × 12 м; плита 10 × 10 м — внутри неё.
        let (floor, pillar, big) = (shape(4.0, 0.2), shape(0.1, 3.0), shape(2.5, 2.5));
        let scene_with = |big_tint: Option<f32>| {
            let put = |id: ShapeId, x: f64, y: f64, tint: f32| {
                let mut instance = reyvec_vector::Instance::new(id, Affine::translate((x, y)));
                instance.tint = Color::linear(tint, 1.0, 1.0, 1.0);
                SceneItem::Shape(instance)
            };
            let mut items = vec![put(floor, 0.0, 0.0, 1.0), put(pillar, 3.8, 0.0, 1.0)];
            if let Some(t) = big_tint {
                items.push(put(big, 0.5, 0.3, t));
            }
            reyvec_scene::Scene {
                generation: 1,
                items,
                ..reyvec_scene::Scene::default()
            }
        };
        // Плиту перекрашивают каждый кадр.
        let mut level = Level3d::default();
        drop(level.frame(&scene_with(Some(0.50)), &library, LevelRender::Field));
        bake_level_field(&mut level);
        drop(level.frame(&scene_with(Some(0.51)), &library, LevelRender::Field));
        // Ушло больше FIELD_REMOVE_POINTS: поле печётся целиком, но уже без неё.
        bake_level_field(&mut level);
        for i in 0..5 {
            let tint = 0.52 + 0.01 * i as f32;
            drop(level.frame(&scene_with(Some(tint)), &library, LevelRender::Field));
            assert!(
                level.take_field_request().is_none(),
                "кадр {i}: снова целиком"
            );
            assert!(level.take_field_patches().is_empty());
        }
        assert!(matches!(
            level.field_status(),
            LevelFieldStatus::Ready { plates: 2, .. }
        ));
        // Новая большая плита простояла: в поле — полным запеканием в фоне, а не в кадре.
        let mut level = Level3d::default();
        drop(level.frame(&scene_with(None), &library, LevelRender::Field));
        bake_level_field(&mut level);
        let still = scene_with(Some(0.5));
        for _ in 0..FIELD_STABLE_FRAMES {
            drop(level.frame(&still, &library, LevelRender::Field));
        }
        assert!(
            level.take_field_patches().is_empty(),
            "кусок больше FIELD_REMOVE_POINTS"
        );
        assert!(level.take_field_request().is_some());
        assert!(matches!(
            level.field_status(),
            LevelFieldStatus::Ready { rebaking: true, .. }
        ));
    }

    /// Большая плита поверх плиты, перекрашиваемой каждый кадр без пометки «подвижное»:
    /// после полного запекания она не входит в поле, чтобы тут же уйти, — запекания не
    /// идут по кругу, поле работает.
    #[test]
    fn plate_over_changing_one_does_not_loop_full_bakes() {
        let mut library = ShapeLibrary::new(0.01);
        let rect = |w: f64, h: f64| kurbo::Shape::to_path(&kurbo::Rect::new(0.0, 0.0, w, h), 0.1);
        let mut shape = |w: f64, h: f64| {
            library
                .add(reyvec_vector::Shape::fill(
                    rect(w, h),
                    Paint::Solid(Color::linear(0.5, 0.4, 0.3, 1.0)),
                ))
                .unwrap()
        };
        let (floor, pillar, small, big) = (
            shape(4.0, 0.2),
            shape(0.1, 3.0),
            shape(0.3, 0.3),
            shape(2.5, 2.5),
        );
        let scene_with = |tint: f32| {
            let put = |id: ShapeId, x: f64, y: f64, tint: f32| {
                let mut instance = reyvec_vector::Instance::new(id, Affine::translate((x, y)));
                instance.tint = Color::linear(tint, 1.0, 1.0, 1.0);
                SceneItem::Shape(instance)
            };
            reyvec_scene::Scene {
                generation: 1,
                items: vec![
                    put(floor, 0.0, 0.0, 1.0),
                    put(pillar, 3.8, 0.0, 1.0),
                    put(small, 0.6, 0.4, tint),
                    put(big, 0.5, 0.3, 1.0),
                ],
                ..reyvec_scene::Scene::default()
            }
        };
        let mut level = Level3d::default();
        let mut requests = 0;
        for i in 0..3 * FIELD_STABLE_FRAMES {
            let tint = 0.5 + 0.001 * i as f32;
            drop(level.frame(&scene_with(tint), &library, LevelRender::Field));
            if let Some((ticket, bake)) = level.take_field_request() {
                requests += 1;
                let (mut field, stats) = bake.run().expect("поле");
                field.set_texture(1);
                field.set_material_texture(2);
                assert!(level.install_field(ticket, FieldRef(Arc::new(field)), stats, 1.0));
            }
        }
        assert!(requests <= 2, "запеканий целиком: {requests}");
        assert!(matches!(
            level.field_status(),
            LevelFieldStatus::Ready { .. }
        ));
    }

    /// Отказ видеокарты при плите, меняющейся каждый кадр: после отказа запекание просится
    /// не больше одного раза (по простоявшему набору), потом — никогда.
    #[test]
    fn failed_field_with_changing_plate_is_not_baked_again() {
        let mut library = ShapeLibrary::new(0.01);
        let scene_with = stack_scene(&mut library);
        let mut level = Level3d::default();
        let mut requests = 0;
        for i in 0..4 * FIELD_STABLE_FRAMES {
            let tint = Color::linear(0.5 + 0.001 * i as f32, 1.0, 1.0, 1.0);
            drop(level.frame(&scene_with(tint, false), &library, LevelRender::Field));
            if let Some((ticket, _)) = level.take_field_request() {
                requests += 1;
                level.field_failed(ticket, Some("видеокарта не дала картинку поля"));
            }
        }
        assert_eq!(
            requests, 2,
            "первое запекание и одна попытка по простоявшему"
        );
        assert!(matches!(level.field_status(), LevelFieldStatus::Failed(_)));
    }

    /// Неподвижная плита над подвижной, которая то заезжает под неё, то уезжает: каждый
    /// следующий возврат в поле ждёт вдвое дольше.
    #[test]
    fn bouncing_plate_waits_longer_each_time() {
        let mut library = ShapeLibrary::new(0.01);
        let square = kurbo::Shape::to_path(&kurbo::Rect::new(-0.5, -0.5, 0.5, 0.5), 0.1);
        let ids: Vec<ShapeId> = (0..2)
            .map(|i| {
                library
                    .add(reyvec_vector::Shape::fill(
                        square.clone(),
                        Paint::Solid(Color::linear(0.3 + 0.3 * i as f32, 0.5, 0.5, 1.0)),
                    ))
                    .unwrap()
            })
            .collect();
        let scene_with = |m: f64| reyvec_scene::Scene {
            generation: 1,
            items: vec![
                SceneItem::BeginDynamic,
                SceneItem::Shape(reyvec_vector::Instance::new(
                    ids[0],
                    Affine::translate((m, 0.0)),
                )),
                SceneItem::EndDynamic,
                SceneItem::Shape(reyvec_vector::Instance::new(ids[1], Affine::IDENTITY)),
            ],
            ..reyvec_scene::Scene::default()
        };
        let (far, under) = (scene_with(-10.0), scene_with(0.0));
        let mut level = Level3d::default();
        drop(level.frame(&far, &library, LevelRender::Field));
        bake_level_field(&mut level);
        drop(level.frame(&far, &library, LevelRender::Field));
        let in_field = |level: &Level3d| {
            matches!(
                level.field_status(),
                LevelFieldStatus::Ready { plates: 1, .. }
            )
        };
        assert!(in_field(&level));
        for wait in [
            FIELD_STABLE_FRAMES,
            2 * FIELD_STABLE_FRAMES,
            4 * FIELD_STABLE_FRAMES,
        ] {
            drop(level.frame(&under, &library, LevelRender::Field));
            assert!(!in_field(&level), "над подвижной — форма");
            let mut frames = 0;
            while !in_field(&level) {
                drop(level.frame(&far, &library, LevelRender::Field));
                frames += 1;
                assert!(frames <= 4 * FIELD_STABLE_FRAMES);
            }
            assert_eq!(frames, wait);
        }
        level.take_field_patches();
        assert!(level.take_field_request().is_none());
    }

    /// Нижняя плита без пометки «подвижное» мигает (меняет цвет) раз в несколько секунд:
    /// верхняя каждый раз уходит из поля, а возвращается всё позже.
    #[test]
    fn plate_over_blinking_one_waits_longer_each_time() {
        let mut library = ShapeLibrary::new(0.01);
        let scene_with = stack_scene(&mut library);
        let tint = |t: f32| Color::linear(t, 1.0, 1.0, 1.0);
        let mut level = Level3d::default();
        drop(level.frame(&scene_with(tint(1.0), false), &library, LevelRender::Field));
        bake_level_field(&mut level);
        let all_in = |level: &Level3d| {
            matches!(
                level.field_status(),
                LevelFieldStatus::Ready { plates: 3, .. }
            )
        };
        drop(level.frame(&scene_with(tint(1.0), false), &library, LevelRender::Field));
        assert!(all_in(&level));
        let mut waits = Vec::new();
        for blink in 0..3 {
            let scene = scene_with(tint(0.5 + 0.1 * blink as f32), false);
            let mut frames = 0;
            loop {
                drop(level.frame(&scene, &library, LevelRender::Field));
                frames += 1;
                if all_in(&level) {
                    break;
                }
                assert!(frames <= 8 * FIELD_STABLE_FRAMES);
            }
            waits.push(frames);
        }
        // Камень входит через FIELD_STABLE_FRAMES кадров, трава — сразу за ним (кадры покоя
        // у неё считаются со второго кадра), потом трава ждёт вдвое дольше.
        assert_eq!(
            waits,
            [
                FIELD_STABLE_FRAMES + 1,
                2 * FIELD_STABLE_FRAMES + 1,
                4 * FIELD_STABLE_FRAMES + 1
            ]
        );
        assert!(level.take_field_request().is_none());
    }

    /// Видеокарта отказывает в картинке поля, а подвижная плита то заезжает под
    /// неподвижную, то уезжает: два набора чередуются, но каждый просится один раз.
    #[test]
    fn failed_field_with_alternating_set_is_not_baked_again() {
        let mut library = ShapeLibrary::new(0.01);
        let square = kurbo::Shape::to_path(&kurbo::Rect::new(-0.5, -0.5, 0.5, 0.5), 0.1);
        let ids: Vec<ShapeId> = (0..3)
            .map(|i| {
                library
                    .add(reyvec_vector::Shape::fill(
                        square.clone(),
                        Paint::Solid(Color::linear(0.3 + 0.2 * i as f32, 0.5, 0.5, 1.0)),
                    ))
                    .unwrap()
            })
            .collect();
        let scene_with = |m: f64| reyvec_scene::Scene {
            generation: 1,
            items: vec![
                SceneItem::BeginDynamic,
                SceneItem::Shape(reyvec_vector::Instance::new(
                    ids[0],
                    Affine::translate((m, 0.0)),
                )),
                SceneItem::EndDynamic,
                SceneItem::Shape(reyvec_vector::Instance::new(ids[1], Affine::IDENTITY)),
                SceneItem::Shape(reyvec_vector::Instance::new(
                    ids[2],
                    Affine::translate((3.0, 0.0)),
                )),
            ],
            ..reyvec_scene::Scene::default()
        };
        let (far, under) = (scene_with(-10.0), scene_with(0.0));
        let mut level = Level3d::default();
        let mut requests = 0;
        for cycle in 0..4 {
            for (scene, frames) in [(&under, 5), (&far, 2 * FIELD_STABLE_FRAMES)] {
                for _ in 0..frames {
                    drop(level.frame(scene, &library, LevelRender::Field));
                    if let Some((ticket, _)) = level.take_field_request() {
                        requests += 1;
                        level.field_failed(ticket, Some("видеокарта не дала картинку поля"));
                    }
                }
            }
            assert!(requests <= 3, "цикл {cycle}: запросов {requests}");
        }
        assert!(matches!(level.field_status(), LevelFieldStatus::Failed(_)));
    }

    /// Видеокарта не дала картинку поля: для этого набора поле больше не просится (и
    /// строка на панели говорит почему), а не печётся заново каждый кадр.
    #[test]
    fn failed_field_is_not_baked_again() {
        let mut library = ShapeLibrary::new(0.01);
        let scene_with = stack_scene(&mut library);
        let scene = scene_with(Color::linear(1.0, 1.0, 1.0, 1.0), false);
        let mut level = Level3d::default();
        drop(level.frame(&scene, &library, LevelRender::Field));
        let (ticket, _) = level.take_field_request().expect("запрос на запекание");
        level.field_failed(ticket, Some("видеокарта не дала картинку поля"));
        drop(level.frame(&scene, &library, LevelRender::Field));
        assert!(level.take_field_request().is_none());
        assert_eq!(
            level.field_status(),
            LevelFieldStatus::Failed("видеокарта не дала картинку поля")
        );
    }

    /// Перепекание на месте (7.18г): сдвинутая неподвижная фигура выходит из поля в тот же
    /// кадр, на новом месте входит через FIELD_STABLE_FRAMES кадров, и поле после этого —
    /// то же, что полное запекание нового набора (значения байт в байт, цвета — те же).
    /// Новая фигура за краем сетки — формой, а поле печётся целиком в фоне.
    #[test]
    fn moved_still_shape_is_rebaked_in_place() {
        let mut library = ShapeLibrary::new(0.01);
        let rect = |w: f64, h: f64| kurbo::Shape::to_path(&kurbo::Rect::new(0.0, 0.0, w, h), 0.1);
        let mut shape = |w: f64, h: f64, r: f32| {
            library
                .add(reyvec_vector::Shape::fill(
                    rect(w, h),
                    Paint::Solid(Color::linear(r, 0.4, 0.3, 1.0)),
                ))
                .unwrap()
        };
        // Широкий пол задаёт сетку поля, на нём ящик и бочка.
        let (floor, crate_, barrel) = (
            shape(4.0, 0.2, 0.2),
            shape(0.3, 0.3, 0.5),
            shape(0.2, 0.4, 0.8),
        );
        let put = |id: ShapeId, x: f64, y: f64| {
            SceneItem::Shape(reyvec_vector::Instance::new(id, Affine::translate((x, y))))
        };
        let scene_with = |crate_x: f64, barrel_x: f64| reyvec_scene::Scene {
            generation: 1,
            items: vec![
                put(floor, 0.0, 0.0),
                put(crate_, crate_x, 0.2),
                put(barrel, barrel_x, 0.2),
            ],
            ..reyvec_scene::Scene::default()
        };
        let bake_and_install = |level: &mut Level3d| {
            let (ticket, bake) = level.take_field_request().expect("запрос на запекание");
            let (mut field, stats) = bake.run().expect("поле");
            field.set_texture(1);
            field.set_material_texture(2);
            assert!(level.install_field(ticket, FieldRef(Arc::new(field)), stats, 1.0));
        };
        let mut level = Level3d::default();
        let before = scene_with(0.5, 3.0);
        level.frame(&before, &library, LevelRender::Field);
        bake_and_install(&mut level);
        let ticket = level.baked_ticket();
        // Стена и поле.
        assert_eq!(
            level
                .frame(&before, &library, LevelRender::Field)
                .frame
                .shapes
                .len(),
            2
        );
        // Ящик сдвинулся: из поля — в этот же кадр (кусок вокруг старого места), на новом
        // месте он форма.
        let after = scene_with(2.0, 3.0);
        let made = level.frame(&after, &library, LevelRender::Field);
        assert_eq!(made.frame.shapes.len(), 3);
        drop(made);
        let patches = level.take_field_patches();
        assert_eq!(patches.len(), 1);
        assert!(patches.iter().all(|p| Some(p.ticket) == ticket));
        let field = level.baked_field().unwrap();
        let (corner, cell) = (field.corner(), field.cell());
        let at = |x: f32, y: f32| Vec2::new(x, y) * LEVEL_SCALE;
        // Где стоял ящик — теперь пусто над полом.
        let (d, _) = field.distance2(at(0.65, 0.35), 0);
        assert!(d > 0.3, "{d}");
        // Первый кадр на новом месте уже был: ещё FIELD_STABLE_FRAMES − 2 кадра — форма.
        for _ in 2..FIELD_STABLE_FRAMES {
            assert_eq!(
                level
                    .frame(&after, &library, LevelRender::Field)
                    .frame
                    .shapes
                    .len(),
                3
            );
        }
        assert!(level.take_field_patches().is_empty());
        let made = level.frame(&after, &library, LevelRender::Field);
        assert_eq!(made.frame.shapes.len(), 2, "ящик вернулся в поле");
        let patched_materials = made.materials.clone();
        drop(made);
        assert_eq!(level.take_field_patches().len(), 1);
        assert!(level.take_field_request().is_none());
        assert_eq!(
            level.baked_ticket(),
            ticket,
            "то же поле, без полного запекания"
        );
        // Полное запекание нового набора на той же сетке.
        let mut fresh = Level3d::default();
        let first = fresh.frame(&after, &library, LevelRender::Field);
        let fresh_materials = first.materials.clone();
        drop(first);
        let (_, bake) = fresh.take_field_request().unwrap();
        let (whole, _) = bake.run().unwrap();
        let field = level.baked_field().unwrap();
        assert_eq!((whole.corner(), whole.cell()), (corner, cell));
        assert!(
            whole.bytes() == field.bytes(),
            "значения как у полного запекания"
        );
        let color = |list: &[Material3], id: Option<u32>| id.map(|i| list[i as usize].color);
        // Пол, ящик на новом месте, бочка, пол.
        for (x, y) in [(0.1, 0.1), (2.15, 0.35), (3.1, 0.4), (3.9, 0.1)] {
            let p = at(x, y);
            assert_eq!(
                color(&patched_materials, field.material_at(p)),
                color(&fresh_materials, whole.material_at(p)),
                "цвет в ({x}, {y})"
            );
        }
        // Бочка уехала за край сетки: из поля — сразу, на новом месте — форма, а поле
        // печётся целиком в фоне (пока — старое).
        let far = scene_with(2.0, 9.0);
        for _ in 0..FIELD_STABLE_FRAMES {
            level.frame(&far, &library, LevelRender::Field);
        }
        assert_eq!(level.take_field_patches().len(), 1);
        assert!(level.take_field_request().is_some());
        assert!(matches!(
            level.field_status(),
            LevelFieldStatus::Ready { rebaking: true, .. }
        ));
        assert_eq!(
            level
                .frame(&far, &library, LevelRender::Field)
                .frame
                .shapes
                .len(),
            3
        );
    }

    /// Прямоугольник обводки шире самой линии, а у заливки — её путь.
    #[test]
    fn bounds_include_stroke_width() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((1.0, 0.0));
        let stroke = reyvec_vector::Shape {
            path: path.clone(),
            style: reyvec_vector::Style::Stroke(reyvec_vector::Stroke::new(0.2)),
            paint: Paint::Solid(Color::linear(1.0, 1.0, 1.0, 1.0)),
        };
        let rect = bounds_of(&stroke).unwrap();
        // Полоса обводки: толщина по высоте, по длине — плюс круглые концы (по половине
        // толщины с каждой стороны, как у `kurbo::Stroke::new`).
        assert!((rect.height() - 0.2).abs() < 1e-3, "{rect:?}");
        assert!((rect.width() - 1.2).abs() < 1e-3, "{rect:?}");
        // Заливка линии без площади не рисуется.
        let fill = reyvec_vector::Shape {
            path,
            style: reyvec_vector::Style::Fill(reyvec_vector::FillRule::NonZero),
            ..stroke
        };
        assert!(bounds_of(&fill).is_none());
    }

    #[test]
    fn rectangles_need_no_field() {
        let rect = kurbo::Rect::new(-0.1, -0.2, 0.3, 0.4);
        let path = kurbo::Shape::to_path(&rect, 0.1);
        let Some(Flat::Rect { center, size }) = axis_rect(&path) else {
            panic!("не прямоугольник");
        };
        assert!((center[0] - 0.1).abs() < 1e-6 && (center[1] - 0.1).abs() < 1e-6);
        assert!((size[0] - 0.4).abs() < 1e-6 && (size[1] - 0.6).abs() < 1e-6);
        let circle = kurbo::Shape::to_path(&kurbo::Circle::new((0.0, 0.0), 1.0), 0.1);
        assert!(axis_rect(&circle).is_none());
    }

    /// Длинный брусок режется на куски, а расстояние снаружи до них то же, что до целого.
    #[test]
    fn long_solids_split_without_changing_distance() {
        use reyvec_scene::game::{Aabb, Collider};
        let floor = Collider {
            aabb: Aabb {
                min: [-2.1, -0.98],
                max: [11.3, -0.78],
            },
            oneway: false,
            id: 0,
            moved: [0.0; 2],
            slope: 0.0,
        };
        let pieces = solid_shapes(&floor);
        assert_eq!(pieces.len(), 27, "53,6 м по 2 м");
        for piece in &pieces {
            let (_, r) = piece.bounds().unwrap();
            assert!(r < 1.4, "шар куска {r}");
        }
        let whole = Shape3::Box {
            size: [53.6, 0.8, SOLID_FRONT - SOLID_BACK],
            round: 0.0,
        }
        .at([
            4.6 * LEVEL_SCALE,
            -0.88 * LEVEL_SCALE,
            (SOLID_FRONT + SOLID_BACK) * 0.5,
        ]);
        for k in 0..300 {
            let t = k as f32 * 0.37;
            let p = Vec3::new(
                -10.0 + (t * 1.7) % 70.0,
                -6.0 + (t * 2.3) % 10.0,
                -2.0 + (t * 1.1) % 5.0,
            );
            let near = pieces
                .iter()
                .map(|s| s.distance(p))
                .fold(f32::INFINITY, f32::min);
            let d = whole.distance(p);
            if d > 0.0 {
                assert!((near - d).abs() < 1e-4, "{p}: {near} и {d}");
            } else {
                assert!(
                    near <= 1e-4,
                    "{p}: внутри целого, но снаружи кусков ({near})"
                );
            }
        }
        // Короткий брусок остаётся одним.
        let short = Collider {
            aabb: Aabb {
                min: [0.0, 0.0],
                max: [0.4, 0.3],
            },
            ..floor
        };
        assert_eq!(solid_shapes(&short).len(), 1);
    }

    #[test]
    fn camera_shows_same_height_as_flat_view() {
        let cam = camera([2.0, 0.5], 1.0, [0.0, 0.0]);
        let aspect = 16.0 / 9.0;
        // Точки на высоте ±1 от середины плоского вида — у краёв кадра по вертикали.
        for (dy, want) in [(1.0f32, 1.0f32), (-1.0, -1.0)] {
            let p = Vec3::new(2.0 * LEVEL_SCALE, (0.5 + dy) * LEVEL_SCALE, SOLID_FRONT);
            let screen = cam.project(p, aspect).unwrap();
            assert!((screen.y - want).abs() < 0.2, "{dy}: {screen}");
        }
    }

    #[test]
    fn orbit_turns_around_view_center() {
        let center = Vec3::new(2.0 * LEVEL_SCALE, 0.5 * LEVEL_SCALE, SOLID_FRONT);
        let home = camera([2.0, 0.5], 1.0, [0.0, 0.0]);
        // Облёт вправо на 60°: камера правее середины, на том же расстоянии, и середина
        // вида всё так же посередине кадра.
        let turned = camera([2.0, 0.5], 1.0, [60.0, 0.0]);
        assert!(turned.eye.x > center.x + 1.0, "{}", turned.eye);
        let d = |c: &Camera3D| (c.eye - center).length();
        assert!((d(&home) - d(&turned)).abs() < 1e-3);
        let screen = turned.project(center, 16.0 / 9.0).unwrap();
        assert!(screen.truncate().length() < 1e-3, "{screen}");
    }

    /// Длинный шаг в пустоте (7.18д4) в комнате из примера с готовым полем уровня:
    /// начальная оценка клетки никогда не больше настоящего расстояния до сцены (поле,
    /// плоскости и формы вместе), вблизи форм ответ точный, и шаг начинается дальше
    /// клетки во многих местах, но не ближе, чем без свободного расстояния.
    #[test]
    fn room_free_step_never_overestimates() {
        use reyvec_sdf::grid3d::Grid;
        let file = reyvec_scene::SceneFile::parse_in(
            reyvec_scene::EXAMPLE,
            Some(std::path::Path::new(reyvec_scene::EXAMPLE_DIR)),
        )
        .unwrap();
        let mut library = ShapeLibrary::new(0.001);
        let scene = file
            .build(&mut library, &mut reyvec_scene::ShapeCache::new())
            .unwrap();
        let mut level = Level3d::default();
        level.frame(&scene, &library, LevelRender::Field);
        let (ticket, bake) = level.take_field_request().expect("запрос на запекание");
        let (mut field, stats) = bake.run().expect("поле");
        field.set_texture(1);
        field.set_material_texture(2);
        assert!(level.install_field(ticket, FieldRef(Arc::new(field)), stats, 1.0));
        let made = level.frame(&scene, &library, LevelRender::Field);
        let objects = crate::space::programs(&made.frame);
        let grid = Grid::build(&objects);
        assert!(grid.field.is_some(), "поле уровня в сцене");
        let mut old = grid.clone();
        old.clear_free();
        let longer = (0..grid.cells.len())
            .filter(|&c| grid.start(c) > 3.0 * grid.margin)
            .count();
        assert!(
            longer * 10 > grid.cells.len(),
            "дальше трёх клеток шаг начинается в {longer} из {}",
            grid.cells.len()
        );
        let mut seed = 4242u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        let size = grid.cell * Vec3::from(grid.dims.map(|d| d as f32));
        for _ in 0..4000 {
            let p = grid.origin + Vec3::new(rnd(), rnd(), rnd()) * size;
            let truth = objects
                .iter()
                .map(|o| o.distance(p).0)
                .fold(f32::INFINITY, f32::min);
            let (d, _) = grid.near(&objects, p, 1e9);
            let (d0, _) = old.near(&objects, p, 1e9);
            assert!(d <= truth + 1e-4, "{p}: сетка {d}, правда {truth}");
            assert!(d >= d0, "{p}: {d} < {d0}");
            if truth < grid.margin.min(0.2) {
                assert!((d - truth).abs() < 1e-4, "{p}: сетка {d}, правда {truth}");
            }
        }
    }
}
