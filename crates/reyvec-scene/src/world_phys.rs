//! Полигон физики в мире сцены (6.13): сочленения и мягкие тела из файла, выброс тел,
//! мышь (захват и взрыв), рисование мягких тел и ниток сочленений, счётчики.

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use reyvec_game::physics::{BodyId, JointDesc, JointId, Physics, PhysicsStats, SoftDesc, SoftView};
use reyvec_vector::{Affine, Color, ShapeId};

use super::{GlobalPlacement, SlotPlaces};
use crate::{
    GameEvent, Hex, JointOn, JointSpec, JointTo, LineDesc, NodeDesc, SceneObject, ScenePart,
    SoftSpec, SpawnDesc,
};

/// Мышь в мире сцены: где (в координатах сцены), нажата ли кнопка захвата и нажат ли
/// взрыв (срабатывает один раз на следующем шаге).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pointer {
    pub at: [f64; 2],
    pub held: bool,
    pub blast: bool,
}

/// Как рисовать мягкое тело.
pub(super) struct SoftStyle {
    color: Color,
    width: f64,
    depth: f64,
}

struct PendingJoint {
    body: BodyId,
    spec: JointSpec,
    /// Место и поворот предмета (точки сочленения — от них).
    at: [f64; 2],
    angle: f64,
    scale: f64,
}

struct PendingSoft {
    spec: SoftSpec,
    at: [f64; 2],
    style: u32,
}

/// Место для выброшенного тела: заранее созданная невидимая копия образца.
struct Slot {
    entity: Entity,
    body: Option<BodyId>,
    left: f64,
}

struct Emitter {
    spawn: SpawnDesc,
    body: reyvec_game::physics::BodyDesc,
    at: [f64; 2],
    angle: f64,
    slots: Vec<Slot>,
    timer: f64,
}

/// Всё полигонное в физике сцены.
#[derive(Default)]
pub(super) struct Extra {
    /// Тела по имени предмета: для сочленений и привязки верёвок.
    names: HashMap<String, BodyId>,
    joints: Vec<PendingJoint>,
    softs: Vec<PendingSoft>,
    styles: Vec<SoftStyle>,
    lines: Vec<(JointId, LineDesc)>,
    emitters: Vec<Emitter>,
    pointer: Pointer,
    prev_held: bool,
    pub(super) unit_rect: Option<ShapeId>,
    pub(super) unit_circle: Option<ShapeId>,
    pub(super) stats: Option<PhysicsStats>,
    seed: u64,
    /// Ошибки сочленений, найденные при сборке (нет предмета с таким именем).
    pub(super) errors: Vec<String>,
}

/// Радиус взрыва и скорость в его середине.
const BLAST_RADIUS: f64 = 0.6;
const BLAST_SPEED: f64 = 6.0;

impl Extra {
    pub(super) fn add_body(&mut self, name: &str, id: BodyId) {
        self.names.insert(name.to_string(), id);
    }

    pub(super) fn add_joints(&mut self, n: &NodeDesc, body: BodyId) {
        for spec in &n.joints {
            self.joints.push(PendingJoint {
                body,
                spec: spec.clone(),
                at: [n.at.0, n.at.1],
                angle: n.angle,
                scale: n.scale,
            });
        }
    }

    pub(super) fn add_soft(&mut self, n: &NodeDesc, spec: &SoftSpec) {
        let style = self.styles.len() as u32;
        self.styles.push(SoftStyle {
            color: spec.color.0,
            width: spec.width,
            depth: n.depth,
        });
        self.softs.push(PendingSoft {
            spec: spec.clone(),
            at: [n.at.0, n.at.1],
            style,
        });
    }

    pub(super) fn add_emitter(&mut self, n: &NodeDesc, spawn: SpawnDesc, slots: Vec<Entity>) {
        let Some(body) = &n.body else {
            return;
        };
        self.emitters.push(Emitter {
            spawn,
            body: reyvec_game::physics::BodyDesc {
                bullet: true,
                ..body.physics()
            },
            at: [n.at.0, n.at.1],
            angle: n.angle,
            slots: slots
                .into_iter()
                .map(|entity| Slot {
                    entity,
                    body: None,
                    left: 0.0,
                })
                .collect(),
            // Первое тело — через `every` секунд, когда всё в сцене уже на местах.
            timer: spawn.every,
        });
    }

    pub(super) fn has_work(&self) -> bool {
        !self.emitters.is_empty()
    }

    pub(super) fn set_pointer(&mut self, p: Pointer) {
        let blast = self.pointer.blast || p.blast;
        self.pointer = Pointer { blast, ..p };
    }

    /// Сочленения и мягкие тела — после всех предметов: им нужны тела по именам.
    pub(super) fn resolve(&mut self, physics: &mut Physics) {
        for j in std::mem::take(&mut self.joints) {
            let at = local_to_scene(j.at, j.angle, j.scale, j.spec.at);
            let (target, to) = match &j.spec.to {
                JointTo::World | JointTo::Prev => (
                    None,
                    j.spec
                        .to_at
                        .map_or(at, |p| local_to_scene(j.at, j.angle, j.scale, p)),
                ),
                JointTo::Node(name) => {
                    let Some(&other) = self.names.get(name) else {
                        self.errors
                            .push(format!("сочленение: нет предмета с body по имени «{name}»"));
                        continue;
                    };
                    let (oat, oangle) = physics.pose(other).unwrap_or(([0.0, 0.0], 0.0));
                    let local = j.spec.to_at.unwrap_or_else(|| {
                        // Та же точка сцены, что и `at`, в координатах другого тела.
                        let (s, c) = (-oangle.to_radians()).sin_cos();
                        let d = (at[0] - oat[0], at[1] - oat[1]);
                        (d.0 * c - d.1 * s, d.0 * s + d.1 * c)
                    });
                    (Some(other), local_to_scene(oat, oangle, 1.0, local))
                }
            };
            let start = ((to[0] - at[0]).powi(2) + (to[1] - at[1]).powi(2)).sqrt();
            let desc = JointDesc {
                to_at: Some(to),
                motor: j.spec.motor(),
                breaks: j.spec.breaks,
                collide: j.spec.collide,
                ..JointDesc::new(j.spec.kind(start), at)
            };
            if let Some(id) = physics.join(j.body, target, &desc)
                && let Some(line) = j.spec.line
            {
                self.lines.push((id, line));
            }
        }
        for s in std::mem::take(&mut self.softs) {
            let attach = |name: &Option<String>, errors: &mut Vec<String>| {
                let name = name.as_ref()?;
                let found = self.names.get(name).copied();
                if found.is_none() {
                    errors.push(format!(
                        "мягкое тело: нет предмета с body по имени «{name}»"
                    ));
                }
                found
            };
            let mut errors = Vec::new();
            let desc = SoftDesc {
                stiffness: s.spec.stiffness,
                damping: s.spec.damping,
                mass: s.spec.mass,
                pin: s.spec.pin(),
                tear: s.spec.tear,
                attach_start: attach(&s.spec.attach_start, &mut errors),
                attach_end: attach(&s.spec.attach_end, &mut errors),
                style: s.style,
                ..SoftDesc::new(s.spec.kind(s.at))
            };
            self.errors.extend(errors);
            physics.add_soft(&desc);
        }
    }

    /// До шага физики: мышь и выброс новых тел.
    pub(super) fn before_step(&mut self, physics: &mut Physics, dt: f64, shapes: &mut SlotPlaces) {
        let p = self.pointer;
        if p.held && !self.prev_held {
            physics.grab(p.at);
        } else if p.held {
            physics.drag(p.at);
        } else if self.prev_held {
            physics.release();
        }
        self.prev_held = p.held;
        if p.blast {
            self.pointer.blast = false;
            physics.blast(p.at, BLAST_RADIUS, BLAST_SPEED);
        }
        for e in &mut self.emitters {
            e.timer -= dt;
            if e.timer > 0.0 {
                continue;
            }
            e.timer += e.spawn.every;
            // Свободное место или то тело, которому меньше всех осталось жить.
            let Some(slot) = e.slots.iter_mut().min_by(|a, b| a.left.total_cmp(&b.left)) else {
                continue;
            };
            if let Some(old) = slot.body.take() {
                physics.remove(old);
            }
            self.seed = self
                .seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let r = |k: u32| ((self.seed >> (16 + 12 * k)) & 0xfff) as f64 / 4095.0 - 0.5;
            let at = [e.at[0] + r(0) * e.spawn.width, e.at[1]];
            let vel = [
                e.spawn.speed.0 + r(1) * e.spawn.spread * 2.0,
                e.spawn.speed.1 + r(2) * e.spawn.spread * 2.0,
            ];
            let id = physics.add(at, e.angle + r(3) * 40.0, vel, &e.body);
            slot.body = Some(id);
            slot.left = e.spawn.life;
            if let Ok(mut place) = shapes.get_mut(slot.entity) {
                place.at = at;
                place.opacity = 1.0;
            }
        }
    }

    /// После шага: места выброшенных тел, конец их жизни, порванные сочленения.
    pub(super) fn after_step(
        &mut self,
        physics: &mut Physics,
        dt: f64,
        step: u64,
        events: &mut std::collections::VecDeque<(u64, GameEvent)>,
        shapes: &mut SlotPlaces,
    ) {
        for e in &mut self.emitters {
            for slot in &mut e.slots {
                let Some(id) = slot.body else {
                    continue;
                };
                slot.left -= dt;
                let Ok(mut place) = shapes.get_mut(slot.entity) else {
                    continue;
                };
                if slot.left <= 0.0 {
                    physics.remove(id);
                    slot.body = None;
                    slot.left = 0.0;
                    place.opacity = 0.0;
                    continue;
                }
                if let Some((at, angle)) = physics.pose(id) {
                    place.at = at;
                    place.angle = angle;
                    place.opacity = (slot.left / 0.5).min(1.0) as f32;
                }
            }
        }
        for at in physics.take_breaks() {
            events.push_back((
                step,
                GameEvent::Hit {
                    at: [at[0] as f32, at[1] as f32],
                    dir: 0.0,
                },
            ));
        }
        self.stats = physics.has_work().then(|| physics.stats());
    }

    /// Что рисовать поверх обычных фигур: нити и бусины мягких тел, нитки сочленений,
    /// нитка захвата мышью. Каждый отрезок — растянутый квадрат 1×1, каждая бусина —
    /// круг диаметром 1: форма одна на всё, видеокарта только переставляет её.
    pub(super) fn draw(&self, physics: &Physics, out: &mut Vec<FreeItem>) {
        let (Some(rect), Some(circle)) = (self.unit_rect, self.unit_circle) else {
            return;
        };
        let views = physics.soft_views();
        for view in &views {
            let Some(style) = self.styles.get(view.style as usize) else {
                continue;
            };
            if view.closed {
                // Желе рисуется SDF-объектом (`objects`), здесь только его контур.
                continue;
            }
            let place = free_place(style.depth);
            for s in &view.segments {
                let (a, b) = (view.points[s[0] as usize], view.points[s[1] as usize]);
                out.push(segment(rect, style.color, a, b, style.width, place));
            }
            for q in &view.points {
                out.push(dot(circle, style.color, *q, style.width, place));
            }
        }
        for (id, line) in &self.lines {
            if let Some((a, b)) = physics.joint_ends(*id) {
                let place = free_place(1.5);
                out.push(segment(rect, line.color.0, a, b, line.width, place));
                out.push(dot(circle, line.color.0, a, line.width * 1.6, place));
                out.push(dot(circle, line.color.0, b, line.width * 1.6, place));
            }
        }
        if let Some((a, b)) = physics.grip_line() {
            let place = free_place(10.0);
            let c = Hex::parse("#fff3c4cc").map_or(Color::WHITE, |h| h.0);
            out.push(segment(rect, c, a, b, 0.006, place));
            out.push(dot(circle, c, a, 0.022, place));
            out.push(dot(circle, c, b, 0.012, place));
        }
    }

    /// Желе — SDF-объектами: «спицы» от середины к частицам края, слитые плавно, дают
    /// сплошную каплю, которая мнётся вместе с телом.
    pub(super) fn jelly_objects(&self, physics: &Physics) -> Vec<SceneObject> {
        let mut out = Vec::new();
        for view in physics.soft_views() {
            if !view.closed || view.points.is_empty() {
                continue;
            }
            let Some(style) = self.styles.get(view.style as usize) else {
                continue;
            };
            out.push(jelly(&view, style));
        }
        out
    }
}

fn jelly(view: &SoftView, style: &SoftStyle) -> SceneObject {
    let n = view.points.len() as f64;
    let c = view
        .points
        .iter()
        .fold([0.0, 0.0], |s, q| [s[0] + q[0] / n, s[1] + q[1] / n]);
    let center = [c[0] as f32, c[1] as f32];
    let r = (view.radius * 1.2) as f32;
    let parts = view
        .points
        .iter()
        .map(|q| ScenePart {
            a: center,
            b: [q[0] as f32, q[1] as f32],
            r,
        })
        .collect();
    let col = style.color;
    SceneObject {
        parts,
        k: (view.radius * 1.5) as f32,
        color: [col.r, col.g, col.b, col.a],
        glow: false,
        shadow: true,
    }
}

/// Отрезок или бусина, нарисованная не фигурой предмета, а своим местом.
pub(super) struct FreeItem {
    pub id: ShapeId,
    pub tint: Color,
    pub affine: Affine,
    pub place: GlobalPlacement,
}

fn free_place(depth: f64) -> GlobalPlacement {
    GlobalPlacement {
        depth,
        // Своя группа: без тени и прозрачности слой не нужен.
        group: u64::MAX,
        ..GlobalPlacement::ROOT
    }
}

fn segment(
    id: ShapeId,
    tint: Color,
    a: [f64; 2],
    b: [f64; 2],
    width: f64,
    place: GlobalPlacement,
) -> FreeItem {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
    let affine = Affine::translate(((a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5))
        * Affine::rotate(dy.atan2(dx))
        * Affine::scale_non_uniform(len, width);
    FreeItem {
        id,
        tint,
        affine,
        place,
    }
}

fn dot(id: ShapeId, tint: Color, at: [f64; 2], d: f64, place: GlobalPlacement) -> FreeItem {
    FreeItem {
        id,
        tint,
        affine: Affine::translate((at[0], at[1])) * Affine::scale(d),
        place,
    }
}

/// Точка `local` предмета в месте `at`, повёрнутого на `angle` градусов, размера `scale`.
fn local_to_scene(at: [f64; 2], angle: f64, scale: f64, local: (f64, f64)) -> [f64; 2] {
    let (s, c) = angle.to_radians().sin_cos();
    let (x, y) = (local.0 * scale, local.1 * scale);
    [at[0] + x * c - y * s, at[1] + x * s + y * c]
}

/// Копии повтора: у каждой своё место, имя, цвет и сочленения (`Prev` — к предыдущей
/// копии, у первой — к миру; `on` — каким копиям).
pub(super) fn repeat_copies(n: &NodeDesc, name: &str) -> Vec<NodeDesc> {
    let Some(r) = &n.repeat else {
        return Vec::new();
    };
    let mut out: Vec<NodeDesc> = Vec::new();
    for copy in r.copies(n.at, name) {
        let mut c = n.clone();
        c.repeat = None;
        c.at = copy.at;
        c.name = copy.name.clone();
        if let (Some(col), Some(first)) = (copy.color, c.shapes.first_mut()) {
            first.set_color(col);
        }
        let prev = (!copy.first)
            .then(|| out.last().map(|p| p.name.clone()))
            .flatten();
        c.joints = n
            .joints
            .iter()
            .filter(|j| match j.on {
                JointOn::All => true,
                JointOn::First => copy.first,
                JointOn::Last => copy.last,
            })
            .map(|j| {
                let mut j = j.clone();
                if j.to == JointTo::Prev {
                    j.to = prev.clone().map_or(JointTo::World, JointTo::Node);
                    // К предыдущей копии точка `to_at` — в её координатах; к миру —
                    // «та же точка сцены».
                    if j.to == JointTo::World {
                        j.to_at = None;
                    }
                }
                j
            })
            .collect();
        let _ = copy.index;
        out.push(c);
    }
    out
}
