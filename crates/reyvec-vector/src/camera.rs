//! Плоская камера: какая часть сцены видна на экране. Сдвиг и приближение.
//!
//! Камера ничего не пересчитывает у фигур: она даёт одно преобразование «вид» (сцена →
//! экран), которое видеокарта применяет ко всем фигурам сразу. Каждый кадр процессор
//! пишет только это преобразование, сколько бы фигур ни было на сцене.

use crate::{Affine, Point};

/// Насколько можно приблизить и отдалить камеру.
pub const MIN_ZOOM: f64 = 1.0 / 64.0;
pub const MAX_ZOOM: f64 = 4096.0;

/// Плоская камера. Экран: y вверх от -1 до 1, x от `-aspect` до `aspect` (как у сцены
/// без камеры).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2D {
    /// Точка сцены в центре экрана.
    pub center: Point,
    /// Приближение: 1 — сцена как есть, 2 — всё вдвое крупнее.
    pub zoom: f64,
}

impl Default for Camera2D {
    fn default() -> Self {
        Self {
            center: Point::ZERO,
            zoom: 1.0,
        }
    }
}

impl Camera2D {
    /// Камера с центром в `center` и приближением `zoom`.
    pub fn new(center: impl Into<Point>, zoom: f64) -> Self {
        Self {
            center: center.into(),
            zoom: zoom.clamp(MIN_ZOOM, MAX_ZOOM),
        }
    }

    /// Камера стоит как без камеры.
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }

    /// Вид: из координат сцены в координаты экрана.
    pub fn view(&self) -> Affine {
        Affine::scale(self.zoom) * Affine::translate(-self.center.to_vec2())
    }

    /// Точка сцены под точкой экрана `screen`.
    pub fn to_scene(&self, screen: Point) -> Point {
        self.center + screen.to_vec2() / self.zoom
    }

    /// Сдвиг на `delta` в координатах экрана (например, мышь протащили на столько).
    pub fn pan_screen(&mut self, delta: crate::Vec2) {
        self.center -= delta / self.zoom;
    }

    /// Приближает в `factor` раз так, что точка сцены под точкой экрана `screen` остаётся
    /// на месте (как колесо мыши в редакторах).
    pub fn zoom_at(&mut self, screen: Point, factor: f64) {
        let anchor = self.to_scene(screen);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.center = anchor - screen.to_vec2() / self.zoom;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: Point, b: Point) -> bool {
        (a - b).hypot() < 1e-9
    }

    #[test]
    fn view_and_to_scene_are_inverse() {
        let cam = Camera2D::new((0.5, -0.25), 4.0);
        let p = Point::new(0.7, 0.1);
        let screen = cam.view() * p;
        assert!(near(cam.to_scene(screen), p));
        // Центр камеры в центре экрана.
        assert!(near(cam.view() * cam.center, Point::ZERO));
    }

    #[test]
    fn zoom_keeps_point_under_cursor() {
        let mut cam = Camera2D::default();
        let cursor = Point::new(0.8, -0.3);
        let before = cam.to_scene(cursor);
        cam.zoom_at(cursor, 3.0);
        assert!((cam.zoom - 3.0).abs() < 1e-12);
        assert!(near(cam.to_scene(cursor), before));
        cam.zoom_at(cursor, 1e9);
        assert_eq!(cam.zoom, MAX_ZOOM);
        assert!(near(cam.to_scene(cursor), before));
    }

    #[test]
    fn pan_moves_scene_with_the_mouse() {
        let mut cam = Camera2D::new((0.0, 0.0), 2.0);
        let p = Point::new(0.3, 0.3);
        let before = cam.view() * p;
        cam.pan_screen(crate::Vec2::new(0.1, -0.2));
        let after = cam.view() * p;
        assert!(near(after, before + crate::Vec2::new(0.1, -0.2)));
    }
}
