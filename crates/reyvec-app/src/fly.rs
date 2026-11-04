//! Полёт камерой в сцене «3D» (7.1). Клавиши описаны таблицей [`FLY_KEYS`], как
//! привязки управления (решение 0015): по ней строится и подсказка на панели.
//!
//! - W A S D — вперёд, влево, назад, вправо; E и Q — вверх и вниз; Shift — быстрее.
//! - Зажатая левая или правая кнопка мыши — смотреть вокруг.
//! - Колесо — скорость полёта. Tab — перспектива или плоская камера. R — камера в начало.

use reyvec_sdf::FlyInput;
use winit::keyboard::KeyCode;

/// Движение полёта.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlyMove {
    Forward,
    Back,
    Left,
    Right,
    Up,
    Down,
    Fast,
}

/// Клавиши полёта (по месту на клавиатуре, раскладка не важна).
pub(crate) const FLY_KEYS: &[(FlyMove, &[KeyCode])] = &[
    (FlyMove::Forward, &[KeyCode::KeyW, KeyCode::ArrowUp]),
    (FlyMove::Back, &[KeyCode::KeyS, KeyCode::ArrowDown]),
    (FlyMove::Left, &[KeyCode::KeyA, KeyCode::ArrowLeft]),
    (FlyMove::Right, &[KeyCode::KeyD, KeyCode::ArrowRight]),
    (FlyMove::Up, &[KeyCode::KeyE, KeyCode::Space]),
    (FlyMove::Down, &[KeyCode::KeyQ, KeyCode::ControlLeft]),
    (FlyMove::Fast, &[KeyCode::ShiftLeft, KeyCode::ShiftRight]),
];

/// Перспектива ↔ плоская камера.
pub(crate) const TOGGLE_PROJECTION: KeyCode = KeyCode::Tab;

/// Поворот на пиксель движения мыши, радианы.
const LOOK_PER_PIXEL: f32 = 0.004;
/// Скорость полёта: по умолчанию, пределы и шаг колеса.
const SPEED: f32 = 4.0;
const SPEED_MIN: f32 = 0.25;
const SPEED_MAX: f32 = 200.0;
const SPEED_STEP: f32 = 1.25;

/// Что нажато для полёта и сколько мышь повернула с прошлого кадра.
#[derive(Clone, Debug)]
pub(crate) struct Fly {
    held: Vec<FlyMove>,
    /// Зажата кнопка мыши: движение мыши поворачивает камеру.
    pub looking: bool,
    look: [f32; 2],
    pub speed: f32,
}

impl Default for Fly {
    fn default() -> Self {
        Self {
            held: Vec::new(),
            looking: false,
            look: [0.0; 2],
            speed: SPEED,
        }
    }
}

impl Fly {
    /// Клавиша полёта: `true`, если она полёту нужна (её больше никто не получает).
    pub(crate) fn key(&mut self, code: KeyCode, down: bool) -> bool {
        let Some(m) = FLY_KEYS
            .iter()
            .find(|(_, keys)| keys.contains(&code))
            .map(|(m, _)| *m)
        else {
            return false;
        };
        self.held.retain(|h| *h != m);
        if down {
            self.held.push(m);
        }
        true
    }

    /// Мышь сдвинулась на `dx`, `dy` пикселей (y вниз).
    pub(crate) fn mouse(&mut self, dx: f64, dy: f64) {
        if self.looking {
            // Мышь вправо — взгляд вправо, мышь вверх — взгляд вверх.
            self.look[0] += dx as f32 * LOOK_PER_PIXEL;
            self.look[1] -= dy as f32 * LOOK_PER_PIXEL;
        }
    }

    /// Колесо: `steps` щелчков (плюс — быстрее).
    pub(crate) fn wheel(&mut self, steps: f64) {
        self.speed = (self.speed * SPEED_STEP.powf(steps as f32)).clamp(SPEED_MIN, SPEED_MAX);
    }

    /// Всё отпущено (окно потеряло фокус или сцена сменилась).
    pub(crate) fn release(&mut self) {
        self.held.clear();
        self.looking = false;
        self.look = [0.0; 2];
    }

    /// Ввод полёта за кадр; поворот мышью копится между кадрами и здесь обнуляется.
    pub(crate) fn take(&mut self) -> FlyInput {
        let axis = |plus: FlyMove, minus: FlyMove| {
            f32::from(u8::from(self.held.contains(&plus)))
                - f32::from(u8::from(self.held.contains(&minus)))
        };
        let input = FlyInput {
            forward: axis(FlyMove::Forward, FlyMove::Back),
            right: axis(FlyMove::Right, FlyMove::Left),
            up: axis(FlyMove::Up, FlyMove::Down),
            fast: self.held.contains(&FlyMove::Fast),
            look: self.look,
            speed: self.speed,
        };
        self.look = [0.0; 2];
        input
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_make_axes_and_mouse_turns_only_while_held() {
        let mut f = Fly::default();
        assert!(f.key(KeyCode::KeyW, true));
        assert!(f.key(KeyCode::KeyA, true));
        assert!(f.key(KeyCode::ShiftLeft, true));
        assert!(!f.key(KeyCode::KeyZ, true), "Z полёту не нужна");
        f.mouse(100.0, 50.0);
        let i = f.take();
        assert_eq!((i.forward, i.right, i.up, i.fast), (1.0, -1.0, 0.0, true));
        assert_eq!(i.look, [0.0, 0.0], "без кнопки мыши взгляд не крутится");
        f.looking = true;
        f.mouse(100.0, 50.0);
        let i = f.take();
        assert!(i.look[0] > 0.0 && i.look[1] < 0.0);
        assert_eq!(f.take().look, [0.0, 0.0], "поворот отдаётся один раз");
        f.key(KeyCode::KeyW, false);
        assert_eq!(f.take().forward, 0.0);
        f.release();
        assert!(!f.take().fast);
    }

    #[test]
    fn wheel_changes_speed_within_limits() {
        let mut f = Fly::default();
        f.wheel(1.0);
        assert!((f.speed - SPEED * SPEED_STEP).abs() < 1e-4);
        f.wheel(-100.0);
        assert_eq!(f.speed, SPEED_MIN);
        f.wheel(100.0);
        assert_eq!(f.speed, SPEED_MAX);
    }
}
