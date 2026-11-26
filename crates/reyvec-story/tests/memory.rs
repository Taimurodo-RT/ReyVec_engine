//! Сю.9: память и отношения персонажей.

use reyvec_story::{EXAMPLE, Step, Story};

fn example() -> Story {
    let (s, problems) = Story::from_texts(EXAMPLE);
    assert!(problems.is_empty(), "{problems:?}");
    s
}

/// Пройти разговор до конца, выбирая первый вариант.
fn play(s: &mut Story, st: &mut reyvec_story::State, dialog: &str) -> Vec<String> {
    let mut talk = s.talk(dialog).unwrap();
    let mut lines = Vec::new();
    for _ in 0..50 {
        match s.step(&mut talk, st) {
            Step::Line(l) => {
                lines.push(l.text.clone());
                if !l.choices.is_empty() {
                    talk.choose(0, st);
                }
            }
            Step::End => return lines,
            _ => {}
        }
    }
    panic!("разговор не кончился: {lines:?}");
}

#[test]
fn smith_remembers_the_ore_and_greets_warmer() {
    let mut s = example();
    let mut st = s.new_state();
    assert_eq!(
        st.memory["kuznec"].relation("hero"),
        -1,
        "из файла персонажа"
    );
    st.vars
        .insert("kuznec.znakom".into(), reyvec_story::Value::Bool(true));
    let before = play(&mut s, &mut st, "kuznec.privet");
    assert_eq!(before[0], "А, Странник! Заходи.");
    play(&mut s, &mut st, "kuznec.ruda");
    let m = &st.memory["kuznec"];
    assert!(m.knows("ruda_prinesena"));
    assert_eq!(m.relation("hero"), 2);
    let after = play(&mut s, &mut st, "kuznec.privet");
    assert!(after[0].contains("Спасибо ещё раз за руду"), "{after:?}");
    // Сохранение помнит память.
    let again = s.restore(&s.save(&st).unwrap()).unwrap();
    assert_eq!(again.memory, st.memory);
}

#[test]
fn weak_facts_are_forgotten_strong_stay() {
    let s = example();
    let mut st = s.new_state();
    s.remember(&mut st, "kuznec", "pogoda", "Шёл дождь", 1)
        .unwrap();
    s.remember(&mut st, "kuznec", "volk", "Волк напал на стадо", 6)
        .unwrap();
    s.remember(&mut st, "kuznec", "doch", "У него есть дочь", 10)
        .unwrap();
    s.memory_tick(&mut st, 11.0 * 60.0);
    let m = &st.memory["kuznec"];
    assert!(!m.knows("pogoda"), "важность 1 — 10 минут");
    assert!(m.knows("volk"));
    s.memory_tick(&mut st, 10.0 * 3600.0);
    let m = &st.memory["kuznec"];
    assert!(!m.knows("volk"));
    assert!(m.knows("doch"), "важность 10 не забывается");
    assert!(s.remember(&mut st, "boris", "x", "", 5).is_err());
    assert!(s.relate(&mut st, "kuznec", "nikto", 1, true).is_err());
}

#[test]
fn bad_memory_events_are_problems() {
    let mut texts: Vec<(&str, &str)> = EXAMPLE.to_vec();
    let d = r#"(format: "reyvec-dialog", version: 1, id: "plohoi", nodes: [
        Event(id: "a", name: "memory.add", args: ["boris", "x"], next: "b"),
        Event(id: "b", name: "relation.add", args: ["kuznec"], next: "c"),
        Event(id: "c", name: "memory.zabyt", args: [], next: "d"),
        Check(id: "d", when: "memory.boris.x", yes: "e", no: "e"),
        End(id: "e"),
    ])"#;
    texts.push(("dialogs/plohoi.ron", d));
    let (_, problems) = Story::from_texts(&texts);
    let text: Vec<String> = problems.iter().map(|p| p.to_string()).collect();
    let all = text.join("\n");
    assert!(
        all.contains("[a]") && all.contains("нет персонажа «boris»"),
        "{all}"
    );
    assert!(all.contains("[b]"), "{all}");
    assert!(all.contains("нет события memory.zabyt"), "{all}");
    assert!(all.contains("[d]"), "{all}");
}
