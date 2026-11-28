//! Мир сцены на `bevy_ecs` (этап 5.2, разделён на части в 9.1).
//!
//! Каждый предмет файла — сущность с местом ([`Placement`]); его фигуры, источники света
//! и SDF-объекты — дочерние сущности (связь `ChildOf`), вложенные предметы тоже.
//!
//! Части мира:
//!
//! - `parts` — компоненты предметов, ресурсы шага и постоянные;
//! - `spawn` — сборка мира из файла сцены;
//! - `places` — движение по времени, позы костей, твёрдые прямоугольники, итоговые места;
//! - `play` — герой и враги: контроллер, удар, попадания;
//! - `physics` и `phys` — шаг Rapier, обломки, полигон (сочленения, мягкие тела, мышь);
//! - `scripts` — скрипты Luau и сюжет;
//! - `draw` — снимок для рисования ([`Scene`]).
//!
//! Шаг логики — системы одна за другой (см. [`SceneWorld::new`]): движение → итоговые
//! места → твёрдое → герой и враги → физика → скрипты → сюжет → вид боя → кости →
//! итоговые места ещё раз → снимок. Рисование читает только снимок, а не сам мир
//! (решение 0010).

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
    InputFrame, Intent, Space, Surface, Tuning,
};

use crate::assets::{self, MorphTrack};

mod phys;

mod draw;
mod parts;
mod physics;
mod places;
mod play;
mod scripts;
mod spawn;

use crate::{
    Background, GameEvent, LightKind, NodeDesc, PlayerView, Scene, SceneError, SceneFile,
    SceneLight, SceneObject, ShapeCache, ShapeDesc,
};
use draw::*;
pub use parts::*;
pub use phys::Pointer;
use physics::*;
use places::*;
use play::*;
use scripts::*;
use spawn::*;

/// Мир сцены: сущности из файла и расписание систем.
pub struct SceneWorld {
    world: World,
    schedule: Schedule,
    entities: usize,
    /// Номер мира (см. [`Scene::generation`]).
    generation: u64,
}

/// Счётчик миров: у каждого построенного мира свой номер, начиная с 1.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

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
        world.insert_resource(StoryClock(0.0));
        world.insert_resource(PhysicsRes {
            physics: Physics::new(reyvec_game::Tuning::default().gravity()),
            bodies: Vec::new(),
            surfaces: Vec::new(),
            presses: Vec::new(),
            step_dt: 0.0,
            pushes: Vec::new(),
            dt: 0.0,
            breaks: Vec::new(),
            pieces: Vec::new(),
            seed: 0x5eed,
            extra: phys::Extra::default(),
            knocks: std::collections::HashMap::new(),
        });
        let mut scripts = reyvec_script::Scripts::new();
        // Сюжет проекта (Сю.5): папка `story/` рядом со сценой или выше.
        if let Some(story_dir) = file.dir.as_deref().and_then(find_story_dir) {
            let (story, problems) = reyvec_story::Story::load_dir(&story_dir);
            for p in &problems {
                tracing::warn!("сюжет: {} {}: {}", p.file, p.place, p.text);
            }
            scripts.set_story(story);
        }
        world.insert_resource(ScriptRes {
            scripts,
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
            joints: Vec::new(),
            soft: None,
            repeat: None,
            spawn: None,
        };
        spawner.node(&root, None, "сцена")?;
        let entities = spawner.entities;
        {
            // Сочленения и мягкие тела — когда все тела уже есть.
            let mut res = world.resource_mut::<PhysicsRes>();
            let res = &mut *res;
            res.extra.resolve(&mut res.physics);
            if let Some(e) = res.extra.errors.first() {
                return Err(SceneError::new(e.clone()));
            }
        }
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
                drive_story,
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
            generation: GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
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

    /// Пересобирает снимок в то же время сцены, без шага (после команды консоли на
    /// паузе).
    pub fn refresh(&mut self) {
        let t = self.world.resource::<SceneTime>().0;
        self.update_at(t);
    }

    /// Сюжет сцены (папка `story/` проекта), если он есть: его состояние и начатый
    /// скриптом разговор (`story.talk`).
    pub fn story(&self) -> Option<reyvec_script::SharedStory> {
        self.world.resource::<ScriptRes>().scripts.story().cloned()
    }

    /// Новый мир того же проекта (сцену сохранили или открыли другую): сюжет идёт дальше
    /// с того же места — переменные, задания и начатый разговор берутся из `old`.
    /// Сюжет мог поменяться в файлах: состояние сверяется с ним ([`reyvec_story::Story::restore`]).
    pub fn keep_story_from(&mut self, old: &SceneWorld) {
        let (Some(new), Some(old)) = (self.story(), old.story()) else {
            return;
        };
        if std::sync::Arc::ptr_eq(&new, &old) {
            return;
        }
        let old = reyvec_script::lock_story(&old);
        let mut new = reyvec_script::lock_story(&new);
        match old.save().and_then(|text| new.story.restore(&text)) {
            Ok(state) => new.state = state,
            Err(e) => tracing::warn!("сюжет при смене мира: {e}"),
        }
        if old.busy() {
            new.talk = old.talk.clone();
            new.screen = old.screen.clone();
        }
    }

    /// Снимок для рисования после последнего [`SceneWorld::update`].
    pub fn snapshot(&self) -> Scene {
        let mut scene = self.world.resource::<Snapshot>().0.clone();
        scene.entities = self.entities;
        scene.generation = self.generation;
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

    /// Команда консоли разработчика (6.11): что сделано или почему нельзя.
    pub fn debug(&mut self, command: &DebugCommand) -> Result<String, String> {
        let mut q = self.world.query::<(&mut Player, &mut Placement)>();
        let Some((mut p, mut place)) = q.iter_mut(&mut self.world).next() else {
            return Err("в сцене нет героя".into());
        };
        let text = match *command {
            DebugCommand::Teleport(at) => {
                let facing = p.body.facing;
                p.body = Body::new(at);
                p.body.facing = facing;
                format!("герой в ({:.2}, {:.2})", at[0], at[1])
            }
            DebugCommand::Health(hp) => {
                let hp = hp.clamp(1, p.health.max);
                p.health.hp = hp;
                format!("здоровье {hp} из {}", p.health.max)
            }
            DebugCommand::God(on) => {
                p.god = on.unwrap_or(!p.god);
                if p.god {
                    "бессмертие включено".into()
                } else {
                    "бессмертие выключено".into()
                }
            }
            DebugCommand::Respawn => {
                p.body = Body::new(p.spawn);
                p.health.restore();
                "герой в начале уровня".into()
            }
            DebugCommand::Where => format!(
                "герой в ({:.2}, {:.2}), скорость ({:.2}, {:.2}), начало в ({:.2}, {:.2})",
                p.body.pos[0], p.body.pos[1], p.body.vel[0], p.body.vel[1], p.spawn[0], p.spawn[1]
            ),
        };
        place.at = p.body.pos;
        Ok(text)
    }

    /// Твёрдое сцены на последнем шаге и настройки героя (для проверок).
    #[cfg(test)]
    pub(crate) fn colliders(&mut self) -> (Vec<(Collider, Surface)>, Option<Tuning>) {
        let s = self.world.resource::<Solids>();
        let solids = s.0.iter().copied().zip(s.2.iter().cloned()).collect();
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

    /// Мышь для физики (полигон, 6.13): захват телом и взрыв.
    pub fn set_pointer(&mut self, pointer: Pointer) {
        self.world
            .resource_mut::<PhysicsRes>()
            .extra
            .set_pointer(pointer);
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

/// Папка `story/` проекта: рядом со сценой или в одной из папок выше (сцены обычно лежат
/// в `scenes/`, сюжет — в корне проекта).
fn find_story_dir(scene_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    scene_dir
        .ancestors()
        .take(4)
        .map(|d| d.join("story"))
        .find(|d| d.join("vars.ron").is_file() || d.join("dialogs").is_dir())
}
