//! Проверка находит ошибки в данных и не падает на них в игре.

use reyvec_story::{Step, Story};

const VARS: &str = r#"(format: "reyvec-story-vars", vars: {"money": 5, "kuznec.znakom": false})"#;
const KUZNEC: &str = r#"(format: "reyvec-character", id: "kuznec", name: "Кузнец")"#;

fn load(dialog: &str) -> (Story, Vec<String>) {
    let (story, problems) = Story::from_texts(&[
        ("vars.ron", VARS),
        ("characters/kuznec.ron", KUZNEC),
        ("dialogs/d.ron", dialog),
    ]);
    (story, problems.iter().map(ToString::to_string).collect())
}

fn has(problems: &[String], needle: &str) {
    assert!(
        problems.iter().any(|p| p.contains(needle)),
        "нет «{needle}» в {problems:#?}"
    );
}

#[test]
fn finds_broken_arrows_names_and_conditions() {
    let (_, p) = load(
        r#"(format: "reyvec-dialog", id: "d", nodes: [
            Check(id: "a", when: "kuznec.znakomm", yes: "b", no: "nowhere"),
            Line(id: "b", who: "mellnik", text: "Деньги: {money}, {gold}", next: "c"),
            Check(id: "c", when: "money >=", yes: "e", no: "e"),
            Set(id: "s", changes: [(var: "money", to: "много")], next: "e"),
            Event(id: "q", name: "quest.start", args: ["nope"], next: "e"),
            End(id: "e"),
        ])"#,
    );
    has(&p, "стрелка в несуществующую «nowhere»");
    has(&p, "дало nil");
    has(&p, "нет персонажа «mellnik»");
    has(&p, "{gold}");
    has(&p, "[c]: ошибка: условие «money >=»");
    has(&p, "«money» — целое, а записывают текст");
    has(&p, "нет задания «nope»");
    has(&p, "[s]: предупреждение: до карточки не дойти");
}

#[test]
fn bad_names_and_formats() {
    let (_, p) = load(r#"(format: "reyvec-dialog", id: "Кузнец", nodes: [End(id: "e")])"#);
    has(&p, "имя «Кузнец»");
    let (_, p) = load(r#"(format: "reyvec-quest", id: "d", nodes: [])"#);
    has(&p, "format: \"reyvec-quest\"");
    let (_, p) = load("(это не RON");
    assert_eq!(p.len(), 1, "{p:#?}");
}

#[test]
fn player_stops_softly_on_bad_data() {
    let (mut story, _) = load(
        r#"(format: "reyvec-dialog", id: "d", nodes: [
            Check(id: "a", when: "kuznec.nope", yes: "e", no: "e"),
            End(id: "e"),
        ])"#,
    );
    let mut state = story.new_state();
    let mut talk = story.talk("d").unwrap();
    assert_eq!(story.step(&mut talk, &mut state), Step::End);
    assert!(talk.error.as_deref().unwrap().contains("nil"));
    assert_eq!(story.step(&mut talk, &mut state), Step::End);
}

#[test]
fn endless_silent_loop_is_cut() {
    let (mut story, _) = load(
        r#"(format: "reyvec-dialog", id: "d", nodes: [
            Set(id: "a", changes: [(var: "money", add: 1)], next: "b"),
            Check(id: "b", when: "true", yes: "a", no: "a"),
        ])"#,
    );
    let mut state = story.new_state();
    let mut talk = story.talk("d").unwrap();
    assert_eq!(story.step(&mut talk, &mut state), Step::End);
    assert!(talk.error.as_deref().unwrap().contains("по кругу"));
}

#[test]
fn conditions_are_sandboxed() {
    let (mut story, _) = load(r#"(format: "reyvec-dialog", id: "d", nodes: [End(id: "e")])"#);
    let state = story.new_state();
    assert!(story.eval("math.max(money, 3) == 5", &state).unwrap());
    assert!(story.eval("string.len(\"ab\") == 2", &state).unwrap());
    assert!(story.eval("io == nil", &state).unwrap());
    // Писать в мир из условия нельзя.
    assert!(
        story
            .eval("(function() money = 1 return true end)()", &state)
            .unwrap()
    );
    assert!(story.eval("money == 5", &state).unwrap());
    assert!(
        story
            .eval("(function() while true do end end)()", &state)
            .is_err()
    );
}

#[test]
fn file_roundtrip() {
    let text = include_str!("../../../story/dialogs/kuznec.privet.ron");
    let (story, _) = Story::from_texts(&[("dialogs/k.ron", text)]);
    let d = &story.dialogs["kuznec.privet"];
    let again = reyvec_story::format::write(d);
    let (story2, problems) = Story::from_texts(&[("dialogs/k.ron", again.as_str())]);
    assert!(
        problems.iter().all(|p| !p.text.contains("format")),
        "{problems:#?}\n{again}"
    );
    assert_eq!(
        format!("{:?}", story2.dialogs["kuznec.privet"]),
        format!("{d:?}")
    );
}
