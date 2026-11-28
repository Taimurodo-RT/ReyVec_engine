//! Сборка мира из файла сцены: предмет за предметом, с фигурами, светом, телами и скриптами.

use super::*;

pub(super) struct Spawner<'a> {
    pub(super) world: &'a mut World,
    pub(super) library: &'a mut ShapeLibrary,
    pub(super) cache: &'a mut ShapeCache,
    pub(super) dir: Option<&'a std::path::Path>,
    pub(super) order: u32,
    pub(super) entities: usize,
    /// Строится содержимое героя: внутри него бывают кости.
    pub(super) in_player: bool,
    /// Слои столкновений сцены (9.6а) и слой строящегося предмета (вложенные
    /// наследуют слой родителя).
    pub(super) layers: crate::Layers,
    pub(super) layer: Option<String>,
    /// Сколько зон уже собрано (номер следующей).
    pub(super) zones: u32,
}

impl Spawner<'_> {
    /// Рисунок SVG: готовый из кэша или прочитанный и нарезанный здесь же.
    pub(super) fn picture(&mut self, p: &crate::PictureDesc) -> Result<Arc<SvgDrawing>, String> {
        let file = assets::picture_file(self.dir, p)?;
        self.cache.mark_used(&file.key);
        if let Some(d) = self.cache.svgs.get(&file.key) {
            return Ok(d.clone());
        }
        let d = Arc::new(assets::load_picture(&file, p.height, self.library)?);
        self.cache.svgs.insert(file.key, d.clone());
        Ok(d)
    }

    pub(super) fn motif(
        &mut self,
        m: &crate::MorphDesc,
    ) -> Result<Arc<reyvec_motif::MorphFile>, String> {
        let file = assets::motif_file(self.dir, m)?;
        self.cache.mark_used(&file.key);
        if let Some(f) = self.cache.motifs.get(&file.key) {
            return Ok(f.clone());
        }
        let f = Arc::new(assets::load_motif(&file, self.library)?);
        self.cache.motifs.insert(file.key, f.clone());
        Ok(f)
    }

    /// Слой и маска строящегося предмета; без своего слоя — `default`.
    pub(super) fn layer_of(&self, default: u32) -> (u32, u32) {
        self.layers.of_node(self.layer.as_deref(), default)
    }

    pub(super) fn next(&mut self) -> u32 {
        self.order += 1;
        self.order
    }

    /// Кукла врага (9.6г): части из его фигур и их невидимые копии без родителя.
    fn ragdoll(&mut self, n: &NodeDesc, name: &str) -> Result<Ragdoll, SceneError> {
        let (parts, torso) = ragdoll::parts_of(&n.shapes, n.scale);
        let mut entities = Vec::with_capacity(parts.len());
        for (j, shape) in n.shapes.iter().enumerate() {
            let id = shape.shape_id(self.library, self.cache).map_err(|m| {
                SceneError::new(format!("предмет «{name}», кукла, фигура {}: {m}", j + 1))
            })?;
            let color = shape.placement().2;
            let order = self.next();
            entities.push(self.spawn(
                (
                    Name::new(format!("{name}: кукла {}", j + 1)),
                    Placement {
                        depth: n.depth,
                        shadow: n.shadow,
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
                    Dynamic,
                ),
                None,
            ));
        }
        Ok(Ragdoll {
            parts,
            entities,
            scale: n.scale,
            torso,
        })
    }

    /// Запас обломков (6.10): квадраты, которые ждут, пока кого-нибудь разобьют.
    /// Создаётся один раз, при первом манекене.
    pub(super) fn debris_pool(&mut self) -> Result<(), SceneError> {
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
                    Dynamic,
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

    pub(super) fn spawn(&mut self, bundle: impl Bundle, parent: Option<Entity>) -> Entity {
        self.entities += 1;
        let mut e = self.world.spawn(bundle);
        if let Some(p) = parent {
            e.insert(ChildOf(p));
        }
        e.id()
    }

    /// Отрезок 1×1 и круг диаметром 1 для мягких тел и ниток сочленений.
    pub(super) fn units(&mut self) -> Result<(), SceneError> {
        if self
            .world
            .resource::<PhysicsRes>()
            .extra
            .unit_rect
            .is_some()
        {
            return Ok(());
        }
        let fail = |m: String| SceneError::new(format!("нитки физики: {m}"));
        let rect = ShapeDesc::Rect {
            at: (0.0, 0.0),
            size: (1.0, 1.0),
            angle: 0.0,
            corner: 0.0,
            color: crate::Hex(Color::WHITE),
            stroke: None,
            solid: false,
            oneway: false,
        }
        .shape_id(self.library, self.cache)
        .map_err(fail)?;
        let circle = ShapeDesc::Circle {
            at: (0.0, 0.0),
            r: 0.5,
            color: crate::Hex(Color::WHITE),
            stroke: None,
            solid: false,
            oneway: false,
        }
        .shape_id(self.library, self.cache)
        .map_err(fail)?;
        let mut res = self.world.resource_mut::<PhysicsRes>();
        res.extra.unit_rect = Some(rect);
        res.extra.unit_circle = Some(circle);
        Ok(())
    }

    /// Предмет из файла: с повтором — несколько копий, с выбросом — запас невидимых
    /// копий, которые по очереди становятся новыми телами.
    pub(super) fn node(
        &mut self,
        n: &NodeDesc,
        parent: Option<Entity>,
        path: &str,
    ) -> Result<(), SceneError> {
        let name = if n.name.is_empty() {
            path.to_string()
        } else {
            n.name.clone()
        };
        if n.repeat.is_some() {
            for copy in phys::repeat_copies(n, &name) {
                self.node_one(&copy, parent, path)?;
            }
            return Ok(());
        }
        if let Some(spawn) = n.spawn {
            let mut slots = Vec::with_capacity(spawn.max);
            for k in 0..spawn.max {
                let copy = NodeDesc {
                    name: format!("{name} ~{}", k + 1),
                    body: None,
                    spawn: None,
                    opacity: 0.0,
                    ..n.clone()
                };
                let e = self.node_one(&copy, parent, path)?;
                self.world.entity_mut(e).insert((SpawnSlot, Dynamic));
                slots.push(e);
            }
            let layer = self.layers.of_node(
                n.layer.as_deref().or(self.layer.as_deref()),
                reyvec_game::layer::BODY,
            );
            self.world
                .resource_mut::<PhysicsRes>()
                .extra
                .add_emitter(n, spawn, slots, layer);
            return Ok(());
        }
        self.node_one(n, parent, path).map(|_| ())
    }

    pub(super) fn node_one(
        &mut self,
        n: &NodeDesc,
        parent: Option<Entity>,
        path: &str,
    ) -> Result<Entity, SceneError> {
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
        let outer_layer = self.layer.clone();
        if n.layer.is_some() {
            self.layer = n.layer.clone();
        }
        let name = if n.name.is_empty() {
            path.to_string()
        } else {
            n.name.clone()
        };
        let me = self.spawn(
            (Name::new(name.clone()), placement, GlobalPlacement::ROOT),
            parent,
        );
        if let Some(z) = &n.zone {
            let index = self.zones;
            self.zones += 1;
            self.world.entity_mut(me).insert((
                Zone {
                    half: [z.size.0 * 0.5, z.size.1 * 0.5],
                    field: z.field(),
                    index,
                    inside: Vec::new(),
                },
                Dynamic,
            ));
        }
        if let Some(p) = &n.player {
            let layer = self.layer_of(reyvec_game::layer::HERO);
            self.world.entity_mut(me).insert((
                Player {
                    tuning: p.tuning(),
                    body: Body::new(placement.at),
                    spawn: placement.at,
                    attack: p.attack.tuning(),
                    fighter: Fighter::default(),
                    hits: 0,
                    anim: Animator::default(),
                    health: Health::new(p.hp),
                    god: false,
                    layer,
                },
                Dynamic,
            ));
        }
        if let Some(b) = &n.body {
            let (layer, mask) = self.layer_of(reyvec_game::layer::BODY);
            let mut res = self.world.resource_mut::<PhysicsRes>();
            let id = res.physics.add(
                placement.at,
                n.angle,
                [0.0, 0.0],
                &reyvec_game::physics::BodyDesc {
                    layer,
                    mask,
                    ..b.physics()
                },
            );
            res.bodies.push(id);
            res.extra.add_body(&name, id);
            res.extra.add_joints(n, id);
            self.world.entity_mut(me).insert((PhysBody { id }, Dynamic));
            if n.joints.iter().any(|j| j.line.is_some()) {
                self.units()?;
            }
        }
        if let Some(soft) = &n.soft {
            self.world
                .resource_mut::<PhysicsRes>()
                .extra
                .add_soft(n, soft);
            self.units()?;
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
                self.world.entity_mut(me).insert((
                    Scripted {
                        actor,
                        touch: [touch.0, touch.1],
                        opacity: n.opacity,
                        collected: false,
                    },
                    Dynamic,
                ));
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
            let layer = self.layer_of(reyvec_game::layer::ENEMY);
            self.world.entity_mut(me).insert((
                Target {
                    dummy: Dummy::new(placement.at, [d.size.0, d.size.1], d.hp, d.respawn),
                    opacity: n.opacity,
                    color,
                    contact: d.contact,
                    layer,
                    ragdoll: None,
                },
                Dynamic,
            ));
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
                    Solid::rect([s.size.0 * 0.5, s.size.1 * 0.5], s.oneway, 0.0),
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
                        Solid::rect(s.half, s.oneway, s.slope),
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
            self.world.entity_mut(me).insert((
                Motion {
                    base: placement,
                    spin: n.spin,
                    swing: n.swing.map(|w| (w.angle, w.period)),
                    bob: n.bob.map(|b| ([b.by.0, b.by.1], b.period)),
                },
                Dynamic,
            ));
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
            let (solid, oneway) = shape.solid_flags();
            if solid || oneway {
                let mut s = match shape {
                    // Прямоугольник без скругления — как раньше (описанный, если не
                    // повёрнут, иначе настоящей формой).
                    ShapeDesc::Rect {
                        size, corner: 0.0, ..
                    } => Solid::rect([size.0 * 0.5, size.1 * 0.5], oneway, 0.0),
                    _ => {
                        let mut s = Solid::rect([0.0, 0.0], oneway, 0.0);
                        s.form = Some(Arc::new(shape.form()));
                        s
                    }
                };
                s.oneway = oneway;
                self.world.entity_mut(e).insert(s);
            }
        }
        // Кукла (9.6г) — после фигур: формы уже нарезаны, копии их только берут (номера
        // форм те же, что при подготовке сцены в фоне).
        if let Some(d) = &n.dummy
            && d.fall == crate::Fall::Ragdoll
        {
            let doll = self.ragdoll(n, &name)?;
            if let Some(mut t) = self.world.get_mut::<Target>(me) {
                t.ragdoll = Some(doll);
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
            let e = self.spawn(
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
            if object.solid {
                // Форма столкновения — сами круги и капсулы (без сглаживания стыков
                // `blend`: оно добавляет миллиметры, герою их не видно).
                let mut s = Solid::rect([0.0, 0.0], false, 0.0);
                s.form = Some(Arc::new(object.form()));
                self.world.entity_mut(e).insert(s);
            }
        }
        // Твёрдое этого предмета — в его слое.
        let level = self.layer_of(reyvec_game::layer::LEVEL);
        let mut q = self.world.query::<&mut Solid>();
        for mut s in q.iter_mut(self.world) {
            if s.layer.is_none() {
                s.layer = Some(level);
            }
        }
        let was = self.in_player;
        self.in_player |= n.player.is_some();
        for (j, child) in n.children.iter().enumerate() {
            self.node(child, Some(me), &format!("{name} → предмет {}", j + 1))?;
        }
        self.in_player = was;
        self.layer = outer_layer;
        Ok(me)
    }
}
