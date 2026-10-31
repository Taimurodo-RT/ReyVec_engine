//! `reyvec-vector`: векторный слой ReyVec без видеокарты.
//!
//! Здесь живёт описание фигур и всё, что с ними делает процессор:
//!
//! - [`Shape`]: контур из прямых и кривых Безье ([`BezPath`] из `kurbo`), заливка или
//!   обводка и цвет;
//! - [`tessellate`]: разбиение фигуры на треугольники (библиотека `lyon`). Делается один
//!   раз, когда фигура появляется, а не каждый кадр;
//! - [`ShapeLibrary`]: набор фигур с готовыми треугольниками;
//! - [`Instance`]: фигура на сцене в этом кадре (где стоит, как повёрнута, оттенок).
//!
//! Каждый кадр процессор только составляет список [`Instance`]. Двигать, вращать и
//! рисовать фигуры будет видеокарта (`reyvec-gpu`). Как именно она их рисует, этот крейт
//! не знает: описание фигуры не зависит от способа рисования, поэтому способы можно
//! добавлять и менять (см. `docs/decisions/0009-vector-layer.md`).

mod color;
mod library;
mod shape;
mod tessellate;

pub use color::Color;
/// Библиотека кривых целиком: прямоугольники, круги, эллипсы и прочее (`kurbo::Shape::to_path`).
pub use kurbo;
pub use kurbo::{Affine, BezPath, Circle, PathEl, Point, Rect, Vec2};
pub use library::{Instance, ShapeId, ShapeLibrary};
pub use shape::{FillRule, LineCap, LineJoin, Shape, Stroke, Style};
pub use tessellate::{EdgeData, Mesh, TessellationError, tessellate, tessellate_smooth};
