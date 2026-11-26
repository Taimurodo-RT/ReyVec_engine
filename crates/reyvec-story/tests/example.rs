//! Пример «кузнец и руда» целиком: проверка, разговор, задание, перевод.

use reyvec_story::{EXAMPLE, Status, Step, Story, Value};

fn example() -> Story {
    let (story, problems) = Story::from_texts(EXAMPLE);
    assert!(problems.is_empty(), "{problems:#?}");
    story
}

fn line(step: Step) -> reyvec_story::Shown {
    match step {
        Step::Line(s) => s,
        other => panic!("ждали реплику, а пришло {other:?}"),
    }
}

#[test]
fn example_folder_matches_embedded() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../story");
    let (story, problems) = Story::load_dir(&dir);
    assert!(problems.is_empty(), "{problems:#?}");
    assert_eq!(story.dialogs.len(), 2);
    assert_eq!(story.characters.len(), 2);
    assert_eq!(story.quests.len(), 1);
    assert!(story.lang.contains_key("en"));
}

#[test]
fn smith_and_ore_full_path() {
    let mut story = example();
    let mut state = story.new_state();

    // Первая встреча: кузнец не знаком, говорит «чужак».
    let mut talk = story.talk("kuznec.privet").unwrap();
    let first = line(story.step(&mut talk, &mut state));
    assert_eq!(first.who, "kuznec");
    assert_eq!(first.name, "Кузнец Борис");
    assert!(first.text.starts_with("Чего надо"));
    // Настроение — из карточки персонажа.
    assert_eq!(first.settings.mood.as_deref(), Some("spokoiny"));
    assert_eq!(state.vars["kuznec.znakom"], Value::Bool(true));

    let ask = line(story.step(&mut talk, &mut state));
    let texts: Vec<_> = ask.choices.iter().map(|c| c.text.as_str()).collect();
    // «Я принёс руду» скрыт: задания ещё нет.
    assert_eq!(texts, ["Мне нужен меч.", "Просто смотрю."]);
    assert!(talk.waiting());
    // Пока не выбрали, шаг повторяет ту же реплику.
    assert_eq!(line(story.step(&mut talk, &mut state)), ask);
    assert!(talk.choose(0, &mut state));
    let sword = line(story.step(&mut talk, &mut state));
    assert_eq!(sword.settings.camera, Some(true));
    assert_eq!(story.step(&mut talk, &mut state), Step::End);
    assert!(talk.ended() && talk.error.is_none());
    assert_eq!(state.quests["mech"].status, Status::Active);
    assert_eq!(state.quests["mech"].stage, "ruda");

    // Скрипт находит руду.
    for _ in 0..3 {
        story.quest_add(&mut state, "mech", "ruda", 1).unwrap();
    }
    state.vars.insert("hero.ruda".into(), Value::Int(3));
    assert_eq!(state.quests["mech"].stage, "otnesti");

    // Вторая встреча: по имени, «Мне нужен меч» уже скрыт, «Я принёс руду» виден.
    let mut talk = story.talk("kuznec.privet").unwrap();
    let hi = line(story.step(&mut talk, &mut state));
    assert_eq!(hi.text, "А, Странник! Заходи.");
    assert_eq!(hi.settings.mood.as_deref(), Some("dobryi"));
    let ask = line(story.step(&mut talk, &mut state));
    let texts: Vec<_> = ask.choices.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["Я принёс руду.", "Просто смотрю."]);
    talk.choose(0, &mut state);
    let wow = line(story.step(&mut talk, &mut state));
    assert_eq!(talk.dialog, "kuznec.ruda");
    assert!(wow.text.starts_with("Ого"));
    let when = line(story.step(&mut talk, &mut state));
    assert!(when.text.contains("меч"));
    assert_eq!(story.step(&mut talk, &mut state), Step::End);
    assert_eq!(state.vars["hero.ruda"], Value::Int(0));
    assert_eq!(state.vars["kuznec.nastroenie"], Value::Int(1));
    assert_eq!(state.quests["mech"].stage, "zhdat");

    let journal = story.journal(&state);
    assert_eq!(journal.len(), 1);
    assert!(journal[0].text.contains("куёт"));
}

#[test]
fn same_seed_same_path() {
    let mut story = example();
    let run = |story: &mut Story, seed: u64| {
        let mut state = story.new_state();
        state.seed = seed;
        let mut talk = story.talk("kuznec.ruda").unwrap();
        let mut said = Vec::new();
        loop {
            match story.step(&mut talk, &mut state) {
                Step::Line(s) => said.push(s.text),
                Step::End => break,
                other => panic!("{other:?}"),
            }
        }
        said
    };
    assert_eq!(run(&mut story, 7), run(&mut story, 7));
    // Разные зёрна дают обе фразы «когда».
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..20 {
        seen.insert(run(&mut story, seed)[1].clone());
    }
    assert_eq!(seen.len(), 2);
}

#[test]
fn english_with_fallback() {
    let mut story = example();
    let mut state = story.new_state();
    state.lang = "en".into();
    let mut talk = story.talk("kuznec.privet").unwrap();
    let first = line(story.step(&mut talk, &mut state));
    assert!(first.text.starts_with("What do you want"));
    let ask = line(story.step(&mut talk, &mut state));
    assert_eq!(ask.choices[0].text, "I need a sword.");
    // Неизвестный язык — основной текст.
    state.lang = "de".into();
    assert_eq!(
        story.text("kuznec.privet.vopros", "Что тебе нужно?", &state),
        "Что тебе нужно?"
    );
}

#[test]
fn chosen_options_are_marked() {
    let mut story = example();
    let mut state = story.new_state();
    for _ in 0..2 {
        let mut talk = story.talk("kuznec.privet").unwrap();
        story.step(&mut talk, &mut state);
        let ask = line(story.step(&mut talk, &mut state));
        let last = ask.choices.last().unwrap();
        assert_eq!(last.text, "Просто смотрю.");
        talk.choose(ask.choices.len() - 1, &mut state);
        while story.step(&mut talk, &mut state) != Step::End {}
    }
    let mut talk = story.talk("kuznec.privet").unwrap();
    story.step(&mut talk, &mut state);
    let ask = line(story.step(&mut talk, &mut state));
    assert!(ask.choices.last().unwrap().seen);
    assert!(!ask.choices[0].seen);
}
