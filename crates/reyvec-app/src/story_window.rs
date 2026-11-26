//! Сюжет в игре (Сю.7): окно диалога, журнал заданий, сохранение сюжета.
//!
//! Окно пока простое — внизу экрана: кто говорит, реплика, варианты ответа. Клавиши
//! 1–9 выбирают вариант, Пробел или Enter — «дальше», J открывает журнал. Пока идёт
//! разговор, герой стоит (это решает поток логики), а эти клавиши достаются окну.
//! Настоящий вид окон (пузырь над головой, портрет, свой стиль) — с интерфейсом игры
//! `.rui` (решение 0022).
//!
//! Сохранение — файл `story-save.ron` рядом с настройками клавиш: переменные, задания,
//! выборы игрока. Идущий разговор не сохраняется.

use std::path::PathBuf;

use egui::{Align2, Color32, RichText, Stroke};
use reyvec_scene::story::Status;
use reyvec_scene::{Answer, Screen, SharedStory, lock_story};
use winit::keyboard::KeyCode;

use crate::controls::{Input, controls_path};

/// Сколько вариантов ответа можно выбрать цифрой.
const DIGITS: [KeyCode; 9] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

pub(crate) struct StoryWindow {
    /// Открыт журнал заданий.
    pub journal: bool,
    /// Файл сохранения сюжета.
    path: PathBuf,
    /// Что вышло с сохранением или загрузкой.
    pub note: Option<(String, bool)>,
}

impl StoryWindow {
    pub(crate) fn new() -> Self {
        Self::at(controls_path().with_file_name("story-save.ron"))
    }

    pub(crate) fn at(path: PathBuf) -> Self {
        Self {
            journal: false,
            path,
            note: None,
        }
    }

    /// Нажатие клавиши в игре. `true` — оно досталось сюжету (не герою и не песочнице).
    pub(crate) fn key(&mut self, story: &SharedStory, input: Input, down: bool) -> bool {
        // Отпускание всегда игре: иначе клавиша, нажатая до разговора, «залипнет».
        let (Input::Key(code), true) = (input, down) else {
            return false;
        };
        if code == KeyCode::KeyJ {
            self.journal = !self.journal;
            return true;
        }
        let mut h = lock_story(story);
        if !h.busy() {
            return false;
        }
        let choices = match &h.screen {
            Some(Screen::Line(s)) => s.choices.len(),
            Some(Screen::Pause) => 0,
            // Разговор идёт, но реплики ещё нет (пауза с секундами): клавиши всё равно
            // наши, чтобы герой не прыгнул посреди разговора.
            None => return matches!(code, KeyCode::Space | KeyCode::Enter),
        };
        let answer = if let Some(i) = DIGITS.iter().position(|&d| d == code) {
            (i < choices).then_some(Answer::Choose(i))
        } else if matches!(code, KeyCode::Space | KeyCode::Enter | KeyCode::NumpadEnter) {
            (choices == 0).then_some(Answer::Next)
        } else {
            return false;
        };
        if let Some(a) = answer {
            h.answer = Some(a);
        }
        true
    }

    /// Сохранить сюжет в файл.
    pub(crate) fn save(&mut self, story: &SharedStory) {
        let text = lock_story(story).save();
        self.note = Some(
            match text.and_then(|t| std::fs::write(&self.path, t).map_err(|e| e.to_string())) {
                Ok(()) => (format!("Сюжет сохранён: {}", self.path.display()), false),
                Err(e) => (format!("Не сохранить сюжет: {e}"), true),
            },
        );
    }

    /// Загрузить сюжет из файла.
    pub(crate) fn load(&mut self, story: &SharedStory) {
        let text = std::fs::read_to_string(&self.path)
            .map_err(|e| format!("нет сохранения {}: {e}", self.path.display()));
        self.note = Some(match text.and_then(|t| lock_story(story).load(&t)) {
            Ok(()) => ("Сюжет загружен".into(), false),
            Err(e) => (format!("Не загрузить сюжет: {e}"), true),
        });
    }

    /// Рисует окно диалога и журнал.
    pub(crate) fn draw(&mut self, ctx: &egui::Context, story: &SharedStory) {
        let (screen, problem, journal) = {
            let h = lock_story(story);
            let journal = self.journal.then(|| h.story.journal(&h.state));
            (
                h.screen.clone().filter(|_| h.busy()),
                h.problems.last().cloned(),
                journal,
            )
        };
        if let Some(screen) = screen
            && let Some(answer) = dialog_window(ctx, &screen, problem.as_deref())
        {
            lock_story(story).answer = Some(answer);
        }
        if let Some(entries) = journal {
            let mut open = true;
            egui::Window::new("Журнал")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_width(340.0)
                .anchor(Align2::RIGHT_TOP, [-16.0, 60.0])
                .show(ctx, |ui| {
                    if entries.is_empty() {
                        ui.label(RichText::new("Заданий пока нет.").weak());
                    }
                    for e in &entries {
                        let (mark, color) = match e.status {
                            Status::Done => ("✔ ", Color32::from_rgb(120, 200, 120)),
                            Status::Failed => ("✖ ", Color32::from_rgb(220, 100, 90)),
                            _ => ("", ui.visuals().strong_text_color()),
                        };
                        ui.label(
                            RichText::new(format!("{mark}{}", e.title))
                                .size(17.0)
                                .strong()
                                .color(color),
                        );
                        if !e.text.is_empty() {
                            ui.label(&e.text);
                        }
                        for (text, done, need) in &e.goals {
                            ui.label(RichText::new(format!("• {text}: {done} из {need}")).weak());
                        }
                        ui.add_space(8.0);
                    }
                    ui.label(RichText::new("J — закрыть").weak().small());
                });
            if !open {
                self.journal = false;
            }
        }
    }
}

/// Окно реплики внизу экрана. Возвращает ответ, если игрок нажал мышью.
fn dialog_window(ctx: &egui::Context, screen: &Screen, problem: Option<&str>) -> Option<Answer> {
    let mut answer = None;
    let width = (ctx.content_rect().width() - 48.0).clamp(240.0, 760.0);
    egui::Window::new("Разговор")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .fixed_size([width, 0.0])
        .anchor(Align2::CENTER_BOTTOM, [0.0, -24.0])
        .frame(
            egui::Frame::window(&ctx.global_style())
                .fill(Color32::from_rgba_unmultiplied(18, 20, 26, 235))
                .stroke(Stroke::new(1.0, Color32::from_gray(90)))
                .inner_margin(16.0),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            match screen {
                Screen::Line(s) => {
                    let color = parse_hex(&s.color).unwrap_or(Color32::from_rgb(230, 200, 120));
                    ui.label(RichText::new(&s.name).size(18.0).strong().color(color));
                    ui.add_space(4.0);
                    ui.label(RichText::new(&s.text).size(19.0).color(Color32::WHITE));
                    ui.add_space(10.0);
                    if s.choices.is_empty() {
                        if ui
                            .add(egui::Button::new(RichText::new("Дальше  ⏎").size(16.0)))
                            .clicked()
                        {
                            answer = Some(Answer::Next);
                        }
                    } else {
                        for (i, c) in s.choices.iter().enumerate() {
                            // Уже выбранный раньше вариант — бледнее (как в играх с
                            // диалогами: видно, что спрашивал).
                            let text = RichText::new(format!("{}. {}", i + 1, c.text)).size(17.0);
                            let text = if c.seen {
                                text.color(Color32::from_gray(150))
                            } else {
                                text.color(Color32::from_rgb(240, 220, 160))
                            };
                            if ui.add(egui::Button::new(text).frame(false)).clicked() {
                                answer = Some(Answer::Choose(i));
                            }
                        }
                    }
                }
                Screen::Pause => {
                    if ui
                        .add(egui::Button::new(RichText::new("…  ⏎").size(18.0)))
                        .clicked()
                    {
                        answer = Some(Answer::Next);
                    }
                }
            }
            if let Some(p) = problem {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(p)
                        .small()
                        .color(Color32::from_rgb(230, 110, 90)),
                );
            }
        });
    answer
}

fn parse_hex(s: &str) -> Option<Color32> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reyvec_scene::StoryHost;
    use reyvec_scene::story::Story;
    use std::sync::{Arc, Mutex};

    fn story() -> SharedStory {
        let (s, problems) = Story::from_texts(reyvec_scene::story::EXAMPLE);
        assert!(problems.is_empty(), "{problems:?}");
        Arc::new(Mutex::new(StoryHost::new(s)))
    }

    #[test]
    fn keys_answer_the_talk_and_open_the_journal() {
        let s = story();
        let mut w = StoryWindow::at(std::env::temp_dir().join("reyvec-test-story.ron"));
        let key = |c| Input::Key(c);
        // Разговора нет: пробел — герою.
        assert!(!w.key(&s, key(KeyCode::Space), true));
        assert!(w.key(&s, key(KeyCode::KeyJ), true));
        assert!(w.journal);
        {
            let mut h = lock_story(&s);
            h.talk("kuznec.privet").unwrap();
            h.advance(0.0);
        }
        // Реплика без вариантов: цифра забирается, но ничего не выбирает; пробел —
        // «дальше»; отпускание — игре.
        assert!(w.key(&s, key(KeyCode::Digit1), true));
        assert_eq!(lock_story(&s).answer, None);
        assert!(!w.key(&s, key(KeyCode::Space), false));
        assert!(w.key(&s, key(KeyCode::Space), true));
        assert_eq!(lock_story(&s).answer, Some(Answer::Next));
        lock_story(&s).advance(0.0);
        // Варианты: пробел забирается (герой не прыгает), но ничего не выбирает.
        assert!(w.key(&s, key(KeyCode::Space), true));
        assert_eq!(lock_story(&s).answer, None);
        assert!(w.key(&s, key(KeyCode::Digit2), true));
        assert_eq!(lock_story(&s).answer, Some(Answer::Choose(1)));
        // Третьего видимого варианта нет.
        lock_story(&s).answer = None;
        w.key(&s, key(KeyCode::Digit3), true);
        assert_eq!(lock_story(&s).answer, None);
    }

    #[test]
    fn save_and_load_go_through_the_file() {
        let path = std::env::temp_dir().join(format!("reyvec-story-{}.ron", std::process::id()));
        let s = story();
        let mut w = StoryWindow::at(path.clone());
        {
            let mut h = lock_story(&s);
            let StoryHost { story, state, .. } = &mut *h;
            story.quest_start(state, "mech").unwrap();
        }
        w.save(&s);
        assert!(!w.note.as_ref().unwrap().1, "{:?}", w.note);
        let other = story();
        w.load(&other);
        assert!(!w.note.as_ref().unwrap().1, "{:?}", w.note);
        let h = lock_story(&other);
        assert_eq!(h.story.quest_state(&h.state, "mech").status, Status::Active);
        drop(h);
        let _ = std::fs::remove_file(&path);
        w.load(&other);
        assert!(w.note.as_ref().unwrap().1, "нет файла — понятная ошибка");
    }

    #[test]
    fn draws_the_line_choices_and_journal() {
        let _turn = crate::console::DRAWING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let s = story();
        {
            let mut h = lock_story(&s);
            let StoryHost { story, state, .. } = &mut *h;
            story.quest_start(state, "mech").unwrap();
            h.talk("kuznec.privet").unwrap();
            h.advance(0.0);
            h.answer = Some(Answer::Next);
            h.advance(0.0);
        }
        let mut w = StoryWindow::at(std::env::temp_dir().join("reyvec-test-story.ron"));
        w.journal = true;
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| w.draw(ui.ctx(), &s));
        assert!(!out.shapes.is_empty());
        out.textures_delta.clear();
    }
}
