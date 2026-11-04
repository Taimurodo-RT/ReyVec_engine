//! Камера 3D (этап 7.1): где стоит глаз, куда смотрит и как мир ложится на экран.
//!
//! Соглашения движка (решение 0004): правая система координат, Y вверх, 1 единица = 1 м.
//! Глубина обратная (reverse-Z): ближняя плоскость даёт 1, дальняя 0 — так у дальних
//! точек точности больше. Экран: x и y от -1 до 1, y вверх (как в шейдерах движка).
//!
//! Плоская (ортографическая) камера — частный случай: с [`Camera3D::from_2d`] она видит
//! плоскость z = 0 так же, как 2D-камера (`reyvec_vector::Camera2D`). На этом держится
//! 2.5D-режим (7.9): тот же уровень, другая камера.
//!
//! Луч для реймарчинга шейдер получает из обратной матрицы [`Camera3D::inverse_view_proj`]:
//! точка экрана «разворачивается» на ближнюю и дальнюю плоскость, луч идёт от первой ко
//! второй. Так один и тот же шейдер работает и с перспективой, и с плоской камерой.

use glam::{Mat4, Vec3, Vec4};

/// Как камера отображает мир на экран.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// Перспектива: дальнее меньше. `fov_y` — угол обзора по вертикали, радианы.
    Perspective { fov_y: f32 },
    /// Плоская: размер не зависит от расстояния. `height` — сколько метров видно по
    /// вертикали.
    Orthographic { height: f32 },
}

/// Камера 3D: положение глаза, повороты и проекция.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera3D {
    /// Где глаз.
    pub eye: Vec3,
    /// Поворот вокруг вертикали, радианы: 0 — смотрит вдоль -Z, π/2 — вдоль +X.
    pub yaw: f32,
    /// Наклон, радианы: 0 — горизонтально, плюс — вверх. Не больше [`MAX_PITCH`].
    pub pitch: f32,
    pub projection: Projection,
    /// Ближняя и дальняя плоскости, метры.
    pub near: f32,
    pub far: f32,
}

/// Наклон не доходит до вертикали: вверх и вниз ровно камера не смотрит (иначе «верх»
/// экрана не определён).
pub const MAX_PITCH: f32 = 89.0 * std::f32::consts::PI / 180.0;

/// Угол обзора по умолчанию: 60° по вертикали.
pub const DEFAULT_FOV_Y: f32 = std::f32::consts::FRAC_PI_3;

impl Default for Camera3D {
    /// Вид на середину сцены «3D» сверху сбоку.
    fn default() -> Self {
        Self::looking_at(Vec3::new(6.0, 4.0, 9.0), Vec3::new(0.0, 0.8, 0.0))
    }
}

impl Camera3D {
    /// Перспективная камера в `eye`, смотрит на `target`.
    pub fn looking_at(eye: Vec3, target: Vec3) -> Self {
        let d = (target - eye).normalize_or(Vec3::NEG_Z);
        Self {
            eye,
            yaw: d.x.atan2(-d.z),
            pitch: d.y.clamp(-1.0, 1.0).asin().clamp(-MAX_PITCH, MAX_PITCH),
            projection: Projection::Perspective {
                fov_y: DEFAULT_FOV_Y,
            },
            near: 0.05,
            far: 1000.0,
        }
    }

    /// Плоская камера, которая видит плоскость z = 0 как 2D-камера с серединой экрана в
    /// `center` и приближением `zoom` (по вертикали видно 2 / zoom метров).
    pub fn from_2d(center: [f64; 2], zoom: f64) -> Self {
        let distance = 50.0;
        Self {
            eye: Vec3::new(center[0] as f32, center[1] as f32, distance),
            yaw: 0.0,
            pitch: 0.0,
            projection: Projection::Orthographic {
                height: (2.0 / zoom.max(1e-6)) as f32,
            },
            near: 0.05,
            far: 2.0 * distance,
        }
    }

    /// Куда смотрит камера (длина 1).
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(sy * cp, sp, -cy * cp)
    }

    /// Вправо по экрану (длина 1, всегда горизонтально).
    pub fn right(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        Vec3::new(cy, 0.0, sy)
    }

    /// Вверх по экрану (длина 1).
    pub fn up(&self) -> Vec3 {
        self.right().cross(self.forward())
    }

    /// Мир → координаты камеры.
    pub fn view(&self) -> Mat4 {
        Mat4::look_to_rh(self.eye, self.forward(), Vec3::Y)
    }

    /// Координаты камеры → экран с обратной глубиной (ближняя 1, дальняя 0). `aspect` —
    /// ширина / высота.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        // Ближняя и дальняя плоскости переставлены местами: так формулы glam дают
        // обратную глубину.
        match self.projection {
            Projection::Perspective { fov_y } => {
                Mat4::perspective_rh(fov_y, aspect, self.far, self.near)
            }
            Projection::Orthographic { height } => {
                let h = height * 0.5;
                let w = h * aspect;
                Mat4::orthographic_rh(-w, w, -h, h, self.far, self.near)
            }
        }
    }

    /// Мир → экран.
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }

    /// Экран → мир: из неё шейдер строит лучи.
    pub fn inverse_view_proj(&self, aspect: f32) -> Mat4 {
        self.view_proj(aspect).inverse()
    }

    /// Точка мира на экране: x, y от -1 до 1 (y вверх) и глубина (1 — ближняя плоскость,
    /// 0 — дальняя). `None`, если точка позади камеры.
    pub fn project(&self, p: Vec3, aspect: f32) -> Option<Vec3> {
        let c = self.view_proj(aspect) * p.extend(1.0);
        (c.w > 1e-6).then(|| c.truncate() / c.w)
    }

    /// Луч через точку экрана `screen` (x, y от -1 до 1, y вверх): начало на ближней
    /// плоскости и направление длины 1. То же считает шейдер `raymarch.slang`.
    pub fn ray(&self, screen: [f32; 2], aspect: f32) -> (Vec3, Vec3) {
        let inv = self.inverse_view_proj(aspect);
        let unproject = |depth: f32| {
            let p = inv * Vec4::new(screen[0], screen[1], depth, 1.0);
            p.truncate() / p.w
        };
        let near = unproject(1.0);
        let far = unproject(0.0);
        (near, (far - near).normalize())
    }

    /// Один кадр полёта: движение по нажатым клавишам и поворот мышью.
    pub fn fly(&mut self, input: &FlyInput, dt: f32) {
        self.yaw = (self.yaw + input.look[0]).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + input.look[1]).clamp(-MAX_PITCH, MAX_PITCH);
        let speed = input.speed * if input.fast { FAST } else { 1.0 };
        let mut step =
            self.forward() * input.forward + self.right() * input.right + Vec3::Y * input.up;
        if step.length_squared() > 1.0 {
            step = step.normalize();
        }
        self.eye += step * speed * dt;
        // Плоской камере «вперёд» — значит ближе: уменьшаем видимую высоту, чтобы движение
        // было видно (её глаз и так видит всё на линии взгляда).
        if let Projection::Orthographic { height } = &mut self.projection {
            *height = (*height * (1.0 - input.forward * dt)).clamp(0.1, 1000.0);
        }
    }

    /// Перспектива ↔ плоская камера с тем же видом в точке на расстоянии `distance` по
    /// взгляду (размер того, что там, на экране не меняется).
    pub fn toggle_projection(&mut self, distance: f32) {
        self.projection = match self.projection {
            Projection::Perspective { fov_y } => Projection::Orthographic {
                height: 2.0 * distance * (fov_y * 0.5).tan(),
            },
            Projection::Orthographic { .. } => Projection::Perspective {
                fov_y: DEFAULT_FOV_Y,
            },
        };
    }

    /// Название проекции для панели.
    pub fn projection_label(&self) -> &'static str {
        match self.projection {
            Projection::Perspective { .. } => "перспектива",
            Projection::Orthographic { .. } => "плоская",
        }
    }
}

/// Во сколько раз быстрее полёт с зажатым Shift.
pub const FAST: f32 = 4.0;

/// Что нажато для полёта камерой за кадр.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FlyInput {
    /// Вперёд (+1) или назад (-1).
    pub forward: f32,
    /// Вправо (+1) или влево (-1).
    pub right: f32,
    /// Вверх (+1) или вниз (-1), по вертикали мира.
    pub up: f32,
    /// Быстрее (Shift).
    pub fast: bool,
    /// Поворот мышью за кадр, радианы: влево-вправо и вверх-вниз.
    pub look: [f32; 2],
    /// Скорость, метров в секунду.
    pub speed: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn looking_at_points_forward_at_the_target() {
        let c = Camera3D::looking_at(Vec3::new(1.0, 2.0, 5.0), Vec3::new(1.0, 2.0, 0.0));
        assert!(close(c.forward(), Vec3::NEG_Z));
        let c = Camera3D::looking_at(Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0));
        assert!(close(c.forward(), Vec3::X));
        assert!(close(c.right(), Vec3::Z));
        assert!(close(c.up(), Vec3::Y));
        let c = Camera3D::default();
        let to = (Vec3::new(0.0, 0.8, 0.0) - c.eye).normalize();
        assert!(close(c.forward(), to));
    }

    #[test]
    fn target_lands_in_the_middle_of_the_screen_with_reverse_depth() {
        let c = Camera3D::default();
        let p = c.project(Vec3::new(0.0, 0.8, 0.0), 16.0 / 9.0).unwrap();
        assert!(p.x.abs() < 1e-4 && p.y.abs() < 1e-4, "{p}");
        // Ближе — глубина больше; на ближней плоскости 1, на дальней 0.
        let at = |d: f32| c.project(c.eye + c.forward() * d, 1.0).unwrap().z;
        assert!((at(c.near) - 1.0).abs() < 1e-4);
        assert!(at(c.far).abs() < 1e-4);
        assert!(at(1.0) > at(10.0));
        // Позади камеры точки нет.
        assert!(c.project(c.eye - c.forward(), 1.0).is_none());
    }

    #[test]
    fn up_on_screen_is_up_in_the_world_and_right_is_right() {
        let c = Camera3D::looking_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
        let p = c.project(Vec3::new(1.0, 1.0, 0.0), 1.0).unwrap();
        assert!(p.x > 0.0 && p.y > 0.0, "{p}");
    }

    #[test]
    fn ray_through_a_pixel_passes_the_point_seen_there() {
        let aspect = 16.0 / 9.0;
        for c in [Camera3D::default(), Camera3D::from_2d([1.0, -2.0], 0.5)] {
            for p in [Vec3::new(0.0, 0.8, 0.0), Vec3::new(2.0, 0.0, -1.0)] {
                let s = c.project(p, aspect).unwrap();
                let (origin, dir) = c.ray([s.x, s.y], aspect);
                assert!((dir.length() - 1.0).abs() < 1e-4);
                // Расстояние от точки до луча.
                let to = p - origin;
                let off = to - dir * to.dot(dir);
                assert!(off.length() < 1e-3, "{:?}: {off}", c.projection);
            }
        }
        // Луч из середины экрана — по взгляду.
        let c = Camera3D::default();
        assert!(close(c.ray([0.0, 0.0], aspect).1, c.forward()));
    }

    #[test]
    fn flat_camera_sees_the_plane_like_the_2d_camera() {
        // 2D-камера: точка сцены p на экране (p - center) · zoom, x делится на aspect, чтобы
        // попасть в -1…1 (экран сцены 2D идёт от -aspect до aspect).
        let (center, zoom, aspect) = ([1.5, -0.5], 0.8, 16.0f32 / 9.0);
        let c = Camera3D::from_2d(center, zoom);
        for (x, y) in [(0.0, 0.0), (2.0, 1.0), (-3.0, 0.25)] {
            let s = c.project(Vec3::new(x, y, 0.0), aspect).unwrap();
            let sx = (x - center[0] as f32) * zoom as f32 / aspect;
            let sy = (y - center[1] as f32) * zoom as f32;
            assert!((s.x - sx).abs() < 1e-4 && (s.y - sy).abs() < 1e-4, "{s}");
        }
        // Слои глубже по z на плоской камере на экране там же.
        let a = c.project(Vec3::new(2.0, 1.0, 0.0), aspect).unwrap();
        let b = c.project(Vec3::new(2.0, 1.0, -5.0), aspect).unwrap();
        assert!((a.x - b.x).abs() < 1e-5 && (a.y - b.y).abs() < 1e-5 && a.z > b.z);
    }

    #[test]
    fn flying_moves_along_the_view_and_turning_keeps_the_horizon() {
        let mut c = Camera3D::looking_at(Vec3::ZERO, Vec3::NEG_Z);
        let input = FlyInput {
            forward: 1.0,
            speed: 2.0,
            ..FlyInput::default()
        };
        c.fly(&input, 0.5);
        assert!(close(c.eye, Vec3::new(0.0, 0.0, -1.0)));
        // Вперёд и вправо сразу — не быстрее, чем просто вперёд.
        let diagonal = FlyInput {
            forward: 1.0,
            right: 1.0,
            speed: 2.0,
            ..FlyInput::default()
        };
        let before = c.eye;
        c.fly(&diagonal, 0.5);
        assert!(((c.eye - before).length() - 1.0).abs() < 1e-4);
        // Shift ускоряет.
        let before = c.eye;
        c.fly(
            &FlyInput {
                fast: true,
                ..input
            },
            0.5,
        );
        assert!(((c.eye - before).length() - FAST).abs() < 1e-4);
        // Мышью вверх до упора — не дальше MAX_PITCH, правый вектор горизонтален.
        c.fly(
            &FlyInput {
                look: [0.3, 10.0],
                ..FlyInput::default()
            },
            0.016,
        );
        assert!((c.pitch - MAX_PITCH).abs() < 1e-6);
        assert!(c.right().y.abs() < 1e-6);
    }

    #[test]
    fn switching_projection_keeps_the_size_at_the_focus_distance() {
        let mut c = Camera3D::looking_at(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO);
        let size = |c: &Camera3D| c.project(Vec3::new(0.0, 1.0, 0.0), 1.0).unwrap().y;
        let before = size(&c);
        c.toggle_projection(10.0);
        assert_eq!(c.projection_label(), "плоская");
        assert!((size(&c) - before).abs() < 1e-4);
        c.toggle_projection(10.0);
        assert_eq!(c.projection_label(), "перспектива");
    }
}
