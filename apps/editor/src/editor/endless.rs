//! Перетаскивание без края экрана: тянут число в инспекторе или крутят камеру, курсор
//! дошёл до края окна — окно переносит его на другую сторону, а интерфейсу отдаёт
//! положение «как будто курсор ушёл дальше». Так число можно тянуть сколько угодно, как в
//! Blender и Unreal. Отпустили кнопку — курсор снова обычный.

/// Ближе стольких пикселей к краю курсор переносится.
const EDGE: f64 = 8.0;

/// Перенос, о котором окно ещё не прислало новое положение.
#[derive(Clone, Copy, Debug)]
struct Pending {
    from: [f64; 2],
    to: [f64; 2],
    offset: [f64; 2],
}

/// Сдвиг между настоящим курсором и тем, что видит интерфейс.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct EndlessCursor {
    offset: [f64; 2],
    pending: Option<Pending>,
}

impl EndlessCursor {
    /// Курсор окна размером `size` пришёл в `real` (пиксели окна); `active` — сейчас
    /// тянут без края. Ответ: положение для интерфейса и куда перенести курсор (если
    /// пора).
    pub(crate) fn moved(
        &mut self,
        real: [f64; 2],
        size: [f64; 2],
        active: bool,
    ) -> ([f64; 2], Option<[f64; 2]>) {
        if !active {
            *self = Self::default();
            return (real, None);
        }
        if let Some(p) = self.pending {
            if dist(real, p.to) < dist(real, p.from) {
                self.pending = None;
            } else {
                // Старое событие, пришедшее до переноса: со старым сдвигом.
                return (add(real, p.offset), None);
            }
        }
        let shown = add(real, self.offset);
        let mut to = real;
        for a in 0..2 {
            if size[a] <= EDGE * 4.0 {
                continue;
            }
            if real[a] < EDGE {
                to[a] = size[a] - EDGE - 1.0;
            } else if real[a] > size[a] - EDGE {
                to[a] = EDGE + 1.0;
            }
        }
        if to == real {
            return (shown, None);
        }
        self.pending = Some(Pending {
            from: real,
            to,
            offset: self.offset,
        });
        self.offset = [shown[0] - to[0], shown[1] - to[1]];
        (shown, Some(to))
    }
}

fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_wraps_and_the_interface_sees_a_straight_line() {
        let size = [800.0, 600.0];
        let mut c = EndlessCursor::default();
        // Тянут вправо по 100 пикселей: у края курсор переносится влево, а интерфейс
        // видит, что он ушёл дальше края.
        let mut shown = Vec::new();
        let mut x = 500.0;
        for _ in 0..12 {
            let (s, warp) = c.moved([x, 300.0], size, true);
            shown.push(s[0]);
            x = match warp {
                Some(to) => to[0] + 100.0,
                None => x + 100.0,
            };
        }
        for w in shown.windows(2) {
            assert!((w[1] - w[0] - 100.0).abs() < 1e-9, "{shown:?}");
        }
        assert!(shown[11] > 1500.0);
        // Событие, пришедшее после переноса со старым местом, не даёт прыжка.
        let mut c = EndlessCursor::default();
        let (s, warp) = c.moved([795.0, 300.0], size, true);
        let to = warp.expect("у края — перенос");
        assert_eq!(s, [795.0, 300.0]);
        let (old, _) = c.moved([797.0, 300.0], size, true);
        assert_eq!(old, [797.0, 300.0]);
        let (new, _) = c.moved([to[0] + 3.0, 300.0], size, true);
        assert!((new[0] - 798.0).abs() < 1e-9, "{new:?}");
        // Отпустили — курсор обычный.
        let (s, warp) = c.moved([5.0, 5.0], size, false);
        assert_eq!((s, warp), ([5.0, 5.0], None));
    }
}
