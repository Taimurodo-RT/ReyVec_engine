//! Консоль разработчика (6.11): клавиша `~` (Ё) в сцене с героем открывает строку
//! команд для отладки: переставить героя, задать здоровье, бессмертие, пауза и шаги
//! логики по одному, замедление времени, показ столкновений. В журнале консоли —
//! ответы команд, предупреждения и ошибки программы и, по желанию, события игры.
//!
//! Команды пишутся по-русски (есть и короткие английские имена). Разбор строки —
//! [`parse`]; выполняет команды окно, у которого есть поток логики сцены.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Mutex;

use egui::{Align2, Color32, FontId, Key, LayerId, Pos2, Rect, RichText, Stroke};
use reyvec_scene::game::Collider;
use reyvec_scene::{DebugCommand, GameEvent, PlayerView, TIME_SCALE};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// Сколько строк помнит журнал.
const LINES: usize = 300;
/// Сколько команд помнит история (стрелки вверх и вниз).
const HISTORY: usize = 50;

pub(crate) const GOLD: Color32 = Color32::from_rgb(255, 205, 70);
pub(crate) const RED: Color32 = Color32::from_rgb(240, 90, 80);
const GREY: Color32 = Color32::from_rgb(170, 170, 170);
const GREEN: Color32 = Color32::from_rgb(110, 220, 120);
const BLUE: Color32 = Color32::from_rgb(120, 180, 255);

/// Команда консоли.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Command {
    Help,
    Clear,
    /// Для мира сцены (выполняет поток логики).
    World(DebugCommand),
    /// Телепорт без координат: туда, где сейчас мышь (точку подставляет консоль).
    TeleportMouse,
    /// Пауза логики: вкл, выкл или переключить.
    Pause(Option<bool>),
    /// На паузе сделать столько шагов (если паузы нет, она включается).
    Step(u32),
    /// Скорость времени: 1 — обычная.
    Slow(f64),
    /// Показ столкновений.
    Colliders(Option<bool>),
    /// События игры в журнал.
    Events(Option<bool>),
}

/// Что выполнить окну: команда для логики сцены.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Order {
    World(DebugCommand),
    Pause(Option<bool>),
    Step(u32),
    Slow(f64),
}

const HELP: &[(&str, &str)] = &[
    ("помощь", "эта подсказка (help, ?)"),
    (
        "телепорт x y",
        "герой в точку сцены; без чисел — туда, где мышь (tp)",
    ),
    ("где", "где герой и с какой скоростью (where)"),
    ("здоровье N", "задать здоровье героя (hp)"),
    ("бессмертие [вкл|выкл]", "враги не ранят (god)"),
    (
        "вначало",
        "герой в начало уровня, здоровье полное (respawn)",
    ),
    (
        "пауза [вкл|выкл]",
        "остановить логику, меню её не снимает (pause)",
    ),
    ("шаг [N]", "на паузе — N шагов логики по 1/60 с (step)"),
    (
        "медленно [k]",
        "скорость времени, 0,05…4; без числа 0,25; «медленно 1» — обычная (slow)",
    ),
    (
        "столкновения [вкл|выкл]",
        "рамки твёрдого и героя (colliders)",
    ),
    ("события [вкл|выкл]", "события игры в журнал (events)"),
    ("очистить", "очистить журнал (clear)"),
];

/// Разбирает строку консоли.
pub(crate) fn parse(line: &str) -> Result<Command, String> {
    let mut words = line.split_whitespace();
    let Some(name) = words.next() else {
        return Err("пустая команда".into());
    };
    let args: Vec<&str> = words.collect();
    let name = name.to_lowercase();
    let toggle = |args: &[&str]| -> Result<Option<bool>, String> {
        match args {
            [] => Ok(None),
            [a] => match a.to_lowercase().as_str() {
                "вкл" | "on" | "1" | "да" => Ok(Some(true)),
                "выкл" | "off" | "0" | "нет" => Ok(Some(false)),
                other => Err(format!("«{other}»: нужно вкл или выкл")),
            },
            _ => Err("лишние слова: нужно вкл или выкл".into()),
        }
    };
    let none = |args: &[&str], cmd: Command| {
        if args.is_empty() {
            Ok(cmd)
        } else {
            Err(format!("«{name}» без чисел и слов"))
        }
    };
    match name.as_str() {
        "помощь" | "help" | "?" => none(&args, Command::Help),
        "очистить" | "clear" | "cls" => none(&args, Command::Clear),
        "телепорт" | "tp" => match args.as_slice() {
            [] => Ok(Command::TeleportMouse),
            [x, y] => Ok(Command::World(DebugCommand::Teleport([
                number(x)?,
                number(y)?,
            ]))),
            _ => Err("телепорт x y — два числа, или без чисел — к мыши".into()),
        },
        "где" | "where" => none(&args, Command::World(DebugCommand::Where)),
        "здоровье" | "hp" => match args.as_slice() {
            [n] => {
                let n: u32 = n.parse().map_err(|_| format!("«{n}»: нужно целое число"))?;
                Ok(Command::World(DebugCommand::Health(n)))
            }
            _ => Err("здоровье N — одно целое число".into()),
        },
        "бессмертие" | "god" => Ok(Command::World(DebugCommand::God(toggle(&args)?))),
        "вначало" | "заново" | "respawn" => {
            none(&args, Command::World(DebugCommand::Respawn))
        }
        "пауза" | "pause" => Ok(Command::Pause(toggle(&args)?)),
        "шаг" | "step" => match args.as_slice() {
            [] => Ok(Command::Step(1)),
            [n] => match n.parse::<u32>() {
                Ok(n @ 1..=600) => Ok(Command::Step(n)),
                _ => Err(format!("«{n}»: нужно целое от 1 до 600")),
            },
            _ => Err("шаг N — одно целое число".into()),
        },
        "медленно" | "скорость" | "slow" => match args.as_slice() {
            [] => Ok(Command::Slow(0.25)),
            [k] => {
                let k = number(k)?;
                if k > 0.0 {
                    Ok(Command::Slow(k))
                } else {
                    Err("скорость больше нуля (пауза — «пауза»)".into())
                }
            }
            _ => Err("медленно k — одно число".into()),
        },
        "столкновения" | "colliders" => Ok(Command::Colliders(toggle(&args)?)),
        "события" | "events" => Ok(Command::Events(toggle(&args)?)),
        _ => Err(format!("нет команды «{name}». Список — «помощь»")),
    }
}

/// Число с точкой или запятой.
fn number(text: &str) -> Result<f64, String> {
    text.replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("«{text}»: нужно число"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Input,
    Reply,
    Error,
    Warn,
    Info,
    Event,
}

impl Kind {
    fn color(self) -> Color32 {
        match self {
            Kind::Input => Color32::WHITE,
            Kind::Reply => GREEN,
            Kind::Error => RED,
            Kind::Warn => GOLD,
            Kind::Info => GREY,
            Kind::Event => BLUE,
        }
    }
}

/// Строки логов программы, ещё не взятые консолью (их кладёт [`ConsoleLog`]).
static LOG: Mutex<VecDeque<(Kind, String)>> = Mutex::new(VecDeque::new());

/// Забирает строки логов программы, ещё не взятые консолью, с цветом их уровня
/// (для консоли редактора).
pub(crate) fn take_log() -> Vec<(Color32, String)> {
    LOG.lock()
        .map(|mut log| log.drain(..).map(|(k, t)| (k.color(), t)).collect())
        .unwrap_or_default()
}

/// Слой логов: копирует сообщения программы (от «info» и выше) в журнал консоли.
pub struct ConsoleLog;

impl<S: Subscriber> Layer<S> for ConsoleLog {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let kind = match *event.metadata().level() {
            Level::ERROR => Kind::Error,
            Level::WARN => Kind::Warn,
            Level::INFO => Kind::Info,
            _ => return,
        };
        let mut text = Message(String::new());
        event.record(&mut text);
        if let Ok(mut log) = LOG.lock() {
            if log.len() >= LINES {
                log.pop_front();
            }
            log.push_back((kind, text.0));
        }
    }
}

/// Текст сообщения лога и его поля через пробел.
struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else {
            let _ = write!(self.0, "{}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            if !self.0.is_empty() {
                self.0.push(' ');
            }
            self.0.push_str(value);
        } else {
            self.record_debug(field, &value);
        }
    }
}

/// Консоль: открыта ли, строка ввода, журнал и история команд.
pub(crate) struct Console {
    open: bool,
    input: String,
    lines: VecDeque<(Kind, String)>,
    history: Vec<String>,
    /// Где в истории (стрелками), `None` — новая строка.
    browsing: Option<usize>,
    /// Только что открыли клавишей `~`: её символ в строку не попадает.
    swallow: bool,
    colliders: bool,
    events: bool,
    orders: Vec<Order>,
}

/// Что консоли нужно в кадре: камера, герой и твёрдое для рамок.
pub(crate) struct ConsoleView<'a> {
    pub center: [f64; 2],
    pub zoom: f64,
    pub player: Option<&'a PlayerView>,
    pub solids: &'a [Collider],
}

impl Console {
    pub(crate) fn new() -> Self {
        let mut c = Self {
            open: false,
            input: String::new(),
            lines: VecDeque::new(),
            history: Vec::new(),
            browsing: None,
            swallow: false,
            colliders: false,
            events: false,
            orders: Vec::new(),
        };
        c.print(
            Kind::Info,
            "Консоль разработчика. «помощь» — список команд, ~ — закрыть.",
        );
        c
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    /// `~`: открыть или закрыть.
    pub(crate) fn toggle(&mut self) {
        self.open = !self.open;
        self.swallow = self.open;
        self.browsing = None;
    }

    /// Показ столкновений включён (рамки рисуются и при закрытой консоли).
    pub(crate) fn colliders(&self) -> bool {
        self.colliders
    }

    /// Команды для логики сцены, набранные с прошлого вызова.
    pub(crate) fn take_orders(&mut self) -> Vec<Order> {
        std::mem::take(&mut self.orders)
    }

    /// Ответ логики на команду.
    pub(crate) fn reply(&mut self, reply: Result<String, String>) {
        match reply {
            Ok(text) => self.print(Kind::Reply, &text),
            Err(text) => self.print(Kind::Error, &text),
        }
    }

    /// Строка ответа окна (пауза, скорость).
    pub(crate) fn say(&mut self, text: &str) {
        self.print(Kind::Reply, text);
    }

    /// Событие игры: в журнал, если включено «события».
    pub(crate) fn on_event(&mut self, event: &GameEvent) {
        if self.events && !matches!(event, GameEvent::Run { .. }) {
            self.print(Kind::Event, &format!("{event:?}"));
        }
    }

    fn print(&mut self, kind: Kind, text: &str) {
        if self.lines.len() >= LINES {
            self.lines.pop_front();
        }
        self.lines.push_back((kind, text.to_owned()));
    }

    /// Выполняет строку: то, что консоль делает сама, или поручение окну.
    pub(crate) fn submit(&mut self, line: &str, mouse: Option<[f64; 2]>) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        self.print(Kind::Input, &format!("> {line}"));
        if self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_owned());
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
        }
        let command = match parse(line) {
            Ok(c) => c,
            Err(e) => return self.print(Kind::Error, &e),
        };
        match command {
            Command::Help => {
                for (name, what) in HELP {
                    self.print(Kind::Info, &format!("{name:<26} {what}"));
                }
            }
            Command::Clear => self.lines.clear(),
            Command::World(c) => self.orders.push(Order::World(c)),
            Command::TeleportMouse => match mouse {
                Some(at) => self.orders.push(Order::World(DebugCommand::Teleport(at))),
                None => self.print(Kind::Error, "мышь не над окном: телепорт x y"),
            },
            Command::Pause(on) => self.orders.push(Order::Pause(on)),
            Command::Step(n) => self.orders.push(Order::Step(n)),
            Command::Slow(k) => self
                .orders
                .push(Order::Slow(k.clamp(TIME_SCALE.0, TIME_SCALE.1))),
            Command::Colliders(on) => {
                self.colliders = on.unwrap_or(!self.colliders);
                let text = if self.colliders {
                    "столкновения: рамки показаны"
                } else {
                    "столкновения: рамки скрыты"
                };
                self.say(text);
            }
            Command::Events(on) => {
                self.events = on.unwrap_or(!self.events);
                let text = if self.events {
                    "события игры пишутся в журнал"
                } else {
                    "события игры не пишутся"
                };
                self.say(text);
            }
        }
    }

    /// Рисует рамки столкновений и, если открыта, саму консоль.
    pub(crate) fn draw(&mut self, ctx: &egui::Context, view: &ConsoleView) {
        // Логи программы — в журнал, даже пока консоль закрыта.
        if let Ok(mut log) = LOG.lock() {
            for (kind, text) in log.drain(..) {
                self.print(kind, &text);
            }
        }
        let screen = ctx.content_rect();
        let h = screen.height();
        let project = |at: [f64; 2]| {
            Pos2::new(
                screen.center().x + ((at[0] - view.center[0]) * view.zoom) as f32 * h * 0.5,
                screen.center().y - ((at[1] - view.center[1]) * view.zoom) as f32 * h * 0.5,
            )
        };
        if self.colliders {
            let painter = ctx.layer_painter(LayerId::background());
            for c in view.solids {
                let r = Rect::from_two_pos(
                    project([c.aabb.min[0], c.aabb.min[1]]),
                    project([c.aabb.max[0], c.aabb.max[1]]),
                );
                if !r.intersects(screen) {
                    continue;
                }
                let color = if c.oneway {
                    Color32::from_rgb(90, 200, 255)
                } else if c.slope != 0.0 {
                    Color32::from_rgb(255, 160, 60)
                } else {
                    Color32::from_rgb(80, 255, 120)
                };
                painter.rect_stroke(r, 0.0, Stroke::new(1.5, color), egui::StrokeKind::Inside);
            }
            if let Some(p) = view.player {
                let half = [f64::from(p.size[0]) * 0.5, f64::from(p.size[1]) * 0.5];
                let at = [f64::from(p.at[0]), f64::from(p.at[1])];
                let r = Rect::from_two_pos(
                    project([at[0] - half[0], at[1] - half[1]]),
                    project([at[0] + half[0], at[1] + half[1]]),
                );
                painter.rect_stroke(
                    r,
                    0.0,
                    Stroke::new(2.0, Color32::WHITE),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    r.center_top() - egui::vec2(0.0, 4.0),
                    Align2::CENTER_BOTTOM,
                    format!("{:.2}, {:.2}", p.at[0], p.at[1]),
                    FontId::monospace(13.0),
                    Color32::WHITE,
                );
            }
        }
        if !self.open {
            return;
        }
        // Точка сцены под мышью (для «телепорт» без чисел).
        let mouse = ctx.pointer_latest_pos().map(|m| {
            [
                view.center[0] + f64::from((m.x - screen.center().x) / (h * 0.5)) / view.zoom,
                view.center[1] - f64::from((m.y - screen.center().y) / (h * 0.5)) / view.zoom,
            ]
        });
        let mut submit = None;
        egui::Window::new("Консоль")
            .title_bar(false)
            .resizable(false)
            .anchor(Align2::LEFT_BOTTOM, [12.0, -12.0])
            .fixed_size([(screen.width() - 24.0).min(900.0), h * 0.35])
            .frame(egui::Frame::window(&ctx.global_style()).fill(Color32::from_black_alpha(220)))
            .show(ctx, |ui| {
                let rows = (h * 0.35 - 40.0).max(60.0);
                egui::ScrollArea::vertical()
                    .max_height(rows)
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (kind, text) in &self.lines {
                            ui.label(
                                RichText::new(text)
                                    .family(egui::FontFamily::Monospace)
                                    .color(kind.color()),
                            );
                        }
                    });
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.input)
                        .font(egui::TextStyle::Monospace)
                        .hint_text("команда… («помощь» — список)")
                        .desired_width(f32::INFINITY),
                );
                edit.request_focus();
                if self.swallow {
                    // Символ клавиши, открывшей консоль.
                    self.input.retain(|ch| !matches!(ch, '`' | '~' | 'ё' | 'Ё'));
                    self.swallow = false;
                }
                let (enter, up, down) = ui.input(|i| {
                    (
                        i.key_pressed(Key::Enter),
                        i.key_pressed(Key::ArrowUp),
                        i.key_pressed(Key::ArrowDown),
                    )
                });
                if enter {
                    submit = Some(std::mem::take(&mut self.input));
                    self.browsing = None;
                } else if up && !self.history.is_empty() {
                    let i = self
                        .browsing
                        .map_or(self.history.len() - 1, |i| i.saturating_sub(1));
                    self.browsing = Some(i);
                    self.input.clone_from(&self.history[i]);
                } else if down && let Some(i) = self.browsing {
                    if i + 1 < self.history.len() {
                        self.browsing = Some(i + 1);
                        self.input.clone_from(&self.history[i + 1]);
                    } else {
                        self.browsing = None;
                        self.input.clear();
                    }
                }
            });
        if let Some(line) = submit {
            self.submit(&line, mouse);
        }
    }

    #[cfg(test)]
    fn last(&self) -> &str {
        self.lines.back().map_or("", |(_, t)| t.as_str())
    }
}

/// `draw` забирает общий журнал логов: тесты, которые рисуют консоль (и здесь, и в
/// `game_ui`), идут по очереди, иначе один заберёт строки, которых ждёт другой.
#[cfg(test)]
pub(crate) static DRAWING: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_russian_and_english_commands() {
        assert_eq!(parse("помощь"), Ok(Command::Help));
        assert_eq!(parse("  HELP "), Ok(Command::Help));
        assert_eq!(
            parse("телепорт 1,5 -0.25"),
            Ok(Command::World(DebugCommand::Teleport([1.5, -0.25])))
        );
        assert_eq!(parse("tp"), Ok(Command::TeleportMouse));
        assert_eq!(
            parse("здоровье 3"),
            Ok(Command::World(DebugCommand::Health(3)))
        );
        assert_eq!(
            parse("бессмертие"),
            Ok(Command::World(DebugCommand::God(None)))
        );
        assert_eq!(
            parse("god выкл"),
            Ok(Command::World(DebugCommand::God(Some(false))))
        );
        assert_eq!(parse("пауза вкл"), Ok(Command::Pause(Some(true))));
        assert_eq!(parse("шаг"), Ok(Command::Step(1)));
        assert_eq!(parse("шаг 10"), Ok(Command::Step(10)));
        assert_eq!(parse("медленно"), Ok(Command::Slow(0.25)));
        assert_eq!(parse("медленно 1"), Ok(Command::Slow(1.0)));
        assert_eq!(parse("столкновения"), Ok(Command::Colliders(None)));
        assert_eq!(parse("вначало"), Ok(Command::World(DebugCommand::Respawn)));
        assert_eq!(parse("где"), Ok(Command::World(DebugCommand::Where)));
    }

    #[test]
    fn explains_mistakes_in_russian() {
        assert!(parse("").is_err());
        assert!(parse("летать").unwrap_err().contains("помощь"));
        assert!(parse("телепорт 1").unwrap_err().contains("два числа"));
        assert!(parse("телепорт a b").unwrap_err().contains("нужно число"));
        assert!(parse("здоровье -1").unwrap_err().contains("целое"));
        assert!(parse("шаг 0").is_err());
        assert!(parse("шаг 1000").is_err());
        assert!(parse("медленно 0").unwrap_err().contains("пауза"));
        assert!(parse("пауза может").unwrap_err().contains("вкл или выкл"));
        assert!(parse("где 1").is_err());
    }

    #[test]
    fn console_hands_world_commands_to_the_window_and_keeps_history() {
        let mut c = Console::new();
        c.submit("здоровье 2", None);
        c.submit("телепорт", Some([1.0, 2.0]));
        c.submit("медленно 100", None);
        c.submit("шаг 3", None);
        assert_eq!(
            c.take_orders(),
            vec![
                Order::World(DebugCommand::Health(2)),
                Order::World(DebugCommand::Teleport([1.0, 2.0])),
                Order::Slow(TIME_SCALE.1),
                Order::Step(3),
            ]
        );
        assert!(c.take_orders().is_empty());
        c.submit("телепорт", None);
        assert!(c.last().contains("мышь"));
        c.submit("летать", None);
        assert!(c.last().contains("нет команды"));
        c.submit("столкновения", None);
        assert!(c.colliders());
        c.reply(Err("в сцене нет героя".into()));
        assert_eq!(c.last(), "в сцене нет героя");
        assert_eq!(c.history.len(), 7);
        c.submit("помощь", None);
        assert!(c.lines.len() > HELP.len());
        c.submit("очистить", None);
        assert!(c.lines.is_empty());
    }

    /// Ctrl+V: egui-winit читает буфер обмена системы (возможность `clipboard`) и отдаёт
    /// текст событием вставки. Сам буфер Windows здесь не проверить (только на ПК); тест —
    /// что строка консоли принимает такое событие и перевод строки в конце скопированного
    /// текста команде не мешает.
    #[test]
    fn pasted_text_goes_into_the_command_line() {
        let _turn = DRAWING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ctx = egui::Context::default();
        let view = ConsoleView {
            center: [0.0, 0.0],
            zoom: 1.0,
            player: None,
            solids: &[],
        };
        let mut c = Console::new();
        c.toggle();
        let frame = |c: &mut Console, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 720.0))),
                events,
                ..Default::default()
            };
            // Картинки шрифтов здесь некуда отдавать.
            ctx.run_ui(input, |ui| c.draw(ui.ctx(), &view))
                .textures_delta
                .clear();
        };
        frame(&mut c, Vec::new());
        frame(&mut c, vec![egui::Event::Paste("телепорт 1 2\n".into())]);
        assert_eq!(c.input.trim(), "телепорт 1 2");
        let enter = egui::Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut c, vec![enter]);
        assert_eq!(
            c.take_orders(),
            vec![Order::World(DebugCommand::Teleport([1.0, 2.0]))]
        );
    }

    #[test]
    fn program_logs_reach_the_console() {
        let _turn = DRAWING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use tracing_subscriber::layer::SubscriberExt;
        let subscriber = tracing_subscriber::registry().with(ConsoleLog);
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("Файл сцены: ошибка {}", 7);
            tracing::debug!("не видно");
        });
        let mut c = Console::new();
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            c.draw(
                ui.ctx(),
                &ConsoleView {
                    center: [0.0, 0.0],
                    zoom: 1.0,
                    player: None,
                    solids: &[],
                },
            );
        });
        out.textures_delta.clear();
        assert!(
            c.lines
                .iter()
                .any(|(k, t)| *k == Kind::Warn && t == "Файл сцены: ошибка 7"),
            "{:?}",
            c.lines
        );
        assert!(!c.lines.iter().any(|(_, t)| t.contains("не видно")));
    }
}
