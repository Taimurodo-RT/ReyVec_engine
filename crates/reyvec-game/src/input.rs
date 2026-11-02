//! Ввод игрока: действия вместо клавиш.
//!
//! Окно получает нажатия клавиш (раскладка не важна) и пишет в [`InputShared`] действие:
//! «влево», «прыжок». Поток логики 60 раз в секунду читает его через [`InputReader`] и
//! получает [`InputFrame`]: что зажато сейчас и что нажали с прошлого шага. Короткое
//! нажатие, которое началось и кончилось между двумя шагами, не теряется: у каждого
//! действия счётчик нажатий.
//!
//! Между потоками общие только атомарные числа: окно не ждёт логику, логика не ждёт окно.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

/// Действие игрока.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Left,
    Right,
    Up,
    Down,
    Jump,
}

impl Action {
    pub const ALL: [Self; 5] = [Self::Left, Self::Right, Self::Up, Self::Down, Self::Jump];

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
        }
    }

    /// Действие нажато (`down`) или отпущено. Повтор зажатой клавиши системой нажатием не
    /// считается.
    pub fn set(&self, action: Action, down: bool) {
        let bit = action.bit();
        if down {
            let was = self.held.fetch_or(bit, Ordering::AcqRel);
            if was & bit == 0 {
                let ns = self.epoch.elapsed().as_nanos() as u64;
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
}

/// Ввод одного шага логики.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputFrame {
    held: u32,
    pressed: u32,
}

impl InputFrame {
    /// Ввод, где зажаты `held`, а с прошлого шага нажаты `pressed` (для проверок).
    pub fn new(held: &[Action], pressed: &[Action]) -> Self {
        let bits = |a: &[Action]| a.iter().fold(0, |b, x| b | x.bit());
        Self {
            held: bits(held) | bits(pressed),
            pressed: bits(pressed),
        }
    }

    /// Действие зажато сейчас (или было нажато с прошлого шага).
    pub fn held(&self, action: Action) -> bool {
        self.held & action.bit() != 0
    }

    /// Действие нажато с прошлого шага.
    pub fn pressed(&self, action: Action) -> bool {
        self.pressed & action.bit() != 0
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
        if pressed != 0 {
            let at = shared.last_press_ns.load(Ordering::Acquire);
            let ms = shared.now_ns().saturating_sub(at) as f64 / 1e6;
            self.samples += 1;
            self.latency_ms = if self.samples == 1 {
                ms
            } else {
                self.latency_ms * 0.8 + ms * 0.2
            };
        }
        InputFrame {
            held: shared.held.load(Ordering::Acquire) | pressed,
            pressed,
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
