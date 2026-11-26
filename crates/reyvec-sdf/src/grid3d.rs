//! Сетка отсева для сцены «3D» (этап 7.6е): чтобы шаг луча не перебирал все объекты.
//!
//! Пространство вокруг конечных объектов делится на кубические клетки. У каждой клетки
//! свой список объектов, чья граница ближе `margin` (размер клетки) к клетке: от
//! любой точки клетки объекты не из списка дальше `margin`. Шаг луча в точке берёт
//! клетку и считает только её список — так время шага зависит от того, сколько объектов
//! рядом, а не от их общего числа.
//!
//! Граница объекта с 8.8а — плотная коробка ([`Program::bounds_box`]); до того был
//! шар-граница, и длинная стена (шар радиусом в половину её длины) растягивала сетку и
//! попадала почти в каждую клетку. Шары остались для сравнения ([`GridOptions::boxes`]).
//!
//! Объекты не из списка отвечают оценкой, с которой начинается шаг: до 7.18д4 это был
//! `margin`, и в пустоте луч шагал не дальше клетки (у уровня с полем — 30 см, а луч
//! тени к лампе длиной 1–3 м). Теперь у каждой клетки ещё своё свободное расстояние
//! ([`Cell::free_mm`]): от любой её точки до границы любого конечного объекта не
//! ближе. Шаг начинается с большего из двух ([`Grid::start`]).
//!
//! Бесконечные формы (плоскость пола) в сетку не попадают: они считаются всегда.
//! Снаружи сетки ответ — расстояние до коробки, в которой лежат все границы.
//!
//! Сетка строится на процессоре каждый кадр (объекты двигаются): для сотен объектов это
//! десятые доли миллисекунды, свободное расстояние — примерно столько же (каждый объект
//! метит клетки не дальше [`FREE_CAP`] от себя). Шейдер (`include/space.slang`, `scene_near`) и зеркало здесь
//! ([`Grid::near`]) читают одно и то же; тесты сверяют зеркало с полным перебором.

use crate::shape3d::{ESTIMATE_MATERIAL, Program};
use glam::Vec3;

/// К этому числу клеток подбирается размер клетки.
const TARGET_CELLS: f32 = 2048.0;
/// Больше клеток не бывает (буфер кадра не резиновый). До 8.8а был предел
/// [`SPHERE_MAX_CELLS`]; с коробками сетка плотнее, и в цехе на тысячу предметов клетке
/// не хватало места: она росла до 3 м, а списки — до 60 объектов.
pub const MAX_CELLS: usize = 131072;
/// Предел клеток с шарами (как до 8.8а, для сравнения на панели).
pub const SPHERE_MAX_CELLS: usize = 32768;
/// Мелкая сетка ([`Grid::fine`]) — не больше стольких клеток…
pub const FINE_MAX_CELLS: usize = 98304;
/// …и её клетка мельче клетки крупной не больше чем во столько раз (мельче — шаг луча
/// в пустоте между предметами короче, и шагов становится больше, чем экономит список).
const FINE_MAX_SPLIT: f32 = 4.0;
/// Клетка крупной сетки «тесная», если в её списке не меньше стольких неподвижных
/// предметов: по тесным клеткам выбирается место мелкой сетки.
const FINE_BUSY: u32 = 4;
/// Мелкая сетка строится, если тесных клеток набралось на столько пар (клетка, предмет).
const FINE_MIN_PAIRS: u32 = 256;
/// Клетка не мельче 25 см: мельче — длинные списки соседей и лишняя память.
const MIN_CELL: f32 = 0.25;

/// Как строится сетка (для сравнения «было/стало» на панели и в тестах).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridOptions {
    /// Свободное расстояние клеток (7.18д4; иначе шаг в пустоте не длиннее клетки).
    pub free: bool,
    /// Объекты раскладываются по плотным коробкам (8.8а); иначе по шарам-границам, как
    /// до 8.8а.
    pub boxes: bool,
    /// Шаг луча отсеивает предмет ещё и по коробке вокруг середины его шара (слово 3
    /// записи, [`crate::shape3d::box_word`]); иначе только по шару, как до М.1в.
    pub object_boxes: bool,
}

impl Default for GridOptions {
    fn default() -> Self {
        Self {
            free: true,
            boxes: true,
            object_boxes: true,
        }
    }
}

/// Граница объекта для сетки: коробка `lo..hi`, раздутая на `r`. Шар — коробка-точка с
/// радиусом, плотная коробка — с радиусом 0. Расстояние от клетки до такой границы —
/// расстояние между коробками минус `r`, оно складывается из зазоров по осям.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bound {
    index: u32,
    /// Неподвижный предмет ([`Program::still`]).
    still: bool,
    lo: Vec3,
    hi: Vec3,
    r: f32,
}

/// Что нужно, чтобы готовый вклад неподвижного подошёл к новому кадру: та же сетка, те же
/// настройки и столько же конечных объектов.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CacheKey {
    origin: [u32; 3],
    cell: u32,
    dims: [u32; 3],
    options: GridOptions,
    squared: bool,
}

/// Место мелкой сетки: угол, размер клетки и число клеток по осям.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FineBox {
    origin: Vec3,
    cell: f32,
    dims: [u32; 3],
}

/// Сетка прошлых кадров для [`Grid::build_cached`] (8.8б): вклад объектов, которые не
/// двигаются, — их пары (клетка, объект) и свободное расстояние от них.
#[derive(Clone, Debug, Default)]
pub struct GridCache {
    key: Option<CacheKey>,
    /// Неподвижное: места в списке границ и сами границы.
    base: Vec<u32>,
    base_bounds: Vec<Bound>,
    base_pairs: Vec<(u32, u32)>,
    base_free: Vec<f32>,
    /// Где лежит мелкая сетка ([`Grid::fine`]): выбирается по неподвижному вместе с ним.
    fine_box: Option<FineBox>,
    /// Готовое для мелкой сетки (то же, что здесь, но в её клетках).
    fine: Option<Box<GridCache>>,
    /// Сколько раз неподвижное пересчитывалось (для тестов и счётчиков).
    pub rebuilds: usize,
    /// Сколько объектов посчитано заново в последнем кадре.
    pub moved: usize,
}

impl Bound {
    /// Место и размер без номера объекта.
    fn shape_bits(&self) -> [u32; 7] {
        let [a, b, c] = self.lo.to_array().map(f32::to_bits);
        let [d, e, f] = self.hi.to_array().map(f32::to_bits);
        [a, b, c, d, e, f, self.r.to_bits()]
    }
}

impl GridCache {
    /// Находит неподвижное в новых границах `bounds` по порядку (номера могли сдвинуться
    /// из-за отсева по взгляду) и переписывает номера в его парах. `false` — что-то из
    /// неподвижного сдвинулось или пропало.
    fn relocate(&mut self, bounds: &[Bound]) -> bool {
        let mut found = Vec::with_capacity(self.base.len());
        let mut p = 0;
        for b in &self.base_bounds {
            let bits = b.shape_bits();
            while p < bounds.len() && bounds[p].shape_bits() != bits {
                p += 1;
            }
            if p == bounds.len() {
                return false;
            }
            found.push(p as u32);
            p += 1;
        }
        if found == self.base
            && self
                .base_bounds
                .iter()
                .zip(&found)
                .all(|(b, &p)| b.index == bounds[p as usize].index)
        {
            return true;
        }
        // Пары по порядку объектов: номер каждого объекта меняется на новый, порядок
        // сохраняется (неподвижное найдено в том же порядке). Старые номера в
        // `base_bounds` ещё не переписаны, по ним и узнаётся объект пары.
        let mut k = 0;
        for pair in &mut self.base_pairs {
            while self.base_bounds[k].index != pair.1 {
                k += 1;
            }
            pair.1 = bounds[found[k] as usize].index;
        }
        for (b, &p) in self.base_bounds.iter_mut().zip(&found) {
            *b = bounds[p as usize];
        }
        self.base = found;
        true
    }

    /// Забыть всё: следующий кадр считается целиком.
    pub fn clear(&mut self) {
        *self = Self {
            rebuilds: self.rebuilds,
            ..Self::default()
        };
    }
}

/// Две последовательности пар, каждая по порядку объектов, — в одну по порядку объектов
/// (пары одного объекта идут подряд и остаются подряд).
fn merge_by_object(a: &[(u32, u32)], b: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].1 <= b[j].1 {
            out.push(a[i]);
            i += 1;
        } else {
            out.push(b[j]);
            j += 1;
        }
    }
    out.extend_from_slice(&a[i..]);
    out.extend_from_slice(&b[j..]);
    out
}

fn object_bounds(objects: &[Program], boxes: bool) -> Vec<Bound> {
    objects
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let (c, r) = p.bounds?;
            let index = i as u32;
            let still = p.still;
            Some(match p.bounds_box.filter(|_| boxes) {
                Some((lo, hi)) => Bound {
                    index,
                    still,
                    lo,
                    hi,
                    r: 0.0,
                },
                None => Bound {
                    index,
                    still,
                    lo: c,
                    hi: c,
                    r: r.max(0.0),
                },
            })
        })
        .collect()
}

/// Зазор между отрезками `a0..a1` и `b0..b1` (0 — перекрываются).
fn gap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (b0 - a1).max(a0 - b1).max(0.0)
}

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
    /// Отсеивать предметы и по коробке ([`GridOptions::object_boxes`]).
    pub object_boxes: bool,
    /// Мелкая сетка поверх тесной части крупной (8.10б): точка внутри её коробки берёт
    /// клетку и список из неё, остальные — из крупной. В городе на 800 м с холмами клетка
    /// крупной сетки — 11 м, и у земли в её списках по 10–25 предметов; мелкая (клетка в
    /// 2–4 раза меньше, только там, где тесно) оставляет в списке 2–4. Бесконечные
    /// объекты и поле уровня у неё те же, что у крупной (свои пусты).
    pub fine: Option<Box<Grid>>,
}

impl Grid {
    /// Сетка для объектов `objects` (номера в списках — их индексы); поле уровня
    /// ([`crate::shape3d::level_field_object`]) — отдельно от списков.
    pub fn build(objects: &[Program]) -> Self {
        let field = crate::shape3d::level_field_object(objects).map(|i| i as u32);
        Self::build_with(objects, field, GridOptions::default())
    }

    /// То же с полем уровня `field` (номер объекта; `None` — все бесконечные формы в
    /// списке, как до 7.18д3) и настройками `options`.
    pub fn build_with(objects: &[Program], field: Option<u32>, options: GridOptions) -> Self {
        Self::build_cached(objects, field, options, &mut GridCache::default())
    }

    /// То же, но вклад объектов, не менявшихся с прошлого кадра, берётся из `cache`
    /// (8.8б): неподвижное раскладывается по клеткам один раз. Ответ тот же до бита, что
    /// у [`Grid::build_with`].
    pub fn build_cached(
        objects: &[Program],
        field: Option<u32>,
        options: GridOptions,
        cache: &mut GridCache,
    ) -> Self {
        let bounds = object_bounds(objects, options.boxes);
        let max_cells = if options.boxes {
            MAX_CELLS
        } else {
            SPHERE_MAX_CELLS
        };
        let mut grid = Self::geometry(objects, field, &bounds, max_cells);
        grid.object_boxes = options.object_boxes;
        if grid.dims == [0; 3] {
            cache.clear();
            return grid;
        }
        let rebuilt = grid.fill_cached(&bounds, options, cache);
        if rebuilt {
            cache.fine_box = options
                .boxes
                .then(|| grid.dense_box(&cache.base_pairs, &cache.base_bounds))
                .flatten();
            cache.fine = None;
        }
        if let Some(place) = cache.fine_box {
            let mut fine = Grid {
                origin: place.origin,
                cell: place.cell,
                dims: place.dims,
                margin: place.cell,
                inner_lo: grid.inner_lo,
                inner_hi: grid.inner_hi,
                object_boxes: grid.object_boxes,
                ..Grid::default()
            };
            fine.fill_cached(&bounds, options, cache.fine.get_or_insert_default());
            grid.fine = Some(Box::new(fine));
        }
        grid
    }

    /// Списки и свободное расстояние клеток готовой по размерам сетки; вклад неподвижного
    /// берётся из `cache` или считается и запоминается (тогда ответ `true`).
    fn fill_cached(
        &mut self,
        bounds: &[Bound],
        options: GridOptions,
        cache: &mut GridCache,
    ) -> bool {
        let grid = self;
        // У коробок (радиус 0) свободное расстояние хранится квадратом: без корня в каждой
        // клетке, корень — в конце, один на клетку.
        let squared = bounds.iter().all(|b| b.r == 0.0);
        let key = CacheKey {
            origin: grid.origin.to_array().map(f32::to_bits),
            cell: grid.cell.to_bits(),
            dims: grid.dims,
            options,
            squared,
        };
        // Неподвижное знает сцена ([`Program::still`]): движущееся считается каждый кадр.
        let unchanged: Vec<bool> = bounds.iter().map(|b| b.still).collect();
        let still = unchanged.iter().filter(|&&u| u).count();
        // Готовое подходит, если неподвижное то же (номера могли сдвинуться: отсев по
        // взгляду убрал или вернул объект перед ним).
        let reuse = cache.key == Some(key) && still == cache.base.len() && cache.relocate(bounds);
        if !reuse {
            // Неподвижное — то, что не изменилось с прошлого кадра; его вклад считается
            // сейчас и дальше берётся готовым, пока оно не сдвинется.
            cache.base = (0..bounds.len() as u32)
                .filter(|&p| unchanged[p as usize])
                .collect();
            cache.base_bounds = cache.base.iter().map(|&p| bounds[p as usize]).collect();
            cache.base_pairs.clear();
            grid.touch(&cache.base_bounds, &mut cache.base_pairs);
            cache.base_free.clear();
            if options.free {
                cache
                    .base_free
                    .resize(grid.cells_len(), grid.free_cap(squared));
                grid.add_free(&cache.base_bounds, &mut cache.base_free, squared);
            }
            cache.rebuilds += 1;
        }
        let mut in_base = vec![false; bounds.len()];
        for &p in &cache.base {
            in_base[p as usize] = true;
        }
        let rest: Vec<Bound> = bounds
            .iter()
            .zip(&in_base)
            .filter(|(_, b)| !**b)
            .map(|(b, _)| *b)
            .collect();
        let mut pairs = Vec::with_capacity(rest.len() * 32);
        grid.touch(&rest, &mut pairs);
        grid.fill_lists(&merge_by_object(&cache.base_pairs, &pairs));
        if options.free {
            let mut free = cache.base_free.clone();
            grid.add_free(&rest, &mut free, squared);
            grid.finish_free(&free, squared);
        }
        cache.moved = rest.len();
        cache.key = Some(key);
        !reuse
    }

    /// Где строить мелкую сетку: коробка вокруг почти всех неподвижных предметов из
    /// «тесных» клеток этой (крупной) — клеток, у которых в списке `pairs` не меньше
    /// [`FINE_BUSY`] предметов. По каждой оси отбрасывается по сотой доле предметов с
    /// краёв: одинокий тесный угол вдали не растягивает коробку. Клетка — чтобы клеток
    /// вышло около [`FINE_MAX_CELLS`], но не мельче крупной в [`FINE_MAX_SPLIT`] раз.
    /// `None` — тесных клеток мало или мелкая сетка не вышла бы заметно мельче.
    fn dense_box(&self, pairs: &[(u32, u32)], bounds: &[Bound]) -> Option<FineBox> {
        let mut counts = vec![0u32; self.cells_len()];
        for &(k, _) in pairs {
            counts[k as usize] += 1;
        }
        // Предметы тесных клеток (пары идут по порядку предметов, границы — тоже).
        let mut busy: Vec<&Bound> = Vec::new();
        let mut total = 0u32;
        let mut b = 0;
        for &(k, i) in pairs {
            if counts[k as usize] < FINE_BUSY {
                continue;
            }
            total += 1;
            while bounds[b].index != i {
                b += 1;
            }
            if busy.last().is_none_or(|last| last.index != i) {
                busy.push(&bounds[b]);
            }
        }
        if total < FINE_MIN_PAIRS || busy.len() < 16 {
            return None;
        }
        let cut = busy.len() / 100;
        let mut lo = Vec3::ZERO;
        let mut hi = Vec3::ZERO;
        let mut edge: Vec<f32> = Vec::with_capacity(busy.len());
        for axis in 0..3 {
            edge.clear();
            edge.extend(busy.iter().map(|b| b.lo[axis] - b.r));
            lo[axis] = *edge.select_nth_unstable_by(cut, f32::total_cmp).1;
            edge.clear();
            edge.extend(busy.iter().map(|b| b.hi[axis] + b.r));
            let last = busy.len() - 1 - cut;
            hi[axis] = *edge.select_nth_unstable_by(last, f32::total_cmp).1;
        }
        let size = (hi - lo).max(Vec3::splat(MIN_CELL));
        let cell = ((size.x * size.y * size.z) / FINE_MAX_CELLS as f32)
            .cbrt()
            .max(self.cell / FINE_MAX_SPLIT)
            .max(MIN_CELL);
        if cell * 1.5 > self.cell {
            return None;
        }
        let dims = (size / cell).ceil();
        Some(FineBox {
            origin: lo,
            cell,
            dims: [dims.x as u32, dims.y as u32, dims.z as u32],
        })
    }

    /// Сетка без свободного расстояния: шаг в пустоте начинается с `margin`, как до
    /// 7.18д4 (для сравнения на панели и в тестах).
    pub fn clear_free(&mut self) {
        for c in &mut self.cells {
            c.free_mm = 0;
        }
        if let Some(fine) = &mut self.fine {
            fine.clear_free();
        }
    }

    /// Размеры сетки по границам `bounds` (не больше `max_cells` клеток) и бесконечные
    /// объекты; клетки пустые. Без конечных объектов клеток нет (`dims` нули).
    fn geometry(
        objects: &[Program],
        field: Option<u32>,
        bounds: &[Bound],
        max_cells: usize,
    ) -> Self {
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
        if bounds.is_empty() {
            return grid;
        }
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for b in bounds {
            lo = lo.min(b.lo - b.r);
            hi = hi.max(b.hi + b.r);
        }
        let extent = (hi - lo).max(Vec3::splat(MIN_CELL));
        // Клетка — по объёму сцены, но не крупнее радиуса обычного предмета: в большой и
        // густой сцене крупная клетка собирает длинные списки соседей, и каждый шаг
        // луча перебирает их все (на 200 предметах это и было узким местом).
        let mut sizes: Vec<f32> = objects
            .iter()
            .filter_map(|p| p.bounds.map(|b| b.1.max(0.0)))
            .collect();
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
            // Счёт в f64: у сцены с огромным предметом (ошибка в файле) число клеток
            // на первых кругах не влезает в целое.
            if dims.iter().map(|&d| d as f64).product::<f64>() <= max_cells as f64 {
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
        grid
    }

    fn cells_len(&self) -> usize {
        self.dims.iter().map(|&d| d as usize).product()
    }

    /// Пары (клетка, объект) для клеток, к которым граница объекта ближе margin, по
    /// порядку объектов: потом раскладка по клеткам подсчётом ([`Grid::fill_lists`]).
    fn touch(&self, bounds: &[Bound], pairs: &mut Vec<(u32, u32)>) {
        let [nx, ny, nz] = self.dims.map(|d| d as usize);
        let top = Vec3::new(nx as f32 - 1.0, ny as f32 - 1.0, nz as f32 - 1.0);
        let (origin, cell, margin) = (self.origin, self.cell, self.margin);
        // Клетки по осям, которых граница касается, и квадраты зазоров до них; клетка
        // в списке, если сумма зазоров меньше reach².
        let mut gx: Vec<f32> = Vec::new();
        for b in bounds {
            let reach = b.r + margin;
            // Чуть шире (на сотую клетки) с каждой стороны: на самой границе округление в
            // делении могло бы потерять клетку, которую сравнение ниже (и свободное
            // расстояние, `add_free`) считает близкой. Лишние клетки отсеет сравнение.
            let first = ((b.lo - reach - origin) / cell - 0.01)
                .floor()
                .clamp(Vec3::ZERO, top);
            let last = ((b.hi + reach - origin) / cell + 0.01)
                .floor()
                .clamp(Vec3::ZERO, top);
            let axis = |k: usize, o: f32, b0: f32, b1: f32| {
                let lo = o + k as f32 * cell;
                let g = gap(lo, lo + cell, b0, b1);
                g * g
            };
            let far = reach * reach;
            let x0 = first.x as usize;
            gx.clear();
            gx.extend((x0..=last.x as usize).map(|x| axis(x, origin.x, b.lo.x, b.hi.x)));
            for z in first.z as usize..=last.z as usize {
                let dz = axis(z, origin.z, b.lo.z, b.hi.z);
                for y in first.y as usize..=last.y as usize {
                    let dyz = dz + axis(y, origin.y, b.lo.y, b.hi.y);
                    if dyz >= far {
                        continue;
                    }
                    let row = (z * ny + y) * nx + x0;
                    for (k, &dx) in gx.iter().enumerate() {
                        if dyz + dx < far {
                            pairs.push(((row + k) as u32, b.index));
                        }
                    }
                }
            }
        }
    }

    /// Списки клеток из пар (клетка, объект): в списке клетки объекты в порядке пар.
    fn fill_lists(&mut self, pairs: &[(u32, u32)]) {
        let mut counts = vec![0u32; self.cells_len()];
        for &(k, _) in pairs {
            counts[k as usize] += 1;
        }
        let mut start = 0;
        self.cells = counts
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
        self.items = vec![0; start as usize];
        let mut fill: Vec<u32> = self.cells.iter().map(|c| c.start).collect();
        for &(k, i) in pairs {
            let at = &mut fill[k as usize];
            self.items[*at as usize] = i;
            *at += 1;
        }
    }

    /// Свободное расстояние клеток (7.18д4): для клетки — наименьшее по конечным объектам
    /// не из её списка расстояние от её коробки до границы, но не больше [`FREE_CAP`]
    /// (объекты списка шаг считает точно, им оценка не нужна; до 8.8а они тоже входили, и
    /// у клетки рядом с предметом оценка была почти 0 — углы в открытых местах чуть
    /// темнели). Каждый объект
    /// метит только клетки ближе [`FREE_CAP`] к себе (у остальных до него не меньше
    /// FREE_CAP), квадрат расстояния до коробки — сумма трёх заранее посчитанных по осям.
    /// Округление вниз до миллиметра и ещё −1 мм: шейдер находит клетку точки с ошибкой
    /// округления, запас её покрывает.
    fn free_cap(&self, squared: bool) -> f32 {
        if squared {
            FREE_CAP * FREE_CAP
        } else {
            FREE_CAP
        }
    }

    /// Вклад границ `bounds` в свободное расстояние клеток `free` (квадраты, если
    /// `squared`; начальное значение — [`Grid::free_cap`]).
    fn add_free(&self, bounds: &[Bound], free: &mut [f32], squared: bool) {
        let [nx, ny, nz] = self.dims.map(|d| d as usize);
        // Квадрат зазора по оси между отрезком клетки `k` и отрезком границы `b0..b1`.
        let axis = |b0: f32, b1: f32, o: f32, k: usize| {
            let lo = o + k as f32 * self.cell;
            let g = gap(lo, lo + self.cell, b0, b1);
            g * g
        };
        let range = |b0: f32, b1: f32, reach: f32, o: f32, n: usize| {
            let a = ((b0 - reach - o) / self.cell).floor().max(0.0) as usize;
            let b = ((b1 + reach - o) / self.cell).floor().min(n as f32 - 1.0);
            (a, if b < 0.0 { None } else { Some(b as usize) })
        };
        let (mut gx, mut gy, mut gz) = (Vec::new(), Vec::new(), Vec::new());
        for b in bounds {
            let r = b.r;
            let reach = r + FREE_CAP;
            let (x0, Some(x1)) = range(b.lo.x, b.hi.x, reach, self.origin.x, nx) else {
                continue;
            };
            let (y0, Some(y1)) = range(b.lo.y, b.hi.y, reach, self.origin.y, ny) else {
                continue;
            };
            let (z0, Some(z1)) = range(b.lo.z, b.hi.z, reach, self.origin.z, nz) else {
                continue;
            };
            if x0 > x1 || y0 > y1 || z0 > z1 {
                continue;
            }
            gx.clear();
            gx.extend((x0..=x1).map(|k| axis(b.lo.x, b.hi.x, self.origin.x, k)));
            gy.clear();
            gy.extend((y0..=y1).map(|k| axis(b.lo.y, b.hi.y, self.origin.y, k)));
            gz.clear();
            gz.extend((z0..=z1).map(|k| axis(b.lo.z, b.hi.z, self.origin.z, k)));
            let far = reach * reach;
            // Ближе этого объект в списке клетки: то же сравнение, что в `lists`, теми же
            // действиями над теми же числами, чтобы ответ совпадал до бита.
            let listed = (r + self.margin) * (r + self.margin);
            for (z, &dz) in (z0..=z1).zip(&gz) {
                for (y, &dy) in (y0..=y1).zip(&gy) {
                    let dyz = dz + dy;
                    // Вся строка дальше FREE_CAP от границы — её клетки она не меняет.
                    if dyz >= far {
                        continue;
                    }
                    let row = (z * ny + y) * nx;
                    // Без ветвлений: цикл по строке складывается в векторные команды.
                    let cells = free[row + x0..=row + x1].iter_mut().zip(&gx);
                    if squared {
                        for (f, &dx) in cells {
                            let sum = dyz + dx;
                            // Не f32::min: сравнение складывается в одну команду minps.
                            *f = if sum < *f && sum >= listed { sum } else { *f };
                        }
                    } else {
                        for (f, &dx) in cells {
                            let sum = dyz + dx;
                            let d = sum.sqrt() - r;
                            *f = if d < *f && sum >= listed { d } else { *f };
                        }
                    }
                }
            }
        }
    }

    /// Свободное расстояние клеток из [`Grid::add_free`].
    fn finish_free(&mut self, free: &[f32], squared: bool) {
        for (cell, &f) in self.cells.iter_mut().zip(free) {
            let f = if squared { f.sqrt() } else { f };
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
        GRID_HEADER_WORDS
            + self.cells.len() * CELL_WORDS
            + self.items.len()
            + self.unbounded.len()
            + self.fine.as_ref().map_or(0, |f| f.words())
    }

    /// Раскладка для видеокарты, начиная со слова `at` буфера сцены:
    ///
    /// - заголовок, 20 слов: угол сетки (3 числа), размер клетки, клеток по осям
    ///   (3 целых), margin, коробка шаров (6 чисел), где клетки, где списки, где и
    ///   сколько бесконечных объектов, сколько из них не плоскости, где заголовок мелкой
    ///   сетки ([`Grid::fine`], 0 — нет; у неё раскладка та же, читаются из неё только
    ///   угол, размер клетки, число клеток, margin и клетки);
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
            // Где заголовок мелкой сетки (0 — её нет): сразу за этой.
            self.fine
                .as_ref()
                .map_or(0, |_| (unbounded_at + self.unbounded.len()) as u32),
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
        if let Some(fine) = &self.fine {
            let at = out.len();
            fine.write(at, out);
        }
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
            // Как в шейдере: внутри коробки мелкой сетки клетка и список — её.
            let fine = self
                .fine
                .as_deref()
                .and_then(|f| f.cell_at(p).map(|c| (f, c)));
            match fine.or_else(|| self.cell_at(p).map(|c| (self, c))) {
                Some((grid, c)) => {
                    let cell = grid.cells[c];
                    list = &grid.items[cell.start as usize..(cell.start + cell.count) as usize];
                    pick.distance = grid.start(c);
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
                let mut outside = (p - c).length() - r;
                if self.object_boxes {
                    outside = outside.max(crate::shape3d::box_outside(
                        crate::shape3d::box_word(o),
                        c,
                        r,
                        p,
                    ));
                }
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

    /// Как строится сетка: по коробкам (как сейчас) и по шарам (как до 8.8а).
    const BOTH: [GridOptions; 2] = [
        GridOptions {
            free: true,
            boxes: true,
            object_boxes: true,
        },
        GridOptions {
            free: true,
            boxes: false,
            object_boxes: false,
        },
    ];

    fn build(objects: &[Program], options: GridOptions) -> Grid {
        let field = crate::shape3d::level_field_object(objects).map(|i| i as u32);
        Grid::build_with(objects, field, options)
    }

    fn check(objects: &[Program]) {
        for options in BOTH {
            check_with(objects, options);
        }
    }

    fn check_with(objects: &[Program], options: GridOptions) {
        let grid = build(objects, options);
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
    /// ближайшей границы объекта (коробки или шара, 8.8а), округлённое вниз до миллиметра
    /// (и −1 мм), и не больше FREE_CAP; в пустых местах шаг начинается заметно дальше клетки.
    fn free_matches_brute(objects: &[Program]) {
        for options in BOTH {
            let (longer, cells) = free_matches_brute_with(objects, options);
            assert!(
                longer * 4 > cells,
                "{options:?}: дальше двух клеток шаг начинается в {longer} из {cells}"
            );
        }
    }

    /// Сверка с перебором; ответ — в скольких клетках шаг начинается дальше двух клеток
    /// и сколько клеток всего.
    fn free_matches_brute_with(objects: &[Program], options: GridOptions) -> (usize, usize) {
        let grid = build(objects, options);
        let [nx, ny, _] = grid.dims.map(|d| d as usize);
        let mut longer = 0;
        for (k, c) in grid.cells.iter().enumerate() {
            let (x, y, z) = (k % nx, (k / nx) % ny, k / (nx * ny));
            let lo = grid.origin + Vec3::new(x as f32, y as f32, z as f32) * grid.cell;
            let hi = lo + grid.cell;
            let free = f32::from(c.free_mm) * 0.001;
            let listed = &grid.items[c.start as usize..(c.start + c.count) as usize];
            let brute = objects
                .iter()
                .enumerate()
                .filter(|(i, o)| o.bounds.is_some() && !listed.contains(&(*i as u32)))
                .map(|(_, o)| match (o.bounds_box, o.bounds) {
                    (Some((blo, bhi)), _) if options.boxes => {
                        (blo - hi).max(lo - bhi).max(Vec3::ZERO).length()
                    }
                    (_, Some((cc, r))) => (cc.distance(cc.clamp(lo, hi)) - r.max(0.0)).max(0.0),
                    _ => unreachable!(),
                })
                .fold(FREE_CAP, f32::min);
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
        (longer, grid.cells.len())
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

    /// Цех на тысячу предметов (8.7): по шарам 60-метровые стены попадали почти в каждую
    /// клетку, а клетка росла до 3 м (до 63 объектов в списке). По коробкам (8.8а) клетка
    /// мельче, а списки короче; сетка по-прежнему честная.
    #[test]
    fn factory_grid_lists_are_short() {
        let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scenes/3d"));
        let text = include_str!("../../../scenes/3d/factory.ron");
        let frame = crate::SpaceScene::parse_in(text, Some(dir))
            .unwrap()
            .frame(1.3);
        // Как на видеокарте: предметы, потом невидимые заменители станков.
        let objects: Vec<Program> = frame
            .shapes
            .iter()
            .chain(&frame.proxies)
            .map(|s| compile(s).unwrap())
            .collect();
        let stats = |options| {
            let grid = build(&objects, options);
            let longest = grid.cells.iter().map(|c| c.count).max().unwrap();
            let filled: Vec<u32> = grid
                .cells
                .iter()
                .map(|c| c.count)
                .filter(|&c| c > 0)
                .collect();
            let mean = filled.iter().sum::<u32>() as f32 / filled.len() as f32;
            (grid.cell, grid.dims, longest, mean, grid.words())
        };
        let boxes = stats(BOTH[0]);
        let spheres = stats(BOTH[1]);
        println!("коробки {boxes:?}, шары {spheres:?}");
        assert!(
            boxes.0 < spheres.0 * 0.5,
            "клетка {} и {}",
            boxes.0,
            spheres.0
        );
        assert!(boxes.2 <= 24, "в клетке {} объектов", boxes.2);
        assert!(
            boxes.2 * 2 < spheres.2,
            "списки {} и {}",
            boxes.2,
            spheres.2
        );
        // Сетка не съедает буфер кадра (MAX_WORDS = 2М слов).
        assert!(boxes.4 < 400_000, "сетка {} слов", boxes.4);
        check(&objects);
        for options in BOTH {
            free_matches_brute_with(&objects, options);
        }
    }

    /// Сетка с готовым вкладом неподвижного (8.8б) та же до бита, что посчитанная
    /// целиком, кадр за кадром; неподвижное пересчитывается, только когда меняется
    /// набор объектов.
    #[test]
    fn cached_grid_matches_full_build() {
        let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scenes/3d"));
        let factory =
            crate::SpaceScene::parse_in(include_str!("../../../scenes/3d/factory.ron"), Some(dir))
                .unwrap();
        let stress =
            crate::SpaceScene::parse(include_str!("../../../scenes/3d/stress.ron")).unwrap();
        for (name, scene) in [("завод", &factory), ("стресс", &stress)] {
            let frame = |t: f32| -> Vec<Program> {
                let f = scene.frame(t);
                f.shapes
                    .iter()
                    .chain(&f.proxies)
                    .zip(&f.still)
                    .map(|(s, &still)| Program {
                        still,
                        ..compile(s).unwrap()
                    })
                    .collect()
            };
            for options in BOTH {
                let mut cache = GridCache::default();
                for k in 0..4 {
                    let objects = frame(1.0 + k as f32 * 0.016);
                    let field = crate::shape3d::level_field_object(&objects).map(|i| i as u32);
                    let cached = Grid::build_cached(&objects, field, options, &mut cache);
                    let full = Grid::build_with(&objects, field, options);
                    assert!(cached == full, "{name} {options:?}, кадр {k}");
                }
                // Неподвижное раскладывается в первом кадре, дальше берётся готовым.
                assert_eq!(cache.rebuilds, 1, "{name} {options:?}");
                assert!(cache.moved < 400, "{name}: заново {}", cache.moved);
                // Отсев по взгляду убрал движущийся объект: номера неподвижного за ним
                // сдвинулись, но готовое подходит (номера в парах переписаны). Убранное
                // неподвижное — пересчёт.
                let moving = frame(1.0)
                    .iter()
                    .position(|p| !p.still && p.bounds.is_some());
                for (k, drop) in [moving, moving, None, Some(1)].into_iter().enumerate() {
                    let mut objects = frame(1.064 + k as f32 * 0.016);
                    if let Some(d) = drop {
                        objects.remove(d);
                    }
                    let field = crate::shape3d::level_field_object(&objects).map(|i| i as u32);
                    let before = cache.rebuilds;
                    let cached = Grid::build_cached(&objects, field, options, &mut cache);
                    assert!(
                        cached == Grid::build_with(&objects, field, options),
                        "{name} {options:?}, без {drop:?}"
                    );
                    let still_removed = k == 3 && frame(1.0)[1].still;
                    assert_eq!(
                        cache.rebuilds - before,
                        usize::from(still_removed),
                        "{name}: шаг {k}"
                    );
                }
            }
        }
    }

    /// Предметы города «большой мир» — как на видеокарте, с флагами неподвижности.
    fn world(time: f32) -> Vec<Program> {
        let scene = crate::SpaceScene::parse(include_str!("../../../scenes/3d/world.ron")).unwrap();
        let f = scene.frame(time);
        f.shapes
            .iter()
            .zip(&f.still)
            .map(|(s, &still)| Program {
                still,
                ..compile(s).unwrap()
            })
            .collect()
    }

    /// Мелкая сетка (8.10б): в городе на 800 м с холмами она ложится на город (а не на
    /// холмы и небо), её клетка в разы мельче, списки у земли короче в разы, и ответ по
    /// ней так же никогда не больше правды. Слова для видеокарты читаются так же, как
    /// считает зеркало; сетка с готовым неподвижным та же, что посчитанная целиком.
    #[test]
    fn fine_grid_covers_the_town() {
        let objects = world(1.0);
        let grid = build(&objects, GridOptions::default());
        let fine = grid.fine.as_deref().expect("у города есть мелкая сетка");
        assert!(fine.cells.len() <= FINE_MAX_CELLS + FINE_MAX_CELLS / 8);
        assert!(fine.cell * 3.0 < grid.cell, "{} и {}", fine.cell, grid.cell);
        let size = fine.cell * Vec3::from(fine.dims.map(|d| d as f32));
        assert!(
            size.y < 20.0,
            "мелкая сетка по высоте домов, а не холмов: {size}"
        );
        assert!(size.x > 300.0 && size.z > 300.0, "весь город: {size}");
        // Точки у земли в городе: длина списка в мелкой сетке и в крупной.
        let mut seed = 777u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        let (mut coarse_sum, mut fine_sum) = (0u64, 0u64);
        for _ in 0..20000 {
            let p = fine.origin + Vec3::new(rnd(), rnd(), rnd()) * size;
            let (Some(a), Some(b)) = (grid.cell_at(p), fine.cell_at(p)) else {
                continue;
            };
            coarse_sum += u64::from(grid.cells[a].count);
            fine_sum += u64::from(fine.cells[b].count);
            let truth = exact(&objects, p);
            let (d, m) = grid.near(&objects, p, 1e9);
            assert!(d <= truth + 1e-4, "{p}: сетка {d}, правда {truth}");
            if truth < 0.2 {
                assert!((d - truth).abs() < 1e-4, "{p}: сетка {d}, правда {truth}");
                assert!(m.is_some());
            }
        }
        println!("в списке у земли: крупная {coarse_sum}, мелкая {fine_sum}");
        assert!(fine_sum * 3 < coarse_sum, "{fine_sum} и {coarse_sum}");
        // Свободное расстояние клеток мелкой сетки — по тем же правилам (на выборку).
        let [nx, ny, _] = fine.dims.map(|d| d as usize);
        for k in (0..fine.cells.len()).step_by(997) {
            let c = fine.cells[k];
            let (x, y, z) = (k % nx, (k / nx) % ny, k / (nx * ny));
            let lo = fine.origin + Vec3::new(x as f32, y as f32, z as f32) * fine.cell;
            let hi = lo + fine.cell;
            let listed = &fine.items[c.start as usize..(c.start + c.count) as usize];
            let brute = objects
                .iter()
                .enumerate()
                .filter(|(i, o)| o.bounds.is_some() && !listed.contains(&(*i as u32)))
                .filter_map(|(_, o)| o.bounds_box)
                .map(|(blo, bhi)| (blo - hi).max(lo - bhi).max(Vec3::ZERO).length())
                .fold(FREE_CAP, f32::min);
            let free = f32::from(c.free_mm) * 0.001;
            assert!(
                free <= (brute - 0.0009).max(0.0),
                "клетка {k}: {free} > {brute}"
            );
            assert!(free >= brute - 0.0021, "клетка {k}: {free} и {brute}");
            // Всё, что не в списке, не ближе клетки.
            assert!(
                brute >= fine.margin.min(FREE_CAP) - 1e-3,
                "клетка {k}: {brute}"
            );
        }
        words_match_the_mirror(&objects);
        // Кадр за кадром: место мелкой сетки не скачет, неподвижное считается один раз.
        let mut cache = GridCache::default();
        for k in 0..3 {
            let objects = world(1.0 + k as f32 * 0.016);
            let cached = Grid::build_cached(&objects, None, GridOptions::default(), &mut cache);
            let full = Grid::build_with(&objects, None, GridOptions::default());
            assert!(cached == full, "кадр {k}");
            assert!(cached.fine.is_some());
        }
        assert_eq!(cache.rebuilds, 1);
        assert_eq!(cache.fine.as_ref().map(|f| f.rebuilds), Some(1));
    }

    /// Сценам в комнатах мелкая сетка не нужна: клетка у них и так по предметам.
    #[test]
    fn small_scenes_have_no_fine_grid() {
        let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scenes/3d"));
        for (name, text) in [
            ("stress", include_str!("../../../scenes/3d/stress.ron")),
            ("showroom", include_str!("../../../scenes/3d/showroom.ron")),
            ("effects", include_str!("../../../scenes/3d/effects.ron")),
            ("factory", include_str!("../../../scenes/3d/factory.ron")),
            (
                "warehouse",
                include_str!("../../../scenes/3d/warehouse.ron"),
            ),
            ("lamps", include_str!("../../../scenes/3d/lamps.ron")),
        ] {
            let f = crate::SpaceScene::parse_in(text, Some(dir))
                .unwrap()
                .frame(1.0);
            let objects: Vec<Program> = f
                .shapes
                .iter()
                .chain(&f.proxies)
                .zip(&f.still)
                .flat_map(|(s, &still)| {
                    crate::shape3d::compile_parts(s)
                        .unwrap()
                        .into_iter()
                        .map(move |p| Program { still, ..p })
                })
                .collect();
            let grid = build(&objects, GridOptions::default());
            assert!(
                grid.fine.is_none(),
                "{name}: клетка {}, мелкая {:?}",
                grid.cell,
                grid.fine.as_ref().map(|f| (f.cell, f.dims))
            );
        }
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
            // Клетка точки в сетке с заголовком `h` (как в шейдере): её номер и число клеток.
            let cell_in = |h: usize| {
                let origin = Vec3::new(f(h), f(h + 1), f(h + 2));
                let dims = [w[h + 4], w[h + 5], w[h + 6]].map(|d| d as i64);
                let q = ((p - origin) / f(h + 3)).floor();
                let q = [q.x as i64, q.y as i64, q.z as i64];
                (0..3)
                    .all(|k| q[k] >= 0 && q[k] < dims[k])
                    .then(|| ((q[2] * dims[1] + q[1]) * dims[0] + q[0]) as usize)
            };
            let unbounded = w[g + 16] as usize;
            let mut list: Vec<u32> = w[unbounded..unbounded + w[g + 17] as usize].to_vec();
            // Сначала мелкая сетка (слово 19), потом крупная.
            let fine = w[g + 19] as usize;
            let found = (fine != 0)
                .then(|| cell_in(fine).map(|c| (fine, c)))
                .flatten()
                .or_else(|| cell_in(g).map(|c| (g, c)));
            let best = if let Some((h, c)) = found {
                let c = w[h + 14] as usize + c * 2;
                let (start, count) = (w[c] as usize, (w[c + 1] & 0xffff) as usize);
                list.extend(&w[start..start + count]);
                f(h + 7).max((w[c + 1] >> 16) as f32 * 0.001)
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
