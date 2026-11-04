//! Проверки физики.

use super::*;

const DT: f64 = 1.0 / 60.0;

fn floor() -> (u64, Collider) {
    (
        1,
        Collider::solid(Aabb {
            min: [-5.0, -1.0],
            max: [5.0, -0.8],
        }),
    )
}

#[test]
fn box_falls_lands_and_rests_on_the_floor() {
    let mut w = Physics::new(9.3);
    let b = w.add([0.0, 0.0], 0.0, [0.0, 0.0], &BodyDesc::default());
    for _ in 0..180 {
        w.set_solids([floor()]);
        w.step(DT);
    }
    let (at, angle) = w.pose(b).unwrap();
    assert!((at[1] - (-0.8 + 0.075)).abs() < 0.01, "{at:?}");
    assert!(angle.abs() < 1.0 && w.velocity(b).unwrap()[1].abs() < 0.01);
}

#[test]
fn hero_pushes_a_box_and_can_stand_on_it() {
    let mut w = Physics::new(9.3);
    let b = w.add([0.3, -0.725], 0.0, [0.0, 0.0], &BodyDesc::default());
    let hero_half = [0.07, 0.14];
    // Герой идёт вправо и упирается в ящик: ящик едет.
    let mut x = 0.0;
    for _ in 0..90 {
        x += 1.0 * DT;
        let hero = Collider::solid(Aabb::around([x, -0.66], hero_half));
        w.set_solids([floor(), (2, hero)]);
        w.step(DT);
    }
    let (at, _) = w.pose(b).unwrap();
    assert!(
        at[0] > x + 0.07 + 0.07,
        "ящик впереди героя: {at:?}, герой {x}"
    );
    assert!(at[0] > 0.6, "ящик сдвинулся: {at:?}");
    // Для героя ящик — платформа сверху.
    let plat = w.platforms(&[b], 100);
    assert!(
        plat[0].oneway && (plat[0].aabb.max[1] - -0.65).abs() < 0.01,
        "{plat:?}"
    );
    // Героя больше нет — его тело убрано, ящик стоит.
    w.set_solids([floor()]);
    w.step(DT);
    assert_eq!(w.movers.len(), 1);
}

#[test]
fn kicked_debris_flies_spins_and_does_not_pass_walls() {
    let mut w = Physics::new(9.3);
    let d = BodyDesc {
        size: [0.04, 0.04],
        ..BodyDesc::default()
    };
    let wall = (
        3,
        Collider::solid(Aabb {
            min: [1.0, -0.8],
            max: [1.02, 1.0],
        }),
    );
    let bits: Vec<BodyId> = (0..10)
        .map(|i| w.add([0.0, -0.5 + 0.05 * i as f64], 0.0, [0.0, 0.0], &d))
        .collect();
    for (i, b) in bits.iter().enumerate() {
        w.kick(*b, [12.0, 1.0], 20.0 + i as f64);
    }
    for _ in 0..120 {
        w.set_solids([floor(), wall]);
        w.step(DT);
    }
    for b in &bits {
        let (at, _) = w.pose(*b).unwrap();
        assert!(at[0] < 1.0 && at[1] > -0.81, "сквозь стену или пол: {at:?}");
    }
    w.remove(bits[0]);
    assert_eq!(w.len(), 9);
}

#[test]
fn slope_rolls_a_ball_down() {
    let mut w = Physics::new(9.3);
    let ball = w.add(
        [0.3, 0.0],
        0.0,
        [0.0, 0.0],
        &BodyDesc {
            size: [0.1, 0.1],
            shape: BodyShape::Ball,
            ..BodyDesc::default()
        },
    );
    let slope = Collider::slope(
        Aabb {
            min: [0.0, -0.8],
            max: [0.5, -0.3],
        },
        0.5,
    );
    for _ in 0..90 {
        w.set_solids([floor(), (4, slope)]);
        w.step(DT);
    }
    let (at, _) = w.pose(ball).unwrap();
    assert!(at[0] < 0.0, "мяч скатился влево: {at:?}");
}

/// Пол на шаге: уровень приходит в физику каждый шаг, как в сцене.
fn step_with_floor(w: &mut Physics, steps: usize) {
    for _ in 0..steps {
        w.set_solids([floor()]);
        w.step(DT);
    }
}

fn boxy(size: f64) -> BodyDesc {
    BodyDesc {
        size: [size, size],
        ..BodyDesc::default()
    }
}

#[test]
fn pyramid_of_twenty_rows_stands() {
    // 210 ящиков по 0,07: так стоит пирамида полигона. За первую секунду стопка
    // проседает на 3 см (контакты Rapier чуть мягкие: по 1,5 мм на слой), дальше стоит
    // намертво и засыпает: верхний ящик не двигается, не съезжает и не наклоняется.
    let mut w = Physics::new(9.3);
    let s = 0.07;
    let mut top = None;
    for row in 0..20 {
        for i in 0..(20 - row) {
            let x = (i as f64 - (19 - row) as f64 * 0.5) * s;
            let y = -0.8 + s * 0.5 + row as f64 * s;
            top = Some((w.add([x, y], 0.0, [0.0, 0.0], &boxy(s)), [x, y]));
        }
    }
    let (id, was) = top.unwrap();
    step_with_floor(&mut w, 60);
    let (settled, _) = w.pose(id).unwrap();
    step_with_floor(&mut w, 240);
    let (at, angle) = w.pose(id).unwrap();
    assert!(
        (at[0] - was[0]).abs() < 0.01 && (was[1] - at[1]) < 0.04 && angle.abs() < 1.0,
        "верх пирамиды сдвинулся: был {was:?}, стал {at:?}, угол {angle}"
    );
    assert!(
        (at[0] - settled[0]).abs() < 1e-3 && (at[1] - settled[1]).abs() < 1e-3,
        "пирамида ползёт: {settled:?} → {at:?}"
    );
    assert!(w.stats().sleeping > 150, "{:?}", w.stats());
}

#[test]
fn pendulum_on_hinge_swings_and_keeps_its_length() {
    let mut w = Physics::new(9.3);
    let bob = w.add([0.5, 0.0], 0.0, [0.0, 0.0], &boxy(0.05));
    let rod = JointDesc::new(JointKind::Rope { length: 0.5 }, [0.5, 0.0]);
    w.join(bob, None, &JointDesc {
        to_at: Some([0.0, 0.0]),
        ..rod
    })
    .unwrap();
    let mut lowest: f64 = 0.0;
    let mut crossed = false;
    for _ in 0..120 {
        w.step(DT);
        let (at, _) = w.pose(bob).unwrap();
        lowest = lowest.min(at[1]);
        crossed |= at[0] < 0.0;
        let len = (at[0] * at[0] + at[1] * at[1]).sqrt();
        assert!(len < 0.52, "верёвка растянулась: {len}");
    }
    assert!(lowest < -0.45 && crossed, "маятник качнулся: {lowest}, {crossed}");
}

#[test]
fn weld_holds_and_motor_turns_a_wheel() {
    let mut w = Physics::new(9.3);
    // Сварка: два ящика висят, сваренные с миром, и не падают.
    let a = w.add([0.0, 0.5], 0.0, [0.0, 0.0], &boxy(0.1));
    w.join(a, None, &JointDesc::new(JointKind::Weld, [0.0, 0.5]))
        .unwrap();
    // Колесо на шарнире с мотором, который через 0,5 с крутит обратно.
    let wheel = w.add(
        [1.0, 0.0],
        0.0,
        [0.0, 0.0],
        &BodyDesc {
            shape: BodyShape::Ball,
            size: [0.2, 0.2],
            ..BodyDesc::default()
        },
    );
    let motor = Motor {
        speed: 360.0,
        force: 10.0,
        flip: 0.5,
    };
    w.join(wheel, None, &JointDesc {
        motor: Some(motor),
        ..JointDesc::new(JointKind::Hinge { limits: None }, [1.0, 0.0])
    })
    .unwrap();
    let mut angles = Vec::new();
    for i in 0..60 {
        w.step(DT);
        if i == 20 || i == 55 {
            angles.push(w.pose(wheel).unwrap().1);
        }
    }
    let (at, _) = w.pose(a).unwrap();
    assert!((at[1] - 0.5).abs() < 0.005, "сварка не держит: {at:?}");
    assert!(angles[0] > 100.0, "мотор крутит вперёд: {angles:?}");
    assert!(angles[1] < angles[0] - 50.0, "мотор повернул обратно: {angles:?}");
}

#[test]
fn joint_breaks_under_a_heavy_load() {
    let mut w = Physics::new(9.3);
    let light = w.add([0.0, 0.0], 0.0, [0.0, 0.0], &boxy(0.05));
    let heavy = w.add(
        [0.5, 0.0],
        0.0,
        [0.0, 0.0],
        &BodyDesc {
            density: 400.0,
            ..boxy(0.1)
        },
    );
    let hold = |at| JointDesc {
        breaks: Some(5.0),
        ..JointDesc::new(JointKind::Hinge { limits: None }, at)
    };
    let j1 = w.join(light, None, &hold([0.0, 0.0])).unwrap();
    let j2 = w.join(heavy, None, &hold([0.5, 0.0])).unwrap();
    step_with_floor(&mut w, 30);
    assert!(w.joint_alive(j1), "лёгкий груз держится");
    assert!(!w.joint_alive(j2), "тяжёлый оборвался");
    assert_eq!(w.stats().broken, 1);
    assert_eq!(w.take_breaks().len(), 1);
}

#[test]
fn capsule_and_polygon_land_on_the_floor() {
    let mut w = Physics::new(9.3);
    let cap = w.add(
        [0.0, 0.0],
        30.0,
        [0.0, 0.0],
        &BodyDesc {
            size: [0.06, 0.2],
            shape: BodyShape::Capsule,
            ..BodyDesc::default()
        },
    );
    let tri = w.add(
        [0.5, 0.0],
        0.0,
        [0.0, 0.0],
        &BodyDesc {
            shape: BodyShape::Polygon(vec![[-0.1, -0.06], [0.1, -0.06], [0.0, 0.1], [0.0, 0.0]]),
            ..BodyDesc::default()
        },
    );
    step_with_floor(&mut w, 240);
    let (c, _) = w.pose(cap).unwrap();
    let (t, ta) = w.pose(tri).unwrap();
    // Капсула падает набок (низ у пола), треугольник стоит на стороне.
    assert!(c[1] < -0.73 && c[1] > -0.78, "капсула: {c:?}");
    assert!((t[1] - (-0.8 + 0.06)).abs() < 0.02 && ta.abs() < 2.0, "треугольник: {t:?} {ta}");
}

#[test]
fn rope_hangs_a_weight_and_jelly_rests_on_the_floor() {
    let mut w = Physics::new(9.3);
    let weight = w.add(
        [0.0, -0.3],
        0.0,
        [0.0, 0.0],
        &BodyDesc {
            shape: BodyShape::Ball,
            size: [0.08, 0.08],
            density: 20.0,
            ..BodyDesc::default()
        },
    );
    let rope = w.add_soft(&SoftDesc {
        pin: Pin::Start,
        attach_end: Some(weight),
        stiffness: 60.0,
        damping: 1.0,
        ..SoftDesc::new(SoftKind::Rope {
            from: [0.0, 0.5],
            to: [0.0, -0.26],
            count: 20,
        })
    });
    let jelly = w.add_soft(&SoftDesc {
        style: 3,
        ..SoftDesc::new(SoftKind::Jelly {
            center: [1.0, 0.0],
            radius: 0.12,
            count: 16,
        })
    });
    step_with_floor(&mut w, 240);
    let (at, _) = w.pose(weight).unwrap();
    assert!(at[1] > -0.4 && at[1] < -0.2, "груз висит на верёвке: {at:?}");
    let jc = w.soft_center(jelly).unwrap();
    assert!(jc[1] > -0.8 && jc[1] < -0.62, "желе лежит на полу: {jc:?}");
    let views = w.soft_views();
    assert_eq!(views.len(), 2);
    let j = views.iter().find(|v| v.style == 3).unwrap();
    assert!(j.closed && j.points.len() == 16 && j.segments.len() == 16);
    assert!(w.soft_center(rope).is_some());
}

#[test]
fn net_tears_under_a_heavy_ball() {
    let mut w = Physics::new(9.3);
    let net = w.add_soft(&SoftDesc {
        pin: Pin::Sides,
        tear: Some(0.4),
        mass: 0.05,
        stiffness: 40.0,
        ..SoftDesc::new(SoftKind::Net {
            center: [0.0, 0.0],
            size: [0.8, 0.08],
            cols: 21,
            rows: 3,
        })
    });
    w.add(
        [0.0, 0.4],
        0.0,
        [0.0, -2.0],
        &BodyDesc {
            shape: BodyShape::Ball,
            size: [0.16, 0.16],
            density: 300.0,
            ..BodyDesc::default()
        },
    );
    step_with_floor(&mut w, 120);
    assert!(w.soft_pieces(net) > 1, "сетка порвалась");
}

#[test]
fn mouse_drags_a_box_and_blast_throws_it() {
    let mut w = Physics::new(9.3);
    let b = w.add([0.0, -0.725], 0.0, [0.0, 0.0], &boxy(0.15));
    step_with_floor(&mut w, 10);
    assert!(!w.grab([1.0, 1.0]), "мимо");
    assert!(w.grab([0.0, -0.7]));
    for _ in 0..60 {
        w.drag([0.5, 0.0]);
        w.set_solids([floor()]);
        w.step(DT);
    }
    let (at, _) = w.pose(b).unwrap();
    assert!((at[0] - 0.5).abs() < 0.1 && (at[1] - 0.0).abs() < 0.12, "тянется к мыши: {at:?}");
    assert!(w.grip_line().is_some());
    w.release();
    assert!(w.grip_line().is_none());
    w.blast([0.3, 0.0], 0.5, 4.0);
    let vel = w.velocity(b).unwrap();
    assert!(vel[0] > 1.0, "взрыв толкнул вправо: {vel:?}");
}
