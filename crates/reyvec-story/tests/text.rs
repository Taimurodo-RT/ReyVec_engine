//! Сю.13: диалог текстом, как пьеса, — туда и обратно без потерь.

use reyvec_story::text::{from_text, to_text};
use reyvec_story::{Dialog, EXAMPLE, Node, Story};

fn example() -> Story {
    let (s, problems) = Story::from_texts(EXAMPLE);
    assert!(problems.is_empty(), "{problems:?}");
    s
}

fn ron(d: &Dialog) -> String {
    ron::to_string(d).unwrap()
}

#[test]
fn example_dialogs_go_to_text_and_back_unchanged() {
    let s = example();
    for d in s.dialogs.values() {
        let text = to_text(d);
        let back = from_text(&text, d).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
        assert_eq!(ron(&back), ron(d), "{text}");
    }
    let t = to_text(&s.dialogs["kuznec.privet"]);
    assert!(
        t.contains("== chuzhak\nkuznec: Чего надо, чужак? Я кузнец, если что.\n-> vopros"),
        "{t}"
    );
    assert!(
        t.contains("* Мне нужен меч. [если quest.mech.state == \"none\"] -> pro_mech"),
        "{t}"
    );
    assert!(
        t.contains("живая kuznec (20 слов; может: relation.add):"),
        "{t}"
    );
}

#[test]
fn every_card_kind_round_trips() {
    let text = r#"== a
если hero.money >= 10 -> b
иначе -> c

== b
hero.money -= 10
kuznec.znakom = true
hero.name = "Ян \"Меч\""
-> c

== c
событие quest.add("mech", "ruda", 2)
-> d

== d
выполнить open_door("vorota", 1.5, false)
-> e

== e
пауза 1.5
-> f

== f
пауза
-> g

== g
наугад -> h, i

== h
процент 30 -> i | j

== i
перейти kuznec.ruda#nachalo

== j
kuznec: Две строки:\nвторая.
-> k

== k
живая kuznec (12 слов; может: relation.add, memory.add): похвалить героя
запасной: Молодец.
-> z

== z
конец
"#;
    let empty = Dialog {
        format: "reyvec-dialog".into(),
        version: 1,
        id: "t".into(),
        title: String::new(),
        with: Vec::new(),
        start: String::new(),
        lines: Default::default(),
        nodes: Vec::new(),
    };
    let d = from_text(text, &empty).unwrap();
    assert_eq!(d.nodes.len(), 12);
    match &d.nodes[1] {
        Node::Set { changes, .. } => {
            assert_eq!(changes[0].add, Some(-10.0));
            assert_eq!(
                changes[2].to,
                Some(reyvec_story::Value::Text("Ян \"Меч\"".into()))
            );
        }
        n => panic!("{n:?}"),
    }
    match &d.nodes[9] {
        Node::Line { text, .. } => assert_eq!(text, "Две строки:\nвторая."),
        n => panic!("{n:?}"),
    }
    // Новые карточки встают на схеме в ряд, а не друг на друга.
    let xs: Vec<f32> = d
        .nodes
        .iter()
        .map(|n| match n {
            Node::End { at, .. } => at.0,
            _ => -1.0,
        })
        .collect();
    assert!(xs[11] > 0.0);
    let again = from_text(&to_text(&d), &d).unwrap();
    assert_eq!(ron(&again), ron(&d));
}

#[test]
fn positions_settings_and_keys_stay_after_a_text_edit() {
    let s = example();
    let d = &s.dialogs["kuznec.privet"];
    let text = to_text(d).replace("Что тебе нужно?", "Ну, что нужно?");
    let back = from_text(&text, d).unwrap();
    let old = d.node("po_imeni").unwrap();
    let new = back.node("po_imeni").unwrap();
    assert_eq!(ron_node(old), ron_node(new), "настроение и место — те же");
    match back.node("vopros").unwrap() {
        Node::Line { text, .. } => assert_eq!(text, "Ну, что нужно?"),
        n => panic!("{n:?}"),
    }
}

fn ron_node(n: &Node) -> String {
    ron::to_string(n).unwrap()
}

#[test]
fn mistakes_are_shown_by_line() {
    let s = example();
    let d = &s.dialogs["kuznec.privet"];
    let e = from_text(
        "привет\n== a\nkuznec Что?\n== a\nконец\n== B\nконец\n== c\nпауза два\n",
        d,
    )
    .unwrap_err();
    let lines: Vec<usize> = e.iter().map(|e| e.line).collect();
    assert_eq!(lines, [1, 3, 4, 6, 9], "{e:?}");
    assert!(e[1].text.contains("непонятная строка"), "{e:?}");
    assert!(e[2].text.contains("уже есть"), "{e:?}");
}
