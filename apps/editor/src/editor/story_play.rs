//! Проигрыватель диалога в «Сюжете» (Сю.4): разговор как в игре — реплики, варианты ответа,
//! «Дальше». Рядом — мир: значения в начале (их можно задать перед прогоном) и что
//! поменялось по ходу разговора, задания. Скрипты (`Выполнить`) и события игры здесь не
//! выполняются, только записываются в ленту.

use egui::{Color32, Frame, Margin, RichText, Ui, vec2};
use reyvec_story::{Generated, QuestState, State, Status, Step, Story, Talk};

use super::story_ui::{args_text, caption, lang_name, muted, parse_hex, value_edit, value_text};
use super::theme::*;

/// Запись ленты разговора.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Entry {
    /// Реплика: имя, цвет, текст.
    Line(String, Color32, String),
    /// Выбор игрока.
    Pick(String),
    /// Что случилось в мире или в игре (переменная, задание, событие, скрипт, пауза).
    Note(String),
    /// Ошибка в данных диалога.
    Error(String),
    End,
}

/// Состояние проигрывателя.
#[derive(Default)]
pub(crate) struct Player {
    /// Диалог, с которого начат прогон.
    pub(crate) dialog: String,
    talk: Option<Talk>,
    /// Мир сейчас.
    pub(crate) state: State,
    /// Мир в начале прогона (значения можно поменять до «Начать»).
    start: Option<State>,
    pub(crate) log: Vec<Entry>,
    /// Показанные варианты: текст и «уже выбирался».
    choices: Vec<(String, bool)>,
    /// Реплика без вариантов: ждём «Дальше».
    next: bool,
    runs: u64,
}

impl Player {
    /// Идёт ли разговор (есть что нажимать).
    pub(crate) fn active(&self) -> bool {
        self.talk.as_ref().is_some_and(|t| !t.ended())
    }

    /// Где сейчас разговор: (диалог, карточка) — для подсветки на схеме.
    pub(crate) fn at(&self) -> Option<(&str, &str)> {
        let t = self.talk.as_ref().filter(|_| self.active())?;
        Some((t.dialog.as_str(), t.node.as_str()))
    }

    /// Мир в начале: из сюжета, плюс переменные, добавленные с прошлого раза.
    fn start_state(&mut self, story: &Story) -> &mut State {
        let fresh = story.new_state();
        let s = self.start.get_or_insert_with(|| fresh.clone());
        for (k, v) in &fresh.vars {
            s.vars.entry(k.clone()).or_insert_with(|| v.clone());
        }
        s.vars.retain(|k, _| fresh.vars.contains_key(k));
        for (k, q) in fresh.quests {
            s.quests.entry(k).or_insert(q);
        }
        s
    }

    /// Начать диалог заново с мира «в начале».
    pub(crate) fn start(&mut self, story: &mut Story, dialog: &str) {
        let mut state = self.start_state(story).clone();
        self.runs += 1;
        // Каждый прогон — своё зерно, чтобы «наугад» и «процент» шли по-разному.
        state.seed = self.runs;
        self.state = state;
        self.dialog = dialog.to_string();
        self.log.clear();
        self.choices.clear();
        self.next = false;
        match story.talk(dialog) {
            Ok(t) => {
                self.talk = Some(t);
                self.advance(story);
            }
            Err(e) => {
                self.talk = None;
                self.log.push(Entry::Error(e));
            }
        }
    }

    /// Нажат вариант `i`.
    pub(crate) fn choose(&mut self, story: &mut Story, i: usize) {
        let Some(talk) = &mut self.talk else { return };
        let Some((text, _)) = self.choices.get(i).cloned() else {
            return;
        };
        if talk.choose(i, &mut self.state) {
            self.log.push(Entry::Pick(text));
            self.choices.clear();
            self.advance(story);
        }
    }

    /// Нажато «Дальше».
    pub(crate) fn go_on(&mut self, story: &mut Story) {
        if self.next {
            self.next = false;
            self.advance(story);
        }
    }

    /// Ведёт разговор до реплики или конца; всё, что случилось по пути, — в ленту.
    fn advance(&mut self, story: &mut Story) {
        let Some(talk) = &mut self.talk else { return };
        // Защита от диалога из одних событий по кругу.
        for _ in 0..1000 {
            let before = self.state.clone();
            let step = story.step(talk, &mut self.state);
            self.log.extend(world_notes(story, &before, &self.state));
            match step {
                Step::Line(s) => {
                    let color = parse_hex(&s.color).unwrap_or(TEXT);
                    let name = if s.name.is_empty() { s.who } else { s.name };
                    self.log.push(Entry::Line(name, color, s.text));
                    match &s.generated {
                        Some(Generated::Model(m)) => self
                            .log
                            .push(Entry::Note(format!("Реплику написала модель «{m}»"))),
                        Some(Generated::Fallback(None)) => self.log.push(Entry::Note(
                            "Живая реплика: модель не подключена, звучит запасной текст".into(),
                        )),
                        Some(Generated::Fallback(Some(e))) => self.log.push(Entry::Note(format!(
                            "Живая реплика: {e} — звучит запасной текст"
                        ))),
                        None => {}
                    }
                    if s.choices.is_empty() {
                        self.next = true;
                    } else {
                        self.choices = s.choices.into_iter().map(|c| (c.text, c.seen)).collect();
                    }
                    return;
                }
                Step::Event { name, args } => self.log.push(Entry::Note(format!(
                    "Событие для игры: {name}({})",
                    args_text(&args)
                ))),
                Step::Run { func, args } => self.log.push(Entry::Note(format!(
                    "Скрипт: {func}({}) — в проигрывателе не запускается",
                    args_text(&args)
                ))),
                Step::Wait { seconds } => self.log.push(Entry::Note(match seconds {
                    Some(s) => format!("Пауза {s} с"),
                    None => "Пауза до нажатия".into(),
                })),
                Step::End => {
                    if let Some(e) = talk.error.take() {
                        self.log.push(Entry::Error(e));
                    }
                    self.log.push(Entry::End);
                    return;
                }
            }
        }
        self.log
            .push(Entry::Error("события идут по кругу без реплик".into()));
    }

    /// Панель проигрывателя. `dialog` — открытый сейчас диалог.
    pub(crate) fn show(&mut self, ui: &mut Ui, story: &mut Story, dialog: Option<&str>) {
        ui.horizontal(|ui| {
            let target = dialog.unwrap_or(self.dialog.as_str()).to_string();
            let label = if self.talk.is_some() {
                "↻ Сначала"
            } else {
                "▶ Начать"
            };
            if ui
                .add_enabled(!target.is_empty(), egui::Button::new(label))
                .on_hover_text("Пройти диалог с мира «в начале»")
                .clicked()
            {
                self.start(story, &target);
            }
            let mut lang = self.start_state(story).lang.clone();
            let shown = if lang.is_empty() {
                "Основной язык".to_string()
            } else {
                lang_name(&lang)
            };
            let langs: Vec<String> = story.lang.keys().cloned().collect();
            egui::ComboBox::from_id_salt("play-lang")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut lang, String::new(), "Основной язык");
                    for l in &langs {
                        ui.selectable_value(&mut lang, l.clone(), lang_name(l));
                    }
                });
            if let Some(s) = &mut self.start
                && s.lang != lang
            {
                s.lang = lang.clone();
                self.state.lang = lang;
            }
        });
        ui.add_space(GAP_M);
        if self.talk.is_none() && self.log.is_empty() {
            muted(
                ui,
                "Нажмите «Начать»: диалог пойдёт как в игре. Ниже можно задать, каким мир \
                 будет в начале.",
            );
        }
        if let (Some(d), Some(t)) = (dialog, &self.talk)
            && t.dialog != d
            && !t.ended()
        {
            muted(ui, &format!("Разговор сейчас в диалоге «{}»", t.dialog));
        }
        for e in &self.log {
            entry(ui, e);
        }
        let mut pick = None;
        for (i, (text, seen)) in self.choices.iter().enumerate() {
            let color = if *seen { TEXT_DIM } else { TEXT };
            let b = egui::Button::new(RichText::new(format!("{}. {text}", i + 1)).color(color))
                .fill(SELECT_BG)
                .min_size(vec2(ui.available_width(), 0.0));
            let hint = if *seen {
                "Этот вариант уже выбирали"
            } else {
                "Выбрать"
            };
            if ui.add(b).on_hover_text(hint).clicked() {
                pick = Some(i);
            }
        }
        if let Some(i) = pick {
            self.choose(story, i);
        }
        if self.next && ui.button("Дальше ▸").clicked() {
            self.go_on(story);
        }
        // Цифры 1–9 выбирают вариант, пробел — «Дальше», как в игре.
        if !ui.ctx().memory(|m| m.focused().is_some()) {
            let keys = [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
                egui::Key::Num6,
                egui::Key::Num7,
                egui::Key::Num8,
                egui::Key::Num9,
            ];
            let n = self.choices.len();
            let hit = ui.input(|i| keys.iter().take(n).position(|k| i.key_pressed(*k)));
            if let Some(i) = hit {
                self.choose(story, i);
            }
            if self.next && ui.input(|i| i.key_pressed(egui::Key::Space)) {
                self.go_on(story);
            }
        }
        ui.add_space(GAP_L);
        ui.separator();
        self.world(ui, story);
    }

    /// Мир: значения в начале (правятся) и сейчас (что поменялось — цветом), задания.
    fn world(&mut self, ui: &mut Ui, story: &Story) {
        let running = self.talk.is_some();
        caption(ui, "Мир в начале прогона");
        let start = self.start_state(story);
        egui::Grid::new("play-start")
            .num_columns(2)
            .spacing(vec2(GAP_M, GAP_S))
            .show(ui, |ui| {
                for (k, v) in &mut start.vars {
                    ui.label(
                        RichText::new(k)
                            .monospace()
                            .size(FONT_SMALL)
                            .color(TEXT_DIM),
                    );
                    value_edit(ui, v, k);
                    ui.end_row();
                }
            });
        if ui
            .button("Как в сюжете")
            .on_hover_text("Вернуть значения из «Мира»")
            .clicked()
        {
            self.start = None;
        }
        if !running {
            return;
        }
        ui.add_space(GAP_L);
        caption(ui, "Мир сейчас");
        let start = self.start.clone().unwrap_or_default();
        let mut any = false;
        for (k, v) in &self.state.vars {
            let was = start.vars.get(k);
            if was != Some(v) {
                any = true;
                let was = was.map_or("—".into(), value_text);
                ui.label(
                    RichText::new(format!("{k}: {was} → {}", value_text(v)))
                        .color(ACCENT)
                        .size(FONT_SMALL),
                );
            }
        }
        if !any {
            muted(ui, "Переменные не менялись");
        }
        ui.add_space(GAP_M);
        caption(ui, "Задания");
        for (id, q) in &self.state.quests {
            let title = story
                .quests
                .get(id)
                .map_or(id.as_str(), |q| q.title.as_str());
            ui.label(
                RichText::new(format!("{title}: {}", quest_text(q)))
                    .color(if q.status == Status::None {
                        MUTED
                    } else {
                        TEXT
                    })
                    .size(FONT_SMALL),
            );
        }
    }
}

fn entry(ui: &mut Ui, e: &Entry) {
    match e {
        Entry::Line(name, color, text) => {
            Frame::NONE
                .fill(CARD)
                .corner_radius(RADIUS_M)
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(name).color(*color).strong());
                    ui.label(RichText::new(text).color(TEXT));
                });
        }
        Entry::Pick(text) => {
            ui.label(RichText::new(format!("— {text}")).color(SELECT).italics());
        }
        Entry::Note(text) => {
            ui.label(
                RichText::new(format!("· {text}"))
                    .color(MUTED)
                    .size(FONT_SMALL),
            );
        }
        Entry::Error(text) => {
            ui.label(RichText::new(format!("Ошибка: {text}")).color(BAD));
        }
        Entry::End => {
            ui.label(RichText::new("Конец разговора").color(MUTED).italics());
        }
    }
}

fn quest_text(q: &QuestState) -> String {
    let status = match q.status {
        Status::None => "не взято",
        Status::Active => "идёт",
        Status::Done => "выполнено",
        Status::Failed => "провалено",
    };
    if q.stage.is_empty() {
        status.into()
    } else {
        format!("{status}, стадия «{}»", q.stage)
    }
}

/// Что поменялось в мире за шаг: переменные и задания.
fn world_notes(story: &Story, before: &State, after: &State) -> Vec<Entry> {
    let mut out = Vec::new();
    for (k, v) in &after.vars {
        let was = before.vars.get(k);
        if was != Some(v) {
            let was = was.map_or("—".into(), value_text);
            out.push(Entry::Note(format!("{k}: {was} → {}", value_text(v))));
        }
    }
    for (id, q) in &after.quests {
        if before.quests.get(id) != Some(q) {
            let title = story
                .quests
                .get(id)
                .map_or(id.as_str(), |q| q.title.as_str());
            out.push(Entry::Note(format!("Задание «{title}»: {}", quest_text(q))));
        }
    }
    // Память и отношения (Сю.9).
    let empty = reyvec_story::memory::Memory::default();
    for (npc, m) in &after.memory {
        let was = before.memory.get(npc).unwrap_or(&empty);
        let name = story
            .characters
            .get(npc)
            .map_or(npc.as_str(), |c| c.name.as_str());
        for f in &m.facts {
            if was.facts.iter().all(|w| w.key != f.key) {
                out.push(Entry::Note(format!(
                    "{name} запомнил: {} ({}, важность {})",
                    if f.text.is_empty() { &f.key } else { &f.text },
                    f.key,
                    f.importance
                )));
            }
        }
        for f in &was.facts {
            if !m.knows(&f.key) {
                out.push(Entry::Note(format!("{name} забыл: {}", f.key)));
            }
        }
        for (whom, n) in &m.relations {
            let before = was.relation(whom);
            if before != *n {
                out.push(Entry::Note(format!(
                    "{name} → {whom}: отношение {before} → {n}"
                )));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use reyvec_story::Value;

    fn story() -> Story {
        Story::from_texts(reyvec_story::EXAMPLE).0
    }

    fn lines(p: &Player) -> Vec<String> {
        p.log
            .iter()
            .filter_map(|e| match e {
                Entry::Line(_, _, t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn first_meeting_takes_the_sword_quest() {
        let mut s = story();
        let mut p = Player::default();
        p.start(&mut s, "kuznec.privet");
        // Незнакомы: сначала «чужак», потом «что нужно».
        assert!(p.next, "{:?}", p.log);
        assert!(lines(&p)[0].contains("чужак"));
        assert!(
            p.log
                .iter()
                .any(|e| matches!(e, Entry::Note(n) if n.contains("kuznec.znakom")))
        );
        p.go_on(&mut s);
        assert_eq!(p.choices.len(), 2, "руды нет — варианта «принёс руду» нет");
        p.choose(&mut s, 0);
        assert!(lines(&p).last().unwrap().contains("Меч?"));
        p.go_on(&mut s);
        assert!(p.log.contains(&Entry::End), "{:?}", p.log);
        assert!(!p.active());
        assert_eq!(s.quest_state(&p.state, "mech").status, Status::Active);
        assert!(
            p.log
                .iter()
                .any(|e| matches!(e, Entry::Note(n) if n.contains("Меч для героя")))
        );
    }

    #[test]
    fn start_values_change_the_path() {
        let mut s = story();
        let mut p = Player::default();
        p.start_state(&s)
            .vars
            .insert("kuznec.znakom".into(), Value::Bool(true));
        p.start(&mut s, "kuznec.privet");
        assert!(lines(&p)[0].starts_with("А, "), "{:?}", p.log);
        // Сброс к сюжету — снова незнакомы.
        p.start = None;
        p.start(&mut s, "kuznec.privet");
        assert!(lines(&p)[0].contains("чужак"));
    }

    #[test]
    fn highlight_follows_the_talk() {
        let mut s = story();
        let mut p = Player::default();
        p.start(&mut s, "kuznec.privet");
        assert_eq!(p.at(), Some(("kuznec.privet", "chuzhak")));
        p.go_on(&mut s);
        assert_eq!(p.at(), Some(("kuznec.privet", "vopros")));
    }

    #[test]
    fn live_line_without_a_model_says_the_fallback() {
        let mut s = story();
        let mut p = Player::default();
        p.start(&mut s, "kuznec.privet");
        p.go_on(&mut s);
        let last = p.choices.len() - 1;
        p.choose(&mut s, last);
        assert!(
            lines(&p).last().unwrap().contains("руками не трогай"),
            "{:?}",
            p.log
        );
        assert!(
            p.log
                .iter()
                .any(|e| matches!(e, Entry::Note(n) if n.contains("модель не подключена")))
        );
    }
}
