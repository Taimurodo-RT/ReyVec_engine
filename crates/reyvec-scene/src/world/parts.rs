//! Части мира сцены: компоненты предметов, ресурсы шага и постоянные.

use super::*;

/// Место сущности относительно родителя.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub at: [f64; 2],
    /// Глубина: складывается с родительской, больше — ближе к зрителю (рисуется поверх).
    pub depth: f64,
    /// Поворот, градусы против часовой стрелки.
    pub angle: f64,
    pub scale: f64,
    /// Отбрасывает тень; `None` — как у родителя.
    pub shadow: Option<bool>,
    pub opacity: f32,
    /// Отражено слева направо (персонаж смотрит влево, 6.1).
    pub flip: bool,
    /// Вспышка от попадания (6.5): от 0 до 1, фигуры сущности и её детей белеют.
    pub flash: f32,
}

impl Placement {
    /// Без сдвига, поворота и изменений.
    pub const IDENTITY: Self = Self {
        at: [0.0, 0.0],
        depth: 0.0,
        angle: 0.0,
        scale: 1.0,
        shadow: None,
        opacity: 1.0,
        flip: false,
        flash: 0.0,
    };
}

/// Итоговое место сущности в сцене (считает система `propagate`).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct GlobalPlacement {
    pub affine: Affine,
    pub depth: f64,
    /// Итоговый размер (произведение размеров предков).
    pub scale: f64,
    pub shadow: bool,
    pub opacity: f32,
    /// Группа слоя: ближайший предок (или сама сущность), у которого тень или прозрачность
    /// заданы явно. Фигуры разных групп не сливаются в один слой: два небольших слоя
    /// дешевле одного на всю сцену (узкое место света 3).
    pub group: u64,
    /// Вспышка: наибольшая у сущности и её предков.
    pub flash: f32,
    /// Сущность или её предок двигается или меняется по ходу игры (метка [`Dynamic`]).
    pub dynamic: bool,
}

/// Метка: сущность (с детьми) двигается или меняется по ходу игры — герой, тела физики,
/// скрипты, движения, мишени, обломки. В снимке её фигуры стоят между
/// `SceneItem::BeginDynamic` и `EndDynamic`: объёмный вид не запекает их в поле уровня.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Dynamic;

impl GlobalPlacement {
    pub(super) const ROOT: Self = Self {
        affine: Affine::IDENTITY,
        depth: 0.0,
        scale: 1.0,
        shadow: false,
        opacity: 1.0,
        group: 0,
        flash: 0.0,
        dynamic: false,
    };

    pub(super) fn then(&self, p: &Placement, me: Entity, dynamic: bool) -> Self {
        let own = p.shadow.is_some() || p.opacity < 1.0;
        Self {
            affine: self.affine
                * Affine::translate((p.at[0], p.at[1]))
                * Affine::rotate(p.angle.to_radians())
                * Affine::scale_non_uniform(if p.flip { -p.scale } else { p.scale }, p.scale),
            depth: self.depth + p.depth,
            scale: self.scale * p.scale,
            shadow: p.shadow.unwrap_or(self.shadow),
            opacity: self.opacity * p.opacity,
            group: if own { me.to_bits() } else { self.group },
            flash: self.flash.max(p.flash),
            dynamic: self.dynamic || dynamic,
        }
    }

    pub(super) fn point(&self, p: [f32; 2]) -> [f32; 2] {
        let q = self.affine * reyvec_vector::Point::new(f64::from(p[0]), f64::from(p[1]));
        [q.x as f32, q.y as f32]
    }
}

/// Векторная фигура: форма из библиотеки и цвет. `order` — порядок в файле. У
/// перетекающей фигуры Motif (5.5) ещё и то, как она перетекает.
#[derive(Component, Clone, Debug)]
pub struct VectorShape {
    pub id: ShapeId,
    pub tint: Color,
    pub order: u32,
    pub morph: Option<MorphTrack>,
}

/// Рисунок SVG (5.5): готовый список кадра в координатах рисунка.
#[derive(Component, Clone, Debug)]
pub struct SvgPicture {
    pub drawing: Arc<SvgDrawing>,
    pub order: u32,
}

/// Источник света в координатах своей сущности.
#[derive(Component, Clone, Copy, Debug)]
pub struct LightSource {
    pub light: SceneLight,
    pub order: u32,
}

/// SDF-объект в координатах своей сущности.
#[derive(Component, Clone, Debug)]
pub struct SdfShape {
    pub object: SceneObject,
    pub order: u32,
}

/// Движение предмета (5.3): от исходного места `base` в момент времени сцены.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub base: Placement,
    /// Градусов в секунду.
    pub spin: f64,
    /// Размах (градусы) и период (секунды) качания.
    pub swing: Option<(f64, f64)>,
    /// Сдвиг и период покачивания.
    pub bob: Option<([f64; 2], f64)>,
}

impl Motion {
    /// Место в момент `t` секунд.
    pub fn at(&self, t: f64) -> Placement {
        let wave = |period: f64| (std::f64::consts::TAU * t / period).sin();
        let mut p = self.base;
        p.angle += self.spin * t;
        if let Some((angle, period)) = self.swing {
            p.angle += angle * wave(period);
        }
        if let Some((by, period)) = self.bob {
            let k = wave(period);
            p.at[0] += by[0] * k;
            p.at[1] += by[1] * k;
        }
        p
    }
}

/// Твёрдое (6.1) с серединой в месте сущности. Без `form` — прямоугольник с
/// половинами сторон `half` (склон, если `slope`); с `form` — настоящая форма фигуры
/// (9.3), которая поворачивается и растягивается вместе с сущностью.
#[derive(Component, Clone, Debug)]
pub struct Solid {
    pub half: [f64; 2],
    /// Платформа: держит только сверху (6.2).
    pub oneway: bool,
    /// Середина и поворот на прошлом шаге: по ним видно, что предмет едет (6.2).
    pub last: Option<([f64; 2], f64)>,
    /// Склон (6.4): подъём слева направо, 0 — не склон ([`Collider::slope`]).
    pub slope: f64,
    /// Форма фигуры (9.3).
    pub form: Option<std::sync::Arc<reyvec_game::Form>>,
    /// Собранная форма и растяжение, при котором она собрана.
    pub built: Option<([f64; 2], reyvec_game::SharedShape)>,
}

impl Solid {
    /// Прямоугольник (или склон) без своей формы.
    pub fn rect(half: [f64; 2], oneway: bool, slope: f64) -> Self {
        Self {
            half,
            oneway,
            last: None,
            slope,
            form: None,
            built: None,
        }
    }
}

/// Персонаж игрока (6.1): настройки, состояние и место появления; удар (6.5).
/// Команда консоли разработчика для мира сцены (6.11).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DebugCommand {
    /// Поставить героя в точку сцены.
    Teleport([f64; 2]),
    /// Задать здоровье (от 1 до наибольшего).
    Health(u32),
    /// Включить, выключить (`None` — переключить) бессмертие.
    God(Option<bool>),
    /// Вернуть героя в начало с полным здоровьем.
    Respawn,
    /// Где герой и с какой скоростью.
    Where,
}

#[derive(Component, Clone, Debug, PartialEq)]
pub struct Player {
    pub tuning: Tuning,
    pub body: Body,
    pub spawn: [f64; 2],
    pub attack: AttackTuning,
    pub fighter: Fighter,
    /// Сколько попаданий по целям.
    pub hits: u32,
    /// Анимация из костей (6.8).
    pub anim: Animator,
    /// Здоровье (6.10б).
    pub health: Health,
    /// Бессмертие из консоли разработчика: враги не ранят.
    pub god: bool,
}

/// Предмет со скриптом (6.9): его таблица `me` в скрипте, прямоугольник касания героя
/// и место из файла. У манекена скрипт задаёт бег и прыжок, у других предметов — место
/// и видимость.
#[derive(Component)]
pub struct Scripted {
    pub(super) actor: reyvec_script::Actor,
    pub(super) touch: [f64; 2],
    /// Прозрачность из файла (скрытый скриптом — 0).
    pub(super) opacity: f32,
    /// Уже подобран: событие «подобрал» один раз, пока скрипт не сбросит `collected`.
    pub(super) collected: bool,
}

/// Все скрипты мира и счётчик шагов до проверки файлов на правку.
#[derive(Resource)]
pub(super) struct ScriptRes {
    pub(super) scripts: reyvec_script::Scripts,
    pub(super) until_check: u32,
}

/// Как часто проверять файлы скриптов на правку: раз в столько шагов логики (0,25 с).
pub(super) const SCRIPT_CHECK_STEPS: u32 = 15;

/// Кость героя (6.8): какая и её место из файла (поза добавляется к нему).
#[derive(Component, Clone, Copy, Debug)]
pub struct BoneOf {
    pub bone: Bone,
    pub base: Placement,
}

/// Манекен (6.5) и прозрачность его предмета из файла (разбитый не виден).
#[derive(Component, Clone, Debug, PartialEq)]
pub struct Target {
    pub dummy: Dummy,
    pub opacity: f32,
    /// Цвет обломков, когда разбит (6.10): цвет самой большой фигуры.
    pub color: Color,
    /// Урон герою от касания (6.10б).
    pub contact: u32,
}

/// Физическое тело предмета (6.10).
#[derive(Component, Clone, Copy, Debug)]
pub struct PhysBody {
    pub id: BodyId,
}

/// Обломок разбитого врага: заранее созданная сущность с квадратом, тело физики и
/// сколько ему ещё лежать.
pub(super) struct Piece {
    pub(super) entity: Entity,
    pub(super) body: Option<BodyId>,
    pub(super) left: f64,
}

/// Разбит враг: где, его размер, цвет и куда били.
pub(super) struct Break {
    pub(super) at: [f64; 2],
    pub(super) size: [f64; 2],
    pub(super) color: Color,
    pub(super) dir: f64,
}

/// Физика сцены (6.10).
#[derive(Resource)]
pub(super) struct PhysicsRes {
    pub(super) physics: Physics,
    /// Тела предметов (без обломков) по порядку: для героя они платформы.
    pub(super) bodies: Vec<BodyId>,
    /// Тела как поверхности для героя и врагов после прошлого шага физики (9.2).
    pub(super) surfaces: Vec<reyvec_game::Surface>,
    /// Кого толкнули герои на этом шаге: номер поверхности тела и желаемая скорость.
    pub(super) pushes: Vec<(u64, f64)>,
    /// Кто на чём стоит на этом шаге (9.4): герои и враги давят весом на тела и мягкие
    /// тела под ногами.
    pub(super) presses: Vec<Press>,
    /// Сколько шагать физике (пишет `play`: время шага без стоп-кадра).
    pub(super) dt: f64,
    /// Длина последнего шага логики, за который сдвинулись поверхности (для скорости
    /// препятствий частиц, 9.5).
    pub(super) step_dt: f64,
    pub(super) breaks: Vec<Break>,
    pub(super) pieces: Vec<Piece>,
    pub(super) seed: u64,
    /// Полигон (6.13): сочленения, мягкие тела, выброс тел, мышь.
    pub(super) extra: phys::Extra,
}

/// Невидимая заранее созданная копия предмета с `spawn`: место для выброшенного тела.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(super) struct SpawnSlot;

/// Места выброшенных тел (не тела предметов, не герой и не фигуры обломков).
pub(super) type SlotPlaces<'w, 's> = Query<
    'w,
    's,
    &'static mut Placement,
    (
        With<SpawnSlot>,
        Without<PhysBody>,
        Without<Player>,
        Without<VectorShape>,
    ),
>;

/// Сколько обломков от одного разбитого врага и сколько всего в запасе.
pub(super) const PIECES_PER_BREAK: usize = 10;
pub(super) const PIECES: usize = 40;
/// Сколько лежат обломки и сколько из этого времени гаснут, секунды.
pub(super) const PIECE_LIFE: f64 = 3.0;
pub(super) const PIECE_FADE: f64 = 0.6;
/// Насколько шире прямоугольник героя для касания врага и как отбрасывает раненого.
pub(super) const CONTACT_REACH: f64 = 0.02;
pub(super) const HURT_PUSH: [f64; 2] = [2.2, 2.0];
/// Номера платформ из физических тел для героя (не пересекаются с номерами сущностей).
pub(super) const PLATFORM_IDS: u64 = 1 << 62;
/// Номера поверхностей мягких тел для героя (9.4): выше номеров тел.
pub(super) const SOFT_IDS: u64 = PLATFORM_IDS + (1 << 56);

/// Персонаж стоит на теле или мягком теле: номер опоры, полоса по низу его
/// прямоугольника и с какой скоростью он на неё упал (0 — уже стоял).
pub(super) struct Press {
    pub(super) ground: u64,
    pub(super) feet: Aabb,
    pub(super) fall: f64,
}

impl Press {
    /// Опора персонажа, если это тело физики или мягкое тело.
    pub(super) fn of(body: &Body, tuning: &reyvec_game::Tuning, fall: f64) -> Option<Self> {
        let ground = body
            .ground()
            .filter(|g| body.on_ground && *g >= PLATFORM_IDS)?;
        let b = body.bounds(tuning);
        Some(Self {
            ground,
            feet: Aabb {
                min: [b.min[0], b.min[1] - 0.005],
                max: [b.max[0], b.min[1] + 0.005],
            },
            fall,
        })
    }
}

/// Масса героя для толкания тел (плотность 1 на площадь героя ростом 0,28).
pub(super) const HERO_MASS: f64 = 0.04;

/// Фигура взмаха удара: видна только в фазе удара героя.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Strike;

/// Бой на этот шаг: стоп-кадр и тряска камеры.
#[derive(Resource, Default)]
pub(super) struct Combat {
    /// Сколько ещё длится стоп-кадр, секунды.
    pub(super) hit_stop: f64,
    /// Тряска: сколько осталось, полная длительность и размах.
    pub(super) shake_left: f64,
    pub(super) shake_time: f64,
    pub(super) shake: f64,
}

/// События игры (6.6): номер шага логики и что случилось, за последние шаги.
#[derive(Resource, Default)]
pub(super) struct Events {
    pub(super) step: u64,
    pub(super) list: std::collections::VecDeque<(u64, GameEvent)>,
}

/// Сколько шагов логики события лежат в снимке.
pub(super) const EVENT_STEPS: u64 = 16;

/// Нажатия, пришедшие во время стоп-кадра: персонаж получит их на первом шаге после
/// него, иначе короткое нажатие в стоп-кадре потерялось бы.
#[derive(Resource, Default, Clone, Copy)]
pub(super) struct Pending {
    pub(super) jump: bool,
    pub(super) dash: bool,
    pub(super) attack: bool,
    /// Удар нажат мышью «к курсору»: куда бить (6.12).
    pub(super) attack_at: Option<[f64; 2]>,
}

/// Твёрдое сцены в её координатах (собирается после расчёта мест): описанные
/// прямоугольники (для частиц и показа), для каждого — двигается ли он (метка
/// [`Dynamic`] у него или предка), и настоящие формы для мира столкновений (9.3).
#[derive(Resource, Default)]
pub(super) struct Solids(
    pub(super) Vec<Collider>,
    pub(super) Vec<bool>,
    pub(super) Vec<Surface>,
);

/// Ввод игрока на этот шаг логики.
#[derive(Resource, Default, Clone, Copy)]
pub(super) struct GameInput(pub(super) InputFrame);

/// Последнее нажатие, взятое логикой (для замера задержки, 6.3).
#[derive(Resource, Default)]
pub(super) struct LastPress(pub(super) u64);

/// Время сцены на прошлом шаге: от него считается, сколько шагать персонажу. Новый мир
/// начинает с нуля (время сцены с начала).
#[derive(Resource)]
pub(super) struct LastStep(pub(super) f64);

/// Время сцены прошлого шага сюжета (паузы карточек «Подождать», Сю.7).
#[derive(Resource)]
pub(super) struct StoryClock(pub(super) f64);

/// Насколько ниже места появления персонаж считается упавшим из уровня.
pub(super) const FALL_OUT: f64 = 5.0;
/// Самый длинный шаг персонажа; если времени прошло больше, шагов делается несколько.
pub(super) const PLAYER_STEP: f64 = 1.0 / 60.0;

/// Время сцены, секунды (движение считается от него).
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct SceneTime(pub f64);

/// Фон и общий свет сцены.
#[derive(Resource, Clone, Copy, Debug)]
pub(super) struct Settings {
    pub(super) background: Background,
    pub(super) ambient: [f32; 3],
}

/// Последний снимок для рисования.
#[derive(Resource, Default)]
pub(super) struct Snapshot(pub(super) Scene);
