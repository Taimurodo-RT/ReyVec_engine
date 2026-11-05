//! Где предметы сцены лежат в координатах сцены: их итоговое место (с учётом родителей)
//! и примерные границы. Нужно, чтобы щёлкнуть по предмету в сцене, нарисовать рамку
//! выделения и ручки и показать предмет кнопкой F.
//!
//! Границы считаются по описанию из файла, без видеокарты: для прямоугольников, кругов,
//! точек ломаных, твёрдого и персонажей — точно, для рисунков, контуров SVG и фигур Motif —
//! примерно (их размер знает только нарезка).

use reyvec_scene::{LightDesc, NodeDesc, PartDesc, SceneFile, ShapeDesc};

/// Поворот, размер и сдвиг на плоскости: `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Xform {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Xform {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// Место предмета относительно родителя: сдвиг, поворот (градусы), размер.
    pub fn placement(at: [f64; 2], angle: f64, scale: f64) -> Self {
        let (s, c) = angle.to_radians().sin_cos();
        Self {
            a: c * scale,
            b: s * scale,
            c: -s * scale,
            d: c * scale,
            e: at[0],
            f: at[1],
        }
    }

    /// Сначала `other`, потом `self`.
    pub fn then(&self, other: &Xform) -> Self {
        Self {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    pub fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        [
            self.a * p[0] + self.c * p[1] + self.e,
            self.b * p[0] + self.d * p[1] + self.f,
        ]
    }

    /// Обратное преобразование (если предмет не сжат в ноль).
    pub fn inverse(&self) -> Option<Self> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Self {
            a,
            b,
            c,
            d,
            e: -(a * self.e + c * self.f),
            f: -(b * self.e + d * self.f),
        })
    }

    /// Итоговый поворот, градусы.
    pub fn angle(&self) -> f64 {
        self.b.atan2(self.a).to_degrees()
    }
}

/// Прямоугольник по осям сцены.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Bounds {
    pub min: [f64; 2],
    pub max: [f64; 2],
}

impl Bounds {
    fn point(p: [f64; 2]) -> Self {
        Self { min: p, max: p }
    }

    fn add(&mut self, p: [f64; 2]) {
        self.min = [self.min[0].min(p[0]), self.min[1].min(p[1])];
        self.max = [self.max[0].max(p[0]), self.max[1].max(p[1])];
    }

    pub fn union(self, o: Bounds) -> Bounds {
        let mut b = self;
        b.add(o.min);
        b.add(o.max);
        b
    }

    pub fn contains(&self, p: [f64; 2]) -> bool {
        (self.min[0]..=self.max[0]).contains(&p[0]) && (self.min[1]..=self.max[1]).contains(&p[1])
    }

    pub fn center(&self) -> [f64; 2] {
        [
            (self.min[0] + self.max[0]) / 2.0,
            (self.min[1] + self.max[1]) / 2.0,
        ]
    }

    pub fn size(&self) -> [f64; 2] {
        [self.max[0] - self.min[0], self.max[1] - self.min[1]]
    }

    fn area(&self) -> f64 {
        let s = self.size();
        s[0] * s[1]
    }
}

/// Границы набора точек, переведённых `x`.
fn bounds_of(x: &Xform, pts: impl IntoIterator<Item = [f64; 2]>) -> Option<Bounds> {
    let mut it = pts.into_iter().map(|p| x.apply(p));
    let mut b = Bounds::point(it.next()?);
    for p in it {
        b.add(p);
    }
    Some(b)
}

/// Углы прямоугольника с серединой `at`, размером `size`, повёрнутого на `angle`.
fn rect_corners(at: (f64, f64), size: (f64, f64), angle: f64) -> [[f64; 2]; 4] {
    let x = Xform::placement([at.0, at.1], angle, 1.0);
    let (w, h) = (size.0 / 2.0, size.1 / 2.0);
    [[-w, -h], [w, -h], [w, h], [-w, h]].map(|p| x.apply(p))
}

/// Точки вокруг круга (восемь хватает для рамки).
fn circle_pts(at: (f64, f64), r: f64) -> impl Iterator<Item = [f64; 2]> {
    (0..8).map(move |k| {
        let t = k as f64 * std::f64::consts::FRAC_PI_4;
        [at.0 + r * t.cos(), at.1 + r * t.sin()]
    })
}

/// Точки, по которым считаются собственные границы предмета (в его координатах).
fn own_points(n: &NodeDesc) -> Vec<[f64; 2]> {
    let mut pts = Vec::new();
    fn rect(pts: &mut Vec<[f64; 2]>, at: (f64, f64), size: (f64, f64), angle: f64) {
        pts.extend(rect_corners(at, size, angle));
    }
    for s in &n.shapes {
        match s {
            ShapeDesc::Rect {
                at, size, angle, ..
            } => rect(&mut pts, *at, *size, *angle),
            ShapeDesc::Circle { at, r, .. } => rect(&mut pts, *at, (2.0 * r, 2.0 * r), 0.0),
            ShapeDesc::Polygon {
                points, at, angle, ..
            } => {
                let x = Xform::placement([at.0, at.1], *angle, 1.0);
                pts_extend(&mut pts, points.iter().map(|p| x.apply([p.0, p.1])));
            }
            ShapeDesc::Line {
                points,
                width,
                at,
                angle,
                ..
            } => {
                let x = Xform::placement([at.0, at.1], *angle, 1.0);
                for p in points {
                    let q = x.apply([p.0, p.1]);
                    pts_extend(&mut pts, circle_pts((q[0], q[1]), width / 2.0));
                }
            }
            ShapeDesc::Path { at, scale, .. } => {
                rect(&mut pts, *at, (0.2 * scale, 0.2 * scale), 0.0)
            }
        }
    }
    for l in &n.lights {
        match l {
            LightDesc::Point { at, .. } => rect(&mut pts, *at, (0.08, 0.08), 0.0),
            LightDesc::Tube { from, to, .. } => {
                rect(&mut pts, *from, (0.04, 0.04), 0.0);
                rect(&mut pts, *to, (0.04, 0.04), 0.0);
            }
            LightDesc::Sun { .. } => {}
        }
    }
    for o in &n.objects {
        for p in &o.parts {
            match p {
                PartDesc::Circle { at, r } => rect(
                    &mut pts,
                    *at,
                    (2.0 * f64::from(*r), 2.0 * f64::from(*r)),
                    0.0,
                ),
                PartDesc::Capsule { from, to, r } => {
                    let r = f64::from(*r);
                    rect(&mut pts, *from, (2.0 * r, 2.0 * r), 0.0);
                    rect(&mut pts, *to, (2.0 * r, 2.0 * r), 0.0);
                }
            }
        }
    }
    for p in &n.pictures {
        // Ширина рисунка известна только после чтения файла: берём квадрат.
        rect(&mut pts, p.at, (p.height, p.height), p.angle);
    }
    for m in &n.morphs {
        rect(&mut pts, m.at, (0.3 * m.scale, 0.3 * m.scale), m.angle);
    }
    for s in &n.solids {
        rect(&mut pts, s.at, s.size, 0.0);
    }
    if let Some(p) = &n.player {
        rect(&mut pts, (0.0, 0.0), p.size, 0.0);
    }
    if let Some(d) = &n.dummy {
        rect(&mut pts, (0.0, 0.0), d.size, 0.0);
    }
    if let Some(t) = n.touch {
        rect(&mut pts, (0.0, 0.0), t, 0.0);
    }
    if let Some(t) = &n.tiles {
        let cols = t.rows.iter().map(|r| r.chars().count()).max().unwrap_or(0) as f64;
        let rows = t.rows.len() as f64;
        pts.push([0.0, 0.0]);
        pts.push([cols * t.cell, -rows * t.cell]);
    }
    pts
}

fn pts_extend(pts: &mut Vec<[f64; 2]>, it: impl IntoIterator<Item = [f64; 2]>) {
    pts.extend(it);
}

/// Предмет на сцене: путь, итоговое место, свои границы и границы со всеми вложенными.
#[derive(Clone, Debug)]
pub(crate) struct Placed {
    pub path: Vec<usize>,
    /// Из координат предмета в координаты сцены.
    pub xform: Xform,
    /// Из координат родителя в координаты сцены (для перевода сдвига мыши в `at`).
    pub parent: Xform,
    /// Итоговая глубина (сумма глубин предков).
    pub depth: f64,
    pub own: Bounds,
    pub all: Bounds,
}

/// Все предметы сцены (со вложенными) в порядке файла.
pub(crate) fn place_all(file: &SceneFile) -> Vec<Placed> {
    fn walk(
        nodes: &[NodeDesc],
        parent: Xform,
        depth: f64,
        path: &mut Vec<usize>,
        out: &mut Vec<Placed>,
    ) -> Option<Bounds> {
        let mut total: Option<Bounds> = None;
        for (i, n) in nodes.iter().enumerate() {
            path.push(i);
            let xform = parent.then(&Xform::placement([n.at.0, n.at.1], n.angle, n.scale));
            let d = depth + n.depth;
            // Пустой предмет всё равно можно выделить: маленький квадрат вокруг места.
            let own = bounds_of(&xform, own_points(n)).unwrap_or_else(|| {
                let o = xform.apply([0.0, 0.0]);
                Bounds {
                    min: [o[0] - 0.04, o[1] - 0.04],
                    max: [o[0] + 0.04, o[1] + 0.04],
                }
            });
            let me = out.len();
            out.push(Placed {
                path: path.clone(),
                xform,
                parent,
                depth: d,
                own,
                all: own,
            });
            if let Some(kids) = walk(&n.children, xform, d, path, out) {
                out[me].all = own.union(kids);
            }
            let all = out[me].all;
            total = Some(total.map_or(all, |t| t.union(all)));
            path.pop();
        }
        total
    }
    let mut out = Vec::new();
    walk(&file.nodes, Xform::IDENTITY, 0.0, &mut Vec::new(), &mut out);
    out
}

/// Какой предмет под точкой сцены `p`. `deep: false` — предмет верхнего уровня (как
/// актёр в Unreal), `deep: true` — самый вложенный. Из нескольких под точкой выигрывает
/// тот, что ближе к зрителю, а при равной глубине — меньший.
pub(crate) fn pick(placed: &[Placed], p: [f64; 2], deep: bool) -> Option<Vec<usize>> {
    placed
        .iter()
        .filter(|n| {
            if deep {
                n.own.contains(p)
            } else {
                n.path.len() == 1 && n.all.contains(p)
            }
        })
        .max_by(|a, b| {
            let (ba, bb) = if deep { (a.own, b.own) } else { (a.all, b.all) };
            a.depth
                .total_cmp(&b.depth)
                .then(bb.area().total_cmp(&ba.area()))
                .then(a.path.len().cmp(&b.path.len()))
        })
        .map(|n| n.path.clone())
}

/// Предмет по пути среди уже разложенных.
pub(crate) fn find<'a>(placed: &'a [Placed], path: &[usize]) -> Option<&'a Placed> {
    placed.iter().find(|n| n.path == path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    #[test]
    fn xform_inverse_and_angle() {
        let x = Xform::placement([1.0, 2.0], 30.0, 2.0);
        let p = [0.3, -0.7];
        assert!(near(x.inverse().unwrap().apply(x.apply(p)), p));
        assert!((x.angle() - 30.0).abs() < 1e-9);
        let y = Xform::placement([0.5, 0.0], 90.0, 1.0);
        // Сначала y, потом x.
        assert!(near(x.then(&y).apply(p), x.apply(y.apply(p))));
    }

    const SCENE: &str = r##"(
        format: "reyvec-scene", version: 1,
        nodes: [
            (name: "пол", at: (0, -0.9), shapes: [Rect(at: (0, 0), size: (4, 0.2), color: "#444444")]),
            (name: "стол", at: (0.5, -0.5), depth: 1,
             shapes: [Rect(at: (0, 0), size: (0.6, 0.05), color: "#884422")],
             children: [(name: "банка", at: (0.2, 0.1), shapes: [Circle(at: (0, 0), r: 0.05, color: "#ff0000")])]),
        ],
    )"##;

    #[test]
    fn bounds_follow_parents() {
        let file = SceneFile::parse(SCENE).unwrap();
        let placed = place_all(&file);
        assert_eq!(placed.len(), 3);
        let jar = find(&placed, &[1, 0]).unwrap();
        assert!(near(jar.own.center(), [0.7, -0.4]));
        assert!((jar.own.size()[0] - 0.1).abs() < 1e-9);
        let table = find(&placed, &[1]).unwrap();
        assert!(table.all.contains([0.7, -0.36]) && !table.own.contains([0.7, -0.36]));
    }

    #[test]
    fn pick_prefers_front_and_small() {
        let file = SceneFile::parse(SCENE).unwrap();
        let placed = place_all(&file);
        // Банка поверх стола: сверху выбирается стол целиком, вглубь — банка.
        assert_eq!(pick(&placed, [0.7, -0.4], false), Some(vec![1]));
        assert_eq!(pick(&placed, [0.7, -0.4], true), Some(vec![1, 0]));
        assert_eq!(pick(&placed, [-1.5, -0.9], false), Some(vec![0]));
        assert_eq!(pick(&placed, [3.0, 3.0], false), None);
    }

    #[test]
    fn every_node_of_real_scene_has_bounds() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenes/first-room.ron"),
        )
        .unwrap();
        let file = SceneFile::parse(&text).unwrap();
        let placed = place_all(&file);
        assert_eq!(placed.len(), crate::editor::outline::count(&file));
        for p in &placed {
            let s = p.all.size();
            assert!(s[0].is_finite() && s[1].is_finite() && s[0] >= 0.0 && s[1] >= 0.0);
        }
    }
}
