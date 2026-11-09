//! Сетка отсева для сцены «3D» (этап 7.6е): чтобы шаг луча не перебирал все объекты.
//!
//! Пространство вокруг конечных объектов делится на кубические клетки. У каждой клетки
//! свой список объектов, чей шар-граница ближе `margin` (размер клетки) к клетке: от
//! любой точки клетки объекты не из списка дальше `margin`. Шаг луча в точке берёт
//! клетку и считает только её список — так время шага зависит от того, сколько объектов
//! рядом, а не от их общего числа.
//!
//! Объекты не из списка отвечают оценкой, с которой начинается шаг: до 7.18д4 это был
//! `margin`, и в пустоте луч шагал не дальше клетки (у уровня с полем — 30 см, а луч
//! тени к лампе длиной 1–3 м). Теперь у каждой клетки ещё своё свободное расстояние
//! ([`Cell::free_mm`]): от любой её точки до шара-границы любого конечного объекта не
//! ближе. Шаг начинается с большего из двух ([`Grid::start`]).
//!
//! Бесконечные формы (плоскость пола) в сетку не попадают: они считаются всегда.
//! Снаружи сетки ответ — расстояние до коробки, в которой лежат все шары-границы.
//!
//! Сетка строится на процессоре каждый кадр (объекты двигаются): для сотен объектов это
//! десятые доли миллисекунды, свободное расстояние — примерно столько же (каждый объект
//! метит клетки не дальше [`FREE_CAP`] от себя). Шейдер (`include/space.slang`, `scene_near`) и зеркало здесь
//! ([`Grid::near`]) читают одно и то же; тесты сверяют зеркало с полным перебором.

use crate::shape3d::{ESTIMATE_MATERIAL, Program};
use glam::Vec3;

/// К этому числу клеток подбирается размер клетки.
const TARGET_CELLS: f32 = 2048.0;
/// Больше клеток не бывает (буфер кадра не резиновый).
pub const MAX_CELLS: usize = 32768;
/// Клетка не мельче 25 см: мельче — длинные списки соседей и лишняя память.
const MIN_CELL: f32 = 0.25;

/// Слов в заголовке сетки и в записи клетки (раскладку читает шейдер).
pub const GRID_HEADER_WORDS: usize = 20;
pub const CELL_WORDS: usize = 2;
/// Свободное расстояние клетки считается до этого (метры): дальше выигрыш мал (лучи
/// теней к лампам короче нескольких метров), а работа на процессоре растёт как куб.
pub const FREE_CAP: f32 = 2.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cell {
    /// Начало списка в [`Grid::items`] и его длина.
    pub start: u32,
    pub count: u32,
    /// Свободно (7.18д4): от любой точки клетки до шара-границы любого конечного объекта
    /// не ближе стольких миллиметров (не больше [`FREE_CAP`]; 0 — не известно).
    pub free_mm: u16,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grid {
    /// Угол сетки (наименьшие координаты), размер клетки, число клеток по осям.
    pub origin: Vec3,
    pub cell: f32,
    pub dims: [u32; 3],
    /// Объект попадает в список клетки, если его шар ближе этого к клетке.
    pub margin: f32,
    /// Коробка, в которой лежат все шары-границы.
    pub inner_lo: Vec3,
    pub inner_hi: Vec3,
    pub cells: Vec<Cell>,
    pub items: Vec<u32>,
    /// Объекты без границы: считаются в каждой точке. Сначала обычные, в конце —
    /// плоскости, которые луч глаза находит формулой ([`crate::shape3d::OBJECT_PLANE`]).
    pub unbounded: Vec<u32>,
    /// Сколько первых из `unbounded` — не плоскости.
    pub unbounded_solid: usize,
    /// Поле уровня (7.18д3, [`crate::shape3d::level_field_object`]): считается в каждой
    /// точке первым, до `unbounded`; в `unbounded` его нет.
    pub field: Option<u32>,
}

impl Grid {
    /// Сетка для объектов `objects` (номера в списках — их индексы); поле уровня
    /// ([`crate::shape3d::level_field_object`]) — отдельно от списков.
    pub fn build(objects: &[Program]) -> Self {
        let field = crate::shape3d::level_field_object(objects).map(|i| i as u32);
        Self::build_with(objects, field)
    }

    /// То же с полем уровня `field` (номер объекта; `None` — все бесконечные формы в
    /// списке, как до 7.18д3).
    pub fn build_with(objects: &[Program], field: Option<u32>) -> Self {
        let mut grid = Self::build_lists(objects, field);
        grid.fill_free(objects);
        grid
    }

    /// Сетка без свободного расстояния: шаг в пустоте начинается с `margin`, как до
    /// 7.18д4 (для сравнения на панели и в тестах).
    pub fn clear_free(&mut self) {
        for c in &mut self.cells {
            c.free_mm = 0;
        }
    }

    /// Сетка только со списками, без свободного расстояния (как [`Grid::clear_free`], но
    /// без работы на него): шаг в пустоте не длиннее клетки, как до 7.18д4.
    pub fn build_lists(objects: &[Program], field: Option<u32>) -> Self {
        let planes = crate::shape3d::plane_objects(objects);
        let (mut unbounded, flat): (Vec<u32>, Vec<u32>) = (0..objects.len() as u32)
            .filter(|&i| objects[i as usize].bounds.is_none() && Some(i) != field)
            .partition(|i| !planes.contains(i));
        let unbounded_solid = unbounded.len();
        unbounded.extend(flat);
        let mut grid = Grid {
            unbounded,
            unbounded_solid,
            field,
            ..Grid::default()
        };
        let finite: Vec<(u32, Vec3, f32)> = objects
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.bounds.map(|(c, r)| (i as u32, c, r.max(0.0))))
            .collect();
        if finite.is_empty() {
            return grid;
        }
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for &(_, c, r) in &finite {
            lo = lo.min(c - r);
            hi = hi.max(c + r);
        }
        let extent = (hi - lo).max(Vec3::splat(MIN_CELL));
        // Клетка — по объёму сцены, но не крупнее радиуса обычного предмета: в большой и
        // густой сцене крупная клетка собирает длинные списки соседей, и каждый шаг
        // луча перебирает их все (на 200 предметах это и было узким местом).
        let mut sizes: Vec<f32> = finite.iter().map(|f| f.2).collect();
        let mid = sizes.len() / 2;
        let typical = *sizes.select_nth_unstable_by(mid, f32::total_cmp).1;
        let mut cell = ((extent.x * extent.y * extent.z) / TARGET_CELLS)
            .cbrt()
            .min(typical)
            .max(MIN_CELL);
        let dims = loop {
            // Сетка шире коробки на margin = cell с каждой стороны: у её края до любого
            // объекта не меньше клетки, и луч снаружи подходит к ней большими шагами.
            let d = ((extent + 2.0 * cell) / cell).ceil();
            let dims = [d.x as u32, d.y as u32, d.z as u32];
            if dims.iter().map(|&d| d as usize).product::<usize>() <= MAX_CELLS {
                break dims;
            }
            cell *= 1.25;
        };
        let margin = cell;
        grid.origin = lo - margin;
        grid.cell = cell;
        grid.dims = dims;
        grid.margin = margin;
        grid.inner_lo = lo;
        grid.inner_hi = hi;

        // Клетки, которых касается шар объекта, расширенный на margin. Два прохода:
        // сначала счёт (сколько объектов в каждой клетке), потом раскладка по местам —
        // без отдельного списка на клетку.
        let [nx, ny, nz] = dims.map(|d| d as usize);
        let top = Vec3::new(nx as f32 - 1.0, ny as f32 - 1.0, nz as f32 - 1.0);
        let touched = |c: Vec3, r: f32, f: &mut dyn FnMut(usize)| {
            let reach = r + margin;
            let a = ((c - reach - grid.origin) / cell)
                .floor()
                .clamp(Vec3::ZERO, top);
            let b = ((c + reach - grid.origin) / cell)
                .floor()
                .clamp(Vec3::ZERO, top);
            for z in a.z as usize..=b.z as usize {
                for y in a.y as usize..=b.y as usize {
                    for x in a.x as usize..=b.x as usize {
                        let lo = grid.origin + Vec3::new(x as f32, y as f32, z as f32) * cell;
                        if c.distance_squared(c.clamp(lo, lo + cell)) < reach * reach {
                            f((z * ny + y) * nx + x);
                        }
                    }
                }
            }
        };
        let mut counts = vec![0u32; nx * ny * nz];
        for &(_, c, r) in &finite {
            touched(c, r, &mut |k| counts[k] += 1);
        }
        let mut start = 0;
        grid.cells = counts
            .iter()
            .map(|&count| {
                let cell = Cell {
                    start,
                    count,
                    free_mm: 0,
                };
                start += count;
                cell
            })
            .collect();
        grid.items = vec![0; start as usize];
        let mut fill: Vec<u32> = grid.cells.iter().map(|c| c.start).collect();
        for &(i, c, r) in &finite {
            touched(c, r, &mut |k| {
                grid.items[fill[k] as usize] = i;
                fill[k] += 1;
            });
        }
        grid
    }

    /// Свободное расстояние клеток (7.18д4): для клетки — наименьшее по конечным объектам
    /// расстояние от её коробки до шара-границы, но не больше [`FREE_CAP`]. Каждый объект
    /// метит только клетки ближе [`FREE_CAP`] к себе (у остальных до него не меньше
    /// FREE_CAP), квадрат расстояния до коробки — сумма трёх заранее посчитанных по осям.
    /// Округление вниз до миллиметра и ещё −1 мм: шейдер находит клетку точки с ошибкой
    /// округления, запас её покрывает.
    fn fill_free(&mut self, objects: &[Program]) {
        if self.cells.is_empty() {
            return;
        }
        let [nx, ny, nz] = self.dims.map(|d| d as usize);
        let mut free = vec![FREE_CAP; self.cells.len()];
        // Зазор по оси между отрезком клетки и точкой `c` (0 — точка над клеткой).
        let gap = |c: f32, o: f32, k: usize| {
            let lo = o + k as f32 * self.cell;
            let hi = lo + self.cell;
            let g = (lo - c).max(c - hi).max(0.0);
            g * g
        };
        let range = |c: f32, reach: f32, o: f32, n: usize| {
            let a = ((c - reach - o) / self.cell).floor().max(0.0) as usize;
            let b = ((c + reach - o) / self.cell).floor().min(n as f32 - 1.0);
            (a, if b < 0.0 { None } else { Some(b as usize) })
        };
        let (mut gx, mut gy, mut gz) = (Vec::new(), Vec::new(), Vec::new());
        for (c, r) in objects.iter().filter_map(|p| p.bounds) {
            let r = r.max(0.0);
            let reach = r + FREE_CAP;
            let (x0, Some(x1)) = range(c.x, reach, self.origin.x, nx) else {
                continue;
            };
            let (y0, Some(y1)) = range(c.y, reach, self.origin.y, ny) else {
                continue;
            };
            let (z0, Some(z1)) = range(c.z, reach, self.origin.z, nz) else {
                continue;
            };
            if x0 > x1 || y0 > y1 || z0 > z1 {
                continue;
            }
            gx.clear();
            gx.extend((x0..=x1).map(|k| gap(c.x, self.origin.x, k)));
            gy.clear();
            gy.extend((y0..=y1).map(|k| gap(c.y, self.origin.y, k)));
            gz.clear();
            gz.extend((z0..=z1).map(|k| gap(c.z, self.origin.z, k)));
            let far = reach * reach;
            for (z, &dz) in (z0..=z1).zip(&gz) {
                for (y, &dy) in (y0..=y1).zip(&gy) {
                    let dyz = dy + dz;
                    // Вся строка дальше FREE_CAP от шара — её клетки он не меняет.
                    if dyz >= far {
                        continue;
                    }
                    let row = (z * ny + y) * nx;
                    // Без ветвлений: цикл по строке складывается в векторные команды.
                    for (f, &dx) in free[row + x0..=row + x1].iter_mut().zip(&gx) {
                        let d = (dx + dyz).sqrt() - r;
                        // Не f32::min: сравнение складывается в одну команду minps.
                        *f = if d < *f { d } else { *f };
                    }
                }
            }
        }
        for (cell, f) in self.cells.iter_mut().zip(free) {
            // Внутри шара разность отрицательна: свободного места нет.
            cell.free_mm = ((f * 1000.0).floor() - 1.0).clamp(0.0, 65535.0) as u16;
        }
    }

    /// Начальная оценка шага в клетке `c`: объекты не из её списка не ближе этого.
    pub fn start(&self, c: usize) -> f32 {
        self.margin.max(f32::from(self.cells[c].free_mm) * 0.001)
    }

    /// Слов в раскладке для видеокарты ([`Grid::write`]).
    pub fn words(&self) -> usize {
        GRID_HEADER_WORDS + self.cells.len() * CELL_WORDS + self.items.len() + self.unbounded.len()
    }

    /// Раскладка для видеокарты, начиная со слова `at` буфера сцены:
    ///
    /// - заголовок, 20 слов: угол сетки (3 числа), размер клетки, клеток по осям
    ///   (3 целых), margin, коробка шаров (6 чисел), где клетки, где списки, где и
    ///   сколько бесконечных объектов, сколько из них не плоскости, 1 запасное;
    /// - клетки по 2 слова: начало списка (номер слова в буфере) и длина в младших
    ///   16 битах, свободное расстояние в миллиметрах ([`Cell::free_mm`]) в старших;
    ///   клетка (x, y, z) — номер (z · ny + y) · nx + x;
    /// - списки клеток подряд, потом список бесконечных объектов.
    pub fn write(&self, at: usize, out: &mut Vec<u32>) {
        let cells_at = at + GRID_HEADER_WORDS;
        let items_at = cells_at + self.cells.len() * CELL_WORDS;
        let unbounded_at = items_at + self.items.len();
        out.extend(self.origin.to_array().map(f32::to_bits));
        out.push(self.cell.to_bits());
        out.extend(self.dims);
        out.push(self.margin.to_bits());
        out.extend(self.inner_lo.to_array().map(f32::to_bits));
        out.extend(self.inner_hi.to_array().map(f32::to_bits));
        out.extend([
            cells_at as u32,
            items_at as u32,
            unbounded_at as u32,
            self.unbounded.len() as u32,
            self.unbounded_solid as u32,
            0,
        ]);
        for c in &self.cells {
            debug_assert!(c.count < 1 << 16);
            out.extend([
                items_at as u32 + c.start,
                c.count | (u32::from(c.free_mm) << 16),
            ]);
        }
        out.extend(&self.items);
        out.extend(&self.unbounded);
    }

    /// Клетка точки `p` или `None`, если точка вне сетки.
    pub fn cell_at(&self, p: Vec3) -> Option<usize> {
        let q = ((p - self.origin) / self.cell).floor();
        let [nx, ny, nz] = self.dims.map(|d| d as f32);
        if q.x < 0.0 || q.y < 0.0 || q.z < 0.0 || q.x >= nx || q.y >= ny || q.z >= nz {
            return None;
        }
        Some(((q.z * ny + q.y) * nx + q.x) as usize)
    }

    /// Расстояние до ближайшего объекта и его материал — зеркало `scene_near` шейдера.
    /// Ответ-оценка (по шару-границе, по размеру клетки или по коробке) помечен материалом
    /// `None`: настоящее расстояние не меньше.
    pub fn near(&self, objects: &[Program], p: Vec3, near: f32) -> (f32, Option<u32>) {
        let pick = self.pick(objects, p, near, None);
        (pick.distance, pick.material)
    }

    /// То же и чей ответ (зеркало `scene_walk<1>` шейдера, 7.18д5): `winner` — номер
    /// объекта (`None` — ответ-оценка), `second` — не больше расстояния до любого другого
    /// объекта. С `only` считается один этот объект (без сетки и оценок).
    pub fn pick(&self, objects: &[Program], p: Vec3, near: f32, only: Option<u32>) -> Pick {
        // Как в шейдере: сначала оценка по клетке (или по коробке снаружи сетки), потом
        // поле уровня, бесконечные объекты и список клетки одним проходом.
        let mut pick = Pick {
            distance: 1e9,
            material: None,
            winner: None,
            second: 1e9,
        };
        let mut list: &[u32] = &[];
        let mut unbounded: &[u32] = &self.unbounded;
        let single = only.map(|o| [o]);
        if let Some(one) = &single {
            unbounded = &[];
            if Some(one[0]) != self.field {
                list = one;
            }
        } else if !self.cells.is_empty() {
            match self.cell_at(p) {
                Some(c) => {
                    let cell = self.cells[c];
                    list = &self.items[cell.start as usize..(cell.start + cell.count) as usize];
                    pick.distance = self.start(c);
                }
                None => {
                    pick.distance = (p - p.clamp(self.inner_lo, self.inner_hi)).length();
                }
            }
        }
        // Поле уровня — до перебора, как `level_field_near` в шейдере.
        if let Some(f) = self.field
            && only.is_none_or(|o| o == f)
        {
            let (d, m) = objects[f as usize].distance(p);
            pick.real(d, m, f);
        }
        for &i in unbounded.iter().chain(list) {
            let o = &objects[i as usize];
            if let Some((c, r)) = o.bounds {
                let outside = (p - c).length() - r;
                if outside >= pick.distance {
                    pick.second = pick.second.min(outside);
                    continue;
                }
                if outside > near {
                    pick.second = pick.second.min(pick.distance);
                    pick.distance = outside;
                    pick.material = None;
                    pick.winner = None;
                    continue;
                }
            }
            let (d, m) = o.distance(p);
            pick.real(d, m, i);
        }
        pick
    }

    /// Четыре расстояния для нормали в точке попадания `p` на расстоянии `t` по лучу —
    /// зеркало `normal_at` шейдера (7.18д5): по всей сцене с `near` NORMAL_NEAR, а если в
    /// первой точке ближайший объект впереди остальных больше чем на NORMAL_GAP — в трёх
    /// других точках только он. Второе — взят ли один объект.
    pub fn normal_samples(&self, objects: &[Program], p: Vec3, t: f32) -> ([f32; 4], bool) {
        let e = 0.001;
        let near = 0.0005 * t.max(1.0) + NORMAL_NEAR;
        let mut only = None;
        let mut out = [0.0; 4];
        for (k, d) in out.iter_mut().enumerate() {
            let k = k as u32;
            let dir = Vec3::new(
                (((k + 3) >> 1) & 1) as f32,
                ((k >> 1) & 1) as f32,
                (k & 1) as f32,
            ) * 2.0
                - 1.0;
            let pick = self.pick(
                objects,
                p + dir * e,
                if only.is_some() { 1e9 } else { near },
                only,
            );
            *d = pick.distance;
            if k == 0
                && let Some(w) = pick.winner
                && pick.second - pick.distance > NORMAL_GAP
            {
                only = Some(w);
            }
        }
        (out, only.is_some())
    }
}

/// Ближе этого к точкам нормали настоящая форма есть всегда (с запасом на точность
/// попадания и наклон расстояний): дальше объекты отвечают шаром-границей (как
/// NORMAL_NEAR в `include/march.slang`).
pub const NORMAL_NEAR: f32 = 0.02;
/// Насколько ближайший объект должен быть впереди остальных, чтобы нормаль считала его
/// одного: 4√2·L·e при L = 4 и e = 1 мм с запасом (как NORMAL_GAP в `include/march.slang`).
pub const NORMAL_GAP: f32 = 0.024;

/// Ответ [`Grid::pick`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    /// Расстояние (или оценка снизу).
    pub distance: f32,
    /// Материал; `None` — ответ-оценка.
    pub material: Option<u32>,
    /// Чей ответ; `None` — оценка.
    pub winner: Option<u32>,
    /// Не больше расстояния до любого объекта, кроме `winner`.
    pub second: f32,
}

impl Pick {
    /// Настоящее расстояние `d` с материалом `m` объекта `i`.
    fn real(&mut self, d: f32, m: u32, i: u32) {
        if d < self.distance {
            self.second = self.second.min(self.distance);
            self.distance = d;
            // У поля слоя за полосой материал не известен: только оценка (7.18б).
            self.material = (m != ESTIMATE_MATERIAL).then_some(m);
            self.winner = Some(i);
        } else {
            self.second = self.second.min(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape3d::{Shape3, compile};

    fn programs(text: &str) -> Vec<Program> {
        let scene = crate::SpaceScene::parse(text).unwrap();
        scene
            .frame(1.3)
            .shapes
            .iter()
            .map(|s| compile(s).unwrap())
            .collect()
    }

    fn exact(objects: &[Program], p: Vec3) -> f32 {
        objects
            .iter()
            .map(|o| o.distance(p).0)
            .fold(f32::INFINITY, f32::min)
    }

    fn check(objects: &[Program]) {
        let grid = Grid::build(objects);
        assert!(grid.cells.len() <= MAX_CELLS);
        let mut seed = 12345u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        let lo = grid.origin - 3.0;
        let size = grid.cell * Vec3::from(grid.dims.map(|d| d as f32)) + 6.0;
        for _ in 0..20000 {
            let p = lo + Vec3::new(rnd(), rnd(), rnd()) * size;
            let truth = exact(objects, p);
            let (d, m) = grid.near(objects, p, 1e9);
            // Оценка никогда не больше правды: шагать по ней безопасно.
            assert!(d <= truth + 1e-4, "{p}: сетка {d}, правда {truth}");
            // Вблизи формы ответ точный и с материалом.
            if truth < grid.margin.min(0.2) {
                assert!((d - truth).abs() < 1e-4, "{p}: сетка {d}, правда {truth}");
                assert!(m.is_some());
            }
        }
    }

    #[test]
    fn grid_never_overestimates_example_scene() {
        check(&programs(crate::space_file::EXAMPLE));
    }

    #[test]
    fn grid_never_overestimates_stress_scene() {
        check(&programs(include_str!("../../../scenes/3d/stress.ron")));
    }

    /// Свободное расстояние клетки (7.18д4) — точное расстояние от её коробки до
    /// ближайшего шара-границы, округлённое вниз до миллиметра (и −1 мм), и не больше
    /// FREE_CAP; в пустых местах шаг начинается заметно дальше клетки.
    fn free_matches_brute(objects: &[Program]) {
        let grid = Grid::build(objects);
        let [nx, ny, _] = grid.dims.map(|d| d as usize);
        let mut longer = 0;
        for (k, c) in grid.cells.iter().enumerate() {
            let (x, y, z) = (k % nx, (k / nx) % ny, k / (nx * ny));
            let lo = grid.origin + Vec3::new(x as f32, y as f32, z as f32) * grid.cell;
            let hi = lo + grid.cell;
            let brute = objects
                .iter()
                .filter_map(|o| o.bounds)
                .map(|(cc, r)| (cc.distance(cc.clamp(lo, hi)) - r.max(0.0)).max(0.0))
                .fold(FREE_CAP, f32::min);
            let free = f32::from(c.free_mm) * 0.001;
            assert!(
                free <= (brute - 0.0009).max(0.0),
                "клетка {k}: {free} > {brute}"
            );
            assert!(
                free >= brute - 0.0021,
                "клетка {k}: {free} намного меньше {brute}"
            );
            assert!(grid.start(k) >= grid.margin);
            longer += usize::from(grid.start(k) > 2.0 * grid.margin);
        }
        assert!(
            longer * 4 > grid.cells.len(),
            "дальше двух клеток шаг начинается в {longer} из {}",
            grid.cells.len()
        );
    }

    #[test]
    fn free_distance_is_exact_and_conservative() {
        free_matches_brute(&programs(crate::space_file::EXAMPLE));
        free_matches_brute(&programs(include_str!("../../../scenes/3d/stress.ron")));
    }

    /// Без свободного расстояния (сравнение на панели) шаг начинается с margin, как до
    /// 7.18д4; с ним — не ближе, а вблизи формы ответ тот же.
    #[test]
    fn free_never_shortens_steps() {
        let objects = programs(include_str!("../../../scenes/3d/stress.ron"));
        let grid = Grid::build(&objects);
        let mut old = grid.clone();
        old.clear_free();
        assert!(old.cells.iter().all(|c| c.free_mm == 0));
        let mut seed = 99u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        let size = grid.cell * Vec3::from(grid.dims.map(|d| d as f32));
        let mut longer = 0;
        for _ in 0..20000 {
            let p = grid.origin + Vec3::new(rnd(), rnd(), rnd()) * size;
            let (d, _) = grid.near(&objects, p, 1e9);
            let (d0, _) = old.near(&objects, p, 1e9);
            assert!(d >= d0, "{p}: {d} < {d0}");
            if exact(&objects, p) < grid.margin.min(0.2) {
                assert_eq!(d.to_bits(), d0.to_bits(), "{p}");
            }
            longer += usize::from(d > d0);
        }
        assert!(longer > 2000, "длиннее в {longer} точках");
    }

    #[test]
    fn grid_lists_only_nearby_objects() {
        let objects = programs(include_str!("../../../scenes/3d/stress.ron"));
        let grid = Grid::build(&objects);
        let longest = grid.cells.iter().map(|c| c.count).max().unwrap();
        assert!(longest <= 16, "в клетке {longest} объектов");
        assert_eq!(grid.unbounded, vec![0], "пол считается всегда");
    }

    /// Поле слоя без прямоугольника фигуры (так будет у поля уровня, 7.18в) — бесконечный
    /// объект без плоскости: в начале `unbounded`, перед плоскостями; сетку оно не
    /// растягивает; за полосой его материал не известен. Поле фигуры с прямоугольником —
    /// обычный объект с шаром-границей.
    #[test]
    fn field_layer_is_unbounded_and_its_estimate_has_no_material() {
        use crate::layer_field::LayerField;
        use crate::profile2d::Profile;
        use crate::shape3d::FieldRef;
        let profile = Profile::new("M -1 -1 L 1 -1 L 1 1 L -1 1 Z").unwrap();
        let shape = |layer: LayerField| {
            compile(
                &Shape3::Field {
                    field: FieldRef(std::sync::Arc::new(layer)),
                    depth: 0.4,
                    round: 0.0,
                }
                .paint(1),
            )
            .unwrap()
        };
        let field = |band: f32| shape(LayerField::from_profile(&profile, band));
        let plain = programs(include_str!("../../../scenes/3d/stress.ron"));
        let mut bounded = plain.clone();
        bounded.push(field(1.0));
        let grid = Grid::build(&bounded);
        assert_eq!(
            grid.unbounded,
            vec![0],
            "поле фигуры — в клетках, как форма"
        );
        assert_eq!(grid.unbounded_solid, 0);
        check(&bounded);
        let mut objects = plain.clone();
        objects.push(shape(
            LayerField::from_profile(&profile, 1.0).without_rect(),
        ));
        let grid = Grid::build(&objects);
        let last = (objects.len() - 1) as u32;
        assert_eq!(grid.unbounded, vec![0], "поле уровня — не в списке");
        assert_eq!(grid.field, Some(last), "поле уровня считается первым");
        assert_eq!(grid.unbounded_solid, 0);
        assert_eq!(
            grid.dims,
            Grid::build(&plain).dims,
            "сетку поле не растягивает"
        );
        check(&objects);
        // Узкая полоса: далеко от квадрата поле знает только «не ближе полосы».
        let narrow = [shape(
            LayerField::from_profile(&profile, 0.05).without_rect(),
        )];
        let g = Grid::build(&narrow);
        let (d, m) = g.near(&narrow, Vec3::new(1.5, 0.0, 0.0), 1e9);
        assert_eq!(m, None, "за полосой материала нет");
        assert!(
            d > 0.05 && d < 0.5 + 1e-3,
            "{d}: от края сетки, не дальше прямоугольника"
        );
        let (d, m) = g.near(&narrow, Vec3::ZERO, 1e9);
        assert_eq!(m, Some(1));
        assert!(d < 0.0, "{d}");
        // У поля фигуры то же внутри шара-границы.
        let narrow = [field(0.05)];
        let g = Grid::build(&narrow);
        let (d, m) = g.near(&narrow, Vec3::new(1.2, 0.0, 0.0), 1e9);
        assert_eq!(m, None, "за полосой материала нет");
        assert!(d > 0.05 - 1e-3 && d < 0.2 + 1e-3, "{d}");
        let (d, m) = g.near(&narrow, Vec3::ZERO, 1e9);
        assert_eq!(m, Some(1));
        assert!(d < 0.0, "{d}");
    }

    #[test]
    fn only_unbounded_objects_need_no_cells() {
        let floor = compile(&Shape3::Plane {
            normal: [0.0, 1.0, 0.0],
            offset: 0.0,
        })
        .unwrap();
        let grid = Grid::build(&[floor]);
        assert!(grid.cells.is_empty());
        assert_eq!(grid.unbounded, vec![0]);
    }

    /// Раскладка слов читается так же, как в шейдере (`scene_near` в
    /// `include/space.slang`), и даёт тот же ответ, что [`Grid::near`] — и с полем уровня
    /// (7.18д3), которое считается до перебора и в списки не входит.
    #[test]
    fn packed_words_match_the_mirror() {
        use crate::layer_field::LayerField;
        use crate::profile2d::Profile;
        use crate::shape3d::FieldRef;
        let plain = programs(include_str!("../../../scenes/3d/stress.ron"));
        let profile = Profile::new("M -3 -1 L 3 -1 L 3 1 L -3 1 Z").unwrap();
        let mut with_field = plain.clone();
        with_field.push(
            compile(
                &Shape3::Field {
                    field: FieldRef(std::sync::Arc::new(
                        LayerField::from_profile(&profile, 1.0).without_rect(),
                    )),
                    depth: 0.3,
                    round: 0.02,
                }
                .at([0.0, 0.5, 0.0])
                .paint(1),
            )
            .unwrap(),
        );
        assert!(Grid::build(&with_field).field.is_some());
        for objects in [plain, with_field] {
            words_match_the_mirror(&objects);
        }
    }

    /// Нормаль одним объектом (7.18д5) даёт до бита те же четыре расстояния, что и полный
    /// перебор сцены без оценок, как было до 7.18д5: в точках попадания лучей по сценам
    /// «3D», «нагрузка», по коробкам вплотную и по полю уровня, на котором стоит коробка.
    /// Встречаются оба случая: и один объект, и вся сцена.
    #[test]
    fn normal_shortcut_matches_full_scene() {
        use crate::layer_field::LayerField;
        use crate::profile2d::Profile;
        use crate::shape3d::FieldRef;
        let boxed = |size: [f32; 3], at: [f32; 3], m: u32| {
            compile(&Shape3::Box { size, round: 0.0 }.at(at).paint(m)).unwrap()
        };
        let floor = compile(&Shape3::Plane {
            normal: [0.0, 1.0, 0.0],
            offset: 0.0,
        })
        .unwrap();
        // Коробки вплотную: на стыке ближайших двое, нормаль считает вся сцена.
        let touching = vec![
            floor.clone(),
            boxed([1.0, 1.0, 1.0], [-0.5, 0.5, 0.0], 1),
            boxed([1.0, 1.0, 1.0], [0.5, 0.5, 0.0], 2),
            boxed([1.0, 0.5, 1.0], [0.0, 1.25, 0.0], 3),
        ];
        // Плита поля уровня и коробка, стоящая на ней.
        let profile = Profile::new("M -3 -2 L 3 -2 L 3 2 L -3 2 Z").unwrap();
        let mut on_field = vec![
            compile(
                &Shape3::Field {
                    field: FieldRef(std::sync::Arc::new(
                        LayerField::from_profile(&profile, 1.0).without_rect(),
                    )),
                    depth: 0.3,
                    round: 0.02,
                }
                .paint(1),
            )
            .unwrap(),
            boxed([0.6, 0.6, 0.6], [0.0, 0.0, 0.45], 2),
        ];
        on_field.push(boxed([0.4, 0.4, 0.4], [1.5, 0.5, 0.35], 3));
        assert!(Grid::build(&on_field).field.is_some());
        let scenes = [
            programs(crate::space_file::EXAMPLE),
            programs(include_str!("../../../scenes/3d/stress.ron")),
            touching,
            on_field,
        ];
        let (mut single, mut whole) = (0, 0);
        for objects in &scenes {
            let grid = Grid::build(objects);
            let mut seed = 777u32;
            let mut rnd = || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as f32 / u32::MAX as f32
            };
            let lo = grid.origin - 1.0;
            let size = grid.cell * Vec3::from(grid.dims.map(|d| d as f32)) + 2.0;
            for _ in 0..4000 {
                let origin = lo + Vec3::new(rnd(), rnd(), rnd()) * size;
                let dir = (Vec3::new(rnd(), rnd(), rnd()) * 2.0 - 1.0).normalize_or_zero();
                if dir == Vec3::ZERO {
                    continue;
                }
                // Шаги по лучу до попадания с той же точностью, что в шейдере.
                let mut t = 0.0;
                let mut hit = None;
                for _ in 0..200 {
                    let at = origin + dir * t;
                    let (d, _) = grid.near(objects, at, 1e9);
                    if d < 0.0005 * t.max(1.0) {
                        hit = Some(at);
                        break;
                    }
                    t += d;
                    if t > 40.0 {
                        break;
                    }
                }
                let Some(p) = hit else { continue };
                let (fast, one) = grid.normal_samples(objects, p, t);
                let full: [f32; 4] = std::array::from_fn(|k| {
                    let k = k as u32;
                    let dir = Vec3::new(
                        (((k + 3) >> 1) & 1) as f32,
                        ((k >> 1) & 1) as f32,
                        (k & 1) as f32,
                    ) * 2.0
                        - 1.0;
                    grid.near(objects, p + dir * 0.001, 1e9).0
                });
                assert_eq!(
                    fast.map(f32::to_bits),
                    full.map(f32::to_bits),
                    "{p} (один объект: {one}): {fast:?} против {full:?}"
                );
                if one {
                    single += 1;
                } else {
                    whole += 1;
                }
            }
        }
        assert!(single > 1000, "одним объектом: {single}");
        assert!(whole > 20, "всей сценой: {whole}");
    }

    fn words_match_the_mirror(objects: &[Program]) {
        let grid = Grid::build(objects);
        let mut w = vec![0u32; 100];
        grid.write(100, &mut w);
        assert_eq!(w.len(), 100 + grid.words());
        let f = |i: usize| f32::from_bits(w[i]);
        let g = 100;
        let from_words = |p: Vec3| -> (f32, Vec<u32>) {
            let origin = Vec3::new(f(g), f(g + 1), f(g + 2));
            let cell = f(g + 3);
            let dims = [w[g + 4], w[g + 5], w[g + 6]].map(|d| d as i64);
            let q = ((p - origin) / cell).floor();
            let q = [q.x as i64, q.y as i64, q.z as i64];
            let unbounded = w[g + 16] as usize;
            let mut list: Vec<u32> = w[unbounded..unbounded + w[g + 17] as usize].to_vec();
            let inside = (0..3).all(|k| q[k] >= 0 && q[k] < dims[k]);
            let best = if inside {
                let c =
                    w[g + 14] as usize + (((q[2] * dims[1] + q[1]) * dims[0] + q[0]) as usize) * 2;
                let (start, count) = (w[c] as usize, (w[c + 1] & 0xffff) as usize);
                list.extend(&w[start..start + count]);
                f(g + 7).max((w[c + 1] >> 16) as f32 * 0.001)
            } else {
                let lo = Vec3::new(f(g + 8), f(g + 9), f(g + 10));
                let hi = Vec3::new(f(g + 11), f(g + 12), f(g + 13));
                (p - p.clamp(lo, hi)).length()
            };
            (best, list)
        };
        for k in 0..2000 {
            let s = k as f32;
            let p = grid.origin - 2.0
                + Vec3::new((s * 0.37).fract(), (s * 0.61).fract(), (s * 0.83).fract())
                    * (grid.cell * Vec3::from(grid.dims.map(|d| d as f32)) + 4.0);
            let (start, list) = from_words(p);
            let mut best = start;
            if let Some(f) = grid.field {
                assert!(!list.contains(&f), "поле уровня не в списках");
                best = best.min(objects[f as usize].distance(p).0);
            }
            for &i in &list {
                best = best.min(objects[i as usize].distance(p).0);
            }
            let mirror = grid.near(objects, p, 1e9).0;
            assert!(
                (best - mirror).abs() < 1e-5,
                "{p}: слова {best}, зеркало {mirror}"
            );
        }
    }
}
