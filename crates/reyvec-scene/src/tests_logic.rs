//! Проверки логики сцены (этап 9.8–9.11) на комнате логики `scenes/logic-room.ron`.

use super::*;

fn room() -> SceneWorld {
    let path = std::path::Path::new(EXAMPLE_DIR).join("logic-room.ron");
    let text = std::fs::read_to_string(&path).unwrap();
    let file = SceneFile::parse_in(&text, Some(std::path::Path::new(EXAMPLE_DIR))).unwrap();
    SceneWorld::new(&file, &mut ShapeLibrary::new(0.001), &mut ShapeCache::new()).unwrap()
}

fn run(w: &mut SceneWorld, from: f64, seconds: f64) -> f64 {
    let steps = (seconds * 60.0).round() as usize;
    let mut t = from;
    for _ in 0..steps {
        t += 1.0 / 60.0;
        w.update_at(t);
    }
    t
}

fn place(w: &mut SceneWorld, name: &str) -> Placement {
    let e = w.find(name).unwrap_or_else(|| panic!("нет «{name}»"));
    *w.world_mut().get::<Placement>(e).unwrap()
}

fn hp(w: &mut SceneWorld) -> u32 {
    w.snapshot().player.unwrap().hp
}

/// Ставит героя стоять в `x` на полу.
fn stand(w: &mut SceneWorld, x: f64, facing: f64) {
    let mut b = reyvec_game::Body::new([x, -0.64]);
    b.facing = facing;
    w.set_player_body(b);
}

#[test]
fn props_and_tags_come_from_the_file() {
    let mut w = room();
    let spikes = w.props("шипы").unwrap();
    assert_eq!(spikes.get("damage"), Some(&Prop::Number(2.0)));
    assert!(spikes.has_tag("опасно"));
    assert_eq!(w.tagged("опасно"), ["шипы", "жаровня"]);
    assert!(w.props("герой").is_none());
}

#[test]
fn spikes_and_burner_hurt_the_hero() {
    let mut w = room();
    let mut t = run(&mut w, 0.0, 0.3);
    let full = hp(&mut w);
    stand(&mut w, 0.8, 1.0);
    t = run(&mut w, t, 0.1);
    assert_eq!(hp(&mut w), full - 2, "шипы ранят на 2");
    // Неуязвимость прошла; горящая жаровня ранит на 1.
    stand(&mut w, -0.3, 1.0);
    t = run(&mut w, t, 2.0);
    let before = hp(&mut w);
    stand(&mut w, 1.6, 1.0);
    run(&mut w, t, 0.1);
    assert_eq!(hp(&mut w), before - 1, "жаровня ранит на 1");
    // Жаровня мерцает.
    assert!(place(&mut w, "жаровня").flash > 0.0);
}

fn grab(w: &mut SceneWorld, t: f64) -> f64 {
    use reyvec_game::{Action, InputFrame};
    w.set_input(InputFrame::new(&[], &[Action::Grab]));
    let t = run(w, t, 1.0 / 60.0);
    w.set_input(InputFrame::new(&[], &[]));
    run(w, t, 0.5)
}

#[test]
fn glued_crate_is_not_taken_by_hands() {
    let mut w = room();
    let mut t = run(&mut w, 0.0, 0.5);
    // Справа от приклеенного ящика, лицом к нему.
    let glued = place(&mut w, "приклеенный ящик").at;
    stand(&mut w, glued[0] + 0.06 + 0.07 + 0.02, -1.0);
    t = run(&mut w, t, 0.2);
    t = grab(&mut w, t);
    let now = place(&mut w, "приклеенный ящик").at;
    assert!(
        (now[1] - glued[1]).abs() < 0.02,
        "приклеенный поднят: {now:?}"
    );
    // Обычный берётся и поднимается над головой.
    let crate_at = place(&mut w, "ящик").at;
    stand(&mut w, crate_at[0] - 0.06 - 0.07 - 0.02, 1.0);
    t = run(&mut w, t, 0.2);
    grab(&mut w, t);
    let up = place(&mut w, "ящик").at;
    assert!(up[1] > crate_at[1] + 0.2, "ящик не поднят: {up:?}");
}

#[test]
fn glow_prop_lights_up_the_lamp() {
    let mut w = room();
    run(&mut w, 0.0, 0.1);
    let scene = w.snapshot();
    assert_eq!(scene.objects.len(), 1);
    assert!(scene.objects[0].glow);
}

#[test]
fn one_script_reads_each_item_props() {
    let mut w = room();
    let mut t = 0.0;
    let mut spread = [0.0f64; 3];
    let home: Vec<f64> = (1..=3)
        .map(|i| place(&mut w, &format!("шар {i}")).at[1])
        .collect();
    for _ in 0..240 {
        t = run(&mut w, t, 1.0 / 60.0);
        for (i, s) in spread.iter_mut().enumerate() {
            let y = place(&mut w, &format!("шар {}", i + 1)).at[1];
            *s = s.max((y - home[i]).abs());
        }
    }
    eprintln!("размах шаров: {spread:?}");
    assert!((spread[0] - 0.04).abs() < 0.005);
    assert!((spread[1] - 0.1).abs() < 0.01);
    assert!((spread[2] - 0.2).abs() < 0.02);
    // Шар с меткой «прячется» пропадает, пока его касается герой; без метки — нет.
    for (name, hides) in [("шар 2", true), ("шар 1", false)] {
        let at = place(&mut w, name).at;
        let mut b = reyvec_game::Body::new(at);
        b.vel = [0.0, 0.0];
        w.set_player_body(b);
        t = run(&mut w, t, 1.0 / 60.0);
        let gone = place(&mut w, name).opacity == 0.0;
        assert_eq!(gone, hides, "{name}");
    }
}

#[test]
fn wrong_props_are_named_in_the_error() {
    let text = |props: &str| {
        format!("(format: \"reyvec-scene\", version: 1, nodes: [(name: \"шип\", props: {props})])")
    };
    let err = |props: &str| SceneFile::parse(&text(props)).unwrap_err().to_string();
    let e = err("{\"damage\": \"много\"}");
    assert!(e.contains("шип") && e.contains("damage"), "{e}");
    let e = err("{\"grab\": 1}");
    assert!(e.contains("grab") && e.contains("да/нет"), "{e}");
    let e = err("{\" \": 1}");
    assert!(e.contains("пустое имя"), "{e}");
    assert!(SceneFile::parse(&text("{\"свой\": \"что угодно\", \"число\": 3}")).is_ok());
}
