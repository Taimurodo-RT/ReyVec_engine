//! Мир сцены на `bevy_ecs` (этап 5.2).
//!
//! Каждый предмет файла — сущность с местом ([`Placement`]); его фигуры, источники света
//! и SDF-объекты — дочерние сущности (связь `ChildOf`), вложенные предметы тоже. Две
//! системы, одна за другой:
//!
//! 1. `propagate` проходит от корней к листьям и считает итоговое место каждой сущности
//!    ([`GlobalPlacement`]): сдвиг, поворот и размер родителя действуют на детей, глубины
//!    складываются, прозрачности умножаются, отметка «отбрасывает тень» наследуется.
//! 2. `collect` собирает из мира снимок для рисования ([`Scene`]): фигуры по глубине
//!    (поровну — в порядке файла), подряд идущие фигуры с одинаковой тенью и прозрачностью
//!    в один слой, источники и объекты в координатах сцены.
//!
//! Рисование читает только снимок, а не сам мир (решение 0010): в 5.3 мир уйдёт в поток
//! логики, а снимки будут передаваться рисованию.

use bevy_ecs::hierarchy::{ChildOf, Children};
use bevy_ecs::name::Name;
use bevy_ecs::prelude::*;
use std::sync::Arc;

use reyvec_svg::SvgDrawing;
use reyvec_vector::{Affine, BlendMode, Color, Instance, SceneItem, ShapeId, ShapeLibrary};

use reyvec_game::anim::AnimInput;
use reyvec_game::physics::{BodyId, Physics};
use reyvec_game::{
    Aabb, Action, Animator, AttackTuning, Body, Bone, Collider, Dummy, Fighter, Health, Hurt,
    InputFrame, Intent, Tuning,
};

use crate::assets::{self, MorphTrack};
use crate::{
    Background, GameEvent, LightKind, NodeDesc, PlayerView, Scene, SceneError, SceneFile,
    SceneLight, SceneObject, ShapeCache, ShapeDesc,
};

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
}

impl GlobalPlacement {
    const ROOT: Self = Self {
        affine: Affine::IDENTITY,
        depth: 0.0,
        scale: 1.0,
        shadow: false,
        opacity: 1.0,
        group: 0,
        flash: 0.0,
    };

    fn then(&self, p: &Placement, me: Entity) -> Self {
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
        }
    }

    fn point(&self, p: [f32; 2]) -> [f32; 2] {
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

/// Твёрдый прямоугольник (6.1) с серединой в месте сущности и половинами сторон `half`.
/// Если сущность повёрнута, твёрдым становится описанный вокруг неё прямоугольник.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Solid {
    pub half: [f64; 2],
    /// Платформа: держит только сверху (6.2).
    pub oneway: bool,
    /// Середина на прошлом шаге: по ней видно, что предмет едет (6.2).
    pub last: Option<[f64; 2]>,
    /// Склон (6.4): подъём слева направо, 0 — не склон ([`Collider::slope`]).
    pub slope: f64,
}

/// Персонаж игрока (6.1): настройки, состояние и место появления; удар (6.5).
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
}

/// Предмет со скриптом (6.9): его таблица `me` в скрипте, прямоугольник касания героя
/// и место из файла. У манекена скрипт задаёт бег и прыжок, у других предметов — место
/// и видимость.
#[derive(Component)]
pub struct Scripted {
    actor: reyvec_script::Actor,
    touch: [f64; 2],
    /// Прозрачность из файла (скрытый скриптом — 0).
    opacity: f32,
    /// Уже подобран: событие «подобрал» один раз, пока скрипт не сбросит `collected`.
    collected: bool,
}

/// Все скрипты мира и счётчик шагов до проверки файлов на правку.
#[derive(Resource)]
struct ScriptRes {
    scripts: reyvec_script::Scripts,
    until_check: u32,
}

/// Как часто проверять файлы скриптов на правку: раз в столько шагов логики (0,25 с).
const SCRIPT_CHECK_STEPS: u32 = 15;

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

/// Физическое тело предмета (6.10) и его поворот из файла.
#[derive(Component, Clone, Copy, Debug)]
pub struct PhysBody {
    pub id: BodyId,
    pub angle: f64,
}

/// Обломок разбитого врага: заранее созданная сущность с квадратом, тело физики и
/// сколько ему ещё лежать.
struct Piece {
    entity: Entity,
    body: Option<BodyId>,
    left: f64,
}

/// Разбит враг: где, его размер, цвет и куда били.
struct Break {
    at: [f64; 2],
    size: [f64; 2],
    color: Color,
    dir: f64,
}

/// Физика сцены (6.10).
#[derive(Resource)]
struct PhysicsRes {
    physics: Physics,
    /// Тела предметов (без обломков) по порядку: для героя они платформы.
    bodies: Vec<BodyId>,
    /// Платформы для героя после прошлого шага физики.
    platforms: Vec<Collider>,
    /// Сколько шагать физике (пишет `play`: время шага без стоп-кадра).
    dt: f64,
    breaks: Vec<Break>,
    pieces: Vec<Piece>,
    seed: u64,
}

/// Сколько обломков от одного разбитого врага и сколько всего в запасе.
const PIECES_PER_BREAK: usize = 10;
const PIECES: usize = 40;
/// Сколько лежат обломки и сколько из этого времени гаснут, секунды.
const PIECE_LIFE: f64 = 3.0;
const PIECE_FADE: f64 = 0.6;
/// Насколько шире прямоугольник героя для касания врага и как отбрасывает раненого.
const CONTACT_REACH: f64 = 0.02;
const HURT_PUSH: [f64; 2] = [2.2, 2.0];
/// Номера платформ из физических тел для героя (не пересекаются с номерами сущностей).
const PLATFORM_IDS: u64 = 1 << 62;

/// Фигура взмаха удара: видна только в фазе удара героя.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Strike;

/// Бой на этот шаг: стоп-кадр и тряска камеры.
#[derive(Resource, Default)]
struct Combat {
    /// Сколько ещё длится стоп-кадр, секунды.
    hit_stop: f64,
    /// Тряска: сколько осталось, полная длительность и размах.
    shake_left: f64,
    shake_time: f64,
    shake: f64,
}

/// События игры (6.6): номер шага логики и что случилось, за последние шаги.
#[derive(Resource, Default)]
struct Events {
    step: u64,
    list: std::collections::VecDeque<(u64, GameEvent)>,
}

/// Сколько шагов логики события лежат в снимке.
const EVENT_STEPS: u64 = 16;

/// Нажатия, пришедшие во время стоп-кадра: персонаж получит их на первом шаге после
/// него, иначе короткое нажатие в стоп-кадре потерялось бы.
#[derive(Resource, Default, Clone, Copy)]
struct Pending {
    jump: bool,
    dash: bool,
    attack: bool,
}

/// Твёрдые прямоугольники сцены в её координатах (собираются после расчёта мест).
#[derive(Resource, Default)]
struct Solids(Vec<Collider>);

/// Ввод игрока на этот шаг логики.
#[derive(Resource, Default, Clone, Copy)]
struct GameInput(InputFrame);

/// Последнее нажатие, взятое логикой (для замера задержки, 6.3).
#[derive(Resource, Default)]
struct LastPress(u64);

/// Время сцены на прошлом шаге: от него считается, сколько шагать персонажу. Новый мир
/// начинает с нуля (время сцены с начала).
#[derive(Resource)]
struct LastStep(f64);

/// Насколько ниже места появления персонаж считается упавшим из уровня.
const FALL_OUT: f64 = 5.0;
/// Самый длинный шаг персонажа; если времени прошло больше, шагов делается несколько.
const PLAYER_STEP: f64 = 1.0 / 60.0;

/// Время сцены, секунды (движение считается от него).
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct SceneTime(pub f64);

/// Фон и общий свет сцены.
#[derive(Resource, Clone, Copy, Debug)]
struct Settings {
    background: Background,
    ambient: [f32; 3],
}

/// Последний снимок для рисования.
#[derive(Resource, Default)]
struct Snapshot(Scene);

/// Мир сцены: сущности из файла и расписание систем.
pub struct SceneWorld {
    world: World,
    schedule: Schedule,
    entities: usize,
}

impl SceneWorld {
    /// Строит мир по файлу. Новые формы фигур нарезаются в `library`.
    pub fn new(
        file: &SceneFile,
        library: &mut ShapeLibrary,
        cache: &mut ShapeCache,
    ) -> Result<Self, SceneError> {
        cache.begin_use();
        let a = file.ambient.0;
        let mut world = World::new();
        world.insert_resource(Settings {
            background: file.background,
            ambient: [a.r, a.g, a.b],
        });
        world.init_resource::<Snapshot>();
        world.init_resource::<Solids>();
        world.init_resource::<GameInput>();
        world.init_resource::<LastPress>();
        world.init_resource::<Combat>();
        world.init_resource::<Pending>();
        world.init_resource::<Events>();
        world.insert_resource(LastStep(0.0));
        world.insert_resource(PhysicsRes {
            physics: Physics::new(reyvec_game::Tuning::default().gravity()),
            bodies: Vec::new(),
            platforms: Vec::new(),
            dt: 0.0,
            breaks: Vec::new(),
            pieces: Vec::new(),
            seed: 0x5eed,
        });
        world.insert_resource(ScriptRes {
            scripts: reyvec_script::Scripts::new(),
            until_check: SCRIPT_CHECK_STEPS,
        });
        let mut spawner = Spawner {
            world: &mut world,
            library,
            cache,
            dir: file.dir.as_deref(),
            order: 0,
            entities: 0,
            in_player: false,
        };
        // Источники и объекты верхнего уровня (способ 5.1) — в общем корне без места.
        let root = NodeDesc {
            name: "сцена".into(),
            at: (0.0, 0.0),
            depth: 0.0,
            angle: 0.0,
            scale: 1.0,
            shadow: None,
            opacity: 1.0,
            shapes: Vec::new(),
            lights: file.lights.clone(),
            objects: file.objects.clone(),
            children: NodeDesc::all(file),
            spin: 0.0,
            swing: None,
            bob: None,
            pictures: Vec::new(),
            morphs: Vec::new(),
            solids: Vec::new(),
            tiles: None,
            player: None,
            strike: Vec::new(),
            dummy: None,
            bone: None,
            script: None,
            touch: None,
            body: None,
        };
        spawner.node(&root, None, "сцена")?;
        let entities = spawner.entities;
        let mut schedule = Schedule::default();
        world.init_resource::<SceneTime>();
        // Места считаются дважды: сначала для всего, что движется само (платформы едут
        // до того, как шагает персонаж, и везут его в том же шаге), потом для персонажа.
        schedule.add_systems(
            (
                animate,
                propagate,
                gather_solids,
                play,
                physics_step,
                run_scripts,
                combat_look,
                pose_bones,
                propagate_again,
                collect,
            )
                .chain(),
        );
        Ok(Self {
            world,
            schedule,
            entities,
        })
    }

    /// Прогоняет системы мира: итоговые места и снимок.
    pub fn update(&mut self) {
        self.schedule.run(&mut self.world);
    }

    /// Прогоняет системы мира в момент `seconds` от начала сцены: движение предметов,
    /// итоговые места и снимок.
    pub fn update_at(&mut self, seconds: f64) {
        self.world.resource_mut::<SceneTime>().0 = seconds;
        self.update();
    }

    /// Снимок для рисования после последнего [`SceneWorld::update`].
    pub fn snapshot(&self) -> Scene {
        let mut scene = self.world.resource::<Snapshot>().0.clone();
        scene.entities = self.entities;
        scene
    }

    /// События игры за последние шаги (то же, что `Scene::events` снимка, только без
    /// копии всего снимка): для частиц, когда мир шагает много шагов за кадр.
    pub fn events(&self) -> Vec<(u64, GameEvent)> {
        self.world
            .resource::<Events>()
            .list
            .iter()
            .copied()
            .collect()
    }

    /// Мир продолжает уже идущее время сцены: следующий [`SceneWorld::update_at`] шагает
    /// персонажа от момента `seconds`, а не от начала сцены.
    pub fn continue_from(&mut self, seconds: f64) {
        self.world.resource_mut::<LastStep>().0 = seconds;
    }

    /// Ввод игрока для следующих [`SceneWorld::update_at`].
    pub fn set_input(&mut self, input: InputFrame) {
        self.world.resource_mut::<GameInput>().0 = input;
        if input.stamp() != 0 {
            self.world.resource_mut::<LastPress>().0 = input.stamp();
        }
    }

    /// Состояние персонажа игрока, если он есть.
    pub fn player_body(&mut self) -> Option<Body> {
        let mut q = self.world.query::<&Player>();
        q.iter(&self.world).next().map(|p| p.body)
    }

    /// Ставит персонажа в состояние `body` (после сохранения файла сцены персонаж
    /// остаётся там, где был, и с той же скоростью).
    pub fn set_player_body(&mut self, body: Body) {
        let mut q = self.world.query::<(&mut Player, &mut Placement)>();
        if let Some((mut p, mut place)) = q.iter_mut(&mut self.world).next() {
            p.body = body;
            place.at = body.pos;
            place.flip = body.facing < 0.0;
        }
    }

    /// Твёрдое сцены на последнем шаге и настройки героя (для проверок).
    #[cfg(test)]
    pub(crate) fn colliders(&mut self) -> (Vec<Collider>, Option<Tuning>) {
        let solids = self.world.resource::<Solids>().0.clone();
        let mut q = self.world.query::<&Player>();
        let tuning = q.iter(&self.world).next().map(|p| p.tuning);
        (solids, tuning)
    }

    /// Манекены сцены (для проверок).
    #[cfg(test)]
    pub(crate) fn dummies(&mut self) -> Vec<Dummy> {
        // В порядке файла (сущности создаются по порядку).
        let mut q = self.world.query::<(Entity, &Target)>();
        let mut all: Vec<_> = q.iter(&self.world).collect();
        all.sort_by_key(|(e, _)| e.index());
        all.into_iter().map(|(_, t)| t.dummy.clone()).collect()
    }

    /// Сколько сущностей в мире.
    pub fn entity_count(&self) -> usize {
        self.entities
    }

    /// Сам мир (для проверок и будущих систем логики).
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Сущность по имени предмета (первая найденная).
    pub fn find(&mut self, name: &str) -> Option<Entity> {
        let mut q = self.world.query::<(Entity, &Name)>();
        q.iter(&self.world)
            .find(|(_, n)| n.as_str() == name)
            .map(|(e, _)| e)
    }
}

struct Spawner<'a> {
    world: &'a mut World,
    library: &'a mut ShapeLibrary,
    cache: &'a mut ShapeCache,
    dir: Option<&'a std::path::Path>,
    order: u32,
    entities: usize,
    /// Строится содержимое героя: внутри него бывают кости.
    in_player: bool,
}

impl Spawner<'_> {
    /// Рисунок SVG: готовый из кэша или прочитанный и нарезанный здесь же.
    fn picture(&mut self, p: &crate::PictureDesc) -> Result<Arc<SvgDrawing>, String> {
        let file = assets::picture_file(self.dir, p)?;
        self.cache.mark_used(&file.key);
        if let Some(d) = self.cache.svgs.get(&file.key) {
            return Ok(d.clone());
        }
        let d = Arc::new(assets::load_picture(&file, p.height, self.library)?);
        self.cache.svgs.insert(file.key, d.clone());
        Ok(d)
    }

    fn motif(&mut self, m: &crate::MorphDesc) -> Result<Arc<reyvec_motif::MorphFile>, String> {
        let file = assets::motif_file(self.dir, m)?;
        self.cache.mark_used(&file.key);
        if let Some(f) = self.cache.motifs.get(&file.key) {
            return Ok(f.clone());
        }
        let f = Arc::new(assets::load_motif(&file, self.library)?);
        self.cache.motifs.insert(file.key, f.clone());
        Ok(f)
    }

    fn next(&mut self) -> u32 {
        self.order += 1;
        self.order
    }

    /// Запас обломков (6.10): квадраты, которые ждут, пока кого-нибудь разобьют.
    /// Создаётся один раз, при первом манекене.
    fn debris_pool(&mut self) -> Result<(), SceneError> {
        if !self.world.resource::<PhysicsRes>().pieces.is_empty() {
            return Ok(());
        }
        let id = crate::debris_square()
            .shape_id(self.library, self.cache)
            .map_err(|m| SceneError::new(format!("обломки: {m}")))?;
        let mut pieces = Vec::with_capacity(PIECES);
        for _ in 0..PIECES {
            let order = self.next();
            let entity = self.spawn(
                (
                    Name::new("обломок"),
                    Placement {
                        depth: 2.5,
                        opacity: 0.0,
                        shadow: Some(false),
                        ..Placement::IDENTITY
                    },
                    GlobalPlacement::ROOT,
                    VectorShape {
                        id,
                        tint: Color::WHITE,
                        order,
                        morph: None,
                    },
                ),
                None,
            );
            pieces.push(Piece {
                entity,
                body: None,
                left: 0.0,
            });
        }
        self.world.resource_mut::<PhysicsRes>().pieces = pieces;
        Ok(())
    }

    fn spawn(&mut self, bundle: impl Bundle, parent: Option<Entity>) -> Entity {
        self.entities += 1;
        let mut e = self.world.spawn(bundle);
        if let Some(p) = parent {
            e.insert(ChildOf(p));
        }
        e.id()
    }

    fn node(&mut self, n: &NodeDesc, parent: Option<Entity>, path: &str) -> Result<(), SceneError> {
        let placement = Placement {
            at: [n.at.0, n.at.1],
            depth: n.depth,
            angle: n.angle,
            scale: n.scale,
            shadow: n.shadow,
            opacity: n.opacity,
            flip: false,
            flash: 0.0,
        };
        let name = if n.name.is_empty() {
            path.to_string()
        } else {
            n.name.clone()
        };
        let me = self.spawn(
            (Name::new(name.clone()), placement, GlobalPlacement::ROOT),
            parent,
        );
        if let Some(p) = &n.player {
            self.world.entity_mut(me).insert(Player {
                tuning: p.tuning(),
                body: Body::new(placement.at),
                spawn: placement.at,
                attack: p.attack.tuning(),
                fighter: Fighter::default(),
                hits: 0,
                anim: Animator::default(),
                health: Health::new(p.hp),
            });
        }
        if let Some(b) = &n.body {
            let mut res = self.world.resource_mut::<PhysicsRes>();
            let id = res
                .physics
                .add(placement.at, n.angle, [0.0, 0.0], &b.physics());
            res.bodies.push(id);
            self.world
                .entity_mut(me)
                .insert(PhysBody { id, angle: n.angle });
        }
        if let Some(file) = &n.script {
            let path = match self.dir {
                Some(dir) => dir.join(file),
                None => std::path::PathBuf::from(file),
            };
            let mut res = self.world.resource_mut::<ScriptRes>();
            let id = res.scripts.load(&path);
            let touch = n.dummy.map(|d| d.size).or(n.touch).unwrap_or((0.1, 0.1));
            if let Some(actor) = res.scripts.actor(id) {
                self.world.entity_mut(me).insert(Scripted {
                    actor,
                    touch: [touch.0, touch.1],
                    opacity: n.opacity,
                    collected: false,
                });
            }
        }
        if let Some(bone) = n.bone.as_deref().and_then(Bone::parse)
            && self.in_player
        {
            self.world.entity_mut(me).insert(BoneOf {
                bone,
                base: placement,
            });
        }
        if let Some(d) = &n.dummy {
            let color = n
                .shapes
                .iter()
                .map(|s| {
                    let area = match s {
                        ShapeDesc::Rect { size, .. } => size.0 * size.1,
                        ShapeDesc::Circle { r, .. } => std::f64::consts::PI * r * r,
                        _ => 0.0,
                    };
                    (area, s.placement().2.0)
                })
                .fold((0.0, Color::WHITE), |a, b| if b.0 > a.0 { b } else { a })
                .1;
            self.world.entity_mut(me).insert(Target {
                dummy: Dummy::new(placement.at, [d.size.0, d.size.1], d.hp, d.respawn),
                opacity: n.opacity,
                color,
                contact: d.contact,
            });
            self.debris_pool()?;
        }
        for s in &n.solids {
            self.spawn(
                (
                    Placement {
                        at: [s.at.0, s.at.1],
                        ..Placement::IDENTITY
                    },
                    GlobalPlacement::ROOT,
                    Solid {
                        half: [s.size.0 * 0.5, s.size.1 * 0.5],
                        oneway: s.oneway,
                        last: None,
                        slope: 0.0,
                    },
                ),
                Some(me),
            );
        }
        if let Some(tiles) = &n.tiles {
            let fail = |m: String| SceneError::new(format!("предмет «{name}», tiles: {m}"));
            let level = tiles.build().map_err(fail)?;
            for s in &level.solids {
                self.spawn(
                    (
                        Placement {
                            at: s.at,
                            ..Placement::IDENTITY
                        },
                        GlobalPlacement::ROOT,
                        Solid {
                            half: s.half,
                            oneway: s.oneway,
                            last: None,
                            slope: s.slope,
                        },
                    ),
                    Some(me),
                );
            }
            for shape in &level.shapes {
                let id = shape.shape_id(self.library, self.cache).map_err(fail)?;
                let (at, angle, color) = shape.placement();
                let order = self.next();
                self.spawn(
                    (
                        Placement {
                            at: [at.0, at.1],
                            angle,
                            ..Placement::IDENTITY
                        },
                        GlobalPlacement::ROOT,
                        VectorShape {
                            id,
                            tint: color.0,
                            order,
                            morph: None,
                        },
                    ),
                    Some(me),
                );
            }
        }
        if n.spin != 0.0 || n.swing.is_some() || n.bob.is_some() {
            self.world.entity_mut(me).insert(Motion {
                base: placement,
                spin: n.spin,
                swing: n.swing.map(|w| (w.angle, w.period)),
                bob: n.bob.map(|b| ([b.by.0, b.by.1], b.period)),
            });
        }
        for (j, shape) in n.shapes.iter().enumerate() {
            let id = shape
                .shape_id(self.library, self.cache)
                .map_err(|m| SceneError::new(format!("предмет «{name}», фигура {}: {m}", j + 1)))?;
            let (at, angle, color) = shape.placement();
            let order = self.next();
            let e = self.spawn(
                (
                    Placement {
                        at: [at.0, at.1],
                        angle,
                        ..Placement::IDENTITY
                    },
                    GlobalPlacement::ROOT,
                    VectorShape {
                        id,
                        tint: color.0,
                        order,
                        morph: None,
                    },
                ),
                Some(me),
            );
            if let ShapeDesc::Rect {
                size,
                solid,
                oneway,
                ..
            } = shape
                && (*solid || *oneway)
            {
                self.world.entity_mut(e).insert(Solid {
                    half: [size.0 * 0.5, size.1 * 0.5],
                    oneway: *oneway,
                    last: None,
                    slope: 0.0,
                });
            }
        }
        for (j, shape) in n.strike.iter().enumerate() {
            let id = shape.shape_id(self.library, self.cache).map_err(|m| {
                SceneError::new(format!("предмет «{name}», взмах strike {}: {m}", j + 1))
            })?;
            let (at, angle, color) = shape.placement();
            let order = self.next();
            self.spawn(
                (
                    Placement {
                        at: [at.0, at.1],
                        angle,
                        opacity: 0.0,
                        ..Placement::IDENTITY
                    },
                    GlobalPlacement::ROOT,
                    VectorShape {
                        id,
                        tint: color.0,
                        order,
                        morph: None,
                    },
                    Strike,
                ),
                Some(me),
            );
        }
        for (j, p) in n.pictures.iter().enumerate() {
            let fail =
                |m: String| SceneError::new(format!("предмет «{name}», рисунок {}: {m}", j + 1));
            let drawing = self.picture(p).map_err(fail)?;
            let order = self.next();
            self.spawn(
                (
                    Placement {
                        at: [p.at.0, p.at.1],
                        angle: p.angle,
                        ..Placement::IDENTITY
                    },
                    GlobalPlacement::ROOT,
                    SvgPicture { drawing, order },
                ),
                Some(me),
            );
        }
        for (j, m) in n.morphs.iter().enumerate() {
            let fail = |e: String| {
                SceneError::new(format!("предмет «{name}», фигура Motif {}: {e}", j + 1))
            };
            let motif = self.motif(m).map_err(fail)?;
            let (id, track) = assets::pick_morph(&motif, m).map_err(fail)?;
            let order = self.next();
            self.spawn(
                (
                    Placement {
                        at: [m.at.0, m.at.1],
                        angle: m.angle,
                        scale: m.scale,
                        ..Placement::IDENTITY
                    },
                    GlobalPlacement::ROOT,
                    VectorShape {
                        id,
                        tint: m.color.map_or(Color::WHITE, |c| c.0),
                        order,
                        morph: Some(track),
                    },
                ),
                Some(me),
            );
        }
        for light in &n.lights {
            let order = self.next();
            self.spawn(
                (
                    Placement::IDENTITY,
                    GlobalPlacement::ROOT,
                    LightSource {
                        light: light.build(),
                        order,
                    },
                ),
                Some(me),
            );
        }
        for object in &n.objects {
            let order = self.next();
            self.spawn(
                (
                    Placement::IDENTITY,
                    GlobalPlacement::ROOT,
                    SdfShape {
                        object: object.build(),
                        order,
                    },
                ),
                Some(me),
            );
        }
        let was = self.in_player;
        self.in_player |= n.player.is_some();
        for (j, child) in n.children.iter().enumerate() {
            self.node(child, Some(me), &format!("{name} → предмет {}", j + 1))?;
        }
        self.in_player = was;
        Ok(())
    }
}

/// Движение: место предметов с [`Motion`] в текущий момент.
fn animate(time: Res<SceneTime>, mut moving: Query<(&Motion, &mut Placement)>) {
    for (motion, mut placement) in &mut moving {
        *placement = motion.at(time.0);
    }
}

/// Персонаж игрока и манекены: шаги контроллера за время с прошлого шага логики, удар
/// и попадания (6.5). Во время стоп-кадра тела стоят, а свечение и тряска идут.
#[allow(clippy::too_many_arguments)]
fn play(
    time: Res<SceneTime>,
    input: Res<GameInput>,
    solids: Res<Solids>,
    mut last: ResMut<LastStep>,
    mut combat: ResMut<Combat>,
    mut pending: ResMut<Pending>,
    mut events: ResMut<Events>,
    mut phys: ResMut<PhysicsRes>,
    mut players: Query<(&mut Player, &mut Placement)>,
    mut targets: Query<(Entity, &mut Target), Without<Player>>,
) {
    phys.dt = 0.0;
    let dt = (time.0 - last.0).clamp(0.0, 0.25);
    last.0 = time.0;
    events.step += 1;
    let step = events.step;
    while events
        .list
        .front()
        .is_some_and(|&(s, _)| s + EVENT_STEPS <= step)
    {
        events.list.pop_front();
    }
    let input = input.0;
    pending.jump |= input.pressed(Action::Jump);
    pending.dash |= input.pressed(Action::Dash);
    pending.attack |= input.pressed(Action::Attack);

    // Таймеры, которые идут и в стоп-кадре.
    combat.shake_left = (combat.shake_left - dt).max(0.0);
    let heroes: Vec<Aabb> = players
        .iter()
        .map(|(p, _)| p.body.bounds(&p.tuning))
        .collect();
    for (_, mut t) in &mut targets {
        t.dummy
            .tick(dt, |at| heroes.iter().any(|h| h.overlaps(&at)));
    }
    let frozen = combat.hit_stop.min(dt);
    combat.hit_stop -= frozen;
    let dt = dt - frozen;
    if dt <= 1e-9 {
        for (_, mut t) in &mut targets {
            t.dummy.moved = [0.0, 0.0];
        }
        return;
    }
    let presses = std::mem::take(&mut *pending);
    phys.dt = dt;

    // Запас на неточность дробей: ровно один шаг логики — один шаг персонажа.
    let steps = (dt / PLAYER_STEP - 1e-6).ceil().max(1.0) as usize;
    let sdt = dt / steps as f64;
    // Платформы сдвинулись один раз за шаг логики: везут только в первом из шагов.
    let still = |c: &Collider| Collider {
        moved: [0.0, 0.0],
        ..*c
    };
    // Для героя манекены — тоже твёрдые тела (на них можно встать, их не пройти).
    let bodies: Vec<Collider> = targets
        .iter()
        .filter(|(_, t)| t.dummy.alive())
        .map(|(e, t)| t.dummy.collider(e.to_bits()))
        .collect();
    // Физические тела (6.10) для героя — платформы: на них встают, сбоку герой их толкает.
    let with_bodies: Vec<Collider> = solids
        .0
        .iter()
        .chain(&bodies)
        .chain(&phys.platforms)
        .copied()
        .collect();
    let with_bodies_still: Vec<Collider> = with_bodies.iter().map(still).collect();

    let mut stop = false;
    for (mut player, mut place) in &mut players {
        let p = &mut *player;
        for i in 0..steps {
            // Нажатие — только на первом из нескольких шагов.
            let first = i == 0;
            let intent = Intent {
                x: input.x(),
                jump_pressed: first && presses.jump,
                jump_held: input.held(Action::Jump),
                down: input.held(Action::Down),
                dash_pressed: first && presses.dash,
                up: input.held(Action::Up),
            };
            let colliders = if first {
                &with_bodies
            } else {
                &with_bodies_still
            };
            let fall = -p.body.vel[1];
            let ev = p.body.step(&p.tuning, intent, colliders, sdt);
            let feet = [
                p.body.pos[0] as f32,
                (p.body.pos[1] - p.tuning.size[1] * 0.5) as f32,
            ];
            if ev.landed {
                let speed = fall.max(0.0) as f32;
                events
                    .list
                    .push_back((step, GameEvent::Landed { at: feet, speed }));
            }
            if ev.jumped || ev.wall_jumped {
                events
                    .list
                    .push_back((step, GameEvent::Jumped { at: feet }));
            }
            if ev.dashed {
                let dir = p.body.facing as f32;
                let at = [feet[0], p.body.pos[1] as f32];
                events.list.push_back((step, GameEvent::Dashed { at, dir }));
            }
            let can = !p.body.on_ledge();
            let swings = p.fighter.swings;
            p.fighter
                .step(&p.attack, first && presses.attack, can, p.body.facing, sdt);
            if p.fighter.swings > swings {
                let at = [p.body.pos[0] as f32, p.body.pos[1] as f32];
                let dir = p.body.facing as f32;
                events.list.push_back((step, GameEvent::Swing { at, dir }));
            }
            // Анимация (6.8): поза по телу и удару; нога встала на землю — шаг (звук и
            // пыль из-под ног).
            let anim = AnimInput {
                landed: ev.landed,
                ..AnimInput::of(&p.body, &p.tuning, &p.fighter, &p.attack)
            };
            if p.anim.step(&anim, sdt).step {
                let dir = p.body.vel[0].signum() as f32;
                events
                    .list
                    .push_back((step, GameEvent::Run { at: feet, dir }));
            }
            if let Some(hitbox) = p.fighter.hitbox(&p.attack, p.body.pos, p.tuning.half()) {
                for (e, mut t) in &mut targets {
                    if t.dummy.alive()
                        && hitbox.overlaps(&t.dummy.bounds())
                        && p.fighter.first_hit(e.to_bits())
                    {
                        let dir = p.fighter.dir();
                        // Место попадания: край цели со стороны героя, на высоте удара.
                        let b = t.dummy.bounds();
                        let x = if dir > 0.0 { b.min[0] } else { b.max[0] };
                        let y =
                            hitbox.min[1].max(b.min[1]) * 0.5 + hitbox.max[1].min(b.max[1]) * 0.5;
                        events.list.push_back((
                            step,
                            GameEvent::Hit {
                                at: [x as f32, y as f32],
                                dir: dir as f32,
                            },
                        ));
                        let k = p.attack.knockback;
                        let broken = t.dummy.hurt(p.attack.damage, [dir * k[0], k[1]]);
                        let top = [b.center()[0] as f32, b.max[1] as f32 + 0.04];
                        events.list.push_back((
                            step,
                            GameEvent::Damage {
                                at: top,
                                amount: p.attack.damage,
                                left: t.dummy.hp,
                            },
                        ));
                        if broken {
                            // Разбит: обломки разлетаются (6.10).
                            phys.breaks.push(Break {
                                at: t.dummy.body.pos,
                                size: t.dummy.tuning.size,
                                color: t.color,
                                dir,
                            });
                        }
                        p.hits += 1;
                        combat.hit_stop = combat.hit_stop.max(p.attack.hit_stop);
                        if p.attack.shake > 0.0 && p.attack.shake_time > 0.0 {
                            combat.shake = p.attack.shake;
                            combat.shake_time = p.attack.shake_time;
                            combat.shake_left = p.attack.shake_time;
                        }
                    }
                }
            }
            if combat.hit_stop > 0.0 {
                // Попал: остаток шага уже в стоп-кадре.
                stop = true;
                break;
            }
        }
        if p.body.pos[1] < p.spawn[1] - FALL_OUT {
            p.body = Body::new(p.spawn);
        }
        // Враги ранят касанием (6.10б). Враг для героя твёрдый, поэтому вплотную они
        // только касаются: прямоугольник героя чуть шире.
        p.health.tick(dt);
        let me = p.body.bounds(&p.tuning).grow(CONTACT_REACH);
        let touch = targets
            .iter()
            .find(|(_, t)| t.contact > 0 && t.dummy.alive() && me.overlaps(&t.dummy.bounds()));
        if let Some((_, t)) = touch {
            let amount = t.contact;
            let from = t.dummy.body.pos[0];
            let at = [p.body.pos[0] as f32, p.body.pos[1] as f32];
            match p.health.hurt(amount) {
                Hurt::Ignored => {}
                hurt => {
                    // Отбрасывает от врага и чуть вверх.
                    let dir = if p.body.pos[0] >= from { 1.0 } else { -1.0 };
                    p.body.vel = [dir * HURT_PUSH[0], HURT_PUSH[1]];
                    p.body.on_ground = false;
                    events.list.push_back((
                        step,
                        GameEvent::Hurt {
                            at,
                            amount,
                            dir: dir as f32,
                        },
                    ));
                    combat.shake = 0.03;
                    combat.shake_time = 0.25;
                    combat.shake_left = 0.25;
                    if hurt == Hurt::Died {
                        events.list.push_back((step, GameEvent::Died { at }));
                        p.body = Body::new(p.spawn);
                        p.health.restore();
                    }
                }
            }
        }
        place.at = p.body.pos;
        place.flip = p.body.facing < 0.0;
        // Неуязвимый мигает.
        let inv = p.health.invulnerable;
        place.opacity = if inv > 0.0 && (inv * 12.0) as i64 % 2 == 0 {
            0.35
        } else {
            1.0
        };
    }
    // Где герой теперь, после своего шага: в него упираются манекены.
    let heroes_now: Vec<Aabb> = players
        .iter()
        .map(|(p, _)| p.body.bounds(&p.tuning))
        .collect();
    for (e, mut t) in &mut targets {
        let d = &mut t.dummy;
        if stop {
            d.moved = [0.0, 0.0];
            continue;
        }
        // Манекены не проходят друг сквозь друга: отброшенный упирается в соседа. Герой
        // для них тоже твёрдый (6.9): враг со скриптом упирается в героя, а не входит в
        // него.
        let me = e.to_bits();
        let mine: Vec<Collider> = solids
            .0
            .iter()
            .chain(bodies.iter().filter(|c| c.id != me))
            .copied()
            .chain(heroes_now.iter().map(|h| Collider::solid(*h)))
            .collect();
        let mine_still: Vec<Collider> = mine.iter().map(still).collect();
        let mut moved = [0.0, 0.0];
        for i in 0..steps {
            d.step(if i == 0 { &mine } else { &mine_still }, sdt);
            moved[0] += d.moved[0];
            moved[1] += d.moved[1];
        }
        d.moved = moved;
        if d.body.pos[1] < d.spawn[1] - FALL_OUT {
            d.body = Body::new(d.spawn);
            d.moved = [0.0, 0.0];
        }
    }
}

/// Как выглядит бой: манекены на своих местах, белеют от попадания, разбитые не видны;
/// взмах удара виден только в фазе удара.
fn combat_look(
    players: Query<&Player>,
    mut targets: Query<(&Target, &mut Placement), Without<Strike>>,
    mut strikes: Query<&mut Placement, With<Strike>>,
) {
    let striking = players.iter().any(|p| p.fighter.striking());
    for mut place in &mut strikes {
        place.opacity = if striking { 1.0 } else { 0.0 };
    }
    for (t, mut place) in &mut targets {
        place.at = t.dummy.body.pos;
        place.flip = t.dummy.body.facing < 0.0;
        place.flash = (t.dummy.flash / reyvec_game::combat::FLASH_TIME) as f32;
        place.opacity = if t.dummy.alive() { t.opacity } else { 0.0 };
    }
}

/// Обломки: место и цвет квадрата (не тела предметов и не герой).
type PieceShapes<'w, 's> = Query<
    'w,
    's,
    (&'static mut Placement, &'static mut VectorShape),
    (Without<PhysBody>, Without<Player>),
>;

/// Физика (6.10): уровень, герой и враги — кинематические тела, предметы с `body` и
/// обломки — настоящие. Удар героя отбрасывает тела; разбитый враг разлетается на
/// обломки, которые через несколько секунд гаснут.
fn physics_step(
    solids: Res<Solids>,
    mut phys: ResMut<PhysicsRes>,
    mut events: ResMut<Events>,
    mut players: Query<(Entity, &mut Player)>,
    targets: Query<(Entity, &Target)>,
    mut bodies: Query<(Entity, &PhysBody, &mut Placement), Without<Player>>,
    mut shapes: PieceShapes,
) {
    let phys = &mut *phys;
    let dt = phys.dt;
    if dt <= 0.0 || (phys.bodies.is_empty() && phys.pieces.is_empty()) {
        return;
    }
    let step = events.step;
    // Кинематические тела: уровень, герои, целые враги.
    let level = solids.0.iter().map(|c| (c.id, *c));
    let heroes: Vec<(u64, Collider)> = players
        .iter()
        .map(|(e, p)| (e.to_bits(), Collider::solid(p.body.bounds(&p.tuning))))
        .collect();
    let enemies: Vec<(u64, Collider)> = targets
        .iter()
        .filter(|(_, t)| t.dummy.alive())
        .map(|(e, t)| (e.to_bits(), Collider::solid(t.dummy.bounds())))
        .collect();
    phys.physics.set_solids(level.chain(heroes).chain(enemies));
    // Удар героя отбрасывает тела.
    for (_, mut player) in &mut players {
        let p = &mut *player;
        let Some(hitbox) = p.fighter.hitbox(&p.attack, p.body.pos, p.tuning.half()) else {
            continue;
        };
        for (e, b, _) in &bodies {
            let Some(bounds) = phys.physics.bounds(b.id) else {
                continue;
            };
            if hitbox.overlaps(&bounds) && p.fighter.first_hit(e.to_bits()) {
                let dir = p.fighter.dir();
                let k = p.attack.knockback;
                phys.physics
                    .kick(b.id, [dir * k[0] * 1.4, k[1] * 1.2], -dir * 6.0);
                let x = if dir > 0.0 {
                    bounds.min[0]
                } else {
                    bounds.max[0]
                };
                let y = (bounds.min[1] + bounds.max[1]) * 0.5;
                events.list.push_back((
                    step,
                    GameEvent::Hit {
                        at: [x as f32, y as f32],
                        dir: dir as f32,
                    },
                ));
            }
        }
    }
    // Обломки разбитых врагов.
    for brk in std::mem::take(&mut phys.breaks) {
        for i in 0..PIECES_PER_BREAK {
            phys.seed = phys
                .seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let r = |k: u32| ((phys.seed >> (16 + 8 * k)) & 0xff) as f64 / 255.0;
            let (rx, ry, rs, rv) = (r(0), r(1), r(2), r(3));
            // Свободный обломок или тот, что дольше всех лежит.
            let Some(slot) = (0..phys.pieces.len()).min_by(|&a, &b| {
                phys.pieces[a]
                    .left
                    .partial_cmp(&phys.pieces[b].left)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }) else {
                break;
            };
            if let Some(old) = phys.pieces[slot].body.take() {
                phys.physics.remove(old);
            }
            let side = 0.18 + 0.12 * rs;
            let piece = brk.size[0].min(brk.size[1]) * side;
            let at = [
                brk.at[0] + (rx - 0.5) * brk.size[0] * 0.7,
                brk.at[1] + (ry - 0.5) * brk.size[1] * 0.7,
            ];
            let vel = [
                brk.dir * (0.6 + 1.6 * rv) + (rx - 0.5) * 1.2,
                1.0 + 1.8 * ry,
            ];
            let desc = reyvec_game::physics::BodyDesc {
                size: [piece, piece],
                density: 0.5,
                bounce: 0.35,
                ..Default::default()
            };
            let id = phys.physics.add(at, 0.0, vel, &desc);
            phys.physics
                .kick(id, [0.0, 0.0], (rs - 0.5) * 30.0 + i as f64);
            let p = &mut phys.pieces[slot];
            p.body = Some(id);
            p.left = PIECE_LIFE;
            if let Ok((mut place, mut shape)) = shapes.get_mut(p.entity) {
                place.scale = piece;
                shape.tint = brk.color;
            }
        }
    }
    phys.physics.step(dt);
    // Места тел и обломков после шага.
    for (_, b, mut place) in &mut bodies {
        if let Some((at, angle)) = phys.physics.pose(b.id) {
            place.at = at;
            place.angle = b.angle + angle;
        }
    }
    for i in 0..phys.pieces.len() {
        let p = &mut phys.pieces[i];
        let Some(id) = p.body else {
            continue;
        };
        p.left -= dt;
        let Ok((mut place, _)) = shapes.get_mut(p.entity) else {
            continue;
        };
        if p.left <= 0.0 {
            p.body = None;
            p.left = 0.0;
            place.opacity = 0.0;
            phys.physics.remove(id);
            continue;
        }
        if let Some((at, angle)) = phys.physics.pose(id) {
            place.at = at;
            place.angle = angle;
            place.opacity = (p.left / PIECE_FADE).min(1.0) as f32;
        }
    }
    let ids = phys.bodies.clone();
    phys.platforms = phys.physics.platforms(&ids, PLATFORM_IDS);
}

/// Скрипты (6.9): на каждом шаге логики каждый предмет со скриптом решает, что делать.
/// Раз в [`SCRIPT_CHECK_STEPS`] шагов изменённые файлы перечитываются.
fn run_scripts(
    time: Res<SceneTime>,
    combat: Res<Combat>,
    mut res: ResMut<ScriptRes>,
    mut events: ResMut<Events>,
    players: Query<&Player>,
    mut actors: Query<(&mut Scripted, &mut Placement, Option<&mut Target>), Without<Player>>,
) {
    if actors.is_empty() {
        return;
    }
    let res = &mut *res;
    res.until_check = res.until_check.saturating_sub(1);
    if res.until_check == 0 {
        res.until_check = SCRIPT_CHECK_STEPS;
        res.scripts.reload_changed();
    }
    if combat.hit_stop > 0.0 {
        // Стоп-кадр: скрипты тоже ждут.
        return;
    }
    let (hero, hero_box) = match players.iter().next() {
        Some(p) => (
            reyvec_script::HeroIn {
                x: p.body.pos[0],
                y: p.body.pos[1],
                vx: p.body.vel[0],
                vy: p.body.vel[1],
                striking: p.fighter.striking(),
            },
            Some(p.body.bounds(&p.tuning)),
        ),
        None => (reyvec_script::HeroIn::default(), None),
    };
    let step = events.step;
    for (mut s, mut place, target) in &mut actors {
        let s = &mut *s;
        let (pos, vel, on_ground, wall, hp, alive) = match &target {
            Some(t) => {
                let b = &t.dummy.body;
                (
                    b.pos,
                    b.vel,
                    b.on_ground,
                    b.wall,
                    t.dummy.hp,
                    t.dummy.alive(),
                )
            }
            None => (place.at, [0.0, 0.0], false, 0.0, 0, true),
        };
        let bounds = Aabb::around(pos, [s.touch[0] * 0.5, s.touch[1] * 0.5]);
        let input = reyvec_script::ActorIn {
            x: pos[0],
            y: pos[1],
            vx: vel[0],
            vy: vel[1],
            on_ground,
            wall,
            hp,
            alive,
            touching: alive && hero_box.is_some_and(|h| h.overlaps(&bounds)),
            time: time.0,
        };
        let out = res
            .scripts
            .run(&s.actor, &input, &hero, crate::STEP.as_secs_f64());
        match target {
            Some(mut t) => {
                t.dummy.intent.x = out.move_x;
                t.dummy.intent.jump_pressed |= out.jump;
                // Прыжок врага всегда в полную высоту.
                t.dummy.intent.jump_held = true;
            }
            None => {
                if let (Some(x), Some(y)) = (out.x, out.y) {
                    place.at = [x, y];
                }
                place.opacity = if out.hidden { 0.0 } else { s.opacity };
            }
        }
        if out.collected && !s.collected {
            events.list.push_back((
                step,
                GameEvent::Pickup {
                    at: [pos[0] as f32, pos[1] as f32],
                },
            ));
        }
        s.collected = out.collected;
    }
}

/// Кости героя в позе анимации: к месту и повороту из файла добавляется поза.
fn pose_bones(
    players: Query<&Player>,
    mut bones: Query<(&BoneOf, &mut Placement), Without<Player>>,
) {
    let Some(p) = players.iter().next() else {
        return;
    };
    let pose = p.anim.pose();
    for (b, mut place) in &mut bones {
        let bp = pose.bone(b.bone);
        place.at = [b.base.at[0] + bp.offset[0], b.base.at[1] + bp.offset[1]];
        place.angle = b.base.angle + bp.angle;
    }
}

/// Твёрдые прямоугольники в координатах сцены и на сколько каждый сдвинулся за шаг.
fn gather_solids(mut q: Query<(Entity, &mut Solid, &GlobalPlacement)>, mut solids: ResMut<Solids>) {
    solids.0.clear();
    for (e, mut s, g) in &mut q {
        let [hx, hy] = s.half;
        let corners = [[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]].map(|[x, y]| {
            let p = g.affine * reyvec_vector::Point::new(x, y);
            [p.x, p.y]
        });
        let Some(aabb) = Aabb::of_points(corners) else {
            continue;
        };
        let c = g.affine.translation();
        let center = [c.x, c.y];
        let moved = s
            .last
            .map_or([0.0, 0.0], |l| [center[0] - l[0], center[1] - l[1]]);
        s.last = Some(center);
        // Склон: подъём растягивается вместе с предметом, отражённый — в другую сторону.
        let slope = if s.slope == 0.0 {
            0.0
        } else {
            let a = g.affine.as_coeffs();
            let flip = (a[0] * a[3] - a[1] * a[2]).signum();
            s.slope * (aabb.max[1] - aabb.min[1]) / (2.0 * hy).max(1e-12) * flip
        };
        solids.0.push(Collider {
            aabb,
            oneway: s.oneway,
            id: e.to_bits(),
            moved,
            slope,
        });
    }
}

/// Второй расчёт мест: после шага персонажа.
fn propagate_again(roots: Roots, nodes: Nodes, out: Query<&mut GlobalPlacement>) {
    propagate(roots, nodes, out);
}

type Roots<'w, 's> = Query<'w, 's, Entity, (With<Placement>, Without<ChildOf>)>;
type Nodes<'w, 's> = Query<'w, 's, (&'static Placement, Option<&'static Children>)>;

/// Итоговые места: от корней к листьям.
fn propagate(roots: Roots, nodes: Nodes, mut out: Query<&mut GlobalPlacement>) {
    for root in &roots {
        visit(root, &GlobalPlacement::ROOT, &nodes, &mut out);
    }
}

fn visit(
    e: Entity,
    parent: &GlobalPlacement,
    nodes: &Nodes,
    out: &mut Query<&mut GlobalPlacement>,
) {
    let Ok((p, children)) = nodes.get(e) else {
        return;
    };
    let g = parent.then(p, e);
    if let Ok(mut slot) = out.get_mut(e) {
        *slot = g;
    }
    if let Some(children) = children {
        for child in children.iter() {
            visit(child, &g, nodes, out);
        }
    }
}

/// Что рисуется в векторном слое: фигура или рисунок SVG.
enum Drawn<'a> {
    Shape(&'a VectorShape),
    Picture(&'a SvgPicture),
}

impl Drawn<'_> {
    fn order(&self) -> u32 {
        match self {
            Self::Shape(s) => s.order,
            Self::Picture(p) => p.order,
        }
    }
}

/// Снимок для рисования.
#[allow(clippy::too_many_arguments)]
fn collect(
    shapes: Query<(&VectorShape, &GlobalPlacement)>,
    pictures: Query<(&SvgPicture, &GlobalPlacement)>,
    lights: Query<(&LightSource, &GlobalPlacement)>,
    objects: Query<(&SdfShape, &GlobalPlacement)>,
    settings: Res<Settings>,
    time: Res<SceneTime>,
    players: Query<&Player>,
    solids: Res<Solids>,
    press: Res<LastPress>,
    combat: Res<Combat>,
    events: Res<Events>,
    scripts: Res<ScriptRes>,
    scripted: Query<(), With<Scripted>>,
    mut snapshot: ResMut<Snapshot>,
) {
    let mut drawn: Vec<(Drawn, &GlobalPlacement)> = shapes
        .iter()
        .map(|(s, g)| (Drawn::Shape(s), g))
        .chain(pictures.iter().map(|(p, g)| (Drawn::Picture(p), g)))
        .collect();
    drawn.sort_by(|a, b| {
        a.1.depth
            .total_cmp(&b.1.depth)
            .then(a.0.order().cmp(&b.0.order()))
    });
    let mut items = Vec::with_capacity(drawn.len() + 8);
    let mut shape_count = 0;
    // Слой: подряд идущие фигуры одной группы с одинаковой тенью и прозрачностью.
    let mut open: Option<(u64, bool, f32)> = None;
    for (what, g) in &drawn {
        if g.opacity <= 0.0 {
            // Не видно (взмах вне удара, разбитый манекен): не рисуется и не даёт тени.
            continue;
        }
        let key = (g.group, g.shadow, g.opacity);
        if open != Some(key) {
            if open.is_some() {
                items.push(SceneItem::EndLayer);
                open = None;
            }
            if g.shadow || g.opacity < 1.0 {
                items.push(SceneItem::BeginLayer {
                    opacity: g.opacity,
                    blend: BlendMode::Normal,
                    casts_shadow: g.shadow,
                });
                open = Some(key);
            }
        }
        match what {
            Drawn::Shape(shape) => {
                shape_count += 1;
                let mut instance =
                    Instance::new(shape.id, g.affine).with_tint(flashed(shape.tint, g.flash));
                if let Some(track) = &shape.morph {
                    instance = instance.with_morph(track.at(time.0));
                }
                items.push(instance.into());
            }
            Drawn::Picture(p) => {
                shape_count += p.drawing.shape_count;
                items.extend(p.drawing.placed(g.affine));
            }
        }
    }
    if open.is_some() {
        items.push(SceneItem::EndLayer);
    }

    let mut lit: Vec<(u32, SceneLight)> = lights
        .iter()
        .map(|(l, g)| (l.order, place_light(&l.light, g)))
        .collect();
    lit.sort_by_key(|(o, _)| *o);
    let mut sdf: Vec<(u32, SceneObject)> = objects
        .iter()
        .map(|(o, g)| (o.order, place_object(&o.object, g)))
        .collect();
    sdf.sort_by_key(|(o, _)| *o);

    snapshot.0 = Scene {
        background: settings.background,
        ambient: settings.ambient,
        shapes: shape_count,
        items,
        lights: lit.into_iter().map(|(_, l)| l).collect(),
        objects: sdf.into_iter().map(|(_, o)| o).collect(),
        entities: 0,
        player: players.iter().next().map(|p| PlayerView {
            at: [p.body.pos[0] as f32, p.body.pos[1] as f32],
            vel: [p.body.vel[0] as f32, p.body.vel[1] as f32],
            on_ground: p.body.on_ground,
            hanging: p.body.on_ledge(),
            solids: solids.0.len(),
            striking: p.fighter.striking(),
            swings: p.fighter.swings,
            hits: p.hits,
            anim: if p.anim.attacking() {
                "бьёт"
            } else {
                p.anim.clip().label()
            },
            hp: p.health.hp,
            max_hp: p.health.max,
            invulnerable: p.health.invulnerable > 0.0,
        }),
        input_stamp: press.0,
        shake: if combat.shake_time > 0.0 {
            (combat.shake * combat.shake_left / combat.shake_time) as f32
        } else {
            0.0
        },
        events: events.list.iter().copied().collect(),
        solids: solids.0.clone(),
        scripts: crate::ScriptInfo {
            files: scripts.scripts.len(),
            actors: scripted.iter().count(),
            reloads: scripts.scripts.reloads,
            errors: scripts.scripts.errors(),
            memory: scripts.scripts.memory(),
        },
    };
}

/// Цвет фигуры во вспышке `flash` (от 0 до 1): ближе к яркому белому.
fn flashed(c: Color, flash: f32) -> Color {
    if flash <= 0.0 {
        return c;
    }
    let k = flash.min(1.0);
    let to = |v: f32| v + (1.6 - v) * k;
    Color {
        r: to(c.r),
        g: to(c.g),
        b: to(c.b),
        a: c.a,
    }
}

fn place_light(l: &SceneLight, g: &GlobalPlacement) -> SceneLight {
    let s = g.scale as f32;
    match l.kind {
        LightKind::Local => SceneLight {
            a: g.point(l.a),
            b: g.point(l.b),
            reach: l.reach * s,
            softness: l.softness * s,
            ..*l
        },
        LightKind::Directional => {
            // Направление поворачивается вместе с предметом, сдвиг на него не действует.
            let c = g.affine.as_coeffs();
            let (x, y) = (f64::from(l.a[0]), f64::from(l.a[1]));
            let (dx, dy) = (c[0] * x + c[2] * y, c[1] * x + c[3] * y);
            let len = dx.hypot(dy).max(1e-9);
            SceneLight {
                a: [(dx / len) as f32, (dy / len) as f32],
                ..*l
            }
        }
    }
}

fn place_object(o: &SceneObject, g: &GlobalPlacement) -> SceneObject {
    let s = g.scale as f32;
    let mut out = o.clone();
    for p in &mut out.parts {
        p.a = g.point(p.a);
        p.b = g.point(p.b);
        p.r *= s;
    }
    out.k *= s;
    out.color[3] *= g.opacity;
    out
}
