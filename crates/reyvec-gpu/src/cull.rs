//! Отсечение света и SDF-объектов за экраном (этап 5.5в).
//!
//! Видеокарта берёт за кадр не больше [`MAX_LIGHTS`] источников и [`MAX_OBJECTS`]
//! объектов, а свет считает только в пикселях экрана (поле расстояний тоже размером с
//! экран). Поэтому в кадр идут только те, что могут что-то изменить на экране:
//! - лампа или трубка, чей свет (`reach`) достаёт до прямоугольника экрана;
//! - свет издалека всегда: он светит везде;
//! - объект, чей прямоугольник ([`SdfObject::bounds`]) задевает экран.
//!
//! Объект за экраном тень на экран не отбрасывает: тени ищутся по полю, а в поле попадает
//! только то, что на экране. Если на экране больше, чем берёт видеокарта, остаются те, что
//! ближе к середине экрана, а панель показывает, сколько не влезло.

use crate::blobs::{MAX_OBJECTS, SdfObject};
use crate::lights::{Light, LightKind, MAX_LIGHTS};

/// Прямоугольник сцены, который виден на экране.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ViewRect {
    pub lo: [f32; 2],
    pub hi: [f32; 2],
}

impl ViewRect {
    /// Экран камеры с центром `center` и приближением `zoom` (по высоте кадр — 2 / zoom
    /// единиц сцены, по ширине — в `aspect` раз больше).
    pub fn new(center: [f32; 2], zoom: f32, aspect: f32) -> Self {
        let half = [aspect / zoom, 1.0 / zoom];
        Self {
            lo: [center[0] - half[0], center[1] - half[1]],
            hi: [center[0] + half[0], center[1] + half[1]],
        }
    }

    fn center(&self) -> [f32; 2] {
        [
            (self.lo[0] + self.hi[0]) * 0.5,
            (self.lo[1] + self.hi[1]) * 0.5,
        ]
    }

    /// Расстояние от точки до прямоугольника (0 внутри).
    fn distance(&self, p: [f32; 2]) -> f32 {
        let dx = (self.lo[0] - p[0]).max(p[0] - self.hi[0]).max(0.0);
        let dy = (self.lo[1] - p[1]).max(p[1] - self.hi[1]).max(0.0);
        dx.hypot(dy)
    }

    /// Расстояние от отрезка `a`–`b` до прямоугольника (0, если задевает).
    fn segment_distance(&self, a: [f32; 2], b: [f32; 2]) -> f32 {
        // Расстояние до прямоугольника вдоль отрезка — выпуклая функция: ищем её минимум
        // делением отрезка (золотое сечение хватает с запасом для света).
        let at = |t: f32| self.distance([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..24 {
            let m1 = lo + (hi - lo) / 3.0;
            let m2 = hi - (hi - lo) / 3.0;
            if at(m1) <= at(m2) {
                hi = m2;
            } else {
                lo = m1;
            }
        }
        at(0.0).min(at(1.0)).min(at((lo + hi) * 0.5))
    }

    fn overlaps(&self, lo: [f32; 2], hi: [f32; 2]) -> bool {
        lo[0] <= self.hi[0] && hi[0] >= self.lo[0] && lo[1] <= self.hi[1] && hi[1] >= self.lo[1]
    }
}

/// Сколько источников и объектов в сцене, сколько из них ушло в кадр и сколько было на
/// экране, но не влезло в пределы видеокарты.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CullStats {
    pub lights: usize,
    pub lights_drawn: usize,
    pub lights_over: usize,
    pub objects: usize,
    pub objects_drawn: usize,
    pub objects_over: usize,
}

/// Оставляет первые `max` по `key` (меньше — важнее), не меняя порядок оставшихся.
/// Возвращает, сколько выброшено.
fn keep_best<T>(items: &mut Vec<T>, max: usize, key: impl Fn(&T) -> f32) -> usize {
    if items.len() <= max {
        return 0;
    }
    let mut order: Vec<(f32, usize)> = items.iter().enumerate().map(|(i, x)| (key(x), i)).collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut keep = vec![false; items.len()];
    for &(_, i) in &order[..max] {
        keep[i] = true;
    }
    let over = items.len() - max;
    let mut i = 0;
    items.retain(|_| {
        let k = keep[i];
        i += 1;
        k
    });
    over
}

/// Источники, которые светят на экран (не больше [`MAX_LIGHTS`]).
pub(crate) fn lights(all: Vec<Light>, view: &ViewRect, stats: &mut CullStats) -> Vec<Light> {
    stats.lights = all.len();
    let center = view.center();
    let mut visible: Vec<Light> = all
        .into_iter()
        .filter(|l| match l.kind {
            LightKind::Directional => true,
            LightKind::Local => view.segment_distance(l.a, l.b) < l.reach,
        })
        .collect();
    // Важнее тот, чей свет глубже заходит на экран: от середины экрана до источника минус
    // дальность. Свет издалека важнее всех.
    stats.lights_over = keep_best(&mut visible, MAX_LIGHTS, |l| match l.kind {
        LightKind::Directional => f32::MIN,
        LightKind::Local => {
            let ab = [l.b[0] - l.a[0], l.b[1] - l.a[1]];
            let ap = [center[0] - l.a[0], center[1] - l.a[1]];
            let len2 = (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-12);
            let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0);
            let q = [l.a[0] + ab[0] * t, l.a[1] + ab[1] * t];
            (q[0] - center[0]).hypot(q[1] - center[1]) - l.reach
        }
    });
    stats.lights_drawn = visible.len();
    visible
}

/// Объекты, которые видны на экране (не больше [`MAX_OBJECTS`]).
pub(crate) fn objects(
    all: Vec<SdfObject>,
    view: &ViewRect,
    stats: &mut CullStats,
) -> Vec<SdfObject> {
    stats.objects = all.len();
    let center = view.center();
    let mut visible: Vec<SdfObject> = all
        .into_iter()
        .filter(|o| {
            let (lo, hi) = o.bounds();
            view.overlaps(lo, hi)
        })
        .collect();
    stats.objects_over = keep_best(&mut visible, MAX_OBJECTS, |o| {
        let (lo, hi) = o.bounds();
        let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
        (c[0] - center[0]).hypot(c[1] - center[1])
    });
    stats.objects_drawn = visible.len();
    visible
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blobs::Blob;

    fn view() -> ViewRect {
        ViewRect::new([0.0, 0.0], 1.0, 2.0)
    }

    #[test]
    fn view_rect_follows_camera() {
        let v = ViewRect::new([3.0, -1.0], 2.0, 1.5);
        assert_eq!(v.lo, [3.0 - 0.75, -1.5]);
        assert_eq!(v.hi, [3.0 + 0.75, -0.5]);
    }

    #[test]
    fn lights_off_screen_are_dropped() {
        let near = Light::point([2.5, 0.0], [1.0; 3], 1.0, 0.01); // свет заходит на экран
        let far = Light::point([3.5, 0.0], [1.0; 3], 1.0, 0.01); // не достаёт
        let tube = Light::tube([5.0, 1.5], [-5.0, 1.5], [1.0; 3], 0.6, 0.01); // над экраном
        let sun = Light::directional([0.0, 1.0], [1.0; 3], 1.0, 0.01);
        let mut stats = CullStats::default();
        let kept = lights(vec![near, far, tube, sun], &view(), &mut stats);
        assert_eq!(kept, vec![near, tube, sun]);
        assert_eq!(
            (stats.lights, stats.lights_drawn, stats.lights_over),
            (4, 3, 0)
        );
    }

    #[test]
    fn too_many_lights_keep_the_closest() {
        // 20 ламп в ряд через экран: остаются 16 ближайших к середине, порядок сохраняется.
        let all: Vec<Light> = (0..20)
            .map(|i| Light::point([-1.9 + 0.2 * i as f32, 0.0], [1.0; 3], 0.5, 0.01))
            .collect();
        let mut stats = CullStats::default();
        let kept = lights(all.clone(), &view(), &mut stats);
        assert_eq!(kept.len(), MAX_LIGHTS);
        assert_eq!(stats.lights_over, 4);
        assert_eq!(kept, all[2..18].to_vec());
    }

    #[test]
    fn objects_off_screen_are_dropped() {
        let obj = |x: f32| SdfObject {
            blobs: vec![Blob::circle([x, 0.0], 0.1)],
            k: 0.0,
            color: [1.0; 4],
            flags: 0,
        };
        let mut stats = CullStats::default();
        let kept = objects(vec![obj(0.0), obj(2.05), obj(2.5)], &view(), &mut stats);
        assert_eq!(kept, vec![obj(0.0), obj(2.05)]);
        assert_eq!(
            (stats.objects, stats.objects_drawn, stats.objects_over),
            (3, 2, 0)
        );
    }

    #[test]
    fn segment_distance_is_exact_enough() {
        let v = view();
        // Отрезок по диагонали мимо угла экрана (2, 1): ближе всего его конец (3, 2).
        let d = v.segment_distance([3.0, 2.0], [2.0, 3.0]);
        assert!((d - 2.0f32.sqrt()).abs() < 1e-4, "{d}");
        let d = v.segment_distance([-5.0, 0.0], [5.0, 0.0]);
        assert_eq!(d, 0.0);
        let d = v.segment_distance([3.0, -5.0], [3.0, 5.0]);
        assert!((d - 1.0).abs() < 1e-4, "{d}");
    }
}
