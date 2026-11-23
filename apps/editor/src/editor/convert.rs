//! Конвертеры ассетов (Э.6): звук и картинки в другой формат прямо в редакторе.
//!
//! Звук читается из WAV (`hound`), OGG Vorbis (`vorbis_rs`), MP3 (`nanomp3`) и FLAC
//! (`claxon`), пишется в OGG Vorbis (музыка: мало места, качество на выбор) или WAV
//! (короткие звуки: читаются мгновенно). Можно свести стерео в моно. Картинки читаются
//! из PNG, JPEG, BMP, TGA, GIF, WebP (`image`), пишутся в PNG или JPEG, по желанию
//! уменьшенными до заданной стороны. Всё на Rust без внешних программ; работа идёт в
//! своём потоке, окно в [`super::explorer`].

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::num::{NonZeroU8, NonZeroU32};
use std::path::{Path, PathBuf};

/// Что за файл можно конвертировать.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Media {
    Sound,
    Picture,
}

/// Во что конвертировать.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Target {
    /// OGG Vorbis, качество 0…1 (0,4 ≈ 128 кбит/с на стерео).
    Ogg {
        quality: f32,
    },
    /// WAV, 16 бит.
    Wav,
    Png,
    /// JPEG, качество 1…100.
    Jpeg {
        quality: u8,
    },
}

impl Target {
    pub(crate) fn ext(self) -> &'static str {
        match self {
            Target::Ogg { .. } => "ogg",
            Target::Wav => "wav",
            Target::Png => "png",
            Target::Jpeg { .. } => "jpg",
        }
    }

    pub(crate) fn media(self) -> Media {
        match self {
            Target::Ogg { .. } | Target::Wav => Media::Sound,
            Target::Png | Target::Jpeg { .. } => Media::Picture,
        }
    }
}

/// Настройки конвертации.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Settings {
    pub target: Target,
    /// Свести звук в один канал.
    pub mono: bool,
    /// Уменьшить картинку, чтобы большая сторона была не больше (в точках).
    pub max_side: Option<u32>,
}

/// Звук или картинка (по окончанию файла).
pub(crate) fn media_of(path: &Path) -> Option<Media> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        "wav" | "ogg" | "mp3" | "flac" => Some(Media::Sound),
        "png" | "jpg" | "jpeg" | "bmp" | "tga" | "gif" | "webp" => Some(Media::Picture),
        _ => None,
    }
}

/// Имя результата: то же имя с новым окончанием, рядом с исходным (занято — с номером).
pub(crate) fn out_path(src: &Path, target: Target) -> PathBuf {
    let dir = src.parent().unwrap_or(Path::new("."));
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    super::project_files::free_name(dir, &format!("{stem}.{}", target.ext()))
}

/// Короткие сведения о файле для панели свойств: «2:31 · 44 100 Гц · стерео» или
/// «1920 × 1080». `None` — не прочитать быстро.
pub(crate) fn about(path: &Path) -> Option<String> {
    match media_of(path)? {
        Media::Picture => {
            let (w, h) = image::image_dimensions(path).ok()?;
            Some(format!("{w} × {h}"))
        }
        Media::Sound => {
            let ext = path.extension()?.to_string_lossy().to_lowercase();
            let (rate, channels, frames) = match ext.as_str() {
                "wav" => {
                    let r = hound::WavReader::open(path).ok()?;
                    let s = r.spec();
                    (s.sample_rate, s.channels as u32, Some(r.duration() as u64))
                }
                "flac" => {
                    let r = claxon::FlacReader::open(path).ok()?;
                    let s = r.streaminfo();
                    (s.sample_rate, s.channels, s.samples)
                }
                "ogg" => {
                    let file = BufReader::new(File::open(path).ok()?);
                    let d = vorbis_rs::VorbisDecoder::new(file).ok()?;
                    (
                        d.sampling_frequency().get(),
                        d.channels().get() as u32,
                        None,
                    )
                }
                "mp3" => {
                    let bytes = std::fs::read(path).ok()?;
                    let r = nanomp3::SliceReader::<f32>::new(&bytes);
                    let ch = r.channels()?.num() as u32;
                    (r.sample_rate(), ch, r.total_samples())
                }
                _ => return None,
            };
            let mut parts = Vec::new();
            if let Some(n) = frames.filter(|_| rate > 0) {
                let s = n / rate as u64;
                parts.push(format!("{}:{:02}", s / 60, s % 60));
            }
            parts.push(format!("{} Гц", thousands(rate)));
            parts.push(match channels {
                1 => "моно".into(),
                2 => "стерео".into(),
                n => format!("{n} каналов"),
            });
            Some(parts.join(" · "))
        }
    }
}

fn thousands(n: u32) -> String {
    if n >= 1000 {
        format!("{} {:03}", n / 1000, n % 1000)
    } else {
        n.to_string()
    }
}

/// Сконвертировать `src` в `dst`. При ошибке недописанный файл убирается.
pub(crate) fn convert(src: &Path, dst: &Path, s: Settings) -> Result<(), String> {
    let done = match s.target.media() {
        Media::Sound => read_sound(src).and_then(|mut pcm| {
            if s.mono {
                pcm.mix_to_mono();
            }
            match s.target {
                Target::Ogg { quality } => write_ogg(dst, &pcm, quality),
                _ => write_wav(dst, &pcm),
            }
        }),
        Media::Picture => picture(src, dst, s),
    };
    if done.is_err() {
        let _ = std::fs::remove_file(dst);
    }
    done
}

/// Звук в памяти: каналы чередуются, значения −1…1.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub data: Vec<f32>,
}

impl Pcm {
    fn frames(&self) -> usize {
        self.data.len() / self.channels.max(1) as usize
    }

    fn mix_to_mono(&mut self) {
        let c = self.channels.max(1) as usize;
        if c == 1 {
            return;
        }
        self.data = self
            .data
            .chunks_exact(c)
            .map(|f| f.iter().sum::<f32>() / c as f32)
            .collect();
        self.channels = 1;
    }
}

fn err(what: &str, path: &Path, e: impl std::fmt::Display) -> String {
    format!(
        "{what} «{}»: {e}",
        path.file_name().unwrap_or_default().to_string_lossy()
    )
}

/// Прочитать звук целиком.
pub(crate) fn read_sound(path: &Path) -> Result<Pcm, String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let bad = |e: &dyn std::fmt::Display| err("Не прочитать", path, e);
    let pcm = match ext.as_str() {
        "wav" => {
            let r = hound::WavReader::open(path).map_err(|e| bad(&e))?;
            let spec = r.spec();
            let data: Result<Vec<f32>, _> = match spec.sample_format {
                hound::SampleFormat::Float => r.into_samples::<f32>().collect(),
                hound::SampleFormat::Int => {
                    let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
                    r.into_samples::<i32>()
                        .map(|v| v.map(|v| v as f32 * scale))
                        .collect()
                }
            };
            Pcm {
                rate: spec.sample_rate,
                channels: spec.channels,
                data: data.map_err(|e| bad(&e))?,
            }
        }
        "flac" => {
            let mut r = claxon::FlacReader::open(path).map_err(|e| bad(&e))?;
            let info = r.streaminfo();
            let scale = 1.0 / (1u64 << (info.bits_per_sample - 1)) as f32;
            let data: Result<Vec<f32>, _> =
                r.samples().map(|v| v.map(|v| v as f32 * scale)).collect();
            Pcm {
                rate: info.sample_rate,
                channels: info.channels as u16,
                data: data.map_err(|e| bad(&e))?,
            }
        }
        "ogg" => {
            let file = BufReader::new(File::open(path).map_err(|e| bad(&e))?);
            let mut d = vorbis_rs::VorbisDecoder::new(file).map_err(|e| bad(&e))?;
            let (rate, channels) = (d.sampling_frequency().get(), d.channels().get());
            let mut data = Vec::new();
            while let Some(block) = d.decode_audio_block().map_err(|e| bad(&e))? {
                let planes = block.samples();
                let n = planes.first().map_or(0, |p| p.len());
                for i in 0..n {
                    data.extend(planes.iter().map(|p| p[i]));
                }
            }
            Pcm {
                rate,
                channels: channels as u16,
                data,
            }
        }
        "mp3" => {
            let bytes = std::fs::read(path).map_err(|e| bad(&e))?;
            let d = nanomp3::decode_all::<f32>(&bytes);
            let Some(ch) = d.channels else {
                return Err(bad(&"это не MP3 или файл испорчен"));
            };
            Pcm {
                rate: d.sample_rate,
                channels: ch.num() as u16,
                data: d.samples,
            }
        }
        _ => return Err(bad(&"такой звук не читается (WAV, OGG, MP3, FLAC)")),
    };
    if pcm.channels == 0 || pcm.rate == 0 {
        return Err(bad(&"нет каналов или частоты"));
    }
    Ok(pcm)
}

fn write_wav(path: &Path, pcm: &Pcm) -> Result<(), String> {
    let bad = |e: &dyn std::fmt::Display| err("Не записать", path, e);
    let spec = hound::WavSpec {
        channels: pcm.channels,
        sample_rate: pcm.rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| bad(&e))?;
    for &v in &pcm.data {
        let v = (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        w.write_sample(v).map_err(|e| bad(&e))?;
    }
    w.finalize().map_err(|e| bad(&e))
}

fn write_ogg(path: &Path, pcm: &Pcm, quality: f32) -> Result<(), String> {
    let bad = |e: &dyn std::fmt::Display| err("Не записать", path, e);
    let channels = u8::try_from(pcm.channels)
        .ok()
        .and_then(NonZeroU8::new)
        .ok_or_else(|| bad(&"слишком много каналов"))?;
    let rate = NonZeroU32::new(pcm.rate).ok_or_else(|| bad(&"нет частоты"))?;
    let file = BufWriter::new(File::create(path).map_err(|e| bad(&e))?);
    // Номер потока в OGG любой; берём от длины звука, чтобы файл выходил одинаковым.
    let serial = (pcm.data.len() as i32).wrapping_mul(2_654_435_761u32 as i32);
    let mut enc = vorbis_rs::VorbisEncoderBuilder::new_with_serial(rate, channels, file, serial)
        .bitrate_management_strategy(vorbis_rs::VorbisBitrateManagementStrategy::QualityVbr {
            target_quality: quality.clamp(-0.1, 1.0),
        })
        .build()
        .map_err(|e| bad(&e))?;
    let c = pcm.channels as usize;
    const BLOCK: usize = 4096;
    let frames = pcm.frames();
    let mut planes = vec![Vec::with_capacity(BLOCK); c];
    let mut at = 0;
    while at < frames {
        let n = BLOCK.min(frames - at);
        for (ch, plane) in planes.iter_mut().enumerate() {
            plane.clear();
            plane.extend((at..at + n).map(|f| pcm.data[f * c + ch]));
        }
        enc.encode_audio_block(&planes).map_err(|e| bad(&e))?;
        at += n;
    }
    enc.finish().map_err(|e| bad(&e))?;
    Ok(())
}

fn picture(src: &Path, dst: &Path, s: Settings) -> Result<(), String> {
    let mut img = image::open(src).map_err(|e| err("Не прочитать", src, e))?;
    if let Some(side) = s.max_side
        && img.width().max(img.height()) > side
    {
        img = img.resize(side, side, image::imageops::FilterType::Lanczos3);
    }
    let bad = |e: &dyn std::fmt::Display| err("Не записать", dst, e);
    match s.target {
        Target::Jpeg { quality } => {
            let file = BufWriter::new(File::create(dst).map_err(|e| bad(&e))?);
            let mut enc =
                image::codecs::jpeg::JpegEncoder::new_with_quality(file, quality.clamp(1, 100));
            enc.encode_image(&img.to_rgb8()).map_err(|e| bad(&e))
        }
        _ => img
            .save_with_format(dst, image::ImageFormat::Png)
            .map_err(|e| bad(&e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("reyvec-convert-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    /// Секунда стерео: слева 440 Гц, справа тишина.
    fn tone() -> Pcm {
        let rate = 22_050;
        let data = (0..rate)
            .flat_map(|i| {
                let t = i as f32 / rate as f32;
                [0.5 * (std::f32::consts::TAU * 440.0 * t).sin(), 0.0]
            })
            .collect();
        Pcm {
            rate,
            channels: 2,
            data,
        }
    }

    #[test]
    fn sound_round_trips() {
        let d = tmp();
        let wav = d.join("тон.wav");
        write_wav(&wav, &tone()).unwrap();
        assert_eq!(about(&wav).as_deref(), Some("0:01 · 22 050 Гц · стерео"));
        let back = read_sound(&wav).unwrap();
        assert_eq!(
            (back.rate, back.channels, back.frames()),
            (22_050, 2, 22_050)
        );
        assert!((back.data[200] - tone().data[200]).abs() < 1e-3);

        // В OGG и обратно: длина та же (± блок кодера), звук похож, файл меньше.
        let ogg = out_path(&wav, Target::Ogg { quality: 0.4 });
        assert_eq!(ogg.file_name().unwrap(), "тон.ogg");
        let s = Settings {
            target: Target::Ogg { quality: 0.4 },
            mono: false,
            max_side: None,
        };
        convert(&wav, &ogg, s).unwrap();
        assert!(std::fs::metadata(&ogg).unwrap().len() < std::fs::metadata(&wav).unwrap().len());
        let o = read_sound(&ogg).unwrap();
        assert_eq!((o.rate, o.channels), (22_050, 2));
        assert!(o.frames().abs_diff(22_050) < 2048, "{}", o.frames());
        let power = |p: &Pcm, ch: usize| {
            p.data
                .iter()
                .skip(ch)
                .step_by(2)
                .map(|v| v * v)
                .sum::<f32>()
                / p.frames() as f32
        };
        assert!((power(&o, 0) - 0.125).abs() < 0.02, "{}", power(&o, 0));
        assert!(power(&o, 1) < 1e-3);
        assert_eq!(about(&ogg).as_deref(), Some("22 050 Гц · стерео"));

        // Обратно в WAV и в моно: громкость левого канала делится пополам.
        let mono = d.join("тон-моно.wav");
        let s = Settings {
            target: Target::Wav,
            mono: true,
            max_side: None,
        };
        convert(&ogg, &mono, s).unwrap();
        let m = read_sound(&mono).unwrap();
        assert_eq!(m.channels, 1);
        assert!(
            (m.data.iter().map(|v| v * v).sum::<f32>() / m.frames() as f32 - 0.031).abs() < 0.01
        );

        // Испорченный файл — понятная ошибка, недописанного результата нет.
        let broken = d.join("сломан.mp3");
        std::fs::write(&broken, b"not an mp3 at all").unwrap();
        let out = d.join("сломан.ogg");
        let e = convert(&broken, &out, s).unwrap_err();
        assert!(e.contains("сломан.mp3"), "{e}");
        assert!(!out.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// MP3 и FLAC (полсекунды 440 Гц, сделаны заранее, лежат в `testdata`).
    #[test]
    fn mp3_and_flac_read() {
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
        let mp3 = read_sound(&data.join("ton.mp3")).unwrap();
        assert_eq!((mp3.rate, mp3.channels), (22_050, 1));
        assert!(mp3.frames().abs_diff(11_025) < 1200, "{}", mp3.frames());
        assert_eq!(
            about(&data.join("ton.mp3")).as_deref(),
            Some("0:00 · 22 050 Гц · моно")
        );
        let flac = read_sound(&data.join("ton.flac")).unwrap();
        assert_eq!(
            (flac.rate, flac.channels, flac.frames()),
            (22_050, 2, 11_025)
        );
        assert_eq!(
            about(&data.join("ton.flac")).as_deref(),
            Some("0:00 · 22 050 Гц · стерео")
        );
        // Синус амплитуды 1/8 (так делает генератор ffmpeg): мощность ≈ 1/128.
        let power = |p: &Pcm| p.data.iter().map(|v| v * v).sum::<f32>() / p.data.len() as f32;
        assert!((power(&mp3) - 1.0 / 128.0).abs() < 0.002, "{}", power(&mp3));
        // FLAC — то же, разложенное ffmpeg на два канала с понижением на 3 дБ: вдвое тише.
        assert!(
            (power(&flac) - 1.0 / 256.0).abs() < 0.001,
            "{}",
            power(&flac)
        );
    }

    #[test]
    fn pictures_convert_and_shrink() {
        let d = tmp().join("pic");
        std::fs::create_dir_all(&d).unwrap();
        let png = d.join("узор.png");
        let img = image::RgbaImage::from_fn(400, 200, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255])
        });
        img.save(&png).unwrap();
        assert_eq!(about(&png).as_deref(), Some("400 × 200"));
        let jpg = out_path(&png, Target::Jpeg { quality: 85 });
        let s = Settings {
            target: Target::Jpeg { quality: 85 },
            mono: false,
            max_side: Some(100),
        };
        convert(&png, &jpg, s).unwrap();
        assert_eq!(about(&jpg).as_deref(), Some("100 × 50"));
        // Имя занято — рядом с номером.
        assert_eq!(
            out_path(&png, Target::Png).file_name().unwrap(),
            "узор 2.png"
        );
        assert_eq!(media_of(&jpg), Some(Media::Picture));
        assert_eq!(media_of(Path::new("a.FLAC")), Some(Media::Sound));
        assert_eq!(media_of(Path::new("a.ron")), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
