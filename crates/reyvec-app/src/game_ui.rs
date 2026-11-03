//! Игровой интерфейс (6.10б): здоровье героя, монеты, числа урона над целями и меню
//! паузы с настройкой клавиш. Рисуется через egui поверх кадра, только в сцене с героем.
//!
//! Esc открывает и закрывает меню; пока оно открыто, мир стоит на паузе. В меню
//! «Управление» у каждого действия можно заменить клавишу или добавить ещё одну;
//! настройки сразу сохраняются в файл ([`crate::controls`]).

use std::path::PathBuf;
use std::time::Instant;

use egui::{Align2, Color32, FontId, LayerId, Pos2, Rect, RichText, Stroke, Vec2};
use reyvec_scene::game::Action;
use reyvec_scene::{GameEvent, PlayerView};
use winit::keyboard::KeyCode;

use crate::controls::{Controls, MAX_KEYS, controls_path, key_name};

/// Сколько живёт число урона, секунды, и на сколько поднимается (единиц сцены).
const FLOAT_LIFE: f32 = 0.9;
const FLOAT_RISE: f32 = 0.22;
/// Сколько видна надпись «Герой повержен», секунды.
const BANNER_LIFE: f32 = 2.0;

const RED: Color32 = Color32::from_rgb(235, 70, 60);
const GOLD: Color32 = Color32::from_rgb(255, 205, 70);

/// Число над целью: где в сцене, текст, цвет и сколько ему уже секунд.
struct Float {
    at: [f32; 2],
    text: String,
    color: Color32,
    age: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Closed,
    Main,
    Controls,
}

/// Ждём клавишу для действия: заменить его клавиши или добавить ещё одну.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Capture {
    action: Action,
    replace: bool,
}

/// Что нужно окну для кадра интерфейса.
pub(crate) struct GameView {
    pub player: Option<PlayerView>,
    /// Камера: середина и приближение.
    pub center: [f64; 2],
    pub zoom: f64,
}

pub(crate) struct GameUi {
    controls: Controls,
    path: PathBuf,
    menu: Menu,
    capture: Option<Capture>,
    floats: Vec<Float>,
    banner: f32,
    coins: u32,
    /// Строка под настройками: куда сохранено или что не так.
    note: Option<(String, bool)>,
    quit: bool,
    last: Instant,
}

impl GameUi {
    /// Клавиши из файла настроек рядом с программой.
    pub(crate) fn new() -> Self {
        let path = controls_path();
        Self {
            controls: Controls::load(&path),
            path,
            menu: Menu::Closed,
            capture: None,
            floats: Vec::new(),
            banner: 0.0,
            coins: 0,
            note: None,
            quit: false,
            last: Instant::now(),
        }
    }

    /// Действие клавиши (меню закрыто).
    pub(crate) fn action(&self, code: KeyCode) -> Option<Action> {
        if self.menu_open() {
            None
        } else {
            self.controls.action(code)
        }
    }

    pub(crate) fn menu_open(&self) -> bool {
        self.menu != Menu::Closed
    }

    /// Esc: назад по меню, из игры — открыть меню.
    pub(crate) fn escape(&mut self) {
        self.menu = match self.menu {
            Menu::Closed => Menu::Main,
            Menu::Main => Menu::Closed,
            Menu::Controls => Menu::Main,
        };
        self.capture = None;
    }

    /// Меню ждёт клавишу для назначения.
    pub(crate) fn capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// Нажатая клавиша, пока меню ждёт её. Esc — отмена.
    pub(crate) fn capture_key(&mut self, code: KeyCode) {
        let Some(c) = self.capture.take() else {
            return;
        };
        if code == KeyCode::Escape {
            return;
        }
        if self.controls.bind(c.action, code, c.replace) {
            self.save();
        } else {
            self.note = Some(("Эту клавишу назначить нельзя".into(), true));
        }
    }

    /// Нажали «Выйти» в меню.
    pub(crate) fn take_quit(&mut self) -> bool {
        std::mem::take(&mut self.quit)
    }

    /// Событие игры: число урона, монета, надпись.
    pub(crate) fn on_event(&mut self, event: &GameEvent) {
        match *event {
            GameEvent::Damage { at, amount, left } => self.floats.push(Float {
                at,
                text: if left == 0 {
                    format!("−{amount}  разбит!")
                } else {
                    format!("−{amount}")
                },
                color: Color32::WHITE,
                age: 0.0,
            }),
            GameEvent::Hurt { at, amount, .. } => self.floats.push(Float {
                at: [at[0], at[1] + 0.2],
                text: format!("−{amount}"),
                color: RED,
                age: 0.0,
            }),
            GameEvent::Died { .. } => self.banner = BANNER_LIFE,
            GameEvent::Pickup { at } => {
                self.coins += 1;
                self.floats.push(Float {
                    at: [at[0], at[1] + 0.08],
                    text: "+1".into(),
                    color: GOLD,
                    age: 0.0,
                });
            }
            _ => {}
        }
    }

    fn save(&mut self) {
        self.note = Some(match self.controls.save(&self.path) {
            Ok(()) => (format!("Сохранено: {}", self.path.display()), false),
            Err(e) => (format!("Не сохранить {}: {e}", self.path.display()), true),
        });
    }

    /// Рисует интерфейс кадра.
    pub(crate) fn draw(&mut self, ctx: &egui::Context, view: &GameView) {
        let now = Instant::now();
        // На паузе числа не летят и надпись не гаснет.
        let dt = if self.menu_open() {
            0.0
        } else {
            now.duration_since(self.last).as_secs_f32().min(0.1)
        };
        self.last = now;
        for f in &mut self.floats {
            f.age += dt;
        }
        self.floats.retain(|f| f.age < FLOAT_LIFE);
        self.banner = (self.banner - dt).max(0.0);

        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(LayerId::background());
        if let Some(p) = &view.player {
            hud(&painter, screen, p, self.coins);
        }
        let project = |at: [f32; 2]| {
            let h = screen.height();
            let sx = (f64::from(at[0]) - view.center[0]) * view.zoom;
            let sy = (f64::from(at[1]) - view.center[1]) * view.zoom;
            Pos2::new(
                screen.center().x + sx as f32 * h * 0.5,
                screen.center().y - sy as f32 * h * 0.5,
            )
        };
        for f in &self.floats {
            let k = f.age / FLOAT_LIFE;
            let pos = project([f.at[0], f.at[1] + FLOAT_RISE * (1.0 - (1.0 - k).powi(2))]);
            let alpha = ((1.0 - k) / 0.45).min(1.0);
            let size = 24.0 + 8.0 * (1.0 - (k * 5.0).min(1.0));
            outlined(&painter, pos, &f.text, size, f.color.gamma_multiply(alpha));
        }
        if self.banner > 0.0 {
            let alpha = (self.banner / 0.5).min(1.0);
            outlined(
                &painter,
                screen.center() - Vec2::new(0.0, screen.height() * 0.2),
                "Герой повержен",
                44.0,
                RED.gamma_multiply(alpha),
            );
        }
        if self.menu_open() {
            painter.rect_filled(screen, 0.0, Color32::from_black_alpha(150));
            self.menu_window(ctx);
        }
    }

    fn menu_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("Пауза")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| match self.menu {
                Menu::Main | Menu::Closed => self.main_menu(ui),
                Menu::Controls => self.controls_menu(ui),
            });
    }

    fn main_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(260.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Пауза").size(28.0).strong());
            ui.add_space(12.0);
            let big = |t: &str| {
                egui::Button::new(RichText::new(t).size(18.0)).min_size([220.0, 36.0].into())
            };
            if ui.add(big("Продолжить")).clicked() {
                self.menu = Menu::Closed;
            }
            if ui.add(big("Управление")).clicked() {
                self.menu = Menu::Controls;
                self.note = None;
            }
            if ui.add(big("Выйти из программы")).clicked() {
                self.quit = true;
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Esc — вернуться в игру").weak());
        });
    }

    fn controls_menu(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Управление").size(24.0).strong());
        ui.add_space(8.0);
        egui::Grid::new("клавиши")
            .num_columns(4)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                for a in Action::ALL {
                    ui.label(RichText::new(a.label()).size(17.0));
                    let keys = self.controls.keys(a);
                    let waiting = self.capture.filter(|c| c.action == a);
                    if waiting.is_some() {
                        ui.label(RichText::new("нажмите клавишу…").color(GOLD).size(17.0));
                    } else if keys.is_empty() {
                        ui.label(RichText::new("нет клавиши").color(RED).size(17.0));
                    } else {
                        let names: Vec<&str> = keys.iter().filter_map(|k| key_name(*k)).collect();
                        ui.label(RichText::new(names.join("  /  ")).size(17.0));
                    }
                    if ui.button("Заменить").clicked() {
                        self.capture = Some(Capture {
                            action: a,
                            replace: true,
                        });
                    }
                    let full = keys.len() >= MAX_KEYS;
                    if ui
                        .add_enabled(!full, egui::Button::new("Добавить"))
                        .clicked()
                    {
                        self.capture = Some(Capture {
                            action: a,
                            replace: false,
                        });
                    }
                    ui.end_row();
                }
            });
        ui.add_space(6.0);
        if let Some(c) = self.capture {
            ui.label(
                RichText::new(format!(
                    "Нажмите клавишу для «{}». Esc — отмена.",
                    c.action.label()
                ))
                .color(GOLD),
            );
        }
        let unbound = self.controls.unbound();
        if !unbound.is_empty() {
            let names: Vec<&str> = unbound.iter().map(|a| a.label()).collect();
            ui.label(RichText::new(format!("Без клавиши: {}", names.join(", "))).color(RED));
        }
        ui.label(
            RichText::new(
                "Геймпад: стик или крестовина — бег, A (крест) — прыжок, \
                 X (квадрат) или курок — рывок, Y (треугольник) — удар. \
                 Настройка кнопок геймпада — позже.",
            )
            .weak(),
        );
        if let Some((text, bad)) = &self.note {
            let t = RichText::new(text.as_str()).small();
            ui.label(if *bad { t.color(RED) } else { t.weak() });
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("По умолчанию").clicked() {
                self.controls = Controls::default();
                self.capture = None;
                self.save();
            }
            if ui.button("Назад").clicked() {
                self.menu = Menu::Main;
                self.capture = None;
            }
        });
    }
}

/// Здоровье сверху посередине: сердечки-плашки, под ними монеты.
fn hud(painter: &egui::Painter, screen: Rect, p: &PlayerView, coins: u32) {
    let n = p.max_hp.max(1) as f32;
    let (w, gap, h) = (28.0, 6.0, 18.0);
    let total = n * w + (n - 1.0) * gap;
    let left = screen.center().x - total * 0.5;
    let top = screen.top() + 16.0;
    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(left - 10.0, top - 8.0),
            Vec2::new(total + 20.0, h + 42.0),
        ),
        8.0,
        Color32::from_black_alpha(110),
    );
    // Неуязвимый — плашки мигают вместе с героем.
    let blink = p.invulnerable;
    for i in 0..p.max_hp {
        let r = Rect::from_min_size(Pos2::new(left + i as f32 * (w + gap), top), Vec2::new(w, h));
        if i < p.hp {
            let c = if blink {
                Color32::from_rgb(255, 140, 120)
            } else {
                RED
            };
            painter.rect_filled(r, 5.0, c);
        } else {
            painter.rect_stroke(
                r,
                5.0,
                Stroke::new(2.0, Color32::from_gray(120)),
                egui::StrokeKind::Inside,
            );
        }
    }
    let line = top + h + 14.0;
    painter.circle_filled(Pos2::new(screen.center().x - 34.0, line), 7.0, GOLD);
    painter.text(
        Pos2::new(screen.center().x - 22.0, line),
        Align2::LEFT_CENTER,
        format!("Монеты: {coins}"),
        FontId::proportional(16.0),
        Color32::WHITE,
    );
}

/// Текст с тёмной обводкой: читается на любом фоне.
fn outlined(painter: &egui::Painter, pos: Pos2, text: &str, size: f32, color: Color32) {
    let font = FontId::proportional(size);
    let shadow = Color32::from_black_alpha(color.a());
    for d in [[-2.0, 0.0], [2.0, 0.0], [0.0, -2.0], [0.0, 2.0]] {
        painter.text(
            pos + Vec2::new(d[0], d[1]),
            Align2::CENTER_CENTER,
            text,
            font.clone(),
            shadow,
        );
    }
    painter.text(pos, Align2::CENTER_CENTER, text, font, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui() -> GameUi {
        GameUi {
            controls: Controls::default(),
            path: std::env::temp_dir().join("reyvec-test-controls.ron"),
            menu: Menu::Closed,
            capture: None,
            floats: Vec::new(),
            banner: 0.0,
            coins: 0,
            note: None,
            quit: false,
            last: Instant::now(),
        }
    }

    #[test]
    fn escape_walks_the_menu_and_menu_blocks_game_keys() {
        let mut g = ui();
        assert_eq!(g.action(KeyCode::Space), Some(Action::Jump));
        g.escape();
        assert!(g.menu_open());
        assert_eq!(
            g.action(KeyCode::Space),
            None,
            "в меню клавиши не идут в игру"
        );
        g.menu = Menu::Controls;
        g.escape();
        assert_eq!(g.menu, Menu::Main);
        g.escape();
        assert!(!g.menu_open());
    }

    #[test]
    fn capture_binds_the_next_key_and_escape_cancels() {
        let mut g = ui();
        g.capture = Some(Capture {
            action: Action::Jump,
            replace: true,
        });
        assert!(g.capturing());
        g.capture_key(KeyCode::Escape);
        assert!(!g.capturing());
        assert_eq!(g.controls.keys(Action::Jump), &[KeyCode::Space]);
        g.capture = Some(Capture {
            action: Action::Jump,
            replace: true,
        });
        g.capture_key(KeyCode::KeyW);
        assert_eq!(g.controls.keys(Action::Jump), &[KeyCode::KeyW]);
        let _ = std::fs::remove_file(&g.path);
    }

    #[test]
    fn events_make_numbers_coins_and_banner() {
        let mut g = ui();
        g.on_event(&GameEvent::Damage {
            at: [0.0, 0.0],
            amount: 1,
            left: 0,
        });
        g.on_event(&GameEvent::Pickup { at: [0.0, 0.0] });
        g.on_event(&GameEvent::Died { at: [0.0, 0.0] });
        assert_eq!(g.floats.len(), 2);
        assert!(g.floats[0].text.contains("разбит"));
        assert_eq!(g.coins, 1);
        assert!(g.banner > 0.0);
    }

    #[test]
    fn draws_hud_numbers_and_both_menus_without_a_window() {
        let mut g = ui();
        g.on_event(&GameEvent::Hurt {
            at: [0.1, 0.0],
            amount: 1,
            dir: 1.0,
        });
        let view = GameView {
            player: Some(PlayerView {
                hp: 3,
                max_hp: 5,
                ..Default::default()
            }),
            center: [0.0, 0.0],
            zoom: 1.0,
        };
        let ctx = egui::Context::default();
        for menu in [Menu::Closed, Menu::Main, Menu::Controls] {
            g.menu = menu;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| g.draw(ui.ctx(), &view));
            assert!(!out.shapes.is_empty(), "{menu:?}");
            out.textures_delta.clear();
        }
    }
}
