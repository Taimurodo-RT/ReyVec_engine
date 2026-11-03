//! Геймпады (6.3): кнопки и левый стик превращаются в действия игрока.
//!
//! Раскладка как в большинстве платформеров: левый стик или крестовина — бег (и «вниз»
//! для спрыгивания с платформы), нижняя кнопка (A у Xbox, крест у PlayStation) —
//! прыжок, левая (X, квадрат) или правый курок — рывок. События геймпада читаются в
//! начале каждого кадра окна.

use gilrs::{Axis, Button, EventType, Gilrs};
use reyvec_scene::game::Action;

/// Насколько наклонить стик, чтобы считалось нажатием.
const STICK: f32 = 0.5;

pub(crate) struct Gamepads {
    gilrs: Option<Gilrs>,
    /// Имя последнего подключённого геймпада (для панели).
    name: Option<String>,
    /// Стик сейчас наклонён: влево, вправо, вверх, вниз.
    stick: [bool; 4],
}

impl Gamepads {
    pub(crate) fn new() -> Self {
        let gilrs = match Gilrs::new() {
            Ok(g) => Some(g),
            Err(e) => {
                tracing::warn!("Геймпады недоступны: {e}");
                None
            }
        };
        let name = gilrs.as_ref().and_then(|g| {
            g.gamepads()
                .find(|(_, p)| p.is_connected())
                .map(|(_, p)| p.name().to_string())
        });
        if let Some(n) = &name {
            tracing::info!("Геймпад: {n}");
        }
        Self {
            gilrs,
            name,
            stick: [false; 4],
        }
    }

    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Забирает события геймпадов и отдаёт действия в `act` (нажато или отпущено).
    pub(crate) fn poll(&mut self, mut act: impl FnMut(Action, bool)) {
        let Some(gilrs) = self.gilrs.as_mut() else {
            return;
        };
        while let Some(ev) = gilrs.next_event() {
            match ev.event {
                EventType::ButtonPressed(b, _) => {
                    if let Some(a) = button_action(b) {
                        act(a, true);
                    }
                }
                EventType::ButtonReleased(b, _) => {
                    if let Some(a) = button_action(b) {
                        act(a, false);
                    }
                }
                EventType::AxisChanged(axis, v, _) => {
                    let (neg, pos, a_neg, a_pos) = match axis {
                        Axis::LeftStickX => (0, 1, Action::Left, Action::Right),
                        // У стика вверх — плюс.
                        Axis::LeftStickY => (3, 2, Action::Down, Action::Up),
                        _ => continue,
                    };
                    for (i, a, on) in [(neg, a_neg, v < -STICK), (pos, a_pos, v > STICK)] {
                        if self.stick[i] != on {
                            self.stick[i] = on;
                            act(a, on);
                        }
                    }
                }
                EventType::Connected => {
                    let name = gilrs.gamepad(ev.id).name().to_string();
                    tracing::info!("Геймпад подключён: {name}");
                    self.name = Some(name);
                }
                EventType::Disconnected => {
                    tracing::info!("Геймпад отключён");
                    self.name = gilrs
                        .gamepads()
                        .find(|(_, p)| p.is_connected())
                        .map(|(_, p)| p.name().to_string());
                    for a in Action::ALL {
                        act(a, false);
                    }
                    self.stick = [false; 4];
                }
                _ => {}
            }
        }
    }
}

/// Действие кнопки геймпада.
fn button_action(b: Button) -> Option<Action> {
    match b {
        Button::South => Some(Action::Jump),
        Button::West | Button::RightTrigger | Button::RightTrigger2 => Some(Action::Dash),
        Button::North => Some(Action::Attack),
        Button::DPadLeft => Some(Action::Left),
        Button::DPadRight => Some(Action::Right),
        Button::DPadUp => Some(Action::Up),
        Button::DPadDown => Some(Action::Down),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_map_to_actions() {
        assert_eq!(button_action(Button::South), Some(Action::Jump));
        assert_eq!(button_action(Button::West), Some(Action::Dash));
        assert_eq!(button_action(Button::North), Some(Action::Attack));
        assert_eq!(button_action(Button::DPadDown), Some(Action::Down));
        assert_eq!(button_action(Button::Start), None);
    }
}
