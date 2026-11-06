//! Запечённое поле расстояний вокруг камеры (этап 7.10,
//! `docs/decisions/0019-baked-distance-field.md`).
//!
//! Расстояние до форм сцены (без плоскостей: их ищут формулой, 7.6и), посчитанное
//! заранее в точках решётки. Несколько слоёв вокруг камеры, каждый следующий с вдвое
//! большим шагом (clipmap, как Global Distance Field в Unreal). Лучи теней и пробы
//! затенения берут из поля нижнюю оценку расстояния одним чтением вместо расчёта форм.
//!
//! **Почему оценка надёжна.** Расстояние до форм меняется не быстрее, чем на метр за
//! метр (на этом держится весь реймарчинг). Значит, в точке `p` внутри кубика решётки
//! настоящее расстояние не меньше, чем значение в углу минус расстояние до угла.
//! Трилинейная выборка — среднее углов с весами, и сумма весов на расстояния до углов
//! не больше половины диагонали кубика (0,866 шага). Поэтому
//! `правда ≥ выборка − 0,87 · шаг − округление` ([`Field::lower_bound`]).
//!
//! Поле хранит расстояния только в полосе у поверхностей ([`band`]): дальше записана
//! сама полоса («не ближе этого»), а внутри форм — минус полоса. Так сдвиг предмета
//! меняет поле только в полосе вокруг его старого и нового места, и пересчитать надо
//! немного кубиков ([`Field::update`]). Значения хранятся в half float (16 бит);
//! ошибка его округления не больше полосы · 2⁻¹¹, в оценке заложено вдвое больше.
//!
//! Слой адресуется кольцом: точка с мировым номером `w` лежит в ячейке
//! `w mod size` по каждой оси. Камера улетела на кубик — пересчитываются только
//! новые кубики у края, остальные остаются на своих ячейках.
//!
//! Это модель на процессоре; видеокарта (7.10б) делает то же самое, тесты сверяют.

use glam::{IVec3, Vec3};

use crate::shape3d::plane_objects;

/// Точек по стороне слоя.
pub const FIELD_SIZE: i32 = 128;
/// Слоёв.
pub const FIELD_LEVELS: usize = 4;
/// Шаг решётки первого (самого мелкого) слоя, м.
pub const FIELD_STEP: f32 = 1.0 / 32.0;
/// Кубик пересчёта: столько точек по стороне. Слои двигаются за камерой на целые кубики.
pub const BRICK: i32 = 8;
/// Доля шага, на которую трилинейная выборка может быть больше правды (√3 / 2 с запасом).
pub const INTERP_MARGIN: f32 = 0.87;

/// Шаг решётки слоя `level`, м.
pub fn level_step(level: usize) -> f32 {
    FIELD_STEP * (1u32 << level) as f32
}

/// Полоса слоя `level`: дальше этого от форм поле хранит саму полосу. Не меньше 0,5 м,
/// чтобы дальние пробы затенения в углах (до 40 см) брали из поля настоящее расстояние.
pub fn band(level: usize) -> f32 {
    (8.0 * level_step(level)).max(0.5)
}

/// Ошибка оценки слоя `level` из-за округления до half float (с двойным запасом).
fn rounding_margin(level: usize) -> f32 {
    band(level) * (1.0 / 1024.0)
}

/// Округление до half float (10 бит дробной части) к ближайшему — как запись в R16F на
/// видеокарте. Значения поля по модулю не больше полосы, поэтому переполнения нет;
/// у самого нуля half float точнее, чем нужно, и такие числа не трогаем.
pub fn round_half(x: f32) -> f32 {
    if x.abs() < 6.1e-5 {
        return x;
    }
    let bits = x.to_bits();
    let rounded = (bits + 0x1000) & !0x1fff;
    f32::from_bits(rounded)
}

/// Кубик, который надо посчитать: слой и мировой номер его первой точки.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrickJob {
    pub level: usize,
    pub first: IVec3,
}

impl BrickJob {
    /// Середина кубика и радиус шара вокруг него, в котором поле может зависеть от
    /// предмета: полоса слоя плюс половина диагонали кубика.
    pub fn reach(&self) -> (Vec3, f32) {
        let step = level_step(self.level);
        let middle = (self.first.as_vec3() + BRICK as f32 * 0.5) * step;
        (
            middle,
            band(self.level) + INTERP_MARGIN * step * BRICK as f32,
        )
    }
}

/// Где стоят слои и посчитано ли поле: всё, что нужно, чтобы решить, какие кубики
/// пересчитать. Сами числа держит [`Field`] (процессор) или буфер видеокарты.
#[derive(Clone, Debug)]
pub struct FieldWindow {
    pub size: i32,
    /// Мировой номер первой точки каждого слоя (кратен [`BRICK`]); точка `w` слоя
    /// стоит в `w · шаг`.
    pub corners: [IVec3; FIELD_LEVELS],
    built: bool,
}

impl FieldWindow {
    /// Слои по `size³` точек (`size` кратен [`BRICK`]). В игре — [`FIELD_SIZE`], тесты
    /// берут меньше.
    pub fn new(size: i32) -> Self {
        assert!(
            size > 0 && size % BRICK == 0,
            "size {size} не кратен {BRICK}"
        );
        FieldWindow {
            size,
            corners: [IVec3::ZERO; FIELD_LEVELS],
            built: false,
        }
    }

    /// Забыть посчитанное: следующий [`FieldWindow::plan`] пересчитает всё.
    pub fn reset(&mut self) {
        self.built = false;
    }

    /// Угол слоя `level` для камеры в `camera`: камера посередине, с точностью до кубика.
    pub fn corner_for(&self, camera: Vec3, level: usize) -> IVec3 {
        let brick = level_step(level) * BRICK as f32;
        let middle = (camera / brick).round().as_ivec3();
        (middle - IVec3::splat(self.size / BRICK / 2)) * BRICK
    }

    /// Номер ячейки точки `w` в кольце слоя.
    pub fn slot(&self, w: IVec3) -> usize {
        let s = self.size;
        let q = IVec3::new(w.x.rem_euclid(s), w.y.rem_euclid(s), w.z.rem_euclid(s));
        ((q.z * s + q.y) * s + q.x) as usize
    }

    /// Поставить слои вокруг `camera` и вернуть кубики, которые надо пересчитать:
    /// вошедшие в слой и лежащие в полосе вокруг шаров `changed` (старое и новое место
    /// сдвинутых предметов). `full` — все (сцена сменилась целиком).
    pub fn plan(&mut self, camera: Vec3, changed: &[(Vec3, f32)], full: bool) -> Vec<BrickJob> {
        let full = full || !self.built;
        self.built = true;
        let mut jobs = Vec::new();
        let bricks = self.size / BRICK;
        for level in 0..FIELD_LEVELS {
            let old = self.corners[level];
            let new = self.corner_for(camera, level);
            self.corners[level] = new;
            for bz in 0..bricks {
                for by in 0..bricks {
                    for bx in 0..bricks {
                        let job = BrickJob {
                            level,
                            first: new + IVec3::new(bx, by, bz) * BRICK,
                        };
                        let entered = (job.first - old).min_element() < 0
                            || (job.first - old).max_element() >= self.size;
                        let (middle, reach) = job.reach();
                        let near_change = changed
                            .iter()
                            .any(|&(c, r)| (middle - c).length() - r < reach);
                        if full || entered || near_change {
                            jobs.push(job);
                        }
                    }
                }
            }
        }
        jobs
    }
}

/// Предметы, от которых зависит поле в кубике `job`: бесконечные (кроме плоскостей)
/// и те, чей шар-граница ближе [`BrickJob::reach`]. Остальные дальше полосы от каждой
/// точки кубика и на записанное число не влияют.
pub fn brick_objects(objects: &[crate::Program], job: &BrickJob) -> Vec<u32> {
    let (middle, reach) = job.reach();
    let planes = plane_objects(objects);
    (0..objects.len() as u32)
        .filter(|&i| match objects[i as usize].bounds {
            Some((c, r)) => (c - middle).length() - r < reach,
            None => !planes.contains(&i),
        })
        .collect()
}

/// Поле на процессоре: окно слоёв и сами числа.
#[derive(Clone, Debug)]
pub struct Field {
    pub window: FieldWindow,
    /// Числа слоёв: `size³` на слой, ячейка точки — [`FieldWindow::slot`].
    pub values: Vec<Vec<f32>>,
}

/// Сколько кубиков пересчитал [`Field::update`] (для тестов).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FieldUpdate {
    pub bricks: usize,
}

impl Field {
    /// Пустое поле `size³` на слой (`size` кратен [`BRICK`]).
    pub fn new(size: i32) -> Self {
        let window = FieldWindow::new(size);
        let n = (size * size * size) as usize;
        Field {
            window,
            values: vec![vec![0.0; n]; FIELD_LEVELS],
        }
    }

    /// Привести поле к кадру ([`FieldWindow::plan`]) и посчитать кубики.
    /// `distance(p, near)` — расстояние до форм без плоскостей; дальше `near` от формы
    /// можно отвечать оценкой снизу (по шару-границе).
    pub fn update(
        &mut self,
        camera: Vec3,
        changed: &[(Vec3, f32)],
        full: bool,
        distance: &impl Fn(Vec3, f32) -> f32,
    ) -> FieldUpdate {
        let jobs = self.window.plan(camera, changed, full);
        for job in &jobs {
            self.bake_brick(job, distance);
        }
        FieldUpdate { bricks: jobs.len() }
    }

    fn bake_brick(&mut self, job: &BrickJob, distance: &impl Fn(Vec3, f32) -> f32) {
        let step = level_step(job.level);
        let band = band(job.level);
        for z in 0..BRICK {
            for y in 0..BRICK {
                for x in 0..BRICK {
                    let w = job.first + IVec3::new(x, y, z);
                    let d = distance(w.as_vec3() * step, band).clamp(-band, band);
                    let slot = self.window.slot(w);
                    self.values[job.level][slot] = round_half(d);
                }
            }
        }
    }

    /// Трилинейная выборка самого мелкого слоя, где лежит весь кубик решётки вокруг `p`:
    /// значение и номер слоя. `None` — точка вне всех слоёв.
    pub fn sample(&self, p: Vec3) -> Option<(f32, usize)> {
        let size = self.window.size;
        for (level, values) in self.values.iter().enumerate() {
            let corner = self.window.corners[level];
            let g = p / level_step(level);
            let base = g.floor();
            let b = base.as_ivec3();
            if (b - corner).min_element() < 0 || (b - corner).max_element() >= size - 1 {
                continue;
            }
            let f = g - base;
            let mut v = 0.0;
            for k in 0..8 {
                let o = IVec3::new(k & 1, (k >> 1) & 1, (k >> 2) & 1);
                let w = Vec3::select(o.cmpeq(IVec3::ONE), f, Vec3::ONE - f);
                v += w.x * w.y * w.z * values[self.window.slot(b + o)];
            }
            return Some((v, level));
        }
        None
    }

    /// Расстояние до форм в `p` не меньше этого (`None` — точка вне поля). Дальше
    /// полосы слоя оценка не растёт: там поле знает только «не ближе полосы».
    pub fn lower_bound(&self, p: Vec3) -> Option<f32> {
        self.sample(p)
            .map(|(v, level)| v - INTERP_MARGIN * level_step(level) - rounding_margin(level))
    }
}

/// Расстояние до форм без плоскостей для сборки поля: все предметы, у каждого сначала
/// шар-граница; дальше `near` от шара ответ — расстояние до шара (оценка снизу). Сетка
/// отсева (7.6е) здесь не годится: её оценка «не ближе клетки» зависит от размера
/// клетки, а он меняется, когда предметы двигаются, — и поле вдали от них менялось бы.
///
/// Плоскости — ровно те, что шейдер находит формулой ([`plane_objects`]): остальные
/// формы-плоскости (их больше [`crate::shape3d::MAX_PLANES`] или это заменитель модели)
/// шейдер считает как обычные предметы, и поле тоже.
pub fn solid_distance(objects: &[crate::Program], p: Vec3, near: f32) -> f32 {
    let planes = plane_objects(objects);
    let mut best = f32::INFINITY;
    for (i, o) in objects.iter().enumerate() {
        if planes.contains(&(i as u32)) {
            continue;
        }
        if let Some((c, r)) = o.bounds {
            let outside = (p - c).length() - r;
            if outside >= best {
                continue;
            }
            if outside > near {
                best = outside;
                continue;
            }
        }
        best = best.min(o.distance(p).0);
    }
    best
}

/// Шары-границы предметов, которые изменились между кадрами `before` и `after` (старое
/// и новое место): поле вокруг них пересчитывается. `None` — изменилось то, что не
/// описать шарами (другое число предметов, бесконечная форма): пересчитать всё.
pub fn changed_bounds(
    before: &[crate::Program],
    after: &[crate::Program],
) -> Option<Vec<(Vec3, f32)>> {
    if before.len() != after.len() {
        return None;
    }
    let (planes_a, planes_b) = (plane_objects(before), plane_objects(after));
    let mut out = Vec::new();
    for (i, (a, b)) in before.iter().zip(after).enumerate() {
        if a.words == b.words && a.bounds == b.bounds {
            continue;
        }
        let i = i as u32;
        if planes_a.contains(&i) && planes_b.contains(&i) {
            continue; // плоскостей в поле нет
        }
        out.push(a.bounds?);
        out.push(b.bounds?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Program;
    use crate::shape3d::compile;

    fn programs(text: &str, time: f32) -> Vec<Program> {
        let scene = crate::SpaceScene::parse(text).unwrap();
        scene
            .frame(time)
            .shapes
            .iter()
            .map(|s| compile(s).unwrap())
            .collect()
    }

    fn solid(objects: &[Program]) -> Vec<&Program> {
        objects.iter().filter(|o| o.plane().is_none()).collect()
    }

    fn exact(objects: &[&Program], p: Vec3) -> f32 {
        objects
            .iter()
            .map(|o| o.distance(p).0)
            .fold(f32::INFINITY, f32::min)
    }

    fn build(objects: &[Program], camera: Vec3, size: i32) -> Field {
        let mut field = Field::new(size);
        field.update(camera, &[], true, &|p, near| {
            solid_distance(objects, p, near)
        });
        field
    }

    fn rnd(seed: &mut u32) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        *seed as f32 / u32::MAX as f32
    }

    #[test]
    fn half_rounding_matches_r16f_precision() {
        for x in [0.3f32, -0.3, 0.123456, 0.5, -0.49999, 1e-3, 2.0] {
            let r = round_half(x);
            assert!((r - x).abs() <= x.abs() * (1.0 / 2048.0), "{x} -> {r}");
            assert_eq!(r.to_bits() & 0x1fff, 0, "{x}: лишние биты");
        }
    }

    #[test]
    fn lower_bound_never_exceeds_truth() {
        let objects = programs(crate::space_file::EXAMPLE, 2.0);
        let solids = solid(&objects);
        let camera = Vec3::new(3.0, 1.2, 2.0);
        let field = build(&objects, camera, 32);
        let mut seed = 777u32;
        let (mut inside, mut tight) = (0, 0);
        for _ in 0..30000 {
            // Точки вокруг камеры на всех слоях: до 2 м — мелкие, дальше — грубые.
            let reach = [0.5, 1.0, 2.0, 4.0][(rnd(&mut seed) * 4.0) as usize % 4];
            let p = camera
                + (Vec3::new(rnd(&mut seed), rnd(&mut seed), rnd(&mut seed)) * 2.0 - 1.0) * reach;
            let Some(lb) = field.lower_bound(p) else {
                continue;
            };
            inside += 1;
            let truth = exact(&solids, p);
            assert!(lb <= truth + 1e-5, "{p}: поле {lb}, правда {truth}");
            let (_, level) = field.sample(p).unwrap();
            // И оценка полезная: не сильно меньше правды (в пределах полосы).
            if lb >= truth.min(band(level)) - 2.0 * level_step(level) {
                tight += 1;
            }
        }
        assert!(inside > 20000, "в поле только {inside} точек");
        assert_eq!(tight, inside, "оценка слишком грубая");
    }

    #[test]
    fn far_from_camera_there_is_no_field() {
        let objects = programs(crate::space_file::EXAMPLE, 2.0);
        let field = build(&objects, Vec3::ZERO, 16);
        // Самый грубый слой: 16 точек по 25 см — 4 м вокруг камеры.
        assert!(field.lower_bound(Vec3::new(0.3, 0.2, 0.1)).is_some());
        assert!(field.lower_bound(Vec3::new(30.0, 0.0, 0.0)).is_none());
    }

    #[test]
    fn moving_objects_update_only_nearby_bricks() {
        let text = crate::space_file::EXAMPLE;
        let camera = Vec3::new(3.0, 1.2, 2.0);
        let before = programs(text, 2.0);
        let after = programs(text, 2.0 + 1.0 / 60.0);
        let changed = changed_bounds(&before, &after).unwrap();
        assert!(!changed.is_empty(), "в примере сцены что-то движется");
        let mut field = build(&before, camera, 32);
        let done = field.update(camera, &changed, false, &|p, near| {
            solid_distance(&after, p, near)
        });
        let all = FIELD_LEVELS * (32 / BRICK).pow(3) as usize;
        assert!(
            done.bricks > 0 && done.bricks < all,
            "пересчитано {} из {all}",
            done.bricks
        );
        // Ровно как собранное заново.
        let fresh = build(&after, camera, 32);
        assert_eq!(field.values, fresh.values);
    }

    #[test]
    fn camera_flight_rebuilds_only_entered_bricks() {
        let objects = programs(crate::space_file::EXAMPLE, 2.0);
        let distance = |p: Vec3, near: f32| solid_distance(&objects, p, near);
        let mut field = build(&objects, Vec3::new(3.0, 1.2, 2.0), 32);
        // На кубик мелкого слоя (25 см) вдоль X: у мелкого слоя входит один слой
        // кубиков, грубые слои могут не сдвинуться вовсе.
        let camera = Vec3::new(3.25, 1.2, 2.0);
        let done = field.update(camera, &[], false, &distance);
        let side = (32 / BRICK).pow(2) as usize;
        assert!(
            done.bricks >= side && done.bricks <= FIELD_LEVELS * side,
            "{done:?}"
        );
        let fresh = build(&objects, camera, 32);
        assert_eq!(field.window.corners, fresh.window.corners);
        for (level, (a, b)) in field.values.iter().zip(&fresh.values).enumerate() {
            // Ячейки кольца у тех же мировых точек — те же числа.
            for z in 0..32 {
                for y in 0..32 {
                    for x in 0..32 {
                        let w = field.window.corners[level] + IVec3::new(x, y, z);
                        let s = field.window.slot(w);
                        assert_eq!(a[s], b[s], "слой {level}, точка {w}");
                    }
                }
            }
        }
    }

    #[test]
    fn brick_lists_hold_every_object_that_matters() {
        let objects = programs(crate::space_file::EXAMPLE, 2.0);
        let mut window = FieldWindow::new(32);
        let jobs = window.plan(Vec3::new(3.0, 1.2, 2.0), &[], true);
        let mut short = 0;
        for job in &jobs {
            let list = brick_objects(&objects, job);
            let listed: Vec<Program> = list.iter().map(|&i| objects[i as usize].clone()).collect();
            if list.len() < objects.len() {
                short += 1;
            }
            // Углы кубика: по списку — то же число, что по всем предметам.
            let step = level_step(job.level);
            let band = band(job.level);
            for k in [0, 7, 63, 511] {
                let o = IVec3::new(k & 7, (k >> 3) & 7, k >> 6);
                let p = (job.first + o).as_vec3() * step;
                let all = solid_distance(&objects, p, band).clamp(-band, band);
                let some = solid_distance(&listed, p, band).clamp(-band, band);
                assert_eq!(all, some, "{job:?} точка {p}");
            }
        }
        assert!(
            short > jobs.len() / 2,
            "списки почти всегда короче всех предметов"
        );
    }

    #[test]
    fn planes_and_unchanged_objects_are_not_changes() {
        let a = programs(crate::space_file::EXAMPLE, 2.0);
        assert_eq!(changed_bounds(&a, &a), Some(vec![]));
        assert_eq!(changed_bounds(&a, &a[1..]), None);
    }
}
