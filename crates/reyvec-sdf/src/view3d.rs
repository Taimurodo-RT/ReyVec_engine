//! «Мир в конусе зрения» (этап 7.6к): в кадр идут только объекты, которые хоть как-то
//! влияют на картинку. Остальные не попадают в буфер сцены и в сетку отсева — для
//! видеокарты их в этом кадре нет.
//!
//! Объект влияет на кадр, если:
//!
//! - его шар-граница хоть краем в пирамиде камеры (с запасом [`AO_MARGIN`]: затенение в
//!   углах пробует поле до полуметра от видимой поверхности и различает расстояния до
//!   [`AO_REACH`]);
//! - или он может бросить тень на видимое: луч тени от видимой точки к солнцу или к одной
//!   из ламп с тенью проходит через него или рядом (мягкая полутень). Для этого
//!   проверяется «тень» объекта — капсула от него в сторону от света, толще на ширину
//!   полутени, — и если она задевает пирамиду камеры, объект остаётся.
//!
//! Бесконечные объекты (пол) остаются всегда. Проверка только оставляет лишнее, но не
//! выбрасывает нужное: тест `culling_keeps_the_picture` сравнивает лучи глаза и тени по
//! полной и урезанной сцене и требует полного совпадения.

use glam::{Mat4, Vec3, Vec4};

use crate::shape3d::{Lamp3, SHADOW_LAMPS, Shape3};
use crate::space_file::SpaceFrame;

/// Направление на солнце — как SUN в `include/space.slang`.
pub const SUN: Vec3 = Vec3::new(0.4969, 0.7950, 0.3478);
/// Дальше этого луч тени к солнцу не идёт (как в `raymarch_shadow.slang`).
const SUN_FAR: f32 = 30.0;
/// Жёсткость края тени от солнца и от ламп (как в `raymarch_shadow.slang`): полутень у
/// точки на расстоянии t вдоль луча тени шириной до t / k.
const SUN_HARDNESS: f32 = 12.0;
const LAMP_HARDNESS: f32 = 10.0;
/// Дальше этого затенение в углах расстояние не различает (`AO_REACH` в
/// `raymarch_shadow.slang`).
pub const AO_REACH: f32 = 1.0;
/// Запас вокруг пирамиды: пробы затенения в углах уходят от видимой точки до 0,4 м
/// (вдоль нормали и наклонно от неё, плюс подъём точки над поверхностью), и объект
/// дальше [`AO_REACH`] от любой пробы на них не влияет.
pub const AO_MARGIN: f32 = 2.0;

/// Пирамида камеры: шесть плоскостей, внутри — `dot(n, p) + d >= 0`.
#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    /// Пирамида из матрицы «мир → экран» (Vulkan: глубина 0…1, обратная или прямая —
    /// неважно). Способ Грибба и Хартманна: плоскости — суммы и разности строк матрицы.
    pub fn new(view_proj: Mat4) -> Self {
        let r = |i: usize| view_proj.row(i);
        let raw = [
            r(3) + r(0),
            r(3) - r(0),
            r(3) + r(1),
            r(3) - r(1),
            r(2),
            r(3) - r(2),
        ];
        Self {
            planes: raw.map(|p| p / p.truncate().length().max(1e-12)),
        }
    }

    /// Задевает ли шар пирамиду (с запасом). Может сказать «да» лишний раз у углов
    /// пирамиды, но «нет» — только если шар точно снаружи.
    pub fn sphere(&self, c: Vec3, r: f32) -> bool {
        self.planes.iter().all(|p| p.truncate().dot(c) + p.w >= -r)
    }

    /// Задевает ли капсула (отрезок `a`–`b` толщиной `r`) пирамиду: снаружи, только если
    /// оба конца за одной плоскостью дальше `r`.
    pub fn capsule(&self, a: Vec3, b: Vec3, r: f32) -> bool {
        self.planes.iter().all(|p| {
            let n = p.truncate();
            (n.dot(a) + p.w).max(n.dot(b) + p.w) >= -r
        })
    }
}

/// Что известно о сцене кадра для проверки теней.
struct Shadows<'a> {
    lamps: &'a [Lamp3],
    /// Есть ли солнце. У 2.5D-уровня его нет (SCENE_NO_SUN), и заслонки солнца не нужны.
    sun: bool,
    /// Ниже этой высоты видимых точек нет (весь мир над полом), или `None`.
    floor: Option<f32>,
}

impl Shadows<'_> {
    /// Может ли объект с шаром (c, r) бросить тень или полутень на точку пирамиды.
    fn casts(&self, frustum: &Frustum, c: Vec3, r: f32) -> bool {
        // Солнце: луч от видимой точки p идёт к солнцу, объект на нём — p = c − SUN·t.
        // Над самым нижним видимым местом луч поднимается, поэтому тень не длиннее
        // подъёма от пола до верха объекта.
        let sun = self.sun && {
            let length = self
                .floor
                .map_or(SUN_FAR, |y| ((c.y + r - y) / SUN.y).clamp(0.0, SUN_FAR));
            frustum.capsule(c, c - SUN * length, r + length / SUN_HARDNESS)
        };
        sun || self.lamps.iter().take(SHADOW_LAMPS).any(|l| {
            let at = Vec3::from(l.at);
            let d = c.distance(at);
            if d <= r + 1e-3 {
                return frustum.sphere(at, l.range);
            }
            // Точки, которым объект закрывает лампу, лежат в конусе от лампы через
            // объект, не дальше дальности лампы. Конус берём капсулой по его самой
            // широкой части плюс полутень.
            let beyond = (l.range - (d - r)).max(0.0);
            let dir = (c - at) / d;
            let spread = (d + beyond) * r / (d * d - r * r).max(1e-6).sqrt();
            frustum.capsule(c, c + dir * beyond, spread.max(r) + beyond / LAMP_HARDNESS)
        })
    }
}

/// Сколько объектов было и сколько осталось в кадре.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewCullStats {
    pub total: usize,
    pub kept: usize,
}

/// Убирает из кадра объекты, не влияющие на картинку камеры `view_proj`. `sun` — есть ли
/// солнце: без него (2.5D-уровень, SCENE_NO_SUN) объект остаётся только ради взгляда и
/// ламп. Раньше уровень отсеивался как сцена с солнцем, а в нём есть стена, и
/// [`lowest_visible`] не знал нижней границы: каждый объект проверялся как заслонка
/// солнца конусом в 30 м, и в кадр комнаты попадали десятки объектов коридора.
pub fn cull_frame(frame: &mut SpaceFrame, view_proj: Mat4, sun: bool) -> ViewCullStats {
    let frustum = Frustum::new(view_proj);
    let floor = if sun { lowest_visible(frame) } else { None };
    let shadows = Shadows {
        lamps: &frame.lamps,
        sun,
        floor,
    };
    let total = frame.shapes.len() + frame.proxies.len() + frame.glows.len();
    let keep = |s: &Shape3| match s.bounds() {
        None => true,
        Some((c, r)) => frustum.sphere(c, r + AO_MARGIN) || shadows.casts(&frustum, c, r),
    };
    frame.shapes.retain(keep);
    // Заменители моделей невидимы: им важны только тени и углы.
    frame.proxies.retain(keep);
    // Теней светящиеся не дают: нужны, только если их видно.
    frame
        .glows
        .retain(|s| s.bounds().is_none_or(|(c, r)| frustum.sphere(c, r)));
    ViewCullStats {
        total,
        kept: frame.shapes.len() + frame.proxies.len() + frame.glows.len(),
    }
}

/// Ниже какой высоты в кадре нет видимых точек: если все бесконечные формы —
/// горизонтальные полы, это самый нижний из полов и шаров-границ. Иначе (стена, наклонная
/// плоскость, бесконечная форма сложнее плоскости) — неизвестно.
fn lowest_visible(frame: &SpaceFrame) -> Option<f32> {
    let mut low = f32::INFINITY;
    for s in frame.shapes.iter().chain(&frame.proxies) {
        match s.bounds() {
            Some((c, r)) => low = low.min(c.y - r),
            None => {
                let program = crate::shape3d::compile(s).ok()?;
                let (n, d) = program.plane()?;
                if n.y < 0.9999 {
                    return None;
                }
                // n ≈ (0, 1, 0): пол на высоте −d.
                low = low.min(-d / n.y);
            }
        }
    }
    low.is_finite().then_some(low)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape3d::{Program, compile};
    use crate::{Camera3D, SpaceScene};

    #[test]
    fn frustum_sees_what_the_camera_sees() {
        let cam = Camera3D::looking_at(Vec3::new(0.0, 1.0, 5.0), Vec3::new(0.0, 1.0, 0.0));
        let f = Frustum::new(cam.view_proj(16.0 / 9.0));
        assert!(f.sphere(Vec3::new(0.0, 1.0, 0.0), 0.1));
        assert!(!f.sphere(Vec3::new(0.0, 1.0, 10.0), 1.0), "за спиной");
        assert!(!f.sphere(Vec3::new(40.0, 1.0, 0.0), 1.0), "сбоку");
        assert!(
            f.sphere(Vec3::new(0.0, 1.0, 6.0), 1.5),
            "за спиной, но задевает"
        );
        assert!(f.capsule(Vec3::new(0.0, 1.0, 10.0), Vec3::new(0.0, 1.0, -2.0), 0.1));
        // Плоская камера — то же.
        let mut flat = cam;
        flat.toggle_projection(5.0);
        let f = Frustum::new(flat.view_proj(16.0 / 9.0));
        assert!(f.sphere(Vec3::new(0.0, 1.0, 0.0), 0.1));
        assert!(!f.sphere(Vec3::new(40.0, 1.0, 0.0), 1.0));
    }

    /// Без солнца (2.5D-уровень) объект, который держался в кадре только как заслонка
    /// солнца, отсеивается; видимый остаётся.
    #[test]
    fn no_sun_drops_sun_only_casters() {
        let cam = Camera3D::looking_at(Vec3::new(0.0, 1.0, 5.0), Vec3::new(0.0, 1.0, 0.0));
        let view_proj = cam.view_proj(16.0 / 9.0);
        let frame = || SpaceFrame {
            shapes: vec![
                // Стена: с ней нижняя граница видимого неизвестна, и тень солнца
                // считается во всю длину — как в уровне.
                Shape3::Plane {
                    normal: [0.0, 0.0, 1.0],
                    offset: 3.0,
                },
                Shape3::Sphere { radius: 0.3 }.at([0.0, 1.0, 0.0]),
                // Шар за спиной камеры, на луче от видимой точки к солнцу.
                Shape3::Sphere { radius: 0.5 }
                    .at((Vec3::new(0.0, 1.0, 0.0) + SUN * 15.0).to_array()),
            ],
            lamps: Vec::new(),
            meshes: Vec::new(),
            proxies: Vec::new(),
            glows: Vec::new(),
        };
        let mut with_sun = frame();
        assert_eq!(
            cull_frame(&mut with_sun, view_proj, true).kept,
            3,
            "с солнцем заслонка нужна"
        );
        let mut no_sun = frame();
        assert_eq!(
            cull_frame(&mut no_sun, view_proj, false).kept,
            2,
            "без солнца она лишняя"
        );
        assert!(
            no_sun
                .shapes
                .iter()
                .any(|s| s.bounds().is_some_and(|(_, r)| (r - 0.3).abs() < 1e-6)),
            "видимый шар остался"
        );
    }

    /// Лучи глаза и тени от солнца и ламп по урезанной сцене дают ровно то же, что по
    /// полной: отсев картинку не меняет.
    #[test]
    fn culling_keeps_the_picture() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scenes/3d/stress.ron"
        ))
        .unwrap();
        let scene = SpaceScene::parse(&text).unwrap();
        let aspect = 16.0 / 9.0;
        let cameras = [
            scene.home_camera(),
            Camera3D::looking_at(Vec3::new(-3.0, 1.5, 2.0), Vec3::new(4.0, 0.5, -3.0)),
            Camera3D::looking_at(Vec3::new(0.0, 12.0, 0.5), Vec3::new(0.0, 0.0, 0.0)),
            Camera3D::looking_at(Vec3::new(8.0, 0.6, 8.0), Vec3::new(9.0, 0.5, 12.0)),
        ];
        let mut culled_any = false;
        for (k, cam) in cameras.iter().enumerate() {
            let full = scene.frame(1.3);
            let mut part = scene.frame(1.3);
            let stats = cull_frame(&mut part, cam.view_proj(aspect), true);
            culled_any |= stats.kept < stats.total;
            let progs = |f: &SpaceFrame| -> Vec<Program> {
                f.shapes.iter().map(|s| compile(s).unwrap()).collect()
            };
            let (a, b) = (progs(&full), progs(&part));
            let lamps: Vec<Lamp3> = full.lamps.iter().take(SHADOW_LAMPS).copied().collect();
            for y in 0..9 {
                for x in 0..16 {
                    let s = [(x as f32 + 0.5) / 8.0 - 1.0, 1.0 - (y as f32 + 0.5) / 4.5];
                    let (o, d) = cam.ray(s, aspect);
                    let (ta, tb) = (march(&a, o, d), march(&b, o, d));
                    assert_eq!(ta.is_some(), tb.is_some(), "камера {k}, точка {x},{y}");
                    let (Some(ta), Some(tb)) = (ta, tb) else {
                        continue;
                    };
                    assert!((ta - tb).abs() < 1e-3, "камера {k}, {x},{y}: {ta} и {tb}");
                    let p = o + d * ta;
                    let n = normal(&a, p);
                    let from = p + n * 0.02;
                    let mut lights = vec![(SUN, SUN_FAR, SUN_HARDNESS)];
                    // Лампы — как в шейдере: луч тени только там, где свет лампы виден.
                    for l in &lamps {
                        let to = Vec3::from(l.at) - from;
                        let dir = to.normalize();
                        if crate::shape3d::lamp_falloff(l, to.length()) * n.dot(dir) > 0.003 {
                            lights.push((dir, to.length() - 0.35, LAMP_HARDNESS));
                        }
                    }
                    for (dir, far, hard) in lights {
                        if n.dot(dir) <= 0.0 || far <= 0.0 {
                            continue;
                        }
                        let (sa, sb) = (
                            shadow(&a, from, dir, far, hard),
                            shadow(&b, from, dir, far, hard),
                        );
                        // Шаги луча тени у отсеянных объектов были короче, точки полутени
                        // берутся чуть иначе: разница — доли процента.
                        assert!(
                            (sa - sb).abs() < 0.02,
                            "камера {k}, {x},{y}: тень {sa} и {sb}"
                        );
                    }
                    // Затенение в углах.
                    for q in ao_probes(from, n) {
                        let (na, nb) = (near(&a, q).min(AO_REACH), near(&b, q).min(AO_REACH));
                        assert!(
                            (na - nb).abs() < 1e-4,
                            "камера {k}, {x},{y}: углы {na} и {nb}"
                        );
                    }
                }
            }
        }
        assert!(culled_any, "хоть с одной камеры что-то должно отсеяться");
    }

    /// Точки проб затенения в углах, как в `ambient_occlusion` (shadow3d.slang).
    fn ao_probes(p: Vec3, n: Vec3) -> Vec<Vec3> {
        let s = if n.z >= 0.0 { 1.0 } else { -1.0 };
        let a = -1.0 / (s + n.z);
        let b = n.x * n.y * a;
        let t1 = Vec3::new(1.0 + s * n.x * n.x * a, s * b, -s * n.x);
        let t2 = Vec3::new(b, s + n.y * n.y * a, -n.y);
        let mut probes: Vec<Vec3> = [0.04, 0.16, 0.4].iter().map(|h| p + n * *h).collect();
        for side in [t1, t2, -t1, -t2] {
            let dir = (n + 0.9 * side).normalize();
            probes.extend([0.16, 0.4].iter().map(|h| p + dir * *h));
        }
        probes
    }

    fn near(objs: &[Program], p: Vec3) -> f32 {
        objs.iter()
            .map(|o| o.distance(p).0)
            .fold(f32::INFINITY, f32::min)
    }

    fn march(objs: &[Program], o: Vec3, d: Vec3) -> Option<f32> {
        let mut t = 0.0;
        for _ in 0..256 {
            let r = near(objs, o + d * t);
            if r < 1e-3 {
                return Some(t);
            }
            t += r;
            if t > 200.0 {
                break;
            }
        }
        None
    }

    fn normal(objs: &[Program], p: Vec3) -> Vec3 {
        let e = 1e-3;
        let g = |v: Vec3| near(objs, p + v * e) - near(objs, p - v * e);
        Vec3::new(g(Vec3::X), g(Vec3::Y), g(Vec3::Z)).normalize_or(Vec3::Y)
    }

    fn shadow(objs: &[Program], from: Vec3, dir: Vec3, far: f32, k: f32) -> f32 {
        let mut light = 1.0f32;
        let mut t = 0.02;
        while t < far {
            let h = near(objs, from + dir * t);
            light = light.min(k * h / t);
            if light < 1e-3 {
                return 0.0;
            }
            t += h.clamp(0.02, 1.0);
        }
        light.clamp(0.0, 1.0)
    }
}
