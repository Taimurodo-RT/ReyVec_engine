//! Смеситель: складывает звучащие звуки в поток для звуковой карты.
//!
//! Работает в потоке звука (его зовёт звуковая карта, когда ей нужен следующий кусок).
//! Там нельзя ждать и выделять память: команды приходят через очередь без блокировок,
//! место под звучащие звуки выделено заранее, а сами отсчёты звуков общие с банком.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

/// Сколько звуков может звучать сразу. Лишние не играют (шагов и искр много, а слышно
/// всё равно несколько последних).
pub const VOICES: usize = 32;

/// Что сделать смесителю.
pub enum Command {
    /// Сыграть звук: громкость левого и правого канала; `id` нужен, чтобы остановить.
    Play {
        id: u64,
        samples: Arc<[f32]>,
        gain: [f32; 2],
        looped: bool,
    },
    /// Плавно выключить звук `id` (музыку).
    Stop { id: u64 },
}

struct Voice {
    id: u64,
    samples: Arc<[f32]>,
    pos: usize,
    gain: [f32; 2],
    looped: bool,
    /// Плавный выход: сколько отсчётов осталось (0 — не выходит).
    fade: u32,
}

/// Сколько отсчётов длится плавный выход при остановке (около 0,2 с при 48 кГц).
const FADE: u32 = 9_600;

pub struct Mixer {
    commands: Receiver<Command>,
    voices: Vec<Voice>,
}

impl Mixer {
    pub fn new(commands: Receiver<Command>) -> Self {
        Self {
            commands,
            voices: Vec::with_capacity(VOICES),
        }
    }

    /// Звучащих сейчас.
    #[cfg(test)]
    pub fn playing(&self) -> usize {
        self.voices.len()
    }

    fn take_commands(&mut self) {
        while let Ok(c) = self.commands.try_recv() {
            match c {
                Command::Play {
                    id,
                    samples,
                    gain,
                    looped,
                } => {
                    if self.voices.len() < VOICES {
                        self.voices.push(Voice {
                            id,
                            samples,
                            pos: 0,
                            gain,
                            looped,
                            fade: 0,
                        });
                    }
                }
                Command::Stop { id } => {
                    for v in self.voices.iter_mut().filter(|v| v.id == id) {
                        v.fade = FADE;
                    }
                }
            }
        }
    }

    /// Заполняет `out` (каналы подряд, `channels` на кадр) сводом всех звуков.
    pub fn fill(&mut self, out: &mut [f32], channels: usize) {
        self.take_commands();
        let channels = channels.max(1);
        for frame in out.chunks_mut(channels) {
            let mut l = 0.0f32;
            let mut r = 0.0f32;
            for v in &mut self.voices {
                if v.pos >= v.samples.len() {
                    continue;
                }
                let mut s = v.samples[v.pos];
                if v.fade > 0 {
                    s *= v.fade as f32 / FADE as f32;
                    v.fade -= 1;
                    if v.fade == 0 {
                        // Затих полностью: дальше не звучит.
                        v.pos = v.samples.len();
                        v.looped = false;
                        continue;
                    }
                }
                l += s * v.gain[0];
                r += s * v.gain[1];
                v.pos += 1;
                if v.pos >= v.samples.len() && v.looped {
                    v.pos = 0;
                }
            }
            // Мягкое ограничение: много звуков разом не хрипят, а чуть сжимаются.
            let (l, r) = (soft_clip(l), soft_clip(r));
            match frame.len() {
                1 => frame[0] = 0.5 * (l + r),
                _ => {
                    frame[0] = l;
                    frame[1] = r;
                    for c in &mut frame[2..] {
                        *c = 0.0;
                    }
                }
            }
        }
        // Доигравшие уходят (порядок не важен; память не освобождается: отсчёты общие).
        self.voices.retain(|v| v.pos < v.samples.len());
    }
}

fn soft_clip(x: f32) -> f32 {
    if x.abs() <= 0.8 {
        x
    } else {
        // Выше 0,8 плавно подходит к 1 и не переходит её.
        let over = x.abs() - 0.8;
        (0.8 + 0.2 * (over / (over + 0.2))).copysign(x)
    }
}

/// Громкость в децибелах и положение слева направо (-1…1) в громкости каналов: при
/// положении 0 оба канала одинаковы, по краям звук почти целиком в одном канале.
pub fn gain(decibels: f32, pan: f32) -> [f32; 2] {
    let amp = 10f32.powf(decibels / 20.0);
    let a = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    [amp * a.cos(), amp * a.sin()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn plays_mixes_and_drops_finished_sounds() {
        let (tx, rx) = channel();
        let mut m = Mixer::new(rx);
        let one: Arc<[f32]> = vec![0.5; 4].into();
        tx.send(Command::Play {
            id: 1,
            samples: one.clone(),
            gain: [1.0, 0.0],
            looped: false,
        })
        .unwrap();
        tx.send(Command::Play {
            id: 2,
            samples: one,
            gain: [0.0, 0.5],
            looped: false,
        })
        .unwrap();
        let mut out = [9.0f32; 2 * 6];
        m.fill(&mut out, 2);
        assert_eq!(&out[..2], &[0.5, 0.25]);
        assert_eq!(&out[8..], &[0.0, 0.0, 0.0, 0.0], "доиграли — тишина");
        assert_eq!(m.playing(), 0);
    }

    #[test]
    fn music_loops_until_stopped_then_fades() {
        let (tx, rx) = channel();
        let mut m = Mixer::new(rx);
        tx.send(Command::Play {
            id: 7,
            samples: vec![0.1; 100].into(),
            gain: [1.0, 1.0],
            looped: true,
        })
        .unwrap();
        let mut out = vec![0.0f32; 2 * 1000];
        m.fill(&mut out, 2);
        assert!(
            out.iter().all(|&s| (s - 0.1).abs() < 1e-6),
            "играет по кругу"
        );
        tx.send(Command::Stop { id: 7 }).unwrap();
        let mut out = vec![0.0f32; 2 * (FADE as usize + 10)];
        m.fill(&mut out, 2);
        assert!(out[0] > out[2 * 5000], "затихает");
        assert_eq!(out[out.len() - 1], 0.0, "затих");
        assert_eq!(m.playing(), 0);
    }

    #[test]
    fn too_many_sounds_are_dropped_and_loud_mix_stays_below_one() {
        let (tx, rx) = channel();
        let mut m = Mixer::new(rx);
        for id in 0..(VOICES as u64 + 10) {
            tx.send(Command::Play {
                id,
                samples: vec![1.0; 10].into(),
                gain: [1.0, 1.0],
                looped: false,
            })
            .unwrap();
        }
        let mut out = [0.0f32; 2];
        m.fill(&mut out, 2);
        assert_eq!(m.playing(), VOICES);
        assert!(out[0] < 1.0 && out[0] > 0.95, "{}", out[0]);
    }

    #[test]
    fn pan_moves_sound_between_channels() {
        let c = gain(0.0, 0.0);
        assert!((c[0] - c[1]).abs() < 1e-6);
        let r = gain(0.0, 1.0);
        assert!(r[0] < 1e-6 && r[1] > 0.99);
        let quiet = gain(-20.0, 0.0);
        assert!((quiet[0] - 0.1 * c[0]).abs() < 1e-4);
    }
}
