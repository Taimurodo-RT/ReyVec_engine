//! Цена модели (М.7, `docs/models-3d.md`): сколько модель стоит лучу и какой выходит её
//! сетка для режима «Треугольники». Считается на процессоре по той же программе, что
//! исполняет шейдер, в своём потоке: сетка большой модели строится заметное время, а
//! редактор не должен замирать.
//!
//! «Примитивов на шаг» — сколько примитивов программа считает в точке. Точки берутся
//! сеткой 12×12×12 в коробке вокруг модели (с запасом), как шаги лучей, которые идут
//! рядом с ней и сквозь неё. Границы частей (М.1) пропускают далёкие группы, поэтому у
//! хорошо разложенной модели это число меньше числа частей.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glam::Vec3;
use reyvec_sdf::Shape3;
use reyvec_sdf::shape_mesh::{MeshSettings, mesh_program};
use reyvec_sdf::shape3d::{MAX_PROGRAM_WORDS, compile_with};

/// Столько без правок — и модель замеряется.
const QUIET: Duration = Duration::from_millis(300);

/// Точек по стороне при замере «примитивов на шаг».
const SAMPLES: usize = 12;

/// Насколько модель тяжела для лучей.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Weight {
    Light,
    Medium,
    Heavy,
}

impl Weight {
    /// Примитивов на шаг в среднем: до 8 — лёгкая (как простые предметы завода), до
    /// 24 — средняя; больше — тяжёлая: такая модель в кадре десятками копий заметно
    /// дороже (оценка решения 0018: 0,018 мс на предмет в клетке на RTX 5080).
    fn of(mean: f32, words: usize) -> Self {
        if mean > 24.0 || words * 4 > MAX_PROGRAM_WORDS * 3 {
            Self::Heavy
        } else if mean > 8.0 {
            Self::Medium
        } else {
            Self::Light
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Light => "лёгкая",
            Self::Medium => "средняя",
            Self::Heavy => "тяжёлая",
        }
    }
}

/// Сетка модели для режима «Треугольники».
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MeshCost {
    pub triangles: usize,
    /// Клетка сетки, метры: детали тоньше неё могут пропасть.
    pub cell: f32,
    /// Насколько сетка отходит от формы, метры.
    pub error: f32,
    /// Сколько строилась, мс (на этом процессоре, при чтении сцены).
    pub ms: f64,
}

/// Цена модели.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShapeCost {
    /// Примитивов в программе (все части, копии повторов и зеркал по отдельности).
    pub prims: usize,
    /// Слов программы и сколько их можно.
    pub words: usize,
    /// Примитивов на шаг: в среднем и самое большое.
    pub mean: f32,
    pub worst: usize,
    pub weight: Weight,
    /// Нет — модель бесконечна или пуста.
    pub mesh: Option<MeshCost>,
}

/// Замер цены формы.
pub(crate) fn measure(shape: &Shape3) -> Result<ShapeCost, String> {
    let all = compile_with(shape, false).map_err(|e| e.to_string())?;
    let program = compile_with(shape, true).map_err(|e| e.to_string())?;
    let (lo, hi) = program
        .bounds_box
        .ok_or("модель бесконечна: цену не посчитать")?;
    let pad = (hi - lo) * 0.15 + Vec3::splat(0.05);
    let (lo, hi) = (lo - pad, hi + pad);
    let mid = (lo + hi) * 0.5;
    let prims = all.distance_counted(mid).1;
    let (mut sum, mut worst) = (0usize, 0usize);
    let n = SAMPLES;
    for i in 0..n {
        for j in 0..n {
            for k in 0..n {
                let t = (Vec3::new(i as f32, j as f32, k as f32) + 0.5) / n as f32;
                let c = program.distance_counted(lo + (hi - lo) * t).1;
                sum += c;
                worst = worst.max(c);
            }
        }
    }
    let mean = sum as f32 / (n * n * n) as f32;
    let started = Instant::now();
    let mesh = mesh_program(&program, &MeshSettings::default()).map(|m| MeshCost {
        triangles: m.triangle_count(),
        cell: m.cell,
        error: m.max_error(&program),
        ms: started.elapsed().as_secs_f64() * 1000.0,
    });
    Ok(ShapeCost {
        prims,
        words: program.words.len(),
        mean,
        worst,
        weight: Weight::of(mean, program.words.len()),
        mesh,
    })
}

/// Замер в своём потоке: новый замер заменяет старый, ответ берётся, когда готов.
/// Номер замера и его итог.
type Done = Arc<Mutex<Option<(u64, Result<ShapeCost, String>)>>>;

#[derive(Default)]
pub(crate) struct CostJob {
    done: Done,
    /// Номер последней формы, которую попросили замерить.
    asked: u64,
    /// Форма ждёт замера: с какого времени (пока модель тянут мышью, правки идут каждый
    /// кадр — замер начинается, когда они затихнут, и только один за раз).
    waiting: Option<(Shape3, Instant)>,
    /// Последняя форма, отданная на замер.
    last: Option<Shape3>,
    running: Arc<AtomicBool>,
}

impl CostJob {
    /// Замерить форму (когда правки затихнут, см. [`CostJob::tick`]).
    pub(crate) fn start(&mut self, shape: Shape3) {
        if self.last.as_ref() == Some(&shape) && self.waiting.is_none() {
            return;
        }
        self.asked += 1;
        self.waiting = Some((shape, Instant::now()));
    }

    /// Каждый кадр: начать отложенный замер, если пора и прошлый досчитан.
    pub(crate) fn tick(&mut self) {
        let ready = self
            .waiting
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() >= QUIET);
        if !ready || self.running.load(Ordering::Acquire) {
            return;
        }
        let Some((shape, _)) = self.waiting.take() else {
            return;
        };
        self.last = Some(shape.clone());
        let id = self.asked;
        let done = Arc::clone(&self.done);
        let running = Arc::clone(&self.running);
        running.store(true, Ordering::Release);
        std::thread::spawn(move || {
            let r = measure(&shape);
            if let Ok(mut d) = done.lock() {
                *d = Some((id, r));
            }
            running.store(false, Ordering::Release);
        });
    }

    /// Итог замера последней формы: `None` — ещё считается.
    pub(crate) fn result(&self) -> Option<Result<ShapeCost, String>> {
        let d = self.done.lock().ok()?;
        d.as_ref()
            .filter(|(id, _)| *id == self.asked)
            .map(|(_, r)| r.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rack_costs_less_per_step_than_its_parts_and_has_a_mesh() {
        let shape: Shape3 = ron::from_str(
            "Union(shapes: [
                Move(at: (-0.95, 0, 0), shape: Box(size: (0.06, 2, 0.8))),
                Move(at: (0.95, 0, 0), shape: Box(size: (0.06, 2, 0.8))),
                Repeat(count: 8, step: (0, 0.25, 0), shape:
                    Move(at: (0, -0.9, 0), shape: Box(size: (1.96, 0.05, 0.8)))),
            ])",
        )
        .unwrap();
        // Через поток, как в редакторе.
        let mut job = CostJob::default();
        job.start(shape);
        let t = Instant::now();
        while job.result().is_none() && t.elapsed().as_secs() < 120 {
            job.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
        let c = job.result().unwrap().unwrap();
        assert_eq!(c.prims, 10);
        assert!(c.mean <= c.prims as f32 && c.worst <= c.prims, "{c:?}");
        let m = c.mesh.unwrap();
        assert!(m.triangles > 12 && m.cell > 0.0, "{m:?}");
        assert_eq!(c.weight, Weight::of(c.mean, c.words));
        let e = measure(&ron::from_str("Plane(normal: (0, 1, 0), offset: 0)").unwrap());
        assert!(e.is_err());
    }
}
