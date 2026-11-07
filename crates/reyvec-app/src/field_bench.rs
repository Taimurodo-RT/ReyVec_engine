//! Сравнение поля расстояний (7.10): один и тот же кадр без поля, с полем и с «углами из
//! поля». Время сцены «3D» на сравнение замирает, чтобы предметы не двигались и все три
//! замера видели одну картинку; камеру в это время трогать нельзя.

use reyvec_gpu::{GpuFrameTimings, GpuTimingAverage};

/// Кадров после переключения, которые не считаются: поле пересчитывается целиком, а
/// замеры видеокарты приходят с опозданием на кадры в полёте.
const WARMUP: u32 = 30;
/// Кадров замера в каждом положении.
const MEASURE: u32 = 240;

/// Положения переключателей: (поле, углы из поля) и подпись.
const MODES: [((bool, bool), &str); 3] = [
    ((false, false), "без поля"),
    ((true, false), "поле"),
    ((true, true), "поле + углы из поля"),
];

/// Идущее сравнение.
pub struct FieldBench {
    /// Замершее время сцены.
    pub time: f32,
    /// Переключатели до сравнения: вернуть после.
    pub saved: (bool, bool),
    phase: usize,
    frames: u32,
    results: Vec<GpuFrameTimings>,
    average: GpuTimingAverage,
}

/// Что делать после кадра.
pub enum Step {
    /// Сравнение идёт дальше.
    Continue,
    /// Переключиться в это положение (поле, углы из поля).
    Switch((bool, bool)),
    /// Готово: отчёт для панели и журнала.
    Done(String),
}

impl FieldBench {
    /// Начать сравнение; первое положение — [`FieldBench::first_mode`].
    pub fn new(time: f32, saved: (bool, bool)) -> Self {
        FieldBench {
            time,
            saved,
            phase: 0,
            frames: 0,
            results: Vec::new(),
            average: GpuTimingAverage::default(),
        }
    }

    /// Положение переключателей для начала.
    pub fn first_mode() -> (bool, bool) {
        MODES[0].0
    }

    /// Строка для панели, пока сравнение идёт.
    pub fn status(&self) -> String {
        format!(
            "Сравнение поля: {} из 3 ({}), не трогайте камеру",
            self.phase + 1,
            MODES[self.phase].1
        )
    }

    /// Учитывает замер кадра.
    pub fn add(&mut self, timings: Option<&GpuFrameTimings>) -> Step {
        self.frames += 1;
        if self.frames > WARMUP
            && let Some(t) = timings
        {
            self.average.add(t);
        }
        if self.frames < WARMUP + MEASURE {
            return Step::Continue;
        }
        self.results
            .push(self.average.average().unwrap_or_default());
        self.average.clear();
        self.frames = 0;
        self.phase += 1;
        match MODES.get(self.phase) {
            Some((mode, _)) => Step::Switch(*mode),
            None => Step::Done(report(&self.results)),
        }
    }
}

/// Отчёт: по строке на положение — тени, поле и весь кадр видеокарты.
fn report(results: &[GpuFrameTimings]) -> String {
    let pass = |t: &GpuFrameTimings, name: &str| {
        t.passes
            .iter()
            .find(|p| p.name == name)
            .map_or(0.0, |p| p.ms)
    };
    let mut out = String::from("Сравнение поля (среднее за 240 кадров):");
    for (t, (_, label)) in results.iter().zip(MODES) {
        out.push_str(&format!(
            "\n{label}: тени {:.3}, поле {:.3}, реймарчинг {:.3}, кадр {:.3} мс",
            pass(t, "тени 3D"),
            pass(t, "поле"),
            pass(t, "реймарчинг"),
            t.total_ms
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use reyvec_gpu::PassTiming;

    fn frame(shadows: f64) -> GpuFrameTimings {
        GpuFrameTimings {
            total_ms: shadows + 1.0,
            passes: vec![PassTiming {
                name: "тени 3D",
                ms: shadows,
            }],
        }
    }

    #[test]
    fn goes_through_three_modes_and_reports() {
        let mut bench = FieldBench::new(2.0, (true, false));
        let mut switches = Vec::new();
        let mut report = None;
        for k in 0..3 {
            for _ in 0..WARMUP + MEASURE {
                match bench.add(Some(&frame(0.5 + k as f64 * 0.1))) {
                    Step::Continue => {}
                    Step::Switch(mode) => switches.push(mode),
                    Step::Done(text) => report = Some(text),
                }
            }
        }
        assert_eq!(switches, vec![(true, false), (true, true)]);
        let report = report.expect("отчёт");
        assert!(report.contains("без поля: тени 0.500"), "{report}");
        assert!(
            report.contains("поле + углы из поля: тени 0.700"),
            "{report}"
        );
    }
}
