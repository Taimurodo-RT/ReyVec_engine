//! Сю.6: награды, условие стадии, провал по условию, журнал.

use reyvec_story::{Status, Story, Value};

const VARS: &str = r#"(format: "reyvec-story-vars", version: 1, vars: {
    "hero.money": 5, "hero.ruda": 0, "hero.mech": false, "kuznec.zloy": false,
})"#;

const QUEST: &str = r#"(
    format: "reyvec-quest", version: 1, id: "mech", title: "Меч",
    description: "Кузнец выкует меч.",
    stages: [
        (id: "ruda", journal: "Найти руду.", goals: [(id: "ruda", text: "Руда", count: 2)],
            done_when: "hero.money >= 10", next: "otnesti"),
        (id: "otnesti", journal: "Отнести.", done_when: "hero.ruda >= 3"),
    ],
    rewards: [(var: "hero.mech", to: true), (var: "hero.money", add: 50)],
    fail_when: "kuznec.zloy",
)"#;

fn story() -> Story {
    let (s, problems) = Story::from_texts(&[("vars.ron", VARS), ("quests/mech.ron", QUEST)]);
    assert!(problems.is_empty(), "{problems:?}");
    s
}

#[test]
fn stage_waits_for_its_condition_then_rewards_are_given() {
    let mut s = story();
    let mut st = s.new_state();
    s.quest_start(&mut st, "mech").unwrap();
    s.quest_add(&mut st, "mech", "ruda", 2).unwrap();
    s.quest_tick(&mut st).unwrap();
    // Руда есть, но денег меньше 10: стадия ждёт условия.
    assert_eq!(s.quest_state(&st, "mech").stage, "ruda");
    st.vars.insert("hero.money".into(), Value::Int(12));
    s.quest_tick(&mut st).unwrap();
    assert_eq!(s.quest_state(&st, "mech").stage, "otnesti");
    // Стадия без целей — по одному условию.
    st.vars.insert("hero.ruda".into(), Value::Int(3));
    s.quest_tick(&mut st).unwrap();
    assert_eq!(s.quest_state(&st, "mech").status, Status::Done);
    assert_eq!(st.vars["hero.mech"], Value::Bool(true));
    assert_eq!(st.vars["hero.money"], Value::Int(62));
    let j = s.journal(&st);
    assert_eq!(j[0].description, "Кузнец выкует меч.");
}

#[test]
fn fail_condition_fails_only_an_active_quest() {
    let mut s = story();
    let mut st = s.new_state();
    st.vars.insert("kuznec.zloy".into(), Value::Bool(true));
    s.quest_tick(&mut st).unwrap();
    assert_eq!(
        s.quest_state(&st, "mech").status,
        Status::None,
        "не взятое не проваливается"
    );
    s.quest_start(&mut st, "mech").unwrap();
    s.quest_tick(&mut st).unwrap();
    assert_eq!(s.quest_state(&st, "mech").status, Status::Failed);
    assert_eq!(st.vars["hero.mech"], Value::Bool(false), "награды нет");
}

#[test]
fn bad_conditions_and_rewards_are_problems() {
    let quest = QUEST
        .replace("hero.money >= 10", "hero.money >=")
        .replace("\"hero.mech\", to: true", "\"hero.sword\", to: true")
        .replace("fail_when: \"kuznec.zloy\"", "fail_when: \"kuznec.zloi.x\"");
    let (_, problems) = Story::from_texts(&[("vars.ron", VARS), ("quests/mech.ron", &quest)]);
    let text: Vec<String> = problems
        .iter()
        .map(|p| format!("{}: {}", p.place, p.text))
        .collect();
    assert!(
        text.iter().any(|t| t.starts_with("ruda: условие")),
        "{text:?}"
    );
    assert!(
        text.iter()
            .any(|t| t.starts_with("rewards: нет переменной «hero.sword»")),
        "{text:?}"
    );
    assert!(text.iter().any(|t| t.starts_with("fail_when:")), "{text:?}");
}
