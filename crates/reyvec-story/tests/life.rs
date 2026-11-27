//! Сю.12: распорядок дня, разговоры персонажей между собой, сотня персонажей.

use reyvec_story::life::{FAR_STEP, HOUR, LifeEvent, hour};
use reyvec_story::{EXAMPLE, Story};

fn example() -> Story {
    let (s, problems) = Story::from_texts(EXAMPLE);
    assert!(problems.is_empty(), "{problems:?}");
    s
}

/// Прогнать `seconds` игры шагами по 0,1 с; что случилось — по порядку.
fn run(s: &mut Story, st: &mut reyvec_story::State, seconds: f64, d: f64) -> Vec<LifeEvent> {
    let mut out = Vec::new();
    for _ in 0..(seconds * 10.0) as usize {
        s.memory_tick(st, 0.1);
        out.extend(s.life_tick(st, 0.1, &|_| d));
    }
    out
}

#[test]
fn day_goes_round_and_people_move() {
    let mut s = example();
    let mut st = s.new_state();
    assert_eq!(hour(0.0), 8.0, "игра начинается в 8 утра");
    assert_eq!(s.now_of(&st, "kuznec").unwrap().place, "kuznya");
    // К полудню кузнец идёт обедать на площадь.
    st.time = 4.0 * HOUR - 1.0;
    let ev = run(&mut s, &mut st, 2.0, 5.0);
    assert!(
        ev.iter()
            .any(|e| matches!(e, LifeEvent::Moved { who, place, .. }
            if who == "kuznec" && place == "ploshad")),
        "{ev:?}"
    );
    // Ночью (после 22 и до 6) — дома.
    st.time = 20.0 * HOUR;
    assert_eq!(hour(st.time), 4.0);
    assert_eq!(s.now_of(&st, "kuznec").unwrap().doing, "spit");
}

#[test]
fn smith_and_apprentice_talk_when_both_in_the_forge() {
    let mut s = example();
    let mut st = s.new_state();
    // Рядом с героем — слышно все три реплики, по одной в 3 секунды.
    let ev = run(&mut s, &mut st, 10.0, 5.0);
    let said: Vec<_> = ev
        .iter()
        .filter_map(|e| match e {
            LifeEvent::Said { name, text, .. } => Some(format!("{name}: {text}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        said,
        [
            "Кузнец Борис: Петька, мехи раздувай!",
            "Петька: Раздуваю, дядя Борис. Руды-то всё нет…",
            "Кузнец Борис: Будет руда — будет и меч.",
        ]
    );
    // Через минуту (час игры) — снова, а раньше — нет.
    assert!(run(&mut s, &mut st, 30.0, 5.0).is_empty());
    let again = run(&mut s, &mut st, 30.0, 5.0);
    assert!(
        again.iter().any(|e| matches!(e, LifeEvent::Said { .. })),
        "{again:?}"
    );
    // Когда меч готов — разговор другой.
    let mut st = s.new_state();
    s.quest_start(&mut st, "mech").unwrap();
    s.quest_complete(&mut st, "mech").unwrap();
    let ev = run(&mut s, &mut st, 4.0, 5.0);
    let heard = |w: &str| {
        ev.iter()
            .any(|e| matches!(e, LifeEvent::Said { text, .. } if text.contains(w)))
    };
    assert!(heard("скуёте") && !heard("мехи"), "{ev:?}");
}

#[test]
fn far_away_talk_is_silent_and_counted_rarely() {
    let mut s = example();
    let mut st = s.new_state();
    let ev = run(&mut s, &mut st, 20.0, 500.0);
    assert!(
        !ev.iter().any(|e| matches!(e, LifeEvent::Said { .. })),
        "далеко — не слышно: {ev:?}"
    );
    // Разговор всё равно был: время записано.
    assert!(st.life["kuznec"].chatted.contains_key("uchenik"));
    // Далеко — расчёт раз в FAR_STEP: накопленное время меньше шага.
    assert!(st.life["kuznec"].wait < FAR_STEP);
}

#[test]
fn hundred_villagers_live_cheaply() {
    let mut texts: Vec<(String, String)> = EXAMPLE
        .iter()
        .map(|(p, t)| (p.to_string(), t.to_string()))
        .collect();
    for i in 0..100 {
        let other = (i + 1) % 100;
        texts.push((
            format!("characters/zhitel{i}.ron"),
            format!(
                r#"(format: "reyvec-character", version: 1, id: "zhitel{i}", name: "Житель {i}",
                day: [(from: 6, place: "pole{p}", doing: "rabotaet"), (from: 20, place: "dom{i}")],
                chats: [(with: "zhitel{other}", dialog: "kuznec.uchenik", every: 2)])"#,
                p = i / 2
            ),
        ));
    }
    let refs: Vec<(&str, &str)> = texts
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    let (mut s, problems) = Story::from_texts(&refs);
    assert!(problems.is_empty(), "{problems:?}");
    let mut st = s.new_state();
    // Десять жителей рядом с героем, остальные далеко.
    let near = |id: &str| match id
        .strip_prefix("zhitel")
        .and_then(|n| n.parse::<u32>().ok())
    {
        Some(n) if n < 10 => 5.0,
        Some(_) => 1000.0,
        None => f64::INFINITY,
    };
    let start = std::time::Instant::now();
    let mut said = 0;
    for _ in 0..600 {
        s.memory_tick(&mut st, 1.0 / 60.0);
        said += s
            .life_tick(&mut st, 1.0 / 60.0, &near)
            .iter()
            .filter(|e| matches!(e, LifeEvent::Said { .. }))
            .count();
    }
    let per_step = start.elapsed().as_secs_f64() / 600.0;
    assert!(said > 0, "соседи по полю, что рядом, разговаривают");
    assert_eq!(st.life.len(), 102, "все живут");
    // Мягкий предел (тесты собираются без оптимизаций): шаг меньше 2 мс.
    assert!(per_step < 0.002, "{:.3} мс на шаг", per_step * 1000.0);
}
