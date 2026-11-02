//! Набор фигур с готовыми треугольниками и экземпляры фигур на сцене.

use kurbo::Affine;

use crate::{
    Color, CurveMesh, FillMethod, FillRule, Mesh, MorphError, MorphMesh, Paint, Shape, Style,
    TessellationError, curve_mesh, morph_mesh, tessellate_smooth,
};

/// Самый подробный уровень детализации: допуск в 4⁶ = 4096 раз меньше обычного (под
/// самое сильное приближение камеры).
pub const DETAIL_FINEST: i32 = -6;
/// Самый грубый уровень: допуск в 4³ = 64 раза больше обычного (мелкие фигуры вдали).
pub const DETAIL_COARSEST: i32 = 3;
/// Сколько уровней от [`DETAIL_FINEST`] до [`DETAIL_COARSEST`].
pub const DETAIL_LEVELS: usize = (DETAIL_COARSEST - DETAIL_FINEST + 1) as usize;

/// Почему не построить уровень детализации.
#[derive(Debug, thiserror::Error)]
pub enum DetailError {
    #[error("нет фигуры №{0}")]
    UnknownShape(u32),
    #[error("уровень детализации {0} вне {DETAIL_FINEST}..={DETAIL_COARSEST}")]
    BadLevel(i32),
    #[error(transparent)]
    Tessellation(#[from] TessellationError),
    #[error(transparent)]
    Morph(#[from] MorphError),
}

/// Номер фигуры в [`ShapeLibrary`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShapeId(pub u32);

struct Entry {
    shape: Shape,
    mesh: Mesh,
    /// Треугольники для заливки через трафарет (только у заливок).
    curves: Option<CurveMesh>,
    /// Положения точек `curves` у остальных ключевых контуров (только у перетекающих).
    morph: Vec<Vec<[f32; 2]>>,
    /// Ключевые контуры перетекающей фигуры (для уровней детализации).
    keyframes: Vec<kurbo::BezPath>,
    method: FillMethod,
    /// Уровни детализации этой фигуры: номер готовой фигуры-уровня или `u32::MAX`
    /// (индекс = уровень − [`DETAIL_FINEST`]). У самих уровней все пустые.
    details: [u32; DETAIL_LEVELS],
    /// У уровня детализации: исходная фигура и номер уровня.
    detail_of: Option<(ShapeId, i32)>,
}

const NO_DETAIL: u32 = u32::MAX;

/// Фигуры, уже разбитые на треугольники. Разбиение делается один раз в [`Self::add`],
/// поэтому показывать фигуру в каждом кадре процессору почти ничего не стоит.
pub struct ShapeLibrary {
    tolerance: f64,
    entries: Vec<Entry>,
    revision: u64,
}

impl ShapeLibrary {
    /// Пустой набор. `tolerance`: насколько ломаная может отходить от настоящей кривой,
    /// в единицах фигур. Выбирается под самый крупный размер, в котором фигуры покажут.
    pub fn new(tolerance: f64) -> Self {
        Self {
            tolerance,
            entries: Vec::new(),
            revision: 0,
        }
    }

    /// Разбиение с допуском этого набора: им можно резать фигуры в другом потоке
    /// ([`Tessellator::prepare`]), а в набор потом только положить готовое
    /// ([`Self::insert`], [`Self::append`]).
    pub fn tessellator(&self) -> Tessellator {
        Tessellator::new(self.tolerance)
    }

    /// Добавляет фигуру и сразу разбивает её на треугольники. Заливка рисуется
    /// треугольниками ([`FillMethod::Triangles`]). Обводки получают запас под
    /// сглаживание в шейдере: [`Self::aa_margin`].
    pub fn add(&mut self, shape: Shape) -> Result<ShapeId, TessellationError> {
        self.add_with(shape, FillMethod::Triangles)
    }

    /// Как [`Self::add`], но заливку рисовать способом `method`.
    pub fn add_with(
        &mut self,
        shape: Shape,
        method: FillMethod,
    ) -> Result<ShapeId, TessellationError> {
        let prepared = self.tessellator().prepare(shape, method)?;
        Ok(self.insert(prepared))
    }

    /// Добавляет перетекающую заливку: фигуру, которая меняет форму между контурами
    /// `keyframes` (не меньше двух). Какая сейчас форма, задаёт каждый экземпляр
    /// ([`Instance::with_morph`]), смешивает точки видеокарта. Рисуется всегда трафаретом:
    /// заранее разбить на треугольники все промежуточные формы нельзя.
    pub fn add_morph(
        &mut self,
        keyframes: &[kurbo::BezPath],
        rule: FillRule,
        paint: impl Into<Paint>,
    ) -> Result<ShapeId, MorphError> {
        let prepared = self
            .tessellator()
            .prepare_morph(keyframes, rule, paint.into())?;
        Ok(self.insert(prepared))
    }

    /// Кладёт в набор уже разбитую фигуру. Резать ничего не нужно, поэтому это дёшево
    /// и годится для потока кадра.
    pub fn insert(&mut self, prepared: PreparedShape) -> ShapeId {
        debug_assert!(
            (prepared.tolerance - self.tolerance).abs() <= f64::EPSILON * self.tolerance,
            "фигура разбита с другим допуском"
        );
        self.push(prepared.entry)
    }

    /// Кладёт в набор все фигуры пачки по порядку и возвращает номер первой. Фигура `i`
    /// пачки (её [`ShapeSink`] выдал номер `ShapeId(i)`) получает номер `ShapeId(первая + i)`:
    /// номера в готовом списке кадра сдвигаются через [`ShapeId::offset`] или
    /// [`crate::SceneItem::offset_shapes`].
    pub fn append(&mut self, batch: ShapeBatch) -> u32 {
        let first = self.entries.len() as u32;
        for prepared in batch.shapes {
            self.insert(prepared);
        }
        first
    }

    /// Готовый уровень детализации `level` фигуры `id` (0 — сама фигура), если он уже
    /// построен.
    pub fn detail(&self, id: ShapeId, level: i32) -> Option<ShapeId> {
        let k = usize::try_from(level - DETAIL_FINEST).ok()?;
        let d = *self.entries.get(id.0 as usize)?.details.get(k)?;
        (d != NO_DETAIL).then_some(ShapeId(d))
    }

    /// Все уровни детализации фигуры `id`: номер готовой фигуры-уровня или `u32::MAX`
    /// (индекс = уровень − [`DETAIL_FINEST`]). Для таблицы на стороне видеокарты.
    pub fn detail_table(&self, id: ShapeId) -> Option<&[u32; DETAIL_LEVELS]> {
        self.entries.get(id.0 as usize).map(|e| &e.details)
    }

    /// Исходная фигура и уровень, если `id` — уровень детализации другой фигуры.
    pub fn detail_of(&self, id: ShapeId) -> Option<(ShapeId, i32)> {
        self.entries.get(id.0 as usize)?.detail_of
    }

    /// Строит уровень детализации `level` фигуры `id`: та же фигура, разбитая с допуском
    /// `tolerance · 4^level` (меньше нуля — подробнее, под приближение камеры; больше —
    /// грубее, для мелких фигур). Разовая работа процессора: уровень становится новой
    /// фигурой набора и загружается на видеокарту как любая новая фигура. Если уровень уже
    /// есть, просто возвращает его. То же по частям, чтобы резать в другом потоке:
    /// [`Self::detail_job`], [`DetailJob::run`], [`Self::insert_detail`].
    pub fn build_detail(&mut self, id: ShapeId, level: i32) -> Result<ShapeId, DetailError> {
        let job = self.detail_job(id, level)?;
        if let Some(done) = self.detail(id, level) {
            return Ok(done);
        }
        Ok(self.insert_detail(job.run()?))
    }

    /// Задание на уровень детализации `level` фигуры `id`: копия всего, что нужно для
    /// разбиения. Само разбиение ([`DetailJob::run`]) набору не нужно и может идти в
    /// другом потоке.
    pub fn detail_job(&self, id: ShapeId, level: i32) -> Result<DetailJob, DetailError> {
        if !(DETAIL_FINEST..=DETAIL_COARSEST).contains(&level) {
            return Err(DetailError::BadLevel(level));
        }
        let base = self
            .entries
            .get(id.0 as usize)
            .ok_or(DetailError::UnknownShape(id.0))?;
        if let Some((original, _)) = base.detail_of {
            return Err(DetailError::UnknownShape(original.0));
        }
        Ok(DetailJob {
            id,
            level,
            shape: base.shape.clone(),
            keyframes: base.keyframes.clone(),
            curves: base.curves.is_some(),
            method: base.method,
            tolerance: self.tolerance * 4f64.powi(level),
        })
    }

    /// Кладёт готовый уровень детализации. Если такой уровень уже успели положить,
    /// возвращает его, а готовое выбрасывает.
    pub fn insert_detail(&mut self, detail: PreparedDetail) -> ShapeId {
        let PreparedDetail { id, level, entry } = detail;
        if let Some(done) = self.detail(id, level) {
            return done;
        }
        let new = self.push(entry);
        self.entries[id.0 as usize].details[(level - DETAIL_FINEST) as usize] = new.0;
        new
    }

    fn push(&mut self, mut entry: Entry) -> ShapeId {
        let id = self.entries.len() as u32;
        if entry.detail_of.is_none() {
            // Уровень 0 — сама фигура.
            entry.details[(-DETAIL_FINEST) as usize] = id;
        }
        self.entries.push(entry);
        self.revision += 1;
        ShapeId(id)
    }

    /// Способ рисования заливки фигуры (у перетекающих всегда трафарет).
    pub fn method(&self, id: ShapeId) -> Option<FillMethod> {
        self.entries.get(id.0 as usize).map(|e| e.method)
    }

    /// Положения точек [`Self::curves`] у ключевых контуров перетекающей фигуры, кроме
    /// первого (его точки в самих `curves`). Пусто у обычных фигур.
    pub fn morph_keyframes(&self, id: ShapeId) -> &[Vec<[f32; 2]>] {
        self.entries
            .get(id.0 as usize)
            .map_or(&[], |e| e.morph.as_slice())
    }

    /// Фигура по номеру.
    pub fn shape(&self, id: ShapeId) -> Option<&Shape> {
        self.entries.get(id.0 as usize).map(|e| &e.shape)
    }

    /// Треугольники фигуры по номеру.
    pub fn mesh(&self, id: ShapeId) -> Option<&Mesh> {
        self.entries.get(id.0 as usize).map(|e| &e.mesh)
    }

    /// Треугольники заливки через трафарет (`None` у обводок и неизвестных номеров).
    pub fn curves(&self, id: ShapeId) -> Option<&CurveMesh> {
        self.entries.get(id.0 as usize)?.curves.as_ref()
    }

    /// Все фигуры по порядку номеров.
    pub fn iter(&self) -> impl Iterator<Item = (ShapeId, &Shape, &Mesh)> {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, e)| (ShapeId(i as u32), &e.shape, &e.mesh))
    }

    /// Сколько фигур.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Пуст ли набор.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Счётчик изменений: растёт при каждом добавлении. По нему видеокарта узнаёт, что
    /// треугольники пора загрузить заново.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Допуск разбиения кривых.
    pub fn tolerance(&self) -> f64 {
        self.tolerance
    }

    /// Запас вокруг обводок под мягкий край, в единицах фигур. Допуск выбирают как
    /// четверть пикселя в самом крупном показе, поэтому запас в четыре допуска равен
    /// примерно одному пикселю. Если показать фигуру мельче, мягкий край станет уже.
    pub fn aa_margin(&self) -> f64 {
        self.tolerance * 4.0
    }
}

impl ShapeId {
    /// Номер фигуры пачки ([`ShapeBatch`]) после того, как пачку положили в набор с
    /// первым номером `first` ([`ShapeLibrary::append`]).
    #[must_use]
    pub fn offset(self, first: u32) -> Self {
        Self(self.0 + first)
    }
}

/// Фигура, уже разбитая на треугольники, но ещё не в наборе. Резать можно в любом потоке
/// ([`Tessellator`]), класть в набор — в том, где набор ([`ShapeLibrary::insert`]).
pub struct PreparedShape {
    entry: Entry,
    tolerance: f64,
}

/// Разбиение фигур на треугольники без набора: только допуск. Копируется в другой поток,
/// чтобы тяжёлая работа (нарезка кривых) шла там, а поток кадра только клал готовое.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tessellator {
    tolerance: f64,
}

impl Tessellator {
    /// Разбиение с допуском `tolerance` (как у [`ShapeLibrary::new`]).
    pub fn new(tolerance: f64) -> Self {
        Self { tolerance }
    }

    /// Допуск разбиения кривых.
    pub fn tolerance(&self) -> f64 {
        self.tolerance
    }

    /// Запас вокруг обводок под мягкий край (см. [`ShapeLibrary::aa_margin`]).
    pub fn aa_margin(&self) -> f64 {
        self.tolerance * 4.0
    }

    /// Разбивает фигуру, как [`ShapeLibrary::add_with`], но в набор не кладёт.
    pub fn prepare(
        &self,
        shape: Shape,
        method: FillMethod,
    ) -> Result<PreparedShape, TessellationError> {
        let mesh = tessellate_smooth(&shape, self.tolerance, self.aa_margin())?;
        let curves =
            matches!(shape.style, Style::Fill(_)).then(|| curve_mesh(&shape.path, self.tolerance));
        Ok(self.prepared(Entry {
            shape,
            mesh,
            curves,
            morph: Vec::new(),
            keyframes: Vec::new(),
            method,
            details: [NO_DETAIL; DETAIL_LEVELS],
            detail_of: None,
        }))
    }

    /// Сводит контуры перетекающей фигуры, как [`ShapeLibrary::add_morph`], но в набор
    /// не кладёт.
    pub fn prepare_morph(
        &self,
        keyframes: &[kurbo::BezPath],
        rule: FillRule,
        paint: Paint,
    ) -> Result<PreparedShape, MorphError> {
        let MorphMesh {
            mesh,
            keyframes: other,
        } = morph_mesh(keyframes, self.tolerance)?;
        Ok(self.prepared(Entry {
            shape: Shape::fill_with(keyframes[0].clone(), rule, paint),
            mesh: Mesh::default(),
            curves: Some(mesh),
            morph: other,
            keyframes: keyframes.to_vec(),
            method: FillMethod::Stencil,
            details: [NO_DETAIL; DETAIL_LEVELS],
            detail_of: None,
        }))
    }

    fn prepared(&self, entry: Entry) -> PreparedShape {
        PreparedShape {
            entry,
            tolerance: self.tolerance,
        }
    }
}

/// Куда загрузчики (SVG, Motif, файл сцены) складывают фигуры: прямо в набор
/// ([`ShapeLibrary`]) или в пачку ([`ShapeBatch`]), которую режут в другом потоке и
/// потом целиком кладут в набор.
pub trait ShapeSink {
    /// Разбивает фигуру и возвращает её номер (у пачки — номер внутри пачки).
    fn add_with(&mut self, shape: Shape, method: FillMethod) -> Result<ShapeId, TessellationError>;

    /// Перетекающая заливка (см. [`ShapeLibrary::add_morph`]).
    fn add_morph(
        &mut self,
        keyframes: &[kurbo::BezPath],
        rule: FillRule,
        paint: Paint,
    ) -> Result<ShapeId, MorphError>;

    /// Заливка треугольниками (см. [`ShapeLibrary::add`]).
    fn add(&mut self, shape: Shape) -> Result<ShapeId, TessellationError> {
        self.add_with(shape, FillMethod::Triangles)
    }
}

impl ShapeSink for ShapeLibrary {
    fn add_with(&mut self, shape: Shape, method: FillMethod) -> Result<ShapeId, TessellationError> {
        ShapeLibrary::add_with(self, shape, method)
    }

    fn add_morph(
        &mut self,
        keyframes: &[kurbo::BezPath],
        rule: FillRule,
        paint: Paint,
    ) -> Result<ShapeId, MorphError> {
        ShapeLibrary::add_morph(self, keyframes, rule, paint)
    }
}

/// Пачка разбитых фигур, ещё не в наборе. Номера фигур в ней свои, с нуля; в наборе они
/// сдвигаются на номер первой ([`ShapeLibrary::append`], [`ShapeId::offset`]).
pub struct ShapeBatch {
    tessellator: Tessellator,
    shapes: Vec<PreparedShape>,
}

impl ShapeBatch {
    /// Пустая пачка с разбиением `tessellator` (возьмите [`ShapeLibrary::tessellator`]).
    pub fn new(tessellator: Tessellator) -> Self {
        Self {
            tessellator,
            shapes: Vec::new(),
        }
    }

    /// Сколько фигур в пачке.
    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    /// Пуста ли пачка.
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    fn push(&mut self, prepared: PreparedShape) -> ShapeId {
        self.shapes.push(prepared);
        ShapeId(self.shapes.len() as u32 - 1)
    }
}

impl ShapeSink for ShapeBatch {
    fn add_with(&mut self, shape: Shape, method: FillMethod) -> Result<ShapeId, TessellationError> {
        let prepared = self.tessellator.prepare(shape, method)?;
        Ok(self.push(prepared))
    }

    fn add_morph(
        &mut self,
        keyframes: &[kurbo::BezPath],
        rule: FillRule,
        paint: Paint,
    ) -> Result<ShapeId, MorphError> {
        let prepared = self.tessellator.prepare_morph(keyframes, rule, paint)?;
        Ok(self.push(prepared))
    }
}

/// Задание на уровень детализации ([`ShapeLibrary::detail_job`]): копия фигуры и допуск.
#[derive(Clone, Debug)]
pub struct DetailJob {
    id: ShapeId,
    level: i32,
    shape: Shape,
    keyframes: Vec<kurbo::BezPath>,
    curves: bool,
    method: FillMethod,
    tolerance: f64,
}

impl DetailJob {
    /// Для какой фигуры уровень.
    pub fn id(&self) -> ShapeId {
        self.id
    }

    /// Какой уровень.
    pub fn level(&self) -> i32 {
        self.level
    }

    /// Разбивает фигуру с допуском уровня. Набор не нужен: можно в другом потоке.
    pub fn run(self) -> Result<PreparedDetail, DetailError> {
        let tolerance = self.tolerance;
        let entry = if self.keyframes.is_empty() {
            let mesh = tessellate_smooth(&self.shape, tolerance, tolerance * 4.0)?;
            let curves = self.curves.then(|| curve_mesh(&self.shape.path, tolerance));
            Entry {
                shape: self.shape,
                mesh,
                curves,
                morph: Vec::new(),
                keyframes: Vec::new(),
                method: self.method,
                details: [NO_DETAIL; DETAIL_LEVELS],
                detail_of: Some((self.id, self.level)),
            }
        } else {
            let MorphMesh {
                mesh,
                keyframes: other,
            } = morph_mesh(&self.keyframes, tolerance)?;
            Entry {
                shape: self.shape,
                mesh: Mesh::default(),
                curves: Some(mesh),
                morph: other,
                keyframes: Vec::new(),
                method: FillMethod::Stencil,
                details: [NO_DETAIL; DETAIL_LEVELS],
                detail_of: Some((self.id, self.level)),
            }
        };
        Ok(PreparedDetail {
            id: self.id,
            level: self.level,
            entry,
        })
    }
}

/// Готовый уровень детализации, ещё не в наборе ([`ShapeLibrary::insert_detail`]).
pub struct PreparedDetail {
    id: ShapeId,
    level: i32,
    entry: Entry,
}

/// Фигура на сцене в этом кадре. Порядок в списке экземпляров = порядок рисования:
/// следующие рисуются поверх предыдущих.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    pub shape: ShapeId,
    /// Из координат фигуры в координаты сцены: сдвиг, поворот, масштаб, наклон.
    pub transform: Affine,
    /// Множитель цвета фигуры (белый ничего не меняет, альфа делает прозрачнее).
    pub tint: Color,
    /// Форма перетекающей фигуры: 0 = первый ключевой контур, 1 = второй, 1.5 = посередине
    /// между вторым и третьим. У обычных фигур ни на что не влияет.
    pub morph: f32,
}

impl Instance {
    /// Фигура `shape` с преобразованием `transform` без оттенка.
    pub fn new(shape: ShapeId, transform: Affine) -> Self {
        Self {
            shape,
            transform,
            tint: Color::WHITE,
            morph: 0.0,
        }
    }

    /// С оттенком.
    pub fn with_tint(self, tint: Color) -> Self {
        Self { tint, ..self }
    }

    /// С формой перетекания (см. [`Self::morph`]).
    pub fn with_morph(self, morph: f32) -> Self {
        Self { morph, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Rect, Shape as _};

    #[test]
    fn add_tessellates_once_and_bumps_revision() {
        let mut lib = ShapeLibrary::new(0.01);
        assert!(lib.is_empty());
        let a = lib
            .add(Shape::fill(
                Rect::new(0.0, 0.0, 1.0, 1.0).to_path(0.1),
                Color::WHITE,
            ))
            .unwrap();
        let rev = lib.revision();
        let b = lib
            .add(Shape::fill(
                Rect::new(0.0, 0.0, 2.0, 1.0).to_path(0.1),
                Color::WHITE,
            ))
            .unwrap();
        assert_ne!(a, b);
        assert!(lib.revision() > rev);
        assert_eq!(lib.len(), 2);
        assert!((lib.mesh(b).unwrap().area() - 2.0).abs() < 1e-6);
        assert!(lib.mesh(ShapeId(9)).is_none());
        assert_eq!(lib.iter().count(), 2);
        assert_eq!(lib.method(a), Some(FillMethod::Triangles));
        assert!(lib.morph_keyframes(a).is_empty());
    }

    #[test]
    fn detail_levels_are_built_once_with_scaled_tolerance() {
        let mut lib = ShapeLibrary::new(0.01);
        let circle = kurbo::Circle::new((0.0, 0.0), 1.0).to_path(1e-3);
        let id = lib.add(Shape::fill(circle.clone(), Color::WHITE)).unwrap();
        assert_eq!(lib.detail(id, 0), Some(id));
        assert_eq!(lib.detail(id, -2), None);
        let fine = lib.build_detail(id, -2).unwrap();
        let coarse = lib.build_detail(id, 2).unwrap();
        assert_eq!(lib.build_detail(id, -2).unwrap(), fine);
        assert_eq!(lib.detail(id, -2), Some(fine));
        assert_eq!(lib.detail_of(fine), Some((id, -2)));
        assert_eq!(lib.detail_of(id), None);
        assert_eq!(lib.len(), 3);
        let count = |s| lib.mesh(s).unwrap().triangle_count();
        // Допуск в 16 раз меньше: кривая режется примерно в 4 раза мельче.
        assert!(count(fine) > 3 * count(id), "{} {}", count(fine), count(id));
        assert!(count(coarse) < count(id));
        // Подробная заливка ближе к площади круга.
        let err = |s| (lib.mesh(s).unwrap().area() - std::f64::consts::PI).abs();
        assert!(err(fine) < err(id) && err(id) < err(coarse));
        let table = lib.detail_table(id).unwrap();
        assert_eq!(table[(-2 - DETAIL_FINEST) as usize], fine.0);
        assert!(lib.build_detail(id, DETAIL_FINEST - 1).is_err());
        assert!(lib.build_detail(fine, 1).is_err());
        assert!(lib.build_detail(ShapeId(99), 1).is_err());
        // Перетекающая фигура тоже: точек у уровня больше, ключевые контуры те же.
        let square = Rect::new(-1.0, -1.0, 1.0, 1.0).to_path(0.1);
        let m = lib
            .add_morph(&[circle, square], FillRule::NonZero, Color::WHITE)
            .unwrap();
        let mf = lib.build_detail(m, -2).unwrap();
        assert_eq!(lib.method(mf), Some(FillMethod::Stencil));
        assert_eq!(lib.morph_keyframes(mf).len(), 1);
        assert!(lib.curves(mf).unwrap().vertices.len() > lib.curves(m).unwrap().vertices.len());
    }

    #[test]
    fn morph_shapes_use_stencil_and_keep_keyframes() {
        let mut lib = ShapeLibrary::new(0.01);
        let small = Rect::new(0.0, 0.0, 1.0, 1.0).to_path(0.1);
        let big = Rect::new(0.0, 0.0, 3.0, 2.0).to_path(0.1);
        let id = lib
            .add_morph(&[small.clone(), big], FillRule::NonZero, Color::WHITE)
            .unwrap();
        assert_eq!(lib.method(id), Some(FillMethod::Stencil));
        let curves = lib.curves(id).unwrap();
        assert_eq!(lib.morph_keyframes(id).len(), 1);
        assert_eq!(lib.morph_keyframes(id)[0].len(), curves.vertices.len());
        assert_eq!(lib.mesh(id).unwrap().triangle_count(), 0);
        assert!(
            lib.add_morph(&[small], FillRule::NonZero, Color::WHITE)
                .is_err()
        );
        let s = lib
            .add_with(
                Shape::fill(Rect::new(0.0, 0.0, 1.0, 1.0).to_path(0.1), Color::WHITE),
                FillMethod::Stencil,
            )
            .unwrap();
        assert_eq!(lib.method(s), Some(FillMethod::Stencil));
    }
}
