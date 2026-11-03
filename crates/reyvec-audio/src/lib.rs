//! Звук игры (6.7): звуки событий и музыка.
//!
//! Игра просит звук по смыслу ([`Sound`]: шаг, прыжок, удар), с громкостью и
//! положением слева направо. Звуки собираются в фоне (синтез, [`synth`]) под частоту
//! звуковой карты; пока они не готовы, просьбы пропускаются. Смеситель свой
//! ([`mixer`]) поверх `cpal`: библиотека `kira` из плана тянет зависимость под
//! лицензией MPL-2.0, которой нет в `deny.toml`.
//!
//! Нет звуковой карты (сервер проверок, отключённое устройство) — игра идёт без звука,
//! а [`AudioStats::error`] говорит почему.

pub mod mixer;
pub mod synth;

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::mixer::{Command, Mixer};
use crate::synth::Bank;

/// Звук по смыслу.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    Step,
    Jump,
    Land,
    Dash,
    Swing,
    Hit,
}

/// Что видно о звуке на панели.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioStats {
    /// Звуковая карта: название, частота и каналы.
    pub device: Option<String>,
    /// Почему звука нет.
    pub error: Option<String>,
    /// Сколько собирались звуки в фоне, мс (нет — ещё собираются).
    pub load_ms: Option<f64>,
    /// Сколько звуков сыграно и сколько пропущено (звуки ещё не готовы).
    pub played: u64,
    pub skipped: u64,
    pub music: bool,
}

/// Громкость музыки, дБ: тише звуков игры.
const MUSIC_DB: f32 = -10.0;
const MUSIC_ID: u64 = 1;

/// Звук игры: звуковая карта, банк звуков и музыка.
pub struct Audio {
    /// Поток звуковой карты: пока он жив, звук идёт.
    stream: Option<cpal::Stream>,
    commands: Option<Sender<Command>>,
    loading: Option<Receiver<(Bank, f64)>>,
    bank: Option<Bank>,
    stats: AudioStats,
    next_id: u64,
    step_variant: usize,
}

impl Audio {
    /// Открывает звуковую карту по умолчанию и начинает собирать звуки в фоне. Ошибки не
    /// мешают игре: без звуковой карты звук просто молчит.
    pub fn new(music: bool) -> Self {
        let mut audio = Self {
            stream: None,
            commands: None,
            loading: None,
            bank: None,
            stats: AudioStats {
                music,
                ..AudioStats::default()
            },
            next_id: MUSIC_ID + 1,
            step_variant: 0,
        };
        match open() {
            Ok((stream, commands, rate, name)) => {
                tracing::info!("Звук: {name}");
                audio.stats.device = Some(name);
                audio.stream = Some(stream);
                audio.commands = Some(commands);
                let (tx, rx) = channel();
                let spawned = std::thread::Builder::new()
                    .name("reyvec-sounds".into())
                    .spawn(move || {
                        let started = std::time::Instant::now();
                        let bank = Bank::build(rate);
                        let _ = tx.send((bank, started.elapsed().as_secs_f64() * 1000.0));
                    });
                match spawned {
                    Ok(_) => audio.loading = Some(rx),
                    Err(e) => audio.stats.error = Some(format!("поток сборки звуков: {e}")),
                }
            }
            Err(e) => {
                tracing::warn!("Звука не будет: {e}");
                audio.stats.error = Some(e);
            }
        }
        audio
    }

    /// Забирает готовые звуки из фона; вызывать каждый кадр.
    pub fn poll(&mut self) {
        let Some(rx) = &self.loading else {
            return;
        };
        if let Ok((bank, ms)) = rx.try_recv() {
            tracing::info!("Звуки собраны в фоне за {ms:.0} мс");
            self.loading = None;
            self.stats.load_ms = Some(ms);
            self.bank = Some(bank);
            if self.stats.music {
                self.start_music();
            }
        }
    }

    /// Играет звук `sound` с громкостью `decibels` (0 — как есть, -6 — вдвое тише) в
    /// положении `pan` (-1 — слева, 1 — справа).
    pub fn play(&mut self, sound: Sound, decibels: f32, pan: f32) {
        let (Some(bank), Some(commands)) = (&self.bank, &self.commands) else {
            self.stats.skipped += u64::from(self.commands.is_some());
            return;
        };
        let samples = match sound {
            Sound::Step => {
                self.step_variant = (self.step_variant + 1) % bank.steps.len();
                bank.steps[self.step_variant].clone()
            }
            Sound::Jump => bank.jump.clone(),
            Sound::Land => bank.land.clone(),
            Sound::Dash => bank.dash.clone(),
            Sound::Swing => bank.swing.clone(),
            Sound::Hit => bank.hit.clone(),
        };
        self.next_id += 1;
        let sent = commands.send(Command::Play {
            id: self.next_id,
            samples,
            gain: mixer::gain(decibels, pan),
            looped: false,
        });
        if sent.is_ok() {
            self.stats.played += 1;
        }
    }

    fn start_music(&mut self) {
        if let (Some(bank), Some(commands)) = (&self.bank, &self.commands) {
            let _ = commands.send(Command::Play {
                id: MUSIC_ID,
                samples: Arc::clone(&bank.music),
                gain: mixer::gain(MUSIC_DB, 0.0),
                looped: true,
            });
        }
    }

    /// Включает или выключает музыку (выключается плавно).
    pub fn set_music(&mut self, on: bool) {
        if on == self.stats.music {
            return;
        }
        self.stats.music = on;
        if on {
            self.start_music();
        } else if let Some(commands) = &self.commands {
            let _ = commands.send(Command::Stop { id: MUSIC_ID });
        }
    }

    pub fn stats(&self) -> AudioStats {
        self.stats.clone()
    }

    /// Есть ли звуковая карта.
    pub fn has_device(&self) -> bool {
        self.stream.is_some()
    }
}

/// Открывает звуковую карту по умолчанию: поток, очередь команд смесителю, частота и
/// название для панели.
fn open() -> Result<(cpal::Stream, Sender<Command>, u32, String), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("нет звуковой карты по умолчанию")?;
    let config = device
        .default_output_config()
        .map_err(|e| format!("настройки звуковой карты: {e}"))?;
    let name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "звуковая карта".into());
    let rate = config.sample_rate();
    let channels = usize::from(config.channels());
    let (tx, rx) = channel();
    let mixer = Mixer::new(rx);
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, config.config(), mixer, channels),
        cpal::SampleFormat::I16 => build::<i16>(&device, config.config(), mixer, channels),
        cpal::SampleFormat::U16 => build::<u16>(&device, config.config(), mixer, channels),
        cpal::SampleFormat::I32 => build::<i32>(&device, config.config(), mixer, channels),
        other => return Err(format!("формат звука {other:?} не поддержан")),
    }?;
    stream.play().map_err(|e| format!("запуск звука: {e}"))?;
    Ok((
        stream,
        tx,
        rate,
        format!("{name}, {rate} Гц, каналов {channels}"),
    ))
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut mixer: Mixer,
    channels: usize,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    // Свой буфер отсчётов с запасом: в потоке звука память не выделяется (кроме редкого
    // случая, когда звуковая карта просит кусок больше обычного).
    let mut buffer = vec![0.0f32; 8192];
    device
        .build_output_stream::<T, _, _>(
            config,
            move |out: &mut [T], _| {
                if buffer.len() < out.len() {
                    buffer.resize(out.len(), 0.0);
                }
                let buf = &mut buffer[..out.len()];
                mixer.fill(buf, channels);
                for (o, s) in out.iter_mut().zip(buf.iter()) {
                    *o = T::from_sample(*s);
                }
            },
            |e| tracing::warn!("звук: {e}"),
            None,
        )
        .map_err(|e| format!("поток звука: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Без звуковой карты (как на сервере проверок) игра не падает: звуки молчат, а
    /// причина видна на панели. Со звуковой картой звуки собираются в фоне.
    #[test]
    fn works_with_or_without_a_sound_card() {
        let mut audio = Audio::new(false);
        for _ in 0..200 {
            audio.poll();
            if audio.stats().load_ms.is_some() || !audio.has_device() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        audio.play(Sound::Hit, 0.0, 0.0);
        audio.set_music(true);
        audio.set_music(false);
        let s = audio.stats();
        if audio.has_device() {
            assert!(s.device.is_some() && s.error.is_none(), "{s:?}");
        } else {
            assert!(s.error.is_some() && s.played == 0, "{s:?}");
        }
    }
}
