//! Сю.8: переименование во всём сюжете и в скриптах.

use reyvec_story::rename::{Kind, luau_lines, luau_rename, rename_in_expr};
use reyvec_story::{EXAMPLE, Node, Story, Value};

fn example() -> Story {
    let (s, problems) = Story::from_texts(EXAMPLE);
    assert!(problems.is_empty(), "{problems:?}");
    s
}

#[test]
fn dialog_rename_fixes_jumps_keys_and_file() {
    let mut s = example();
    let r = s
        .rename(Kind::Dialog, "kuznec.ruda", "kuznec.ruda2")
        .unwrap();
    assert_eq!(
        r.moved,
        Some((
            "dialogs/kuznec.ruda.ron".into(),
            "dialogs/kuznec.ruda2.ron".into()
        ))
    );
    assert!(r.dialogs.contains(&"kuznec.privet".to_string()), "{r:?}");
    let jump = s.dialogs["kuznec.privet"]
        .nodes
        .iter()
        .find_map(|n| match n {
            Node::Jump { dialog, .. } => Some(dialog.clone()),
            _ => None,
        });
    assert_eq!(jump.as_deref(), Some("kuznec.ruda2"));
    assert!(s.lang["en"].contains_key("kuznec.ruda2.nachalo"));
    assert!(!s.lang["en"].keys().any(|k| k.starts_with("kuznec.ruda.")));
    assert_eq!(r.langs, ["en"]);
    assert!(reyvec_story::check::check(&mut s).is_empty());
}

#[test]
fn var_and_quest_renames_reach_conditions_texts_and_rewards() {
    let mut s = example();
    s.rename(Kind::Var, "kuznec.znakom", "kuznec.znaet")
        .unwrap();
    s.rename(Kind::Var, "hero.name", "hero.imya").unwrap();
    s.rename(Kind::Var, "kuznec.nastroenie", "kuznec.dushevnost")
        .unwrap();
    let r = s.rename(Kind::Quest, "mech", "mech_geroya").unwrap();
    assert!(r.dialogs.contains(&"kuznec.privet".to_string()));
    let problems = reyvec_story::check::check(&mut s);
    assert!(problems.is_empty(), "{problems:?}");
    let q = &s.quests["mech_geroya"];
    assert!(q.rewards.iter().any(|c| c.var == "kuznec.dushevnost"));
    // Игра проходит с новыми именами.
    let mut st = s.new_state();
    let mut talk = s.talk("kuznec.privet").unwrap();
    st.vars.insert("kuznec.znaet".into(), Value::Bool(true));
    match s.step(&mut talk, &mut st) {
        reyvec_story::Step::Line(l) => assert_eq!(l.text, "А, Странник! Заходи."),
        other => panic!("{other:?}"),
    }
    assert!(s.can_rename(Kind::Var, "hero.money", "hero.ruda").is_err());
    assert!(s.can_rename(Kind::Var, "hero.money", "Деньги").is_err());
}

#[test]
fn character_rename_fixes_speakers_and_cast() {
    let mut s = example();
    s.rename(Kind::Character, "kuznec", "boris").unwrap();
    assert!(
        s.dialogs
            .values()
            .all(|d| !d.with.contains(&"kuznec".into()))
    );
    assert!(reyvec_story::check::check(&mut s).is_empty());
}

#[test]
fn expressions_rename_whole_names_only() {
    let mut e = r#"hero.money >= 10 and hero.money2 < 3 and superhero.money and x == "hero.money""#
        .to_string();
    assert_eq!(rename_in_expr(&mut e, "hero.money", "hero.gold"), 1);
    assert!(e.starts_with("hero.gold >= 10 and hero.money2"), "{e}");
    assert!(e.ends_with(r#""hero.money""#), "{e}");
}

#[test]
fn scripts_are_found_and_renamed() {
    let src = "if quest.stage(\"mech\") == \"otnesti\" then\n  story.talk('kuznec.ruda')\nend\nmystory.talk(\"kuznec.ruda\")\nstory.talk( \"kuznec.ruda.x\")\n";
    assert_eq!(
        luau_lines(src, Kind::Dialog, "kuznec.ruda"),
        [(2, "story.talk('kuznec.ruda')".to_string())]
    );
    assert_eq!(luau_lines(src, Kind::Quest, "mech").len(), 1);
    let (out, n) = luau_rename(src, Kind::Dialog, "kuznec.ruda", "kuznec.ruda2");
    assert_eq!(n, 1);
    assert!(out.contains("story.talk('kuznec.ruda2')"));
    assert!(out.contains("mystory.talk(\"kuznec.ruda\")"));
}
