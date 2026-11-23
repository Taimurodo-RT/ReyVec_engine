//! Окно «Конвертировать…» в проводнике (Э.6): звук в OGG или WAV, картинки в PNG или
//! JPEG. Работа идёт в своём потоке, готовые файлы появляются рядом с исходными и
//! выбираются; Ctrl+Z уносит их в корзину. Сами конвертеры — в
//! [`crate::editor::convert`].

use std::sync::mpsc;

use super::*;
use crate::editor::convert::{self, Media, Settings, Target};

/// Окно настроек конвертации.
#[derive(Clone, Debug)]
pub(crate) struct Converting {
    files: Vec<PathBuf>,
    media: Media,
    /// OGG (звук) или PNG (картинка) выбран.
    first: bool,
    /// Качество OGG 0…1.
    ogg_quality: f32,
    jpeg_quality: u8,
    mono: bool,
    shrink: bool,
    max_side: u32,
}

/// Идущая конвертация: сколько всего, что готово, откуда ждать.
pub(crate) struct ConvertJob {
    total: usize,
    made: Vec<PathBuf>,
    errors: Vec<String>,
    rx: mpsc::Receiver<Result<PathBuf, String>>,
}

impl Converting {
    fn settings(&self) -> Settings {
        let target = match (self.media, self.first) {
            (Media::Sound, true) => Target::Ogg {
                quality: self.ogg_quality,
            },
            (Media::Sound, false) => Target::Wav,
            (Media::Picture, true) => Target::Png,
            (Media::Picture, false) => Target::Jpeg {
                quality: self.jpeg_quality,
            },
        };
        Settings {
            target,
            mono: self.mono,
            max_side: self.shrink.then_some(self.max_side),
        }
    }
}

impl Explorer {
    /// Выбранные файлы одного вида (звук или картинки), которые можно конвертировать.
    pub(super) fn convertible(&self) -> Option<(Media, Vec<PathBuf>)> {
        let first = convert::media_of(self.selected.first()?)?;
        self.selected
            .iter()
            .all(|p| p.is_file() && convert::media_of(p) == Some(first))
            .then(|| (first, self.selected.clone()))
    }

    /// Открыть окно для выбранного.
    pub(super) fn ask_convert(&mut self) {
        if self.converting_job.is_some() {
            return self.say("Предыдущая конвертация ещё идёт", true);
        }
        let Some((media, files)) = self.convertible() else {
            return;
        };
        // Музыку по умолчанию — в OGG, короткий звук (до 2 МБ WAV) — тоже в OGG: обратное
        // (WAV) выбирают сами. Картинку — в PNG.
        self.converting = Some(Converting {
            files,
            media,
            first: true,
            ogg_quality: 0.4,
            jpeg_quality: 88,
            mono: false,
            shrink: false,
            max_side: 1024,
        });
    }

    pub(super) fn convert_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut c) = self.converting.take() else {
            return;
        };
        let mut go = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("explorer-convert")).show(ctx, |ui| {
            ui.set_width(420.0);
            let what = if c.files.len() == 1 {
                format!("«{}»", pf::file_name(&c.files[0]))
            } else {
                files_word(c.files.len())
            };
            heading(ui, &format!("Конвертировать {what}"));
            ui.add_space(GAP_L);
            match c.media {
                Media::Sound => {
                    ui.radio_value(&mut c.first, true, "OGG Vorbis")
                        .on_hover_text("Сжатый звук: музыка, речь, длинные звуки");
                    ui.label(
                        RichText::new("Музыка и длинные звуки: файл в 5–10 раз меньше WAV")
                            .small()
                            .color(MUTED),
                    );
                    if c.first {
                        ui.horizontal(|ui| {
                            ui.add_space(GAP_XL);
                            ui.label("Качество");
                            ui.add(egui::Slider::new(&mut c.ogg_quality, 0.1..=1.0).show_value(false));
                            let kbps = ogg_kbps(c.ogg_quality, c.mono);
                            ui.label(
                                RichText::new(format!(
                                    "{} из 10, ≈ {kbps} кбит/с",
                                    (c.ogg_quality * 10.0).round()
                                ))
                                .color(TEXT_DIM),
                            );
                        });
                    }
                    ui.add_space(GAP_M);
                    ui.radio_value(&mut c.first, false, "WAV")
                        .on_hover_text("Несжатый звук, 16 бит");
                    ui.label(
                        RichText::new("Короткие звуки (шаг, удар): без потерь, но файл большой")
                            .small()
                            .color(MUTED),
                    );
                    ui.add_space(GAP_M);
                    ui.checkbox(&mut c.mono, "Свести в моно")
                        .on_hover_text("Один канал: вдвое меньше; для звуков, которые движок сам разносит влево-вправо");
                }
                Media::Picture => {
                    ui.radio_value(&mut c.first, true, "PNG");
                    ui.label(
                        RichText::new("Без потерь, прозрачность сохраняется")
                            .small()
                            .color(MUTED),
                    );
                    ui.add_space(GAP_M);
                    ui.radio_value(&mut c.first, false, "JPEG");
                    ui.label(
                        RichText::new("Фото и фоны: файл меньше, но без прозрачности")
                            .small()
                            .color(MUTED),
                    );
                    if !c.first {
                        ui.horizontal(|ui| {
                            ui.add_space(GAP_XL);
                            ui.label("Качество");
                            ui.add(egui::Slider::new(&mut c.jpeg_quality, 50..=100));
                        });
                    }
                    ui.add_space(GAP_M);
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut c.shrink, "Уменьшить: большая сторона до");
                        ui.add_enabled(
                            c.shrink,
                            egui::DragValue::new(&mut c.max_side)
                                .range(16..=16384)
                                .suffix(" точек"),
                        );
                    });
                }
            }
            ui.add_space(GAP_L);
            ui.label(
                RichText::new("Исходный файл остаётся, новый появится рядом. Ctrl+Z уберёт его.")
                    .small()
                    .color(MUTED),
            );
            ui.add_space(GAP_L);
            ui.horizontal(|ui| {
                let b =
                    egui::Button::new(RichText::new("Конвертировать").color(ON_ACCENT)).fill(ACCENT);
                if ui.add(b).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                    go = true;
                }
                if ui.button("Отмена").clicked() {
                    close = true;
                }
            });
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                close = true;
            }
        });
        if modal.should_close() {
            close = true;
        }
        if go {
            self.start_convert(&c.files, c.settings());
        } else if !close {
            self.converting = Some(c);
        }
    }

    /// Запустить конвертацию в своём потоке.
    pub(super) fn start_convert(&mut self, files: &[PathBuf], s: Settings) {
        let (tx, rx) = mpsc::channel();
        let mut jobs: Vec<(PathBuf, PathBuf)> = Vec::new();
        for src in files {
            let dst = convert::out_path(src, s.target);
            // Занять имя пустым файлом: следующий файл с тем же именем получит номер.
            let _ = std::fs::write(&dst, b"");
            jobs.push((src.clone(), dst));
        }
        let total = jobs.len();
        std::thread::Builder::new()
            .name("конвертер".into())
            .spawn(move || {
                for (src, dst) in jobs {
                    let r = convert::convert(&src, &dst, s).map(|()| dst.clone());
                    if r.is_err() {
                        let _ = std::fs::remove_file(&dst);
                    }
                    if tx.send(r).is_err() {
                        return;
                    }
                }
            })
            .expect("поток конвертера");
        self.converting_job = Some(ConvertJob {
            total,
            made: Vec::new(),
            errors: Vec::new(),
            rx,
        });
        self.say(format!("Конвертирую: {}…", files_word(total)), false);
    }

    /// Забрать готовое из потока конвертера.
    pub(super) fn poll_convert(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.converting_job else {
            return;
        };
        while let Ok(r) = job.rx.try_recv() {
            match r {
                Ok(p) => job.made.push(p),
                Err(e) => job.errors.push(e),
            }
        }
        let got = job.made.len() + job.errors.len();
        if got < job.total {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            let text = format!("Конвертирую: готово {got} из {}…", job.total);
            self.note = Some((text, false));
            return;
        }
        let Some(job) = self.converting_job.take() else {
            return;
        };
        if !job.made.is_empty() {
            self.selected = job.made.clone();
            self.anchor = job.made.last().cloned();
            self.scroll_to = true;
            let n = job.made.len();
            self.did(Done::Created(job.made));
            self.say(
                format!("Сконвертировано: {}. Ctrl+Z — убрать", files_word(n)),
                false,
            );
        }
        if let Some(e) = job.errors.first() {
            let more = job.errors.len() - 1;
            let tail = if more > 0 {
                format!(" (и ещё ошибок: {more})")
            } else {
                String::new()
            };
            self.say(format!("{e}{tail}"), true);
        }
    }

    /// Звук: длительность, частота, каналы; картинка: размер (считается раз на выбор).
    pub(super) fn media_fact(&mut self, ui: &mut Ui, item: &Item) {
        if item.folder || convert::media_of(&item.path).is_none() {
            return;
        }
        if self.about.as_ref().is_none_or(|(p, _)| p != &item.path) {
            self.about = Some((item.path.clone(), convert::about(&item.path)));
        }
        if let Some((_, Some(text))) = &self.about {
            let name = match convert::media_of(&item.path) {
                Some(Media::Sound) => "Звук",
                _ => "Размер в точках",
            };
            fact(ui, name, &text.clone());
        }
    }
}

/// Примерный поток OGG на 44,1 кГц для качества 0…1 (таблица Vorbis), моно — меньше.
fn ogg_kbps(q: f32, mono: bool) -> u32 {
    const STEREO: [(f32, f32); 6] = [
        (0.0, 64.0),
        (0.2, 96.0),
        (0.4, 128.0),
        (0.6, 192.0),
        (0.8, 256.0),
        (1.0, 500.0),
    ];
    let q = q.clamp(0.0, 1.0);
    let mut k = 0;
    while k + 2 < STEREO.len() && q > STEREO[k + 1].0 {
        k += 1;
    }
    let ((q0, b0), (q1, b1)) = (STEREO[k], STEREO[k + 1]);
    let b = b0 + (b1 - b0) * (q - q0) / (q1 - q0);
    let b = if mono { b * 0.6 } else { b };
    ((b / 8.0).round() * 8.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kbps_table() {
        assert_eq!(ogg_kbps(0.4, false), 128);
        assert_eq!(ogg_kbps(0.5, false), 160);
        assert!(ogg_kbps(0.4, true) < 128);
    }
}
