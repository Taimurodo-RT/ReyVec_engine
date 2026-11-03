//! Уровень из тайлов (6.4): клетки, нарисованные буквами в файле сцены.
//!
//! ```text
//! tiles: (cell: 0.1, color: "#45403b", edge: "#8a7a62", rows: [
//!     "..........",
//!     "....==....",
//!     "../##\..##",
//!     "##########",
//! ])
//! ```
//!
//! `#` — сплошной блок, `/` и `\` — склоны по 45°, `=` — доска (держит только сверху),
//! `.` или пробел — пусто. Левый верхний угол сетки — место предмета, строки идут вниз.
//!
//! Из клеток получаются векторные фигуры и твёрдое для персонажа. Соседние блоки
//! сливаются в большие прямоугольники: и фигур, и твёрдых прямоугольников во много раз
//! меньше, чем клеток, а персонаж не цепляется за швы между клетками. Блоки под склоном
//! становятся частью склона (столбик до низа), иначе их углы торчали бы из склона
//! ступеньками.

use serde::Deserialize;

use crate::{Hex, ShapeDesc};

/// Описание уровня из тайлов в файле сцены.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TilesDesc {
    /// Размер клетки.
    pub cell: f64,
    /// Цвет блоков и склонов.
    pub color: Hex,
    /// Цвет кромки поверху (камень, мох); без него кромки нет.
    #[serde(default)]
    pub edge: Option<Hex>,
    /// Цвет досок `=`; без него — как у блоков.
    #[serde(default)]
    pub plank: Option<Hex>,
    /// Строки клеток сверху вниз.
    pub rows: Vec<String>,
}

/// Самая большая сетка: строк × столбцов.
const MAX_CELLS: usize = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cell {
    Empty,
    Block,
    Plank,
    Up,
    Down,
}

impl Cell {
    fn slope(self) -> bool {
        matches!(self, Self::Up | Self::Down)
    }
}

/// Твёрдое из тайлов: середина, половины сторон, доска ли и подъём склона (0 — не склон).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TileSolid {
    pub at: [f64; 2],
    pub half: [f64; 2],
    pub oneway: bool,
    pub slope: f64,
}

/// Готовый уровень: фигуры и твёрдое в координатах предмета.
#[derive(Debug, Default)]
pub(crate) struct TileLevel {
    pub shapes: Vec<ShapeDesc>,
    pub solids: Vec<TileSolid>,
}

impl TilesDesc {
    pub(crate) fn check(&self) -> Result<(), String> {
        self.grid().map(|_| ())
    }

    fn grid(&self) -> Result<Vec<Vec<Cell>>, String> {
        if !(self.cell.is_finite() && self.cell > 0.0 && self.cell <= 100.0) {
            return Err(format!(
                "cell {} — нужно больше 0 и не больше 100",
                self.cell
            ));
        }
        if self.rows.is_empty() {
            return Err("rows пустой — нужна хотя бы одна строка клеток".into());
        }
        let mut total = 0;
        let mut grid = Vec::with_capacity(self.rows.len());
        for (r, row) in self.rows.iter().enumerate() {
            let mut cells = Vec::new();
            for (c, ch) in row.chars().enumerate() {
                cells.push(match ch {
                    '#' => Cell::Block,
                    '=' => Cell::Plank,
                    '/' => Cell::Up,
                    '\\' => Cell::Down,
                    '.' | ' ' => Cell::Empty,
                    other => {
                        return Err(format!(
                            "строка {}, клетка {}: «{other}» — бывают # (блок), / и \\ (склоны), \
                             = (доска), . или пробел (пусто)",
                            r + 1,
                            c + 1
                        ));
                    }
                });
            }
            total += cells.len();
            if total > MAX_CELLS {
                return Err(format!("больше {MAX_CELLS} клеток"));
            }
            grid.push(cells);
        }
        Ok(grid)
    }

    /// Фигуры и твёрдое уровня.
    pub(crate) fn build(&self) -> Result<TileLevel, String> {
        let grid = self.grid()?;
        let at = |r: usize, c: usize| {
            grid.get(r)
                .and_then(|row| row.get(c))
                .copied()
                .unwrap_or(Cell::Empty)
        };
        let rows = grid.len();
        let cols = grid.iter().map(Vec::len).max().unwrap_or(0);
        let s = self.cell;
        // Клетка (r, c): x от c·s до (c+1)·s, y от −(r+1)·s до −r·s.
        let rect = |r0: usize, c0: usize, r1: usize, c1: usize| -> ([f64; 2], [f64; 2]) {
            let (x0, x1) = (c0 as f64 * s, c1 as f64 * s);
            let (y1, y0) = (-(r0 as f64) * s, -(r1 as f64) * s);
            (
                [(x0 + x1) * 0.5, (y0 + y1) * 0.5],
                [(x1 - x0) * 0.5, (y1 - y0) * 0.5],
            )
        };

        // Блоки под склонами: часть склона, а не отдельное твёрдое.
        let mut under = vec![vec![false; cols]; rows];
        for r in 1..rows {
            let (done, rest) = under.split_at_mut(r);
            let above = &done[r - 1];
            for (c, u) in rest[0].iter_mut().enumerate() {
                *u = at(r, c) == Cell::Block && (at(r - 1, c).slope() || above[c]);
            }
        }
        let is_under = |r: usize, c: usize| under.get(r).and_then(|row| row.get(c)) == Some(&true);

        let mut level = TileLevel::default();
        let color = self.color;
        // Фигуры блоков: все блоки, слитые в прямоугольники.
        for (r0, c0, r1, c1) in merge(rows, cols, |r, c| at(r, c) == Cell::Block) {
            let (center, half) = rect(r0, c0, r1, c1);
            level.shapes.push(ShapeDesc::Rect {
                at: (center[0], center[1]),
                size: (half[0] * 2.0, half[1] * 2.0),
                angle: 0.0,
                corner: 0.0,
                color,
                stroke: None,
                solid: false,
                oneway: false,
            });
        }
        // Твёрдое блоков: без тех, что под склонами.
        for (r0, c0, r1, c1) in merge(rows, cols, |r, c| {
            at(r, c) == Cell::Block && !is_under(r, c)
        }) {
            let (center, half) = rect(r0, c0, r1, c1);
            level.solids.push(TileSolid {
                at: center,
                half,
                oneway: false,
                slope: 0.0,
            });
        }
        // Склоны: треугольник и твёрдый столбик до низа блоков под ним.
        for r in 0..rows {
            for c in 0..cols {
                let cell = at(r, c);
                if !cell.slope() {
                    continue;
                }
                let (x0, y0) = (c as f64 * s, -((r + 1) as f64) * s);
                // Точки от угла клетки: все склоны одного направления — одна форма.
                let tip = if cell == Cell::Up { (s, s) } else { (0.0, s) };
                level.shapes.push(ShapeDesc::Polygon {
                    points: vec![(0.0, 0.0), (s, 0.0), tip],
                    at: (x0, y0),
                    angle: 0.0,
                    color,
                    stroke: None,
                });
                let mut bottom = r + 1;
                while is_under(bottom, c) {
                    bottom += 1;
                }
                let (center, half) = rect(r, c, bottom, c + 1);
                level.solids.push(TileSolid {
                    at: center,
                    half,
                    oneway: false,
                    slope: if cell == Cell::Up { s } else { -s },
                });
            }
        }
        // Доски: подряд в строке — одна доска у верха клеток.
        let plank = self.plank.unwrap_or(color);
        let thick = s * 0.25;
        for (r, c0, c1) in runs(rows, cols, |r, c| at(r, c) == Cell::Plank) {
            let (x0, x1) = (c0 as f64 * s, c1 as f64 * s);
            let top = -(r as f64) * s;
            let center = [(x0 + x1) * 0.5, top - thick * 0.5];
            let half = [(x1 - x0) * 0.5, thick * 0.5];
            level.shapes.push(ShapeDesc::Rect {
                at: (center[0], center[1]),
                size: (half[0] * 2.0, thick),
                angle: 0.0,
                corner: thick * 0.3,
                color: plank,
                stroke: None,
                solid: false,
                oneway: false,
            });
            level.solids.push(TileSolid {
                at: center,
                half,
                oneway: true,
                slope: 0.0,
            });
        }
        // Кромка по открытому верху блоков и по склонам.
        if let Some(edge) = self.edge {
            let width = s * 0.15;
            let open =
                |r: usize, c: usize| r == 0 || matches!(at(r - 1, c), Cell::Empty | Cell::Plank);
            // Точки от начала линии: одинаковые кромки — одна форма.
            let mut line = |from: (f64, f64), to: (f64, f64)| {
                level.shapes.push(ShapeDesc::Line {
                    points: vec![(0.0, 0.0), (to.0 - from.0, to.1 - from.1)],
                    width,
                    at: from,
                    angle: 0.0,
                    color: edge,
                });
            };
            for (r, c0, c1) in runs(rows, cols, |r, c| at(r, c) == Cell::Block && open(r, c)) {
                let y = -(r as f64) * s;
                line((c0 as f64 * s, y), (c1 as f64 * s, y));
            }
            for r in 0..rows {
                for c in 0..cols {
                    let (x0, x1) = (c as f64 * s, (c + 1) as f64 * s);
                    let (y0, y1) = (-((r + 1) as f64) * s, -(r as f64) * s);
                    match at(r, c) {
                        Cell::Up => line((x0, y0), (x1, y1)),
                        Cell::Down => line((x0, y1), (x1, y0)),
                        _ => {}
                    }
                }
            }
        }
        Ok(level)
    }
}

/// Подряд идущие клетки в строках: (строка, первый столбец, столбец после последнего).
fn runs(
    rows: usize,
    cols: usize,
    take: impl Fn(usize, usize) -> bool,
) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    for r in 0..rows {
        let mut c = 0;
        while c < cols {
            if take(r, c) {
                let c0 = c;
                while c < cols && take(r, c) {
                    c += 1;
                }
                out.push((r, c0, c));
            } else {
                c += 1;
            }
        }
    }
    out
}

/// Клетки, слитые в прямоугольники: сначала вправо, потом вниз, пока вся полоса той же
/// ширины годится. Возвращает (строка, столбец, строка после, столбец после).
fn merge(
    rows: usize,
    cols: usize,
    take: impl Fn(usize, usize) -> bool,
) -> Vec<(usize, usize, usize, usize)> {
    let mut used = vec![vec![false; cols]; rows];
    let mut out = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            if used[r][c] || !take(r, c) {
                continue;
            }
            let mut c1 = c;
            while c1 < cols && !used[r][c1] && take(r, c1) {
                c1 += 1;
            }
            let mut r1 = r + 1;
            while r1 < rows && (c..c1).all(|k| !used[r1][k] && take(r1, k)) {
                r1 += 1;
            }
            for row in used.iter_mut().take(r1).skip(r) {
                for u in row.iter_mut().take(c1).skip(c) {
                    *u = true;
                }
            }
            out.push((r, c, r1, c1));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(rows: &[&str]) -> TilesDesc {
        TilesDesc {
            cell: 0.1,
            color: Hex::parse("#445566").unwrap(),
            edge: Some(Hex::parse("#887766").unwrap()),
            plank: None,
            rows: rows.iter().map(|r| (*r).to_string()).collect(),
        }
    }

    #[test]
    fn blocks_merge_into_few_rectangles() {
        let level = desc(&["....", "####", "####"]).build().unwrap();
        assert_eq!(level.solids.len(), 1);
        let s = level.solids[0];
        assert!((s.at[0] - 0.2).abs() < 1e-12 && (s.at[1] + 0.2).abs() < 1e-12);
        assert!((s.half[0] - 0.2).abs() < 1e-12 && (s.half[1] - 0.1).abs() < 1e-12);
        // Прямоугольник и кромка поверху.
        assert_eq!(level.shapes.len(), 2);
    }

    #[test]
    fn slopes_swallow_blocks_beneath() {
        let level = desc(&["../.", "./##", "/###", "####"]).build().unwrap();
        let slopes: Vec<_> = level.solids.iter().filter(|s| s.slope != 0.0).collect();
        assert_eq!(slopes.len(), 3);
        // Склон в строке 0 (столбец 2): столбик до самого низа.
        let tall = slopes
            .iter()
            .find(|s| (s.at[0] - 0.25).abs() < 1e-12)
            .unwrap();
        assert!((tall.half[1] - 0.2).abs() < 1e-12, "{tall:?}");
        assert!((tall.at[1] + 0.2).abs() < 1e-12, "{tall:?}");
        assert!((tall.slope - 0.1).abs() < 1e-12);
        // Твёрдый блок остался только справа от склонов.
        let blocks: Vec<_> = level.solids.iter().filter(|s| s.slope == 0.0).collect();
        assert_eq!(blocks.len(), 1);
        assert!((blocks[0].at[0] - 0.35).abs() < 1e-12, "{:?}", blocks[0]);
    }

    #[test]
    fn planks_are_thin_and_oneway() {
        let level = desc(&["==.==="]).build().unwrap();
        let planks: Vec<_> = level.solids.iter().filter(|s| s.oneway).collect();
        assert_eq!(planks.len(), 2);
        assert!((planks[1].half[0] - 0.15).abs() < 1e-12);
        assert!((planks[1].at[1] + planks[1].half[1]).abs() < 1e-12);
    }

    #[test]
    fn bad_cell_is_named() {
        let err = desc(&["##", "#x"]).check().unwrap_err();
        assert!(err.contains("строка 2, клетка 2"), "{err}");
    }
}
