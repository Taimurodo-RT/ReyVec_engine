//! Ввод игрока: действия вместо клавиш.
//!
//! Окно получает нажатия клавиш (раскладка не важна) и пишет в [`InputShared`] действие:
//! «влево», «прыжок». Поток логики 60 раз в секунду читает его через [`InputReader`] и
//! получает [`InputFrame`]: что зажато сейчас и что нажали с прошлого шага. Короткое
//! нажатие, которое началось и кончилось между двумя шагами, не теряется: у каждого
//! действия счётчик нажатий.
//!
//! Между потоками общие только атомарные числа: окно не ждёт логику, логика не ждёт окно.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

/// Действие игрока.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Left,
    Right,
    Up,
    Down,
    Jump,
    Dash,
    /// Удар (6.5).
    Attack,
}

impl Action {
    pub const ALL: [Self; 7] = [
        Self::Left,
        Self::Right,
        Self::Up,
        Self::Down,
        Self::Jump,
        Self::Dash,
        Self::Attack,
    ];

    /// Название для меню.
    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "Влево",
            Self::Right => "Вправо",
            Self::Up => "Вверх",
            Self::Down => "Вниз",
            Self::Jump => "Прыжок",
            Self::Dash => "Рывок",
            Self::Attack => "Удар",
        }
    }

    /// Имя в файле настроек управления.
    pub fn key(self) -> &'static str {
        match self {
            Self::Left => "влево",
            Self::Right => "вправо",
            Self::Up => "вверх",
            Self::Down => "вниз",
            Self::Jump => "прыжок",
            Self::Dash => "рывок",
            Self::Attack => "удар",
        }
    }

    /// Действие по имени из файла настроек.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key() == name)
    }

    fn bit(self) -> u32 {
        1 << self as u32
    }
}

const COUNT: usize = Action::ALL.len();

/// Ввод, общий для окна и потока логики.
#[derive(Debug)]
pub struct InputShared {
    epoch: Instant,
    held: AtomicU32,
    presses: [AtomicU32; COUNT],
    /// Когда было последнее нажатие (наносекунды от `epoch`).
    last_press_ns: AtomicU64,
    /// Точка прицела (6.12): где курсор в координатах сцены (биты `f64`).
    aim: [AtomicU64; 2],
    aim_on: AtomicBool,
    /// Нажатия «с прицелом» с прошлого чтения: действие направлено в точку прицела
    /// (удар мышью в сторону курсора).
    aimed: AtomicU32,
}

impl Default for InputShared {
    fn default() -> Self {
        Self::new()
    }
}

impl InputShared {
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            held: AtomicU32::new(0),
            presses: std::array::from_fn(|_| AtomicU32::new(0)),
            last_press_ns: AtomicU64::new(0),
            aim: [AtomicU64::new(0), AtomicU64::new(0)],
            aim_on: AtomicBool::new(false),
            aimed: AtomicU32::new(0),
        }
    }

    /// Точка прицела в координатах сцены (`None` — курсор вне окна).
    pub fn set_aim(&self, at: Option<[f64; 2]>) {
        if let Some(at) = at {
            self.aim[0].store(at[0].to_bits(), Ordering::Relaxed);
            self.aim[1].store(at[1].to_bits(), Ordering::Relaxed);
        }
        self.aim_on.store(at.is_some(), Ordering::Release);
    }

    /// Нажатие, направленное в точку прицела: как [`Self::set`] с `down`, и шаг узнает,
    /// что это действие нужно сделать в сторону курсора ([`InputFrame::aimed`]).
    pub fn press_aimed(&self, action: Action) {
        self.aimed.fetch_or(action.bit(), Ordering::AcqRel);
        self.set(action, true);
    }

    /// Действие нажато (`down`) или отпущено. Повтор зажатой клавиши системой нажатием не
    /// считается.
    pub fn set(&self, action: Action, down: bool) {
        let bit = action.bit();
        if down {
            let was = self.held.fetch_or(bit, Ordering::AcqRel);
            if was & bit == 0 {
                let ns = (self.epoch.elapsed().as_nanos() as u64).max(1);
                self.last_press_ns.store(ns, Ordering::Release);
                self.presses[action as usize].fetch_add(1, Ordering::AcqRel);
            }
        } else {
            self.held.fetch_and(!bit, Ordering::AcqRel);
        }
    }

    /// Всё отпущено (окно потеряло фокус: отпускание клавиш оно уже не узнает).
    pub fn release_all(&self) {
        self.held.store(0, Ordering::Release);
    }

    fn now_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64
    }

    /// Сколько прошло с нажатия, отмеченного в [`InputFrame::stamp`], мс.
    pub fn age_ms(&self, stamp: u64) -> f64 {
        self.now_ns().saturating_sub(stamp) as f64 / 1e6
    }
}

/// Ввод одного шага логики.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InputFrame {
    held: u32,
    pressed: u32,
    /// Из нажатых — направленные в точку прицела.
    aimed: u32,
    /// Точка прицела (курсор в координатах сцены).
    aim: Option<[f64; 2]>,
    /// Когда было последнее нажатие, взятое в этот шаг (0 — нажатий не было): по нему
    /// кадр узнаёт, сколько прошло от нажатия до ответа на экране.
    stamp: u64,
}

impl InputFrame {
    /// Ввод, где зажаты `held`, а с прошлого шага нажаты `pressed` (для проверок).
    pub fn new(held: &[Action], pressed: &[Action]) -> Self {
        let bits = |a: &[Action]| a.iter().fold(0, |b, x| b | x.bit());
        Self {
            held: bits(held) | bits(pressed),
            pressed: bits(pressed),
            aimed: 0,
            aim: None,
            stamp: 0,
        }
    }

    /// Тот же ввод с точкой прицела `aim`; `aimed` из нажатых направлены в неё.
    pub fn with_aim(mut self, aim: [f64; 2], aimed: &[Action]) -> Self {
        self.aim = Some(aim);
        self.aimed = aimed.iter().fold(0, |b, x| b | x.bit()) & self.pressed;
        self
    }

    /// Точка прицела, если курсор над окном.
    pub fn aim(&self) -> Option<[f64; 2]> {
        self.aim
    }

    /// Действие нажато с прошлого шага и направлено в точку прицела.
    pub fn aimed(&self, action: Action) -> bool {
        self.aimed & self.pressed & action.bit() != 0
    }

    /// Действие зажато сейчас (или было нажато с прошлого шага).
    pub fn held(&self, action: Action) -> bool {
        self.held & action.bit() != 0
    }

    /// Действие нажато с прошлого шага.
    pub fn pressed(&self, action: Action) -> bool {
        self.pressed & action.bit() != 0
    }

    /// Когда было нажатие, взятое в этот шаг (0 — не было), см. [`InputShared::age_ms`].
    pub fn stamp(&self) -> u64 {
        self.stamp
    }

    /// Направление по горизонтали: -1, 0 или 1.
    pub fn x(&self) -> f64 {
        f64::from(i8::from(self.held(Action::Right)) - i8::from(self.held(Action::Left)))
    }
}

/// Читатель ввода в потоке логики: помнит, какие нажатия уже видел.
#[derive(Debug, Default)]
pub struct InputReader {
    seen: [u32; COUNT],
    /// Среднее время от нажатия до шага логики, который его взял, мс.
    latency_ms: f64,
    samples: u64,
}

impl InputReader {
    /// Ввод для этого шага. Нажатие, отпущенное до шага, всё равно приходит как
    /// «нажато и зажато» на один шаг: прыжок от короткого тычка не пропадает.
    pub fn read(&mut self, shared: &InputShared) -> InputFrame {
        let mut pressed = 0;
        for a in Action::ALL {
            let n = shared.presses[a as usize].load(Ordering::Acquire);
            if n != self.seen[a as usize] {
                self.seen[a as usize] = n;
                pressed |= a.bit();
            }
        }
        let mut stamp = 0;
        if pressed != 0 {
            let at = shared.last_press_ns.load(Ordering::Acquire);
            stamp = at;
            let ms = shared.now_ns().saturating_sub(at) as f64 / 1e6;
            self.samples += 1;
            self.latency_ms = if self.samples == 1 {
                ms
            } else {
                self.latency_ms * 0.8 + ms * 0.2
            };
        }
        let aim = shared.aim_on.load(Ordering::Acquire).then(|| {
            [
                f64::from_bits(shared.aim[0].load(Ordering::Relaxed)),
                f64::from_bits(shared.aim[1].load(Ordering::Relaxed)),
            ]
        });
        InputFrame {
            held: shared.held.load(Ordering::Acquire) | pressed,
            pressed,
            aimed: shared.aimed.swap(0, Ordering::AcqRel) & pressed,
            aim,
            stamp,
        }
    }

    /// Сколько в среднем ждёт нажатие до шага логики, мс (`None`, пока нажатий не было).
    pub fn latency_ms(&self) -> Option<f64> {
        (self.samples > 0).then_some(self.latency_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_tap_between_steps_is_not_lost() {
        let shared = InputShared::new();
        let mut reader = InputReader::default();
        shared.set(Action::Jump, true);
        shared.set(Action::Jump, false);
        let f = reader.read(&shared);
        assert!(f.pressed(Action::Jump));
        assert!(f.held(Action::Jump));
        let f = reader.read(&shared);
        assert!(!f.pressed(Action::Jump));
        assert!(!f.held(Action::Jump));
        assert!(reader.latency_ms().is_some());
    }

    #[test]
    fn aimed_press_carries_the_cursor_point_for_one_step() {
        let shared = InputShared::new();
        let mut reader = InputReader::default();
        assert_eq!(reader.read(&shared).aim(), None);
        shared.set_aim(Some([1.5, -0.25]));
        shared.press_aimed(Action::Attack);
        shared.set(Action::Attack, false);
        let f = reader.read(&shared);
        assert!(f.pressed(Action::Attack) && f.aimed(Action::Attack));
        assert_eq!(f.aim(), Some([1.5, -0.25]));
        // Следующее нажатие клавишей — уже не к курсору.
        shared.set(Action::Attack, true);
        let f = reader.read(&shared);
        assert!(f.pressed(Action::Attack) && !f.aimed(Action::Attack));
        shared.set_aim(None);
        assert_eq!(reader.read(&shared).aim(), None);
    }

    #[test]
    fn key_repeat_is_not_a_new_press() {
        let shared = InputShared::new();
        let mut reader = InputReader::default();
        shared.set(Action::Right, true);
        assert!(reader.read(&shared).pressed(Action::Right));
        shared.set(Action::Right, true);
        let f = reader.read(&shared);
        assert!(!f.pressed(Action::Right));
        assert!(f.held(Action::Right));
        assert_eq!(f.x(), 1.0);
    }

    #[test]
    fn both_directions_cancel() {
        let f = InputFrame::new(&[Action::Left, Action::Right], &[]);
        assert_eq!(f.x(), 0.0);
        assert_eq!(InputFrame::new(&[Action::Left], &[]).x(), -1.0);
    }

    #[test]
    fn release_all_clears_held() {
        let shared = InputShared::new();
        let mut reader = InputReader::default();
        shared.set(Action::Left, true);
        shared.release_all();
        reader.read(&shared);
        assert!(!reader.read(&shared).held(Action::Left));
    }
}
