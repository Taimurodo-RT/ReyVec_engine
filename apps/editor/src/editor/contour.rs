//! Контур выдавливания и выточки (М.5): точки контура для правки мышью. Контур в файле
//! модели — строка как в SVG (`M 0 0 L 1 0 Q 1 1 0 1 Z`), в метрах и с Y вверх; здесь он
//! разобран на точки и отрезки к ним (прямые и изгибы) и записывается обратно строкой.

use kurbo::{
    BezPath, CubicBez, Line, ParamCurve, ParamCurveNearest, PathEl, PathSeg, Point, QuadBez,
};

use super::ron_edit::num;

/// Как контур приходит в точку от предыдущей.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Seg {
    Line,
    /// Изгиб с одной ручкой.
    Quad([f64; 2]),
    /// Изгиб с двумя ручками.
    Cubic([f64; 2], [f64; 2]),
}

/// Точка контура и отрезок, которым к ней приходят.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Node {
    pub p: [f64; 2],
    pub seg: Seg,
}

/// Один замкнутый (или открытый) контур. У замкнутого первая точка хранит отрезок от
/// последней точки к ней.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Contour {
    pub nodes: Vec<Node>,
    pub closed: bool,
}

/// Что тянут: точку контура (`kind` 0) или ручку изгиба отрезка к ней (1, 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Grip {
    pub contour: usize,
    pub node: usize,
    pub kind: u8,
}

fn pt(p: Point) -> [f64; 2] {
    [p.x, p.y]
}

fn kp(p: [f64; 2]) -> Point {
    Point::new(p[0], p[1])
}

/// Разбирает контур из строки. `None` — строка не разобралась или в ней нет контура.
pub(crate) fn parse(path: &str) -> Option<Vec<Contour>> {
    let bez = BezPath::from_svg(path).ok()?;
    let mut out = Vec::new();
    let mut cur: Option<Contour> = None;
    let finish = |c: Option<Contour>, out: &mut Vec<Contour>| {
        if let Some(c) = c.filter(|c| c.nodes.len() > 1) {
            out.push(c);
        }
    };
    for el in bez.elements() {
        let node = match *el {
            PathEl::MoveTo(p) => {
                finish(cur.take(), &mut out);
                cur = Some(Contour {
                    nodes: vec![Node {
                        p: pt(p),
                        seg: Seg::Line,
                    }],
                    closed: false,
                });
                continue;
            }
            PathEl::LineTo(p) => Node {
                p: pt(p),
                seg: Seg::Line,
            },
            PathEl::QuadTo(c, p) => Node {
                p: pt(p),
                seg: Seg::Quad(pt(c)),
            },
            PathEl::CurveTo(a, b, p) => Node {
                p: pt(p),
                seg: Seg::Cubic(pt(a), pt(b)),
            },
            PathEl::ClosePath => {
                if let Some(c) = cur.as_mut() {
                    c.closed = true;
                    // Последний отрезок пришёл в первую точку — это и есть замыкание.
                    let first = c.nodes[0].p;
                    if c.nodes.len() > 2
                        && let Some(last) = c.nodes.last().copied()
                        && (last.p[0] - first[0]).abs() < 1e-9
                        && (last.p[1] - first[1]).abs() < 1e-9
                    {
                        c.nodes.pop();
                        c.nodes[0].seg = last.seg;
                    }
                }
                finish(cur.take(), &mut out);
                continue;
            }
        };
        cur.as_mut()?.nodes.push(node);
    }
    finish(cur, &mut out);
    (!out.is_empty()).then_some(out)
}

/// Число контура: коротко, без «.0» у целых.
fn short(v: f64) -> String {
    let v = if v == 0.0 { 0.0 } else { v };
    let s = num(v);
    s.strip_suffix(".0").map_or(s.clone(), str::to_owned)
}

fn seg_text(out: &mut String, n: &Node) {
    let p = |v: [f64; 2]| format!("{} {}", short(v[0]), short(v[1]));
    let s = match n.seg {
        Seg::Line => format!(" L {}", p(n.p)),
        Seg::Quad(c) => format!(" Q {} {}", p(c), p(n.p)),
        Seg::Cubic(a, b) => format!(" C {} {} {}", p(a), p(b), p(n.p)),
    };
    out.push_str(&s);
}

/// Контур строкой для файла.
pub(crate) fn to_path(contours: &[Contour]) -> String {
    let mut out = String::new();
    for c in contours {
        let Some(first) = c.nodes.first() else {
            continue;
        };
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("M {} {}", short(first.p[0]), short(first.p[1])));
        for n in &c.nodes[1..] {
            seg_text(&mut out, n);
        }
        if c.closed {
            if first.seg != Seg::Line {
                seg_text(&mut out, first);
            }
            out.push_str(" Z");
        }
    }
    out
}

/// Отрезок, которым контур приходит в точку `n` (у открытого контура в первую — нет).
pub(crate) fn segment(c: &Contour, n: usize) -> Option<PathSeg> {
    let to = c.nodes.get(n)?;
    let from = if n > 0 {
        c.nodes[n - 1].p
    } else if c.closed {
        c.nodes.last()?.p
    } else {
        return None;
    };
    Some(match to.seg {
        Seg::Line => PathSeg::Line(Line::new(kp(from), kp(to.p))),
        Seg::Quad(q) => PathSeg::Quad(QuadBez::new(kp(from), kp(q), kp(to.p))),
        Seg::Cubic(a, b) => PathSeg::Cubic(CubicBez::new(kp(from), kp(a), kp(b), kp(to.p))),
    })
}

/// Все точки и ручки контура с их местами.
pub(crate) fn grips(contours: &[Contour]) -> Vec<(Grip, [f64; 2])> {
    let mut out = Vec::new();
    for (ci, c) in contours.iter().enumerate() {
        for (ni, n) in c.nodes.iter().enumerate() {
            let g = |kind| Grip {
                contour: ci,
                node: ni,
                kind,
            };
            // У открытого контура ручки первой точки ни к чему не относятся.
            if ni > 0 || c.closed {
                match n.seg {
                    Seg::Line => {}
                    Seg::Quad(q) => out.push((g(1), q)),
                    Seg::Cubic(a, b) => {
                        out.push((g(1), a));
                        out.push((g(2), b));
                    }
                }
            }
            out.push((g(0), n.p));
        }
    }
    out
}

/// Место точки или ручки.
pub(crate) fn grip_at(contours: &[Contour], g: Grip) -> Option<[f64; 2]> {
    let n = contours.get(g.contour)?.nodes.get(g.node)?;
    match (g.kind, n.seg) {
        (0, _) => Some(n.p),
        (1, Seg::Quad(q)) => Some(q),
        (1, Seg::Cubic(a, _)) => Some(a),
        (2, Seg::Cubic(_, b)) => Some(b),
        _ => None,
    }
}

/// Ставит точку или ручку на место `p`. Точку тянут вместе с ручками её отрезков, чтобы
/// изгиб сохранял форму.
pub(crate) fn set_grip(contours: &mut [Contour], g: Grip, p: [f64; 2]) {
    let Some(c) = contours.get_mut(g.contour) else {
        return;
    };
    let len = c.nodes.len();
    let Some(old) = c.nodes.get(g.node).map(|n| n.p) else {
        return;
    };
    let d = [p[0] - old[0], p[1] - old[1]];
    let shift = |v: &mut [f64; 2]| {
        v[0] += d[0];
        v[1] += d[1];
    };
    match g.kind {
        0 => {
            let n = &mut c.nodes[g.node];
            n.p = p;
            // Ручка, ближняя к этой точке, едет с ней: у отрезка к ней — вторая (или
            // единственная), у отрезка от неё — первая.
            match &mut n.seg {
                Seg::Quad(q) => shift(q),
                Seg::Cubic(_, b) => shift(b),
                Seg::Line => {}
            }
            let next = if g.node + 1 < len {
                Some(g.node + 1)
            } else if c.closed {
                Some(0)
            } else {
                None
            };
            if let Some(next) = next.filter(|&i| i != g.node)
                && let Seg::Cubic(a, _) = &mut c.nodes[next].seg
            {
                shift(a);
            }
        }
        1 => match &mut c.nodes[g.node].seg {
            Seg::Quad(q) => *q = p,
            Seg::Cubic(a, _) => *a = p,
            Seg::Line => {}
        },
        _ => {
            if let Seg::Cubic(_, b) = &mut c.nodes[g.node].seg {
                *b = p;
            }
        }
    }
}

/// Отрезок, ближайший к точке `p`: контур, точка, в которую он приходит, место на нём
/// (0…1) и расстояние.
pub(crate) fn nearest_segment(
    contours: &[Contour],
    p: [f64; 2],
) -> Option<(usize, usize, f64, f64)> {
    let mut best: Option<(usize, usize, f64, f64)> = None;
    for (ci, c) in contours.iter().enumerate() {
        for ni in 0..c.nodes.len() {
            let Some(seg) = segment(c, ni) else { continue };
            let n = seg.nearest(kp(p), 1e-6);
            let d = n.distance_sq.sqrt();
            if best.is_none_or(|b| d < b.3) {
                best = Some((ci, ni, n.t, d));
            }
        }
    }
    best
}

/// Новая точка на отрезке, приходящем в точку `node`, в месте `t`. Изгиб делится на два
/// так, что форма не меняется. Возвращает номер новой точки.
pub(crate) fn insert(
    contours: &mut [Contour],
    contour: usize,
    node: usize,
    t: f64,
) -> Option<usize> {
    let c = contours.get_mut(contour)?;
    let seg = segment(c, node)?;
    let (a, b) = (seg.subsegment(0.0..t), seg.subsegment(t..1.0));
    let to_seg = |s: PathSeg| match s {
        PathSeg::Line(_) => Seg::Line,
        PathSeg::Quad(q) => Seg::Quad(pt(q.p1)),
        PathSeg::Cubic(k) => Seg::Cubic(pt(k.p1), pt(k.p2)),
    };
    let new = Node {
        p: pt(seg.eval(t)),
        seg: to_seg(a),
    };
    c.nodes[node].seg = to_seg(b);
    // Замыкающий отрезок (в первую точку) — новая точка встаёт в конец.
    let at = if node == 0 { c.nodes.len() } else { node };
    c.nodes.insert(at, new);
    Some(at)
}

/// Убирает точку. Замкнутому контуру нужно хотя бы три точки, открытому — две.
pub(crate) fn remove(contours: &mut [Contour], contour: usize, node: usize) -> bool {
    let Some(c) = contours.get_mut(contour) else {
        return false;
    };
    let min = if c.closed { 3 } else { 2 };
    if c.nodes.len() <= min || node >= c.nodes.len() {
        return false;
    }
    c.nodes.remove(node);
    if c.closed && node == 0 {
        // Новая первая точка: в неё теперь приходят от последней — прямой.
        c.nodes[0].seg = Seg::Line;
    } else if node < c.nodes.len() {
        // Отрезок к следующей точке теперь идёт от предыдущей: прямой.
        c.nodes[node].seg = Seg::Line;
    }
    if !c.closed {
        c.nodes[0].seg = Seg::Line;
    }
    true
}

/// Отрезок к точке — изгиб: ручка в середине, сдвинутая вбок на четверть длины.
pub(crate) fn make_curve(contours: &mut [Contour], contour: usize, node: usize) {
    let Some(c) = contours.get_mut(contour) else {
        return;
    };
    let Some(PathSeg::Line(l)) = segment(c, node) else {
        return;
    };
    let mid = l.eval(0.5);
    let d = l.p1 - l.p0;
    let side = kurbo::Vec2::new(-d.y, d.x) * 0.25;
    c.nodes[node].seg = Seg::Quad(pt(mid + side));
}

/// Отрезок к точке — прямой.
pub(crate) fn make_line(contours: &mut [Contour], contour: usize, node: usize) {
    if let Some(n) = contours
        .get_mut(contour)
        .and_then(|c| c.nodes.get_mut(node))
    {
        n.seg = Seg::Line;
    }
}

/// Растягивает контур: по X в `sx`, по Y в `sy` раз.
pub(crate) fn scale(contours: &mut [Contour], sx: f64, sy: f64) {
    let s = |v: &mut [f64; 2]| {
        v[0] *= sx;
        v[1] *= sy;
    };
    for n in contours.iter_mut().flat_map(|c| c.nodes.iter_mut()) {
        s(&mut n.p);
        match &mut n.seg {
            Seg::Line => {}
            Seg::Quad(q) => s(q),
            Seg::Cubic(a, b) => {
                s(a);
                s(b);
            }
        }
    }
}

/// Прямоугольник контура с ручками: левый нижний и правый верхний углы.
pub(crate) fn bounds(contours: &[Contour]) -> Option<([f64; 2], [f64; 2])> {
    grips(contours)
        .into_iter()
        .map(|(_, p)| p)
        .fold(None, |acc, p| {
            let (lo, hi) = acc.unwrap_or((p, p));
            Some((
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            ))
        })
}

/// Точки отрезка для рисования.
pub(crate) fn sample(seg: PathSeg, n: usize) -> Vec<[f64; 2]> {
    (0..=n).map(|i| pt(seg.eval(i as f64 / n as f64))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VASE: &str = "M 0 0 L 0.32 0 L 0.32 0.07 L 0.2 0.12 Q 0.1 0.35 0.1 0.6 L 0 0.69 Z";

    #[test]
    fn contour_reads_and_writes_back() {
        let c = parse(VASE).unwrap();
        assert_eq!(c.len(), 1);
        assert!(c[0].closed);
        assert_eq!(c[0].nodes.len(), 6);
        assert_eq!(c[0].nodes[4].seg, Seg::Quad([0.1, 0.35]));
        assert_eq!(to_path(&c), VASE);
        // Изгиб в первую точку: записывается отрезком к ней перед Z.
        let curved = "M 0 0 L 1 0 L 1 1 Q 0.5 0.5 0 0 Z";
        let c = parse(curved).unwrap();
        assert_eq!(c[0].nodes.len(), 3);
        assert_eq!(c[0].nodes[0].seg, Seg::Quad([0.5, 0.5]));
        assert_eq!(to_path(&c), curved);
        assert!(reyvec_sdf::profile2d::Profile::new(&to_path(&c)).is_ok());
        assert!(parse("не контур").is_none());
    }

    #[test]
    fn points_are_added_moved_and_removed() {
        let mut c = parse("M 0 0 L 1 0 L 1 1 L 0 1 Z").unwrap();
        // Двойной щелчок посередине нижней стороны.
        let (ci, ni, t, d) = nearest_segment(&c, [0.5, -0.01]).unwrap();
        assert_eq!((ci, ni), (0, 1));
        assert!(d < 0.02 && (t - 0.5).abs() < 1e-3);
        let at = insert(&mut c, ci, ni, t).unwrap();
        assert_eq!(at, 1);
        assert_eq!(c[0].nodes.len(), 5);
        assert_eq!(c[0].nodes[1].p, [0.5, 0.0]);
        // Новую точку тянут вниз — получается «домик» снизу.
        set_grip(
            &mut c,
            Grip {
                contour: 0,
                node: 1,
                kind: 0,
            },
            [0.5, -0.3],
        );
        assert!(to_path(&c).contains("L 0.5 -0.3"));
        // Изгиб и обратно.
        make_curve(&mut c, 0, 2);
        assert!(matches!(c[0].nodes[2].seg, Seg::Quad(_)));
        assert_eq!(grips(&c).len(), 6);
        make_line(&mut c, 0, 2);
        assert!(remove(&mut c, 0, 1));
        assert_eq!(to_path(&c), "M 0 0 L 1 0 L 1 1 L 0 1 Z");
        // Меньше трёх точек у замкнутого не бывает.
        assert!(remove(&mut c, 0, 0));
        assert!(!remove(&mut c, 0, 0));
        // Замыкающий отрезок делится: новая точка в конце.
        let mut c = parse("M 0 0 L 1 0 L 0 1 Z").unwrap();
        let at = insert(&mut c, 0, 0, 0.5).unwrap();
        assert_eq!(at, 3);
        assert_eq!(c[0].nodes[3].p, [0.0, 0.5]);
    }

    #[test]
    fn curve_split_keeps_its_shape() {
        let mut c = parse("M 0 0 L 1 0 Q 1 1 0 1 Z").unwrap();
        let before = segment(&c[0], 2).unwrap().eval(0.75);
        let at = insert(&mut c, 0, 2, 0.5).unwrap();
        let after = segment(&c[0], at + 1).unwrap().eval(0.5);
        assert!((before - after).hypot() < 1e-9);
        scale(&mut c, 2.0, 0.5);
        assert_eq!(bounds(&c).unwrap(), ([0.0, 0.0], [2.0, 0.5]));
    }
}
