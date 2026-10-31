//! Набор фигур с готовыми треугольниками и экземпляры фигур на сцене.

use kurbo::Affine;

use crate::{Color, Mesh, Shape, TessellationError, tessellate_smooth};

/// Номер фигуры в [`ShapeLibrary`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShapeId(pub u32);

struct Entry {
    shape: Shape,
    mesh: Mesh,
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

    /// Добавляет фигуру и сразу разбивает её на треугольники. Обводки получают запас под
    /// сглаживание в шейдере: [`Self::aa_margin`].
    pub fn add(&mut self, shape: Shape) -> Result<ShapeId, TessellationError> {
        let mesh = tessellate_smooth(&shape, self.tolerance, self.aa_margin())?;
        self.entries.push(Entry { shape, mesh });
        self.revision += 1;
        Ok(ShapeId(self.entries.len() as u32 - 1))
    }

    /// Фигура по номеру.
    pub fn shape(&self, id: ShapeId) -> Option<&Shape> {
        self.entries.get(id.0 as usize).map(|e| &e.shape)
    }

    /// Треугольники фигуры по номеру.
    pub fn mesh(&self, id: ShapeId) -> Option<&Mesh> {
        self.entries.get(id.0 as usize).map(|e| &e.mesh)
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
}

impl Instance {
    /// Фигура `shape` с преобразованием `transform` без оттенка.
    pub fn new(shape: ShapeId, transform: Affine) -> Self {
        Self {
            shape,
            transform,
            tint: Color::WHITE,
        }
    }

    /// С оттенком.
    pub fn with_tint(self, tint: Color) -> Self {
        Self { tint, ..self }
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
    }
}
