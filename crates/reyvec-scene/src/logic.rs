//! Логика сцены в своём потоке (этап 5.3, решение 0010).
//!
//! Поток логики шагает с постоянным шагом [`STEP`] (60 раз в секунду): двигает предметы,
//! считает итоговые места и отдаёт снимок. Рисование не ждёт логику и логика не ждёт
//! рисование: у рисования всегда есть два последних снимка, и между ними оно плавно
//! дорисовывает движение ([`Scene::lerp`]). На мониторе 144 Гц кадров больше, чем шагов
//! логики, но движение всё равно плавное. Цена — задержка в один шаг (16,7 мс).
//!
//! Общие данные между потоками — только снимки под коротким замком (кладём и берём
//! готовое) и очередь команд; сам мир живёт только в потоке логики.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use reyvec_game::{Action, InputReader, InputShared};
use reyvec_vector::{Affine, SceneItem};

use crate::{Scene, SceneWorld};

/// Шаг логики: 60 раз в секунду (решение 0004).
pub const STEP: Duration = Duration::from_nanos(1_000_000_000 / 60);
/// Если логика отстала больше чем на столько шагов (например, окно тянули мышкой и
/// система не давала потоку работать), догонять не пытаемся: время сцены просто идёт
/// дальше с текущего момента.
const MAX_CATCH_UP: u32 = 5;

enum Command {
    /// Новый мир (файл сцены сохранили): время сцены продолжается.
    Replace(Box<SceneWorld>),
}

struct Frames {
    prev: Scene,
    cur: Scene,
    /// Когда положен `cur`.
    cur_at: Instant,
}

struct Shared {
    frames: Mutex<Frames>,
    stop: AtomicBool,
    steps: AtomicU64,
    /// Время одного шага логики (среднее, микросекунды).
    step_us: AtomicU64,
    /// Ввод игрока: пишет окно, читает поток логики в начале шага (6.1).
    input: InputShared,
    /// Среднее время от нажатия до шага логики, который его взял (микросекунды; 0 —
    /// нажатий ещё не было).
    input_us: AtomicU64,
}

/// Поток логики сцены.
pub struct SceneLogic {
    commands: Sender<Command>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl SceneLogic {
    /// Запускает поток логики с миром `world`; время сцены начинается с `start` секунд.
    pub fn start(mut world: SceneWorld, start: f64) -> Self {
        world.continue_from(start);
        world.update_at(start);
        let first = world.snapshot();
        let shared = Arc::new(Shared {
            frames: Mutex::new(Frames {
                prev: first.clone(),
                cur: first,
                cur_at: Instant::now(),
            }),
            stop: AtomicBool::new(false),
            steps: AtomicU64::new(0),
            step_us: AtomicU64::new(0),
            input: InputShared::new(),
            input_us: AtomicU64::new(0),
        });
        let (tx, rx) = channel();
        let inner = shared.clone();
        let thread = std::thread::Builder::new()
            .name("логика сцены".into())
            .spawn(move || run(world, start, rx, &inner))
            .expect("поток логики сцены");
        Self {
            commands: tx,
            shared,
            thread: Some(thread),
        }
    }

    /// Заменяет мир (после сохранения файла сцены). Время сцены не сбрасывается.
    pub fn replace(&self, world: SceneWorld) {
        let _ = self.commands.send(Command::Replace(Box::new(world)));
    }

    /// Снимок для кадра в момент `now`: между двумя последними шагами логики.
    pub fn frame(&self, now: Instant) -> Scene {
        let frames = self.shared.frames.lock().expect("снимки логики");
        let since = now.saturating_duration_since(frames.cur_at).as_secs_f64();
        let alpha = (since / STEP.as_secs_f64()).clamp(0.0, 1.0);
        Scene::lerp(&frames.prev, &frames.cur, alpha as f32)
    }

    /// Действие игрока нажато или отпущено. Поток логики возьмёт его в ближайший шаг.
    pub fn set_action(&self, action: Action, down: bool) {
        self.shared.input.set(action, down);
    }

    /// Всё отпущено (окно потеряло фокус).
    pub fn release_all(&self) {
        self.shared.input.release_all();
    }

    /// Сколько в среднем нажатие ждёт шага логики, мс (`None`, пока нажатий не было).
    pub fn input_ms(&self) -> Option<f64> {
        match self.shared.input_us.load(Ordering::Relaxed) {
            0 => None,
            us => Some(us as f64 / 1000.0),
        }
    }

    /// Сколько прошло с нажатия с отметкой `stamp` ([`Scene::input_stamp`]), мс.
    pub fn press_age_ms(&self, stamp: u64) -> f64 {
        self.shared.input.age_ms(stamp)
    }

    /// Сколько шагов логики сделано.
    pub fn steps(&self) -> u64 {
        self.shared.steps.load(Ordering::Relaxed)
    }

    /// Среднее время одного шага логики, мс.
    pub fn step_ms(&self) -> f64 {
        self.shared.step_us.load(Ordering::Relaxed) as f64 / 1000.0
    }
}

impl Drop for SceneLogic {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

fn run(mut world: SceneWorld, start: f64, commands: Receiver<Command>, shared: &Shared) {
    let step = STEP.as_secs_f64();
    let mut step_index: u64 = 0;
    let mut next = Instant::now() + STEP;
    let mut average_us = 0.0f64;
    let mut input = InputReader::default();
    while !shared.stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now < next {
            std::thread::park_timeout(next - now);
            continue;
        }
        // Отстали сильно — не догоняем, а продолжаем с текущего момента.
        let behind = ((now - next).as_secs_f64() / step) as u32;
        if behind > MAX_CATCH_UP {
            next = now;
        }
        while let Ok(Command::Replace(new)) = commands.try_recv() {
            // Файл сцены сохранили: персонаж остаётся там, где был.
            let body = world.player_body();
            world = *new;
            world.continue_from(start + step_index as f64 * step);
            if let Some(body) = body {
                world.set_player_body(body);
            }
        }
        let started = Instant::now();
        world.set_input(input.read(&shared.input));
        if let Some(ms) = input.latency_ms() {
            shared
                .input_us
                .store(((ms * 1000.0) as u64).max(1), Ordering::Relaxed);
        }
        step_index += 1;
        world.update_at(start + step_index as f64 * step);
        let snapshot = world.snapshot();
        let took = started.elapsed().as_secs_f64() * 1e6;
        average_us = if step_index == 1 {
            took
        } else {
            average_us * 0.95 + took * 0.05
        };
        {
            let mut frames = shared.frames.lock().expect("снимки логики");
            frames.prev = std::mem::replace(&mut frames.cur, snapshot);
            frames.cur_at = Instant::now();
        }
        shared.steps.store(step_index, Ordering::Relaxed);
        shared.step_us.store(average_us as u64, Ordering::Relaxed);
        next += STEP;
    }
}

fn lerp2(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn lerp_affine(a: Affine, b: Affine, t: f64) -> Affine {
    let (a, b) = (a.as_coeffs(), b.as_coeffs());
    Affine::new(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t))
}

impl Scene {
    /// Сцена между снимками `prev` и `cur` (`t` от 0 до 1). Сглаживаются места фигур,
    /// источников и SDF-объектов. Если снимки устроены по-разному (между ними сменился
    /// файл сцены), берётся `cur`.
    pub fn lerp(prev: &Scene, cur: &Scene, t: f32) -> Scene {
        let mut out = cur.clone();
        if prev.items.len() == cur.items.len() {
            for (o, p) in out.items.iter_mut().zip(&prev.items) {
                if let (SceneItem::Shape(o), SceneItem::Shape(p)) = (o, p)
                    && o.shape == p.shape
                {
                    o.transform = lerp_affine(p.transform, o.transform, f64::from(t));
                }
            }
        }
        if prev.lights.len() == cur.lights.len() {
            for (o, p) in out.lights.iter_mut().zip(&prev.lights) {
                if o.kind == p.kind {
                    o.a = lerp2(p.a, o.a, t);
                    o.b = lerp2(p.b, o.b, t);
                }
            }
        }
        if prev.objects.len() == cur.objects.len() {
            for (o, p) in out.objects.iter_mut().zip(&prev.objects) {
                if o.parts.len() == p.parts.len() {
                    for (op, pp) in o.parts.iter_mut().zip(&p.parts) {
                        op.a = lerp2(pp.a, op.a, t);
                        op.b = lerp2(pp.b, op.b, t);
                    }
                }
            }
        }
        if let (Some(o), Some(p)) = (out.player.as_mut(), prev.player) {
            o.at = lerp2(p.at, o.at, t);
        }
        out
    }
}
