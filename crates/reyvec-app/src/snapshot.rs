//! Снимки кадра: запись и чтение PNG, сравнение кадра с эталонной картинкой.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

pub use reyvec_gpu::FrameImage;

/// Ошибки чтения и записи картинок.
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    /// Файл не открылся или не записался.
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Файл не PNG или PNG не того вида.
    #[error("{path}: {message}")]
    Png { path: PathBuf, message: String },
}

/// Сохраняет кадр в PNG (RGBA, 8 бит, sRGB). Папки по пути создаются сами.
pub fn save_png(path: &Path, image: &FrameImage) -> Result<(), ImageError> {
    let io = |source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    };
    let png_err = |e: png::EncodingError| ImageError::Png {
        path: path.to_path_buf(),
        message: e.to_string(),
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let file = File::create(path).map_err(io)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut writer = encoder.write_header().map_err(png_err)?;
    writer.write_image_data(&image.rgba).map_err(png_err)?;
    writer.finish().map_err(png_err)
}

/// Читает PNG с RGBA по 8 бит (так их пишет [`save_png`]).
pub fn load_png(path: &Path) -> Result<FrameImage, ImageError> {
    let png_err = |message: String| ImageError::Png {
        path: path.to_path_buf(),
        message,
    };
    let file = File::open(path).map_err(|source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|e| png_err(e.to_string()))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| png_err("картинка слишком большая".into()))?;
    let mut buf = vec![0; size];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| png_err(e.to_string()))?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(png_err(format!(
            "нужен RGBA по 8 бит, а в файле {:?} {:?}",
            info.color_type, info.bit_depth
        )));
    }
    buf.truncate(info.buffer_size());
    Ok(FrameImage {
        width: info.width,
        height: info.height,
        rgba: buf,
    })
}

/// Насколько кадр отличается от эталона.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageDiff {
    /// Сколько пикселей отличаются больше допуска хотя бы в одном канале.
    pub differing_pixels: usize,
    /// Всего пикселей.
    pub total_pixels: usize,
    /// Наибольшая разница одного канала (0..255).
    pub max_channel_diff: u8,
}

impl ImageDiff {
    /// Доля отличающихся пикселей, от 0 до 1.
    pub fn differing_fraction(&self) -> f64 {
        if self.total_pixels == 0 {
            return 0.0;
        }
        self.differing_pixels as f64 / self.total_pixels as f64
    }
}

/// Сравнивает две картинки одного размера. Пиксель считается отличающимся, если хоть один
/// канал разнится больше чем на `tolerance`. `None`, если размеры разные.
pub fn compare(actual: &FrameImage, expected: &FrameImage, tolerance: u8) -> Option<ImageDiff> {
    if actual.width != expected.width || actual.height != expected.height {
        return None;
    }
    let mut differing = 0;
    let mut max = 0u8;
    for (a, e) in actual
        .rgba
        .chunks_exact(4)
        .zip(expected.rgba.chunks_exact(4))
    {
        let d = a
            .iter()
            .zip(e)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        max = max.max(d);
        if d > tolerance {
            differing += 1;
        }
    }
    Some(ImageDiff {
        differing_pixels: differing,
        total_pixels: (actual.width * actual.height) as usize,
        max_channel_diff: max,
    })
}

/// Картинка отличий: совпавшие пиксели тусклые серые, отличающиеся ярко-красные.
pub fn diff_image(actual: &FrameImage, expected: &FrameImage, tolerance: u8) -> FrameImage {
    let rgba = actual
        .rgba
        .chunks_exact(4)
        .zip(expected.rgba.chunks_exact(4))
        .flat_map(|(a, e)| {
            let d = a
                .iter()
                .zip(e)
                .map(|(x, y)| x.abs_diff(*y))
                .max()
                .unwrap_or(0);
            if d > tolerance {
                [255, 0, 0, 255]
            } else {
                let grey = (u16::from(a[0]) + u16::from(a[1]) + u16::from(a[2])) / 12;
                [grey as u8, grey as u8, grey as u8, 255]
            }
        })
        .collect();
    FrameImage {
        width: actual.width,
        height: actual.height,
        rgba,
    }
}

/// Папка снимков рядом с программой (`dist\snapshots`), а если её путь неизвестен, текущая.
pub fn snapshot_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default()
        .join("snapshots")
}

/// Первый свободный файл вида `кадр-001.png` в папке `dir`.
pub fn next_snapshot_path(dir: &Path) -> PathBuf {
    (1..)
        .map(|n| dir.join(format!("кадр-{n:03}.png")))
        .find(|p| !p.exists())
        .expect("номера не кончаются")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(pixels: &[[u8; 4]], width: u32) -> FrameImage {
        FrameImage {
            width,
            height: pixels.len() as u32 / width,
            rgba: pixels.iter().flatten().copied().collect(),
        }
    }

    #[test]
    fn png_round_trip() {
        let dir = std::env::temp_dir().join(format!("reyvec-png-{}", std::process::id()));
        let path = dir.join("вложенная папка").join("кадр.png");
        let img = image(
            &[
                [255, 0, 0, 255],
                [0, 255, 0, 128],
                [0, 0, 255, 0],
                [9, 9, 9, 9],
            ],
            2,
        );
        save_png(&path, &img).unwrap();
        assert_eq!(load_png(&path).unwrap(), img);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compare_counts_pixels_over_tolerance() {
        let a = image(&[[10, 10, 10, 255], [100, 100, 100, 255]], 2);
        let b = image(&[[12, 10, 10, 255], [100, 140, 100, 255]], 2);
        let d = compare(&a, &b, 2).unwrap();
        assert_eq!(d.differing_pixels, 1);
        assert_eq!(d.max_channel_diff, 40);
        assert!((d.differing_fraction() - 0.5).abs() < 1e-9);
        assert!(compare(&a, &image(&[[0; 4]], 1), 2).is_none());
        let diff = diff_image(&a, &b, 2);
        assert_eq!(diff.pixel(1, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn snapshot_names_skip_existing_files() {
        let dir = std::env::temp_dir().join(format!("reyvec-snap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(next_snapshot_path(&dir), dir.join("кадр-001.png"));
        std::fs::write(dir.join("кадр-001.png"), b"x").unwrap();
        assert_eq!(next_snapshot_path(&dir), dir.join("кадр-002.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
