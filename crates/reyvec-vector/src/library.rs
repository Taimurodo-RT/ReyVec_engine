//! Набор фигур с готовыми треугольниками и экземпляры фигур на сцене.

use kurbo::Affine;

use crate::{
    Color, CurveMesh, FillMethod, FillRule, Mesh, MorphError, MorphMesh, Paint, Shape, Style,
    TessellationError, curve_mesh, morph_mesh, tessellate_smooth,
};

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
    method: FillMethod,
}

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
        let mesh = tessellate_smooth(&shape, self.tolerance, self.aa_margin())?;
        let curves =
            matches!(shape.style, Style::Fill(_)).then(|| curve_mesh(&shape.path, self.tolerance));
        Ok(self.push(Entry {
            shape,
            mesh,
            curves,
            morph: Vec::new(),
            method,
        }))
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
        let MorphMesh {
            mesh,
            keyframes: other,
        } = morph_mesh(keyframes, self.tolerance)?;
        Ok(self.push(Entry {
            shape: Shape::fill_with(keyframes[0].clone(), rule, paint),
            mesh: Mesh::default(),
            curves: Some(mesh),
            morph: other,
            method: FillMethod::Stencil,
        }))
    }

    fn push(&mut self, entry: Entry) -> ShapeId {
        self.entries.push(entry);
        self.revision += 1;
        ShapeId(self.entries.len() as u32 - 1)
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
