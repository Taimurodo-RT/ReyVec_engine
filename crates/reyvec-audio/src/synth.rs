//! Звуки и музыка-заготовки, собранные из простых волн и шума (6.7).
//!
//! Так у движка с первого дня есть звук без чужих файлов и лицензий. Звучит это как в
//! старых играх; настоящие записи потом подменяют заготовки без переделки игры: игра
//! просит «шаг» или «удар», а не конкретный файл.

use std::f32::consts::TAU;
use std::sync::Arc;

/// Повторяемый шум (splitmix64): один и тот же звук при каждом запуске.
struct Noise(u64);

impl Noise {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    /// Число от -1 до 1.
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }
}

/// Простой фильтр низких частот: убирает шипение у шума, частота среза в Гц.
struct LowPass {
    y: f32,
}

impl LowPass {
    fn step(&mut self, x: f32, cutoff: f32, rate: f32) -> f32 {
        let k = 1.0 - (-TAU * cutoff / rate).exp();
        self.y += (x - self.y) * k;
        self.y
    }
}

/// Огибающая: быстрое нарастание за `attack` секунд, затем затухание с постоянной `decay`.
fn envelope(t: f32, attack: f32, decay: f32) -> f32 {
    if t < attack {
        t / attack
    } else {
        (-(t - attack) / decay).exp()
    }
}

/// Плавный конец, чтобы обрезка звука не щёлкала.
fn fade_out(samples: &mut [f32], rate: f32) {
    let n = ((0.01 * rate) as usize).min(samples.len());
    let len = samples.len();
    for (i, s) in samples[len - n..].iter_mut().enumerate() {
        *s *= 1.0 - i as f32 / n as f32;
    }
}

fn render(seconds: f32, rate: f32, mut f: impl FnMut(f32) -> f32) -> Vec<f32> {
    let n = (seconds * rate) as usize;
    let mut out: Vec<f32> = (0..n).map(|i| f(i as f32 / rate)).collect();
    fade_out(&mut out, rate);
    out
}

/// Шаг: глухой короткий стук (шум без верхов и низкий тон). `variant` слегка меняет
/// высоту и шум, чтобы шаги не звучали одинаково.
pub fn step(rate: f32, variant: u32) -> Vec<f32> {
    let mut noise = Noise::new(11 + u64::from(variant));
    let mut lp = LowPass { y: 0.0 };
    let pitch = 95.0 + 12.0 * variant as f32;
    render(0.09, rate, |t| {
        let thump = (TAU * pitch * t).sin() * envelope(t, 0.002, 0.025);
        let scuff = lp.step(noise.next(), 1100.0, rate) * envelope(t, 0.001, 0.018);
        0.55 * thump + 0.9 * scuff
    })
}

/// Прыжок: короткий тон, который быстро идёт вверх.
pub fn jump(rate: f32) -> Vec<f32> {
    let mut phase = 0.0f32;
    render(0.17, rate, |t| {
        let freq = 260.0 + 900.0 * t;
        phase += TAU * freq / rate;
        let wave = phase.sin() + 0.25 * (3.0 * phase).sin();
        0.32 * wave * envelope(t, 0.005, 0.06)
    })
}

/// Приземление: низкий удар и шорох.
pub fn land(rate: f32) -> Vec<f32> {
    let mut noise = Noise::new(23);
    let mut lp = LowPass { y: 0.0 };
    let mut phase = 0.0f32;
    render(0.22, rate, |t| {
        let freq = 90.0 - 40.0 * t;
        phase += TAU * freq / rate;
        let thump = phase.sin() * envelope(t, 0.002, 0.06);
        let dust = lp.step(noise.next(), 600.0, rate) * envelope(t, 0.002, 0.05);
        0.7 * thump + 1.2 * dust
    })
}

/// Рывок: шум, у которого верхи быстро уходят (свист воздуха).
pub fn dash(rate: f32) -> Vec<f32> {
    let mut noise = Noise::new(37);
    let mut lp = LowPass { y: 0.0 };
    render(0.24, rate, |t| {
        let cutoff = 2600.0 - 9000.0 * t;
        lp.step(noise.next(), cutoff.max(300.0), rate) * envelope(t, 0.03, 0.07) * 0.9
    })
}

/// Взмах: короткий свист.
pub fn swing(rate: f32) -> Vec<f32> {
    let mut noise = Noise::new(41);
    let mut lp = LowPass { y: 0.0 };
    render(0.16, rate, |t| {
        let cutoff = 1200.0 + 9000.0 * t;
        lp.step(noise.next(), cutoff, rate) * envelope(t, 0.04, 0.04) * 0.7
    })
}

/// Попадание: щелчок, хлопок шума и низкий тон, падающий вниз.
pub fn hit(rate: f32) -> Vec<f32> {
    let mut noise = Noise::new(53);
    let mut lp = LowPass { y: 0.0 };
    let mut phase = 0.0f32;
    render(0.3, rate, |t| {
        let freq = 150.0 * (-t * 9.0).exp() + 55.0;
        phase += TAU * freq / rate;
        let body = phase.sin() * envelope(t, 0.001, 0.09);
        let crack = lp.step(noise.next(), 3500.0, rate) * envelope(t, 0.0005, 0.03);
        0.6 * body + 0.5 * crack
    })
}

/// Подобрал: две высокие ноты колокольчиком (ми и си), вторая чуть позже.
pub fn pickup(rate: f32) -> Vec<f32> {
    render(0.32, rate, |t| {
        let bell = |f: f32, t: f32| {
            if t < 0.0 {
                return 0.0;
            }
            let e = envelope(t, 0.002, 0.09);
            ((TAU * f * t).sin() + 0.3 * (TAU * 2.0 * f * t).sin()) * e
        };
        0.3 * bell(1318.5, t) + 0.3 * bell(1975.5, t - 0.07)
    })
}

/// Героя ранили (6.10б): глухой удар и короткая нисходящая «жалоба» квадратной волной.
pub fn hurt(rate: f32) -> Vec<f32> {
    let mut noise = Noise::new(71);
    let mut lp = LowPass { y: 0.0 };
    let mut phase = 0.0f32;
    render(0.34, rate, |t| {
        let freq = 420.0 * (-t * 4.0).exp() + 140.0;
        phase += freq / rate;
        let square = if phase.fract() < 0.5 { 1.0 } else { -1.0 };
        let tone = lp.step(square, 1800.0, rate) * envelope(t, 0.004, 0.12);
        let thud = noise.next() * envelope(t, 0.001, 0.025);
        0.45 * tone + 0.35 * thud
    })
}

/// Частота ноты MIDI (69 — ля первой октавы, 440 Гц).
fn note(midi: f32) -> f32 {
    440.0 * 2f32.powf((midi - 69.0) / 12.0)
}

/// Музыка-заготовка: спокойный круг из четырёх аккордов (ля минор, фа, до, соль) по
/// 4 секунды, мягкая подложка, перебор и бас. Ровно 16 секунд: конец переходит в
/// начало без щелчка, звук играет по кругу.
pub fn music(rate: f32) -> Vec<f32> {
    // Корни аккордов и минор/мажор.
    const CHORDS: [(f32, bool); 4] = [(57.0, true), (53.0, false), (60.0, false), (55.0, false)];
    const BAR: f32 = 4.0;
    const EIGHTH: f32 = 0.25;
    let n = (BAR * CHORDS.len() as f32 * rate) as usize;
    let mut out = vec![0.0f32; n];
    for (bar, &(root, minor)) in CHORDS.iter().enumerate() {
        let third = if minor { 3.0 } else { 4.0 };
        let notes = [root, root + third, root + 7.0, root + 12.0];
        let start = (bar as f32 * BAR * rate) as usize;
        let len = (BAR * rate) as usize;
        for i in 0..len {
            let t = i as f32 / rate;
            // Подложка: треугольные волны, плавно входят и выходят в пределах такта.
            let swell = (std::f32::consts::PI * t / BAR).sin().powf(0.6);
            let mut pad = 0.0;
            for &m in &notes[..3] {
                let p = (note(m) * t).fract();
                pad += 4.0 * (p - 0.5).abs() - 1.0;
            }
            // Перебор: восьмые по нотам аккорда вверх и вниз, каждая затухает.
            let step = (t / EIGHTH) as usize;
            let order = [0, 1, 2, 3, 2, 1, 2, 3];
            let m = notes[order[step % order.len()]] + 12.0;
            let local = t - step as f32 * EIGHTH;
            // Последняя нота такта гаснет к его концу: стык тактов и круга без щелчка.
            let tail = ((BAR - t) / 0.05).min(1.0);
            let pluck = (TAU * note(m) * local).sin() * (-local / 0.18).exp() * tail;
            // Бас на сильные доли.
            let beat = t % 1.0;
            let bass = (TAU * note(root - 12.0) * t).sin() * (-beat / 0.35).exp() * swell;
            out[start + i] = 0.06 * pad * swell + 0.1 * pluck + 0.12 * bass;
        }
    }
    out
}

/// Все звуки игры, собранные под частоту устройства.
pub struct Bank {
    pub rate: u32,
    pub steps: Vec<Arc<[f32]>>,
    pub jump: Arc<[f32]>,
    pub land: Arc<[f32]>,
    pub dash: Arc<[f32]>,
    pub swing: Arc<[f32]>,
    pub hit: Arc<[f32]>,
    pub pickup: Arc<[f32]>,
    pub hurt: Arc<[f32]>,
    pub music: Arc<[f32]>,
}

impl Bank {
    /// Собирает все звуки (музыка — самая долгая часть, доли секунды).
    pub fn build(rate: u32) -> Self {
        let r = rate as f32;
        Self {
            rate,
            steps: (0..3).map(|v| step(r, v).into()).collect(),
            jump: jump(r).into(),
            land: land(r).into(),
            dash: dash(r).into(),
            swing: swing(r).into(),
            hit: hit(r).into(),
            pickup: pickup(r).into(),
            hurt: hurt(r).into(),
            music: music(r).into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(s: &[f32]) -> f32 {
        s.iter().fold(0.0, |m, v| m.max(v.abs()))
    }

    #[test]
    fn effects_are_short_audible_and_do_not_clip() {
        let rate = 48_000.0;
        let all = [
            ("шаг", step(rate, 0)),
            ("прыжок", jump(rate)),
            ("приземление", land(rate)),
            ("рывок", dash(rate)),
            ("взмах", swing(rate)),
            ("попадание", hit(rate)),
            ("подобрал", pickup(rate)),
            ("ранен", hurt(rate)),
        ];
        for (name, s) in &all {
            let seconds = s.len() as f32 / rate;
            assert!((0.05..=0.35).contains(&seconds), "{name}: {seconds} с");
            let p = peak(s);
            assert!((0.1..=1.0).contains(&p), "{name}: пик {p}");
            // Конец тихий: обрезка не щёлкает.
            assert!(s.last().unwrap().abs() < 1e-3, "{name}");
        }
    }

    #[test]
    fn steps_differ_and_sounds_repeat_exactly() {
        let rate = 44_100.0;
        assert_ne!(step(rate, 0), step(rate, 1));
        assert_eq!(hit(rate), hit(rate));
    }

    #[test]
    fn music_is_sixteen_seconds_and_loops_without_a_click() {
        let rate = 8_000.0;
        let m = music(rate);
        assert_eq!(m.len(), 16 * 8_000);
        assert!(peak(&m) <= 0.6, "пик {}", peak(&m));
        // Стык конца и начала: соседние отсчёты близки.
        let jump = (m[m.len() - 1] - m[0]).abs();
        assert!(jump < 0.02, "скачок на стыке {jump}");
        // Звучит весь круг, а не только начало.
        let quarter = m.len() / 4;
        for k in 0..4 {
            let part = &m[k * quarter..(k + 1) * quarter];
            assert!(peak(part) > 0.05, "такт {k} тихий");
        }
    }
}
