//! Что показывать поверх сцены (Р.6): режимы просмотра «контуры», «физика», «слои
//! глубины» и метки — границы сцены, радиусы света, кости, места появления, значки
//! невидимых предметов, сводка. Всё рисует egui поверх кадра движка по данным файла
//! сцены, так что работает на любой видеокарте.

use std::collections::HashSet;

use egui::{Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, pos2, vec2};
use reyvec_scene::{LightDesc, NodeDesc, PartDesc, SceneFile, ShapeDesc};

use super::geom::{Bounds, Placed, Xform};
use super::outline;
use super::theme::*;
use super::viewport::View;

/// Как показывать сцену.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewMode {
    /// Как в игре: свет, тени, свечение.
    Lit,
    /// Только контуры фигур поверх затемнённой сцены.
    Outlines,
    /// Тела, опоры и зоны касания, как их видит физика.
    Physics,
    /// Предметы раскрашены по глубине: ближе к зрителю — теплее.
    Depth,
}

impl ViewMode {
    pub const ALL: [ViewMode; 4] = [
        ViewMode::Lit,
        ViewMode::Outlines,
        ViewMode::Physics,
        ViewMode::Depth,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Lit => "Со светом",
            ViewMode::Outlines => "Контуры",
            ViewMode::Physics => "Физика",
            ViewMode::Depth => "Слои глубины",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            ViewMode::Lit => "Как в игре: свет, тени, свечение",
            ViewMode::Outlines => "Контуры всех фигур: видно, где что лежит и что чем закрыто",
            ViewMode::Physics => "Тела, опоры и зоны касания: то, обо что ударяется герой",
            ViewMode::Depth => "Цвет по глубине: синие — дальше, оранжевые — ближе к зрителю",
        }
    }
}

/// Метки поверх сцены; каждую можно выключить в меню «Показать».
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Show {
    pub bounds: bool,
    pub lights: bool,
    pub bones: bool,
    pub spawns: bool,
    pub icons: bool,
    pub stats: bool,
    /// Карта всей сцены в углу: где камера, щелчок — перелететь.
    pub map: bool,
}

impl Default for Show {
    fn default() -> Self {
        Self {
            bounds: true,
            lights: false,
            bones: false,
            spawns: true,
            icons: true,
            stats: false,
            map: true,
        }
    }
}

impl Show {
    /// Пункты меню: название и флажок.
    pub fn items(&mut self) -> [(&'static str, &mut bool); 7] {
        [
            ("Границы сцены", &mut self.bounds),
            ("Радиусы света", &mut self.lights),
            ("Кости героя", &mut self.bones),
            ("Места появления", &mut self.spawns),
            ("Значки невидимых предметов", &mut self.icons),
            ("Сводка в углу", &mut self.stats),
            ("Карта сцены", &mut self.map),
        ]
    }
}

/// Карта всей сцены в правом нижнем углу окна сцены: верхние предметы рамками, выбранный
/// цветом, видимая часть — белой рамкой. Возвращает точку сцены, куда щёлкнули или тянут.
pub(crate) fn minimap(
    ui: &mut egui::Ui,
    rect: Rect,
    view: View,
    placed: &[Placed],
    selected: Option<&[usize]>,
) -> Option<[f64; 2]> {
    let top: Vec<&Placed> = placed.iter().filter(|p| p.path.len() == 1).collect();
    let world = top.iter().map(|p| p.all).reduce(Bounds::union)?;
    let cam = Bounds {
        min: view.to_scene(egui::pos2(rect.left(), rect.bottom())),
        max: view.to_scene(egui::pos2(rect.right(), rect.top())),
    };
    let world = world.union(cam);
    let size = world.size();
    if size[0] <= 0.0 || size[1] <= 0.0 {
        return None;
    }
    let w = 200.0_f32;
    let h = (w * (size[1] / size[0]) as f32).clamp(60.0, 150.0);
    let map = Rect::from_min_size(
        egui::pos2(rect.right() - w - 12.0, rect.bottom() - h - 12.0),
        egui::vec2(w, h),
    );
    if map.top() < rect.top() + 40.0 {
        return None;
    }
    let inner = map.shrink(6.0);
    // Масштаб один по обеим осям: пропорции сцены не искажаются.
    let k = (f64::from(inner.width()) / size[0]).min(f64::from(inner.height()) / size[1]);
    let c = world.center();
    let to = |p: [f64; 2]| {
        inner.center() + egui::vec2(((p[0] - c[0]) * k) as f32, (-(p[1] - c[1]) * k) as f32)
    };
    let from = |q: egui::Pos2| {
        let d = q - inner.center();
        [c[0] + f64::from(d.x) / k, c[1] - f64::from(d.y) / k]
    };
    let painter = ui.painter_at(map);
    painter.rect(
        map,
        RADIUS_M,
        PANEL.gamma_multiply(0.92),
        Stroke::new(LINE, BORDER_STRONG),
        egui::StrokeKind::Inside,
    );
    for p in &top {
        let on = selected.is_some_and(|s| s.first() == p.path.first());
        let r = Rect::from_two_pos(to(p.all.min), to(p.all.max));
        let color = if on { ACCENT } else { MUTED };
        painter.rect_stroke(r, 0, Stroke::new(LINE, color), egui::StrokeKind::Middle);
    }
    let cr = Rect::from_two_pos(to(cam.min), to(cam.max));
    painter.rect_stroke(
        cr,
        0,
        Stroke::new(LINE_FOCUS, egui::Color32::WHITE),
        egui::StrokeKind::Middle,
    );
    let resp = ui
        .interact(
            map,
            ui.scope_id().with("minimap"),
            egui::Sense::click_and_drag(),
        )
        .on_hover_text("Карта сцены: щёлкните или тяните, чтобы перелететь камерой");
    if resp.clicked() || resp.dragged() {
        return resp.interact_pointer_pos().map(from);
    }
    None
}

/// Закладки камеры: Ctrl+1…9 — запомнить, 1…9 — вернуться.
pub(crate) type Bookmarks = [Option<([f64; 2], f64)>; 9];

/// Состояние надстроек окна сцены.
#[derive(Debug)]
pub(crate) struct Overlay {
    pub mode: ViewMode,
    pub show: Show,
    pub bookmarks: Bookmarks,
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            mode: ViewMode::Lit,
            show: Show::default(),
            bookmarks: [None; 9],
        }
    }
}

/// Что нужно для рисования.
pub(crate) struct Ctx<'a> {
    pub painter: &'a egui::Painter,
    pub view: View,
    pub rect: Rect,
    pub file: &'a SceneFile,
    pub placed: &'a [Placed],
    pub hidden: &'a HashSet<Vec<usize>>,
}

impl Ctx<'_> {
    fn ui(&self, x: &Xform, p: [f64; 2]) -> Pos2 {
        self.view.to_ui(x.apply(p))
    }

    fn visible(&self) -> impl Iterator<Item = (&Placed, &NodeDesc)> {
        self.placed.iter().filter_map(|p| {
            let hidden = (1..=p.path.len()).any(|k| self.hidden.contains(&p.path[..k]));
            if hidden {
                return None;
            }
            Some((p, outline::node_at(self.file, &p.path)?))
        })
    }

    /// Точки сцены в точки окна, замкнутой линией.
    fn closed(&self, x: &Xform, pts: &[[f64; 2]], stroke: Stroke) {
        let pts: Vec<Pos2> = pts.iter().map(|&p| self.ui(x, p)).collect();
        self.painter.add(Shape::closed_line(pts, stroke));
    }
}

impl Overlay {
    /// Рисует режим просмотра и метки.
    pub fn draw(&self, c: &Ctx) {
        match self.mode {
            ViewMode::Lit => {}
            ViewMode::Outlines => {
                c.painter
                    .rect_filled(c.rect, 0, Color32::from_black_alpha(190));
                outlines(c);
            }
            ViewMode::Physics => {
                c.painter
                    .rect_filled(c.rect, 0, Color32::from_black_alpha(140));
                physics(c);
            }
            ViewMode::Depth => {
                c.painter
                    .rect_filled(c.rect, 0, Color32::from_black_alpha(190));
                depth(c);
            }
        }
        let s = &self.show;
        if s.bounds {
            bounds(c);
        }
        if s.lights {
            lights(c);
        }
        if s.bones {
            bones(c);
        }
        if s.spawns {
            spawns(c);
        }
        if s.icons {
            icons(c);
        }
    }
}

/// Сколько единиц сцены в единице предмета (размер с учётом родителей).
fn scale_of(x: &Xform) -> f64 {
    x.a.hypot(x.b)
}

fn rect_pts(at: (f64, f64), size: (f64, f64), angle: f64) -> Vec<[f64; 2]> {
    let x = Xform::placement([at.0, at.1], angle, 1.0);
    let (w, h) = (size.0 / 2.0, size.1 / 2.0);
    [[-w, -h], [w, -h], [w, h], [-w, h]]
        .map(|p| x.apply(p))
        .to_vec()
}

fn circle_pts(at: (f64, f64), r: f64) -> Vec<[f64; 2]> {
    (0..32)
        .map(|k| {
            let t = k as f64 * std::f64::consts::TAU / 32.0;
            [at.0 + r * t.cos(), at.1 + r * t.sin()]
        })
        .collect()
}

fn capsule_pts(from: (f64, f64), to: (f64, f64), r: f64) -> Vec<[f64; 2]> {
    let a = (to.1 - from.1).atan2(to.0 - from.0);
    let half = |c: (f64, f64), start: f64| {
        (0..=12).map(move |k| {
            let t = start + std::f64::consts::PI * k as f64 / 12.0;
            [c.0 + r * t.cos(), c.1 + r * t.sin()]
        })
    };
    let h = std::f64::consts::FRAC_PI_2;
    half(to, a - h).chain(half(from, a + h)).collect()
}

/// Контуры фигур, SDF-частей и рисунков.
fn outlines(c: &Ctx) {
    let line = Stroke::new(LINE, Color32::from_white_alpha(200));
    let soft = Stroke::new(LINE, Color32::from_rgba_unmultiplied(120, 200, 255, 170));
    for (p, n) in c.visible() {
        let x = &p.xform;
        for s in &n.shapes {
            match s {
                ShapeDesc::Rect {
                    at, size, angle, ..
                } => c.closed(x, &rect_pts(*at, *size, *angle), line),
                ShapeDesc::Circle { at, r, .. } => c.closed(x, &circle_pts(*at, *r), line),
                ShapeDesc::Polygon {
                    points, at, angle, ..
                } => {
                    let px = Xform::placement([at.0, at.1], *angle, 1.0);
                    let pts: Vec<_> = points.iter().map(|q| px.apply([q.0, q.1])).collect();
                    c.closed(x, &pts, line);
                }
                ShapeDesc::Line {
                    points, at, angle, ..
                } => {
                    let px = Xform::placement([at.0, at.1], *angle, 1.0);
                    let pts: Vec<Pos2> = points
                        .iter()
                        .map(|q| c.ui(x, px.apply([q.0, q.1])))
                        .collect();
                    c.painter.add(Shape::line(pts, line));
                }
                ShapeDesc::Path { at, scale, .. } => {
                    let pts = rect_pts(*at, (0.2 * scale, 0.2 * scale), 0.0);
                    let pts: Vec<Pos2> = pts.iter().map(|&q| c.ui(x, q)).collect();
                    let mut closed = pts.clone();
                    closed.push(pts[0]);
                    c.painter
                        .extend(Shape::dashed_line(&closed, line, 4.0, 3.0));
                }
            }
        }
        for o in &n.objects {
            for part in &o.parts {
                match part {
                    PartDesc::Circle { at, r } => {
                        c.closed(x, &circle_pts(*at, f64::from(*r)), soft);
                    }
                    PartDesc::Capsule { from, to, r } => {
                        c.closed(x, &capsule_pts(*from, *to, f64::from(*r)), soft);
                    }
                }
            }
        }
        for pic in &n.pictures {
            c.closed(
                x,
                &rect_pts(pic.at, (pic.height, pic.height), pic.angle),
                soft,
            );
        }
    }
}

/// Опоры, тела, герой, манекены и зоны касания.
fn physics(c: &Ctx) {
    let solid = Stroke::new(1.5, Color32::from_rgb(107, 227, 138));
    let body = Stroke::new(1.5, ACCENT);
    let hero = Stroke::new(1.5, SELECT);
    let dummy = Stroke::new(1.5, BAD);
    let touch = Stroke::new(LINE, Color32::from_rgb(255, 214, 102));
    for (p, n) in c.visible() {
        let x = &p.xform;
        for s in &n.solids {
            let pts = rect_pts(s.at, s.size, 0.0);
            c.closed(x, &pts, solid);
            let fill: Vec<Pos2> = pts.iter().map(|&q| c.ui(x, q)).collect();
            c.painter.add(Shape::convex_polygon(
                fill,
                solid.color.gamma_multiply(0.15),
                Stroke::NONE,
            ));
        }
        if let Some(t) = &n.tiles {
            // Клетки уровня: рамка вокруг всей сетки.
            let cols = t.rows.iter().map(|r| r.chars().count()).max().unwrap_or(0) as f64;
            let rows = t.rows.len() as f64;
            let (w, h) = (cols * t.cell, rows * t.cell);
            c.closed(x, &rect_pts((w / 2.0, -h / 2.0), (w, h), 0.0), solid);
        }
        if let Some(b) = &n.body {
            let pts = if !b.points.is_empty() {
                b.points.iter().map(|q| [q.0, q.1]).collect()
            } else if b.round {
                circle_pts((0.0, 0.0), b.size.0 / 2.0)
            } else if b.capsule {
                let (w, h) = (b.size.0 / 2.0, b.size.1 / 2.0);
                if w > h {
                    capsule_pts((-(w - h), 0.0), (w - h, 0.0), h)
                } else {
                    capsule_pts((0.0, -(h - w)), (0.0, h - w), w)
                }
            } else {
                rect_pts((0.0, 0.0), b.size, 0.0)
            };
            c.closed(x, &pts, body);
            if b.fixed {
                let o = c.ui(x, [0.0, 0.0]);
                c.painter.text(
                    o,
                    Align2::CENTER_CENTER,
                    "опора",
                    FontId::proportional(FONT_SMALL),
                    body.color,
                );
            }
        }
        if let Some(pl) = &n.player {
            c.closed(x, &rect_pts((0.0, 0.0), pl.size, 0.0), hero);
        }
        if let Some(d) = &n.dummy {
            c.closed(x, &rect_pts((0.0, 0.0), d.size, 0.0), dummy);
        }
        if let Some(t) = n.touch {
            let pts: Vec<Pos2> = rect_pts((0.0, 0.0), t, 0.0)
                .iter()
                .map(|&q| c.ui(x, q))
                .collect();
            let mut closed = pts.clone();
            closed.push(pts[0]);
            c.painter
                .extend(Shape::dashed_line(&closed, touch, 5.0, 3.0));
        }
    }
    legend(
        c,
        &[
            ("опоры", solid.color),
            ("тела", body.color),
            ("герой", hero.color),
            ("манекены", dummy.color),
            ("касание", touch.color),
        ],
    );
}

/// Цвет глубины: `t` от 0 (дальше всех) до 1 (ближе всех).
fn depth_color(t: f64) -> Color32 {
    let far = [70.0, 110.0, 255.0];
    let near = [255.0, 165.0, 74.0];
    let m = |k: usize| (far[k] + (near[k] - far[k]) * t.clamp(0.0, 1.0)) as u8;
    Color32::from_rgb(m(0), m(1), m(2))
}

/// Предметы с фигурами раскрашены по глубине.
fn depth(c: &Ctx) {
    let drawn: Vec<_> = c
        .visible()
        .filter(|(_, n)| !n.shapes.is_empty() || !n.pictures.is_empty() || !n.objects.is_empty())
        .collect();
    let (lo, hi) = drawn.iter().fold((f64::MAX, f64::MIN), |(lo, hi), (p, _)| {
        (lo.min(p.depth), hi.max(p.depth))
    });
    let span = (hi - lo).max(1e-6);
    let mut order: Vec<_> = drawn.iter().collect();
    order.sort_by(|a, b| a.0.depth.total_cmp(&b.0.depth));
    for (p, _) in order {
        let t = (p.depth - lo) / span;
        let col = depth_color(t);
        let r = Rect::from_two_pos(c.view.to_ui(p.own.min), c.view.to_ui(p.own.max));
        c.painter.rect(
            r,
            2,
            col.gamma_multiply(0.35),
            Stroke::new(LINE, col),
            egui::StrokeKind::Middle,
        );
    }
    if lo <= hi {
        legend(
            c,
            &[
                (&format!("дальше {lo:.1}"), depth_color(0.0)),
                (&format!("ближе {hi:.1}"), depth_color(1.0)),
            ],
        );
    }
}

/// Подписи цветов в левом нижнем углу окна сцены.
fn legend(c: &Ctx, items: &[(&str, Color32)]) {
    let mut at = pos2(c.rect.left() + 12.0, c.rect.bottom() - 14.0);
    for (text, col) in items {
        let g =
            c.painter
                .layout_no_wrap((*text).to_owned(), FontId::proportional(FONT_SMALL), TEXT);
        let r = Rect::from_min_size(
            at - vec2(0.0, g.size().y + 6.0),
            vec2(g.size().x + 26.0, g.size().y + 8.0),
        );
        c.painter
            .rect_filled(r, RADIUS_S, Color32::from_black_alpha(170));
        c.painter.rect_filled(
            Rect::from_center_size(pos2(r.left() + 10.0, r.center().y), vec2(8.0, 8.0)),
            2,
            *col,
        );
        c.painter.galley(r.min + vec2(20.0, 4.0), g, TEXT);
        at.x = r.right() + 6.0;
    }
}

/// Пунктирная рамка вокруг всей сцены.
fn bounds(c: &Ctx) {
    let all = c
        .placed
        .iter()
        .filter(|p| p.path.len() == 1)
        .map(|p| p.all)
        .reduce(super::geom::Bounds::union);
    let Some(b) = all else {
        return;
    };
    let r = Rect::from_two_pos(c.view.to_ui(b.min), c.view.to_ui(b.max));
    let s = Stroke::new(LINE, ACCENT.gamma_multiply(0.7));
    let pts = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    c.painter.extend(Shape::dashed_line(&pts, s, 8.0, 5.0));
    c.painter.text(
        r.left_top() + vec2(4.0, -4.0),
        Align2::LEFT_BOTTOM,
        format!("сцена {:.2} × {:.2}", b.size()[0], b.size()[1]),
        FontId::proportional(FONT_SMALL),
        s.color,
    );
}

fn light_color(c: &reyvec_scene::Hex) -> Color32 {
    let [r, g, b, _] = c.0.to_srgb_array();
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// Докуда достаёт свет каждого источника.
fn lights(c: &Ctx) {
    for (p, n) in c.visible() {
        let x = &p.xform;
        let k = scale_of(x);
        for l in &n.lights {
            match l {
                LightDesc::Point {
                    at, color, reach, ..
                } => {
                    let col = light_color(color);
                    let o = c.ui(x, [at.0, at.1]);
                    let r = (f64::from(*reach) * k * c.view.points_per_unit()) as f32;
                    let pts: Vec<Pos2> = (0..=48)
                        .map(|i| {
                            let t = i as f32 * std::f32::consts::TAU / 48.0;
                            o + vec2(t.cos(), t.sin()) * r
                        })
                        .collect();
                    c.painter.extend(Shape::dashed_line(
                        &pts,
                        Stroke::new(LINE, col.gamma_multiply(0.8)),
                        6.0,
                        4.0,
                    ));
                    sun_icon(c.painter, o, col);
                }
                LightDesc::Tube {
                    from,
                    to,
                    color,
                    reach,
                    ..
                } => {
                    let col = light_color(color);
                    let pts = capsule_pts(*from, *to, f64::from(*reach));
                    let pts: Vec<Pos2> = pts.iter().map(|&q| c.ui(x, q)).collect();
                    let mut closed = pts.clone();
                    closed.push(pts[0]);
                    c.painter.extend(Shape::dashed_line(
                        &closed,
                        Stroke::new(LINE, col.gamma_multiply(0.8)),
                        6.0,
                        4.0,
                    ));
                    c.painter.line_segment(
                        [c.ui(x, [from.0, from.1]), c.ui(x, [to.0, to.1])],
                        Stroke::new(2.0, col),
                    );
                }
                LightDesc::Sun { .. } => {}
            }
        }
    }
}

fn sun_icon(p: &egui::Painter, o: Pos2, col: Color32) {
    p.circle_filled(o, 4.0, col);
    for k in 0..8 {
        let t = k as f32 * std::f32::consts::FRAC_PI_4;
        let d = vec2(t.cos(), t.sin());
        p.line_segment([o + d * 6.0, o + d * 9.0], Stroke::new(1.2, col));
    }
}

/// Кости: от родителя к точке крепления.
fn bones(c: &Ctx) {
    let s = Stroke::new(2.0, Color32::from_rgb(200, 160, 255));
    for (p, n) in c.visible() {
        if n.bone.is_none() {
            continue;
        }
        let from = c.view.to_ui(p.parent.apply([0.0, 0.0]));
        let to = c.view.to_ui(p.xform.apply([0.0, 0.0]));
        c.painter.line_segment([from, to], s);
        c.painter.circle_filled(to, 3.5, s.color);
        c.painter.circle_stroke(from, 3.5, s);
    }
}

/// Флажки: откуда начинает герой, где появляются манекены и тела.
fn spawns(c: &Ctx) {
    for (p, n) in c.visible() {
        let (text, col) = if n.player.is_some() {
            ("старт героя", SELECT)
        } else if n.dummy.is_some() {
            ("манекен", BAD)
        } else if n.spawn.is_some() {
            ("выброс тел", ACCENT)
        } else {
            continue;
        };
        let base = c.view.to_ui(p.own.center()) + vec2(0.0, 0.0);
        let foot = pos2(base.x, c.view.to_ui(p.own.min).y);
        let top = foot - vec2(0.0, 26.0);
        c.painter.line_segment([foot, top], Stroke::new(1.5, col));
        c.painter.add(Shape::convex_polygon(
            vec![top, top + vec2(12.0, 4.0), top + vec2(0.0, 8.0)],
            col,
            Stroke::NONE,
        ));
        c.painter.text(
            top + vec2(15.0, 4.0),
            Align2::LEFT_CENTER,
            text,
            FontId::proportional(FONT_SMALL),
            col,
        );
    }
}

/// Значки предметов, у которых нет ничего видимого: свет, скрипт, пустой.
fn icons(c: &Ctx) {
    for (p, n) in c.visible() {
        let seen = !n.shapes.is_empty()
            || !n.pictures.is_empty()
            || !n.morphs.is_empty()
            || !n.objects.is_empty()
            || n.tiles.is_some()
            || n.player.is_some()
            || n.dummy.is_some()
            || n.bone.is_some()
            || !n.children.is_empty();
        if seen {
            continue;
        }
        let o = c.view.to_ui(p.xform.apply([0.0, 0.0]));
        if !c.rect.contains(o) {
            continue;
        }
        let (mark, col) = if !n.lights.is_empty() {
            ("☀", Color32::from_rgb(255, 214, 102))
        } else if n.script.is_some() {
            ("{}", Color32::from_rgb(180, 140, 255))
        } else if n.body.is_some() {
            ("◆", ACCENT)
        } else if !n.solids.is_empty() {
            ("▭", Color32::from_rgb(107, 227, 138))
        } else {
            ("·", MUTED)
        };
        c.painter.circle(
            o,
            10.0,
            Color32::from_black_alpha(170),
            Stroke::new(LINE, col),
        );
        c.painter.text(
            o,
            Align2::CENTER_CENTER,
            mark,
            FontId::proportional(FONT_SMALL),
            col,
        );
    }
}

/// Сводка в правом верхнем углу окна сцены.
pub(crate) fn stats(painter: &egui::Painter, rect: Rect, lines: &[String]) {
    let font = FontId::monospace(FONT_SMALL);
    let galleys: Vec<_> = lines
        .iter()
        .map(|l| painter.layout_no_wrap(l.clone(), font.clone(), TEXT_DIM))
        .collect();
    let w = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max) + 16.0;
    let h = galleys.iter().map(|g| g.size().y + 2.0).sum::<f32>() + 12.0;
    let r = Rect::from_min_size(pos2(rect.right() - w - 12.0, rect.top() + 12.0), vec2(w, h));
    painter.rect(
        r,
        RADIUS_M,
        Color32::from_black_alpha(180),
        Stroke::new(LINE, BORDER),
        egui::StrokeKind::Inside,
    );
    let mut y = r.top() + 6.0;
    for g in galleys {
        let gh = g.size().y;
        painter.galley(pos2(r.left() + 8.0, y), g, TEXT_DIM);
        y += gh + 2.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_have_closed_outlines() {
        assert_eq!(rect_pts((0.0, 0.0), (2.0, 1.0), 0.0)[2], [1.0, 0.5]);
        assert_eq!(circle_pts((1.0, 0.0), 0.5).len(), 32);
        let cap = capsule_pts((0.0, 0.0), (1.0, 0.0), 0.25);
        let xs = cap.iter().map(|p| p[0]);
        let (lo, hi) = xs.fold((f64::MAX, f64::MIN), |(l, h), x| (l.min(x), h.max(x)));
        assert!((lo + 0.25).abs() < 1e-9 && (hi - 1.25).abs() < 1e-9);
        assert_eq!(depth_color(0.0), Color32::from_rgb(70, 110, 255));
        assert_eq!(depth_color(1.0), Color32::from_rgb(255, 165, 74));
        assert!((scale_of(&Xform::placement([1.0, 2.0], 30.0, 2.0)) - 2.0).abs() < 1e-9);
    }
}
