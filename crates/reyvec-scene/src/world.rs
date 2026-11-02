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

use crate::assets::{self, MorphTrack};
use crate::{
    Background, LightKind, NodeDesc, Scene, SceneError, SceneFile, SceneLight, SceneObject,
    ShapeCache,
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
}

impl GlobalPlacement {
    const ROOT: Self = Self {
        affine: Affine::IDENTITY,
        depth: 0.0,
        scale: 1.0,
        shadow: false,
        opacity: 1.0,
        group: 0,
    };

    fn then(&self, p: &Placement, me: Entity) -> Self {
        let own = p.shadow.is_some() || p.opacity < 1.0;
        Self {
            affine: self.affine
                * Affine::translate((p.at[0], p.at[1]))
                * Affine::rotate(p.angle.to_radians())
                * Affine::scale(p.scale),
            depth: self.depth + p.depth,
            scale: self.scale * p.scale,
            shadow: p.shadow.unwrap_or(self.shadow),
            opacity: self.opacity * p.opacity,
            group: if own { me.to_bits() } else { self.group },
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
        let a = file.ambient.0;
        let mut world = World::new();
        world.insert_resource(Settings {
            background: file.background,
            ambient: [a.r, a.g, a.b],
        });
        world.init_resource::<Snapshot>();
        let mut spawner = Spawner {
            world: &mut world,
            library,
            cache,
            dir: file.dir.as_deref(),
            order: 0,
            entities: 0,
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
        };
        spawner.node(&root, None, "сцена")?;
        let entities = spawner.entities;
        let mut schedule = Schedule::default();
        world.init_resource::<SceneTime>();
        schedule.add_systems((animate, propagate, collect).chain());
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
}

impl Spawner<'_> {
    /// Рисунок SVG: готовый из кэша или прочитанный и нарезанный здесь же.
    fn picture(&mut self, p: &crate::PictureDesc) -> Result<Arc<SvgDrawing>, String> {
        let file = assets::picture_file(self.dir, p)?;
        if let Some(d) = self.cache.svgs.get(&file.key) {
            return Ok(d.clone());
        }
        let d = Arc::new(assets::load_picture(&file, p.height, self.library)?);
        self.cache.svgs.insert(file.key, d.clone());
        Ok(d)
    }

    fn motif(&mut self, m: &crate::MorphDesc) -> Result<Arc<reyvec_motif::MorphFile>, String> {
        let file = assets::motif_file(self.dir, m)?;
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
        for (j, child) in n.children.iter().enumerate() {
            self.node(child, Some(me), &format!("{name} → предмет {}", j + 1))?;
        }
        Ok(())
    }
}

/// Движение: место предметов с [`Motion`] в текущий момент.
fn animate(time: Res<SceneTime>, mut moving: Query<(&Motion, &mut Placement)>) {
    for (motion, mut placement) in &mut moving {
        *placement = motion.at(time.0);
    }
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
                let mut instance = Instance::new(shape.id, g.affine).with_tint(shape.tint);
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
    };
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
