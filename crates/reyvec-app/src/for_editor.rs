//! Всё, что редактор (`apps/editor`) берёт из `reyvec-app` сверх общего (`run`, снимки
//! кадров, логи). Редактор живёт отдельно и к внутренностям движка не ходит: если
//! правка песочницы или графики меняет что-то из этого списка, это видно сразу, и
//! править надо только здесь и в местах вызова в редакторе (decisions/0024).

pub use crate::app::{input_of, to_screen};
pub use crate::console::{GOLD, RED, take_log};
pub use crate::controls::{Controls, Input, MouseKey, Wheel, controls_path};
pub use crate::panel::fonts;
