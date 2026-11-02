//! Загрузка в фоне (этап 5.4).
//!
//! Тяжёлая часть любой загрузки — разбор файла и нарезка кривых на треугольники. Ей не
//! нужен ни набор фигур, ни видеокарта, поэтому она идёт в своём потоке «загрузка»:
//! поток кадра отдаёт задание ([`Loader::submit`]) и рисует дальше, а когда задание
//! готово, забирает результат ([`Loader::poll`]) и только кладёт готовые треугольники в
//! набор (`ShapeLibrary::append`, `insert_detail`). Это копирование, а не нарезка: оно в
//! сотни раз быстрее.
//!
//! Задания идут по одному и по порядку. Набор фигур остаётся в потоке кадра, поэтому
//! никаких замков вокруг него нет: в фон уходят копии (текст файла, байты рисунка, копия
//! фигуры для уровня детализации), назад приходят готовые пачки.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use reyvec_motif::{MorphFile, load_morph_file};
use reyvec_svg::{SvgDrawing, SvgOptions, load_svg};
use reyvec_vector::{DetailJob, PreparedDetail, ShapeBatch, Tessellator};

use crate::{PreparedScene, SceneError, SceneFile};

/// Что загрузить.
pub enum LoadJob {
    /// Файл сцены: разбор и нарезка форм, которых нет среди `known`
    /// (`ShapeCache::keys`).
    Scene {
        text: String,
        /// Папка файла сцены: от неё считаются пути к рисункам.
        dir: Option<PathBuf>,
        known: HashSet<String>,
    },
    /// Рисунок SVG.
    Svg {
        name: String,
        data: Arc<[u8]>,
        options: SvgOptions,
    },
    /// Перетекающие фигуры Motif (`*.reyvec.json`).
    Motif { name: String, data: Arc<[u8]> },
    /// Уровни детализации (`ShapeLibrary::detail_job`).
    Details(Vec<DetailJob>),
}

/// Что получилось.
pub enum LoadResult {
    Scene(Result<PreparedScene, SceneError>),
    /// Рисунок и его фигуры. Номера фигур в рисунке — номера в пачке: после
    /// `ShapeLibrary::append` их сдвигают через `SvgDrawing::offset_shapes`.
    Svg {
        name: String,
        result: Result<(SvgDrawing, ShapeBatch), String>,
    },
    /// Как у SVG: номера сдвигаются через `MorphFile::offset_shapes`.
    Motif {
        name: String,
        result: Result<(MorphFile, ShapeBatch), String>,
    },
    /// Готовые уровни (не получившиеся пропущены и записаны в журнал).
    Details(Vec<PreparedDetail>),
}

/// Готовое задание.
pub struct Loaded {
    /// Номер задания (его вернул [`Loader::submit`]).
    pub job: u64,
    pub result: LoadResult,
    /// Сколько задание делалось в фоне.
    pub took: Duration,
}

/// Поток загрузки.
pub struct Loader {
    jobs: Option<Sender<(u64, LoadJob)>>,
    done: Receiver<Loaded>,
    thread: Option<JoinHandle<()>>,
    next: u64,
    pending: usize,
}

impl Loader {
    /// Запускает поток загрузки. Фигуры режутся разбиением `tessellator`
    /// (`ShapeLibrary::tessellator` того набора, куда их потом положат).
    pub fn start(tessellator: Tessellator) -> Self {
        let (jobs, inbox) = channel::<(u64, LoadJob)>();
        let (outbox, done) = channel();
        let thread = std::thread::Builder::new()
            .name("загрузка".into())
            .spawn(move || {
                for (job, what) in inbox {
                    let started = Instant::now();
                    let result = run(what, tessellator);
                    let loaded = Loaded {
                        job,
                        result,
                        took: started.elapsed(),
                    };
                    if outbox.send(loaded).is_err() {
                        break;
                    }
                }
            })
            .expect("поток загрузки");
        Self {
            jobs: Some(jobs),
            done,
            thread: Some(thread),
            next: 0,
            pending: 0,
        }
    }

    /// Отдаёт задание в фон и сразу возвращает его номер.
    pub fn submit(&mut self, job: LoadJob) -> u64 {
        self.next += 1;
        if let Some(jobs) = &self.jobs
            && jobs.send((self.next, job)).is_ok()
        {
            self.pending += 1;
        }
        self.next
    }

    /// Готовое задание, если есть (не ждёт).
    pub fn poll(&mut self) -> Option<Loaded> {
        let loaded = self.done.try_recv().ok()?;
        self.pending -= 1;
        Some(loaded)
    }

    /// Ждёт следующее готовое задание (для тестов и картинок без окна). `None`, если
    /// заданий в работе нет.
    pub fn wait(&mut self) -> Option<Loaded> {
        if self.pending == 0 {
            return None;
        }
        let loaded = self.done.recv().ok()?;
        self.pending -= 1;
        Some(loaded)
    }

    /// Сколько заданий в работе.
    pub fn pending(&self) -> usize {
        self.pending
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        // Без отправителя очередь заданий кончается, и поток выходит после текущего.
        self.jobs = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(job: LoadJob, tessellator: Tessellator) -> LoadResult {
    match job {
        LoadJob::Scene { text, dir, known } => LoadResult::Scene(SceneFile::prepare(
            &text,
            dir.as_deref(),
            tessellator,
            &known,
        )),
        LoadJob::Svg {
            name,
            data,
            options,
        } => {
            let mut batch = ShapeBatch::new(tessellator);
            let result = load_svg(&data, &mut batch, options)
                .map(|d| (d, batch))
                .map_err(|e| e.to_string());
            LoadResult::Svg { name, result }
        }
        LoadJob::Motif { name, data } => {
            let mut batch = ShapeBatch::new(tessellator);
            let result = load_morph_file(&data, &mut batch)
                .map(|f| (f, batch))
                .map_err(|e| e.to_string());
            LoadResult::Motif { name, result }
        }
        LoadJob::Details(jobs) => LoadResult::Details(
            jobs.into_iter()
                .filter_map(|job| {
                    let (id, level) = (job.id(), job.level());
                    job.run()
                        .map_err(|e| {
                            tracing::warn!("уровень детализации {level} фигуры {}: {e}", id.0);
                        })
                        .ok()
                })
                .collect(),
        ),
    }
}
