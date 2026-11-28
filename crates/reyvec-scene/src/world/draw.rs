//! Снимок мира для рисования: фигуры по глубине, свет и SDF-объекты в координатах сцены.

use super::*;

/// Что рисуется в векторном слое: фигура или рисунок SVG.
pub(super) enum Drawn<'a> {
    Shape(&'a VectorShape),
    Picture(&'a SvgPicture),
    /// Нитки мягких тел и сочленений (6.13): поверх всего на своей глубине.
    Free(&'a phys::FreeItem),
}

impl Drawn<'_> {
    pub(super) fn order(&self) -> u32 {
        match self {
            Self::Shape(s) => s.order,
            Self::Picture(p) => p.order,
            Self::Free(_) => u32::MAX,
        }
    }
}

/// Снимок для рисования.
#[allow(clippy::too_many_arguments)]
pub(super) fn collect(
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
    phys: Res<PhysicsRes>,
    mut snapshot: ResMut<Snapshot>,
) {
    let mut free = Vec::new();
    phys.extra.draw(&phys.physics, &mut free);
    let mut drawn: Vec<(Drawn, &GlobalPlacement)> = shapes
        .iter()
        .map(|(s, g)| (Drawn::Shape(s), g))
        .chain(pictures.iter().map(|(p, g)| (Drawn::Picture(p), g)))
        .chain(free.iter().map(|f| (Drawn::Free(f), &f.place)))
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
    // Подряд идущие движущиеся фигуры: метки ставятся на сменах, независимо от слоёв.
    let mut moving = false;
    for (what, g) in &drawn {
        if g.opacity <= 0.0 {
            // Не видно (взмах вне удара, разбитый манекен): не рисуется и не даёт тени.
            continue;
        }
        let dynamic = g.dynamic || matches!(what, Drawn::Shape(s) if s.morph.is_some());
        if dynamic != moving {
            items.push(if dynamic {
                SceneItem::BeginDynamic
            } else {
                SceneItem::EndDynamic
            });
            moving = dynamic;
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
            Drawn::Free(f) => {
                shape_count += 1;
                items.push(Instance::new(f.id, f.affine).with_tint(f.tint).into());
            }
        }
    }
    if open.is_some() {
        items.push(SceneItem::EndLayer);
    }
    if moving {
        items.push(SceneItem::EndDynamic);
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
    let jellies = phys.extra.jelly_objects(&phys.physics);

    snapshot.0 = Scene {
        // Ставит `SceneWorld::snapshot`.
        generation: 0,
        background: settings.background,
        ambient: settings.ambient,
        shapes: shape_count,
        items,
        lights: lit.into_iter().map(|(_, l)| l).collect(),
        objects: sdf.into_iter().map(|(_, o)| o).chain(jellies).collect(),
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
            size: [p.tuning.size[0] as f32, p.tuning.size[1] as f32],
            god: p.god,
        }),
        input_stamp: press.0,
        shake: if combat.shake_time > 0.0 {
            (combat.shake * combat.shake_left / combat.shake_time) as f32
        } else {
            0.0
        },
        events: events.list.iter().copied().collect(),
        solids: solids.0.clone(),
        solids_dynamic: solids.1.clone(),
        obstacles: {
            let mut out = Vec::new();
            for s in solids.2.iter().chain(&phys.surfaces) {
                reyvec_game::obstacle::obstacles(s, phys.step_dt, &mut out);
            }
            out
        },
        scripts: crate::ScriptInfo {
            files: scripts.scripts.len(),
            actors: scripted.iter().count(),
            reloads: scripts.scripts.reloads,
            errors: scripts.scripts.errors(),
            memory: scripts.scripts.memory(),
        },
        physics: phys.extra.stats,
    };
}

/// Цвет фигуры во вспышке `flash` (от 0 до 1): ближе к яркому белому.
pub(super) fn flashed(c: Color, flash: f32) -> Color {
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

pub(super) fn place_light(l: &SceneLight, g: &GlobalPlacement) -> SceneLight {
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

pub(super) fn place_object(o: &SceneObject, g: &GlobalPlacement) -> SceneObject {
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
