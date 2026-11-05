//! Несколько предметов сразу (Р.8): выравнивание по краю или середине, ровные промежутки,
//! линейка в сцене. Считается по границам из [`geom`](super::geom), пишется правкой `at`.

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, Ui, pos2, vec2};

use super::geom::{Bounds, Xform};
use super::theme::*;
use super::viewport::View;

/// Как выровнять выбранные.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Align {
    Left,
    CenterX,
    Right,
    Top,
    CenterY,
    Bottom,
    /// Ровные промежутки по ширине.
    SpreadX,
    /// Ровные промежутки по высоте.
    SpreadY,
}

impl Align {
    pub const ALL: [Align; 8] = [
        Align::Left,
        Align::CenterX,
        Align::Right,
        Align::Top,
        Align::CenterY,
        Align::Bottom,
        Align::SpreadX,
        Align::SpreadY,
    ];

    fn hint(self) -> &'static str {
        match self {
            Align::Left => "По левому краю",
            Align::CenterX => "По середине слева направо",
            Align::Right => "По правому краю",
            Align::Top => "По верхнему краю",
            Align::CenterY => "По середине сверху вниз",
            Align::Bottom => "По нижнему краю",
            Align::SpreadX => "Ровные промежутки по ширине",
            Align::SpreadY => "Ровные промежутки по высоте",
        }
    }
}

/// Предмет для выравнивания: границы в сцене, перевод из родителя, своё `at`.
pub(crate) struct Item {
    pub path: Vec<usize>,
    pub bounds: Bounds,
    pub parent: Xform,
    pub at: [f64; 2],
}

/// Только верхние из выбранных: вложенный в выбранного поедет вместе с ним.
pub(crate) fn roots(paths: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = paths
        .iter()
        .filter(|p| !paths.iter().any(|q| q.len() < p.len() && p.starts_with(q)))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Новые `at` для выравнивания `how`. Предметы, которые не сдвигаются, не попадают.
pub(crate) fn arrange(items: &[Item], how: Align) -> Vec<(Vec<usize>, [f64; 2])> {
    if items.len() < 2 {
        return Vec::new();
    }
    let all = items
        .iter()
        .map(|i| i.bounds)
        .reduce(Bounds::union)
        .expect("не пусто");
    // Сдвиг каждого в координатах сцены.
    let mut shifts: Vec<[f64; 2]> = items
        .iter()
        .map(|i| {
            let b = i.bounds;
            let c = b.center();
            match how {
                Align::Left => [all.min[0] - b.min[0], 0.0],
                Align::CenterX => [all.center()[0] - c[0], 0.0],
                Align::Right => [all.max[0] - b.max[0], 0.0],
                Align::Top => [0.0, all.max[1] - b.max[1]],
                Align::CenterY => [0.0, all.center()[1] - c[1]],
                Align::Bottom => [0.0, all.min[1] - b.min[1]],
                Align::SpreadX | Align::SpreadY => [0.0, 0.0],
            }
        })
        .collect();
    if let Some(k) = match how {
        Align::SpreadX => Some(0),
        Align::SpreadY => Some(1),
        _ => None,
    } {
        // Крайние стоят, между ними — одинаковые промежутки.
        let mut order: Vec<usize> = (0..items.len()).collect();
        order.sort_by(|&a, &b| items[a].bounds.min[k].total_cmp(&items[b].bounds.min[k]));
        let size = |i: usize| items[i].bounds.max[k] - items[i].bounds.min[k];
        let total: f64 = order.iter().map(|&i| size(i)).sum();
        let span = all.max[k] - all.min[k];
        let gap = (span - total) / (items.len() - 1) as f64;
        let mut x = all.min[k];
        for &i in &order {
            shifts[i][k] = x - items[i].bounds.min[k];
            x += size(i) + gap;
        }
    }
    items
        .iter()
        .zip(shifts)
        .filter(|(_, s)| s[0].abs() > 1e-9 || s[1].abs() > 1e-9)
        .map(|(i, s)| {
            let world = i.parent.apply(i.at);
            let moved = [world[0] + s[0], world[1] + s[1]];
            let local = i.parent.inverse().map_or(moved, |inv| inv.apply(moved));
            let round = |v: f64| (v * 10000.0).round() / 10000.0;
            (i.path.clone(), [round(local[0]), round(local[1])])
        })
        .collect()
}

/// Что нажали на полосе над сценой.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BarPick {
    Align(Align),
    Group,
}

/// Полоса выравнивания для нескольких выбранных: значки и «Сгруппировать».
pub(crate) fn bar(ui: &mut Ui, count: usize) -> Option<BarPick> {
    let mut pick = None;
    egui::Frame::NONE
        .fill(PANEL)
        .stroke(Stroke::new(LINE, BORDER_STRONG))
        .corner_radius(RADIUS_M)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP_XS;
                ui.label(
                    egui::RichText::new(format!("выбрано {count}"))
                        .size(FONT_SMALL)
                        .color(TEXT_DIM),
                );
                ui.add_space(GAP_S);
                for (k, a) in Align::ALL.into_iter().enumerate() {
                    if k == 3 || k == 6 {
                        ui.add(egui::Separator::default().vertical().spacing(GAP_S));
                    }
                    let (r, resp) = ui.allocate_exact_size(vec2(24.0, 22.0), egui::Sense::click());
                    let hot = resp.hovered();
                    if hot {
                        ui.painter().rect_filled(r, RADIUS_S, HOVER);
                    }
                    paint_align(
                        ui.painter(),
                        r.center(),
                        a,
                        if hot { Color32::WHITE } else { TEXT_DIM },
                    );
                    if resp.on_hover_text(a.hint()).clicked() {
                        pick = Some(BarPick::Align(a));
                    }
                }
                ui.add(egui::Separator::default().vertical().spacing(GAP_S));
                if ui
                    .button(egui::RichText::new("Сгруппировать").size(FONT_SMALL))
                    .on_hover_text("Вложить выбранные в новый предмет (Ctrl+G)")
                    .clicked()
                {
                    pick = Some(BarPick::Group);
                }
            });
        });
    pick
}

/// Значок выравнивания: линия края и две полоски разной длины.
fn paint_align(p: &Painter, c: Pos2, a: Align, color: Color32) {
    let s = Stroke::new(1.4, color);
    let bar = |r: Rect| p.rect_filled(r, 1, color);
    match a {
        Align::Left | Align::CenterX | Align::Right => {
            let x = match a {
                Align::Left => c.x - 6.0,
                Align::CenterX => c.x,
                _ => c.x + 6.0,
            };
            p.line_segment([pos2(x, c.y - 7.0), pos2(x, c.y + 7.0)], s);
            for (dy, w) in [(-3.0, 10.0), (3.5, 6.0)] {
                let left = match a {
                    Align::Left => x + 1.5,
                    Align::CenterX => x - w / 2.0,
                    _ => x - 1.5 - w,
                };
                bar(Rect::from_min_size(
                    pos2(left, c.y + dy - 1.5),
                    vec2(w, 3.0),
                ));
            }
        }
        Align::Top | Align::CenterY | Align::Bottom => {
            let y = match a {
                Align::Top => c.y - 6.0,
                Align::CenterY => c.y,
                _ => c.y + 6.0,
            };
            p.line_segment([pos2(c.x - 7.0, y), pos2(c.x + 7.0, y)], s);
            for (dx, h) in [(-3.0, 10.0), (3.5, 6.0)] {
                let top = match a {
                    Align::Top => y + 1.5,
                    Align::CenterY => y - h / 2.0,
                    _ => y - 1.5 - h,
                };
                bar(Rect::from_min_size(pos2(c.x + dx - 1.5, top), vec2(3.0, h)));
            }
        }
        Align::SpreadX => {
            for dx in [-6.0, 0.0, 6.0] {
                bar(Rect::from_center_size(pos2(c.x + dx, c.y), vec2(3.0, 10.0)));
            }
        }
        Align::SpreadY => {
            for dy in [-6.0, 0.0, 6.0] {
                bar(Rect::from_center_size(pos2(c.x, c.y + dy), vec2(10.0, 3.0)));
            }
        }
    }
}

/// Линейка: отрезок от `a` до `b` (точки сцены), длина и угол у середины.
pub(crate) fn draw_ruler(p: &Painter, view: &View, a: [f64; 2], b: [f64; 2]) {
    let (pa, pb) = (view.to_ui(a), view.to_ui(b));
    let s = Stroke::new(LINE_FOCUS, ACCENT);
    p.line_segment([pa, pb], s);
    // Засечки поперёк концов.
    let d = pb - pa;
    let n = if d.length() > 0.5 {
        vec2(-d.y, d.x).normalized() * 6.0
    } else {
        vec2(0.0, 6.0)
    };
    for q in [pa, pb] {
        p.line_segment([q - n, q + n], s);
    }
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    let angle = (b[1] - a[1]).atan2(b[0] - a[0]).to_degrees();
    let text = format!(
        "{len:.3}  ·  Δx {:.3}  Δy {:.3}  ·  {angle:.0}°",
        b[0] - a[0],
        b[1] - a[1]
    );
    let at = pa + d * 0.5 + vec2(0.0, -14.0);
    let galley = p.layout_no_wrap(text, FontId::monospace(FONT_SMALL), Color32::WHITE);
    let r = Align2::CENTER_CENTER
        .anchor_size(at, galley.size())
        .expand2(vec2(6.0, 3.0));
    p.rect_filled(r, RADIUS_S, PANEL);
    p.galley(r.shrink2(vec2(6.0, 3.0)).min, galley, Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(i: usize, min: [f64; 2], max: [f64; 2]) -> Item {
        let b = Bounds { min, max };
        Item {
            path: vec![i],
            bounds: b,
            parent: Xform::IDENTITY,
            at: b.center(),
        }
    }

    #[test]
    fn align_and_spread() {
        let items = [
            item(0, [0.0, 0.0], [0.2, 0.2]),
            item(1, [0.5, 0.3], [0.6, 0.4]),
            item(2, [1.0, -0.1], [1.4, 0.1]),
        ];
        let left = arrange(&items, Align::Left);
        assert_eq!(left, [(vec![1], [0.05, 0.35]), (vec![2], [0.2, 0.0])]);
        let top = arrange(&items, Align::Top);
        assert_eq!(top[0], (vec![0], [0.1, 0.3]));
        // Промежутки: 0.2..0.5 и 0.6..1.0 → оба (1.4 − 0.7) / 2 = 0.35.
        let spread = arrange(&items, Align::SpreadX);
        assert_eq!(spread, [(vec![1], [0.6, 0.35])]);
        // Родитель повёрнут на 90°: сдвиг сцены вправо — это `at` вниз по его оси y.
        let mut rotated = item(1, [0.5, 0.3], [0.6, 0.4]);
        rotated.parent = Xform::placement([0.0, 0.0], 90.0, 1.0);
        rotated.at = [0.35, -0.55];
        let r = arrange(&[item(0, [0.0, 0.0], [0.2, 0.2]), rotated], Align::Left);
        let [x, y] = r[0].1;
        assert!(
            (x - 0.35).abs() < 1e-6 && (y + 0.05).abs() < 1e-6,
            "{x} {y}"
        );
    }

    #[test]
    fn nested_follow_parent() {
        let r = roots(&[vec![2, 1], vec![2], vec![0], vec![0]]);
        assert_eq!(r, [vec![0], vec![2]]);
    }
}
