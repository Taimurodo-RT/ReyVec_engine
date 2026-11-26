//! Сю.10: карточка «Сгенерировать» — модель, рамки, запасной текст.

use std::sync::{Arc, Mutex};

use reyvec_story::writer::{Canned, Reply, accept, prompt};
use reyvec_story::{EXAMPLE, Generated, State, Step, Story, Value};

fn example() -> Story {
    let (s, problems) = Story::from_texts(EXAMPLE);
    assert!(problems.is_empty(), "{problems:?}");
    s
}

/// До реплики «Просто смотрю» в первой встрече.
fn just_looking(s: &mut Story, st: &mut State) -> reyvec_story::Shown {
    let mut talk = s.talk("kuznec.privet").unwrap();
    for _ in 0..10 {
        match s.step(&mut talk, st) {
            Step::Line(l) if l.choices.is_empty() && l.generated.is_some() => return l,
            Step::Line(l) => {
                // «Просто смотрю.» — последний вариант (и в переводе).
                if !l.choices.is_empty() {
                    talk.choose(l.choices.len() - 1, st);
                }
            }
            other => panic!("{other:?}"),
        }
    }
    panic!("нет карточки «Сгенерировать»");
}

#[test]
fn without_a_model_the_fallback_sounds() {
    let mut s = example();
    let mut st = s.new_state();
    let l = just_looking(&mut s, &mut st);
    assert_eq!(l.text, "Смотри, только руками не трогай.");
    assert_eq!(l.generated, Some(Generated::Fallback(None)));
    let mut st = s.new_state();
    st.lang = "en".into();
    let l = just_looking(&mut s, &mut st);
    assert_eq!(l.text, "Look all you want, just don't touch.");
}

#[test]
fn model_text_is_used_and_only_allowed_actions_happen() {
    let mut s = example();
    let asked = Arc::new(Mutex::new(None));
    s.writer = Some(Box::new(Canned {
        reply: Ok(Reply {
            text: "«Гляди, гляди. Руды бы мне…»".into(),
            events: vec![
                (
                    "relation.add".into(),
                    vec![
                        Value::Text("kuznec".into()),
                        Value::Text("hero".into()),
                        Value::Int(1),
                    ],
                ),
                ("quest.start".into(), vec![Value::Text("mech".into())]),
            ],
        }),
        asked: Arc::clone(&asked),
    }));
    let mut st = s.new_state();
    s.remember(&mut st, "kuznec", "dozhd", "Всю неделю льёт дождь", 4)
        .unwrap();
    let l = just_looking(&mut s, &mut st);
    assert_eq!(l.text, "Гляди, гляди. Руды бы мне…");
    assert_eq!(l.generated, Some(Generated::Model("заглушка".into())));
    assert_eq!(st.memory["kuznec"].relation("hero"), 0, "−1 + 1");
    assert_eq!(
        s.quest_state(&st, "mech").status,
        reyvec_story::Status::None,
        "quest.start не разрешён карточкой"
    );
    let req = asked.lock().unwrap().clone().unwrap();
    let p = prompt(&req);
    assert!(p.contains("Кузнец Борис") && p.contains("Ворчливый"), "{p}");
    assert!(p.contains("Всю неделю льёт дождь"), "{p}");
    assert!(p.contains("не длиннее 20 слов"), "{p}");
    assert!(p.contains("relation.add"), "{p}");
}

#[test]
fn bad_answers_fall_back() {
    let mut s = example();
    let long = "слово ".repeat(40);
    for reply in [
        Ok(Reply {
            text: "   ".into(),
            events: Vec::new(),
        }),
        Ok(Reply {
            text: long,
            events: Vec::new(),
        }),
        Err("нет сети".to_string()),
    ] {
        s.writer = Some(Box::new(Canned {
            reply,
            asked: Arc::default(),
        }));
        let mut st = s.new_state();
        let l = just_looking(&mut s, &mut st);
        assert_eq!(l.text, "Смотри, только руками не трогай.");
        assert!(matches!(l.generated, Some(Generated::Fallback(Some(_)))));
    }
    let req = s.request(&s.new_state(), "kuznec", "x", 5, &[]);
    assert!(
        accept(
            &req,
            Reply {
                text: "раз два три".into(),
                events: Vec::new()
            }
        )
        .is_ok()
    );
}

#[test]
fn generate_card_is_checked() {
    let mut texts: Vec<(&str, &str)> = EXAMPLE.to_vec();
    let d = r#"(format: "reyvec-dialog", version: 1, id: "plohoi", nodes: [
        Generate(id: "a", who: "boris", task: "", fallback: "", allowed: ["story.set"], next: "b"),
        End(id: "b"),
    ])"#;
    texts.push(("dialogs/plohoi.ron", d));
    let (_, problems) = Story::from_texts(&texts);
    let all: Vec<String> = problems.iter().map(|p| p.to_string()).collect();
    let all = all.join("\n");
    for want in [
        "нет персонажа «boris»",
        "нет запасного текста",
        "нет задачи",
        "нельзя разрешить «story.set»",
    ] {
        assert!(all.contains(want), "{want}: {all}");
    }
}
