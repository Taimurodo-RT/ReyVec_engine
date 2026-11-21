//! Части предмета в инспекторе (Р.7): фигуры, свет, SDF, рисунки, опоры, тело, касание —
//! списком, с «+ Добавить» и «×»; каталог новых предметов («+» в иерархии); настройки
//! сцены. Всё, что меняется, уходит действиями ([`Action`]) и пишется в текст файла.

use std::path::Path;

use egui::{Color32, RichText, Ui, vec2};
use reyvec_scene::{Background, Hex, LightDesc, NodeDesc, SceneFile, ShapeDesc};

use super::doc::{Change, Edit};
use super::ron_edit::{num, pair, string};
use super::theme::*;
use super::ui::{Action, FieldEdit, card, num_field, prop};

/// Цвет файла сцены в цвет egui.
fn color_of(h: &Hex) -> [u8; 3] {
    let [r, g, b, _] = h.0.to_srgb_array();
    [r, g, b].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn hex(c: [u8; 3]) -> String {
    string(&format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
}

/// Кнопка-образец цвета: палитра по щелчку. Новый цвет — значение для файла.
fn color_field(ui: &mut Ui, h: &Hex, enabled: bool) -> Option<String> {
    let mut c = color_of(h);
    let r = ui.add_enabled_ui(enabled, |ui| {
        egui::color_picker::color_edit_button_srgb(ui, &mut c)
    });
    (r.inner.changed() && c != color_of(h)).then(|| hex(c))
}

/// Что за часть: название для людей.
fn shape_name(s: &ShapeDesc) -> &'static str {
    match s {
        ShapeDesc::Rect { .. } => "Прямоугольник",
        ShapeDesc::Circle { .. } => "Круг",
        ShapeDesc::Polygon { .. } => "Многоугольник",
        ShapeDesc::Line { .. } => "Линия",
        ShapeDesc::Path { .. } => "Контур SVG",
    }
}

/// Строит действия для полей одной части.
struct Part<'a> {
    path: &'a [usize],
    field: &'static str,
    index: usize,
    editing: bool,
    actions: &'a mut Vec<Action>,
    remove: bool,
}

impl Part<'_> {
    fn set(&mut self, sub: &'static str, value: String, commit: bool) {
        self.actions.push(Action::Edit {
            edit: Edit {
                path: self.path.to_vec(),
                change: Change::Item {
                    field: self.field,
                    index: self.index,
                    sub,
                    value,
                },
            },
            commit,
        });
    }

    /// Строка: название, образец цвета, числа и «×».
    fn row(&mut self, ui: &mut Ui, name: &str, add: impl FnOnce(&mut Ui, &mut Self)) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_S;
            ui.label(RichText::new(name).color(TEXT));
            add(ui, self);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(self.editing, egui::Button::new("×").small())
                    .on_hover_text("Убрать эту часть")
                    .clicked()
                {
                    self.remove = true;
                }
            });
        });
    }

    fn color(&mut self, ui: &mut Ui, h: &Hex) {
        if let Some(v) = color_field(ui, h, self.editing) {
            self.set("color", v, true);
        }
    }

    /// Число части: тянуть мышью или ввести.
    fn number(&mut self, ui: &mut Ui, sub: &'static str, v: f64, speed: f64, hint: &str) {
        let r = num_field(ui, v, speed, None, "", self.editing);
        ui.label(RichText::new(hint).small().color(MUTED));
        match r {
            Some(FieldEdit::Value(x, commit)) => self.set(sub, num(x), commit),
            Some(FieldEdit::End) => self.actions.push(Action::EndEdit),
            None => {}
        }
    }

    /// Пара чисел (размер): ширина и высота, каждое своим полем.
    fn size(&mut self, ui: &mut Ui, sub: &'static str, v: (f64, f64)) {
        let w = num_field(ui, v.0, 0.005, None, "", self.editing);
        let h = num_field(ui, v.1, 0.005, None, "", self.editing);
        for (k, f) in [w, h].into_iter().enumerate() {
            match f {
                Some(FieldEdit::Value(x, commit)) => {
                    let mut p = [v.0, v.1];
                    p[k] = x.max(0.001);
                    self.set(sub, pair(p), commit);
                }
                Some(FieldEdit::End) => self.actions.push(Action::EndEdit),
                None => {}
            }
        }
    }
}

/// Карточка «Части» выбранного предмета.
pub(crate) fn parts_card(
    ui: &mut Ui,
    path: &[usize],
    node: &NodeDesc,
    editing: bool,
    actions: &mut Vec<Action>,
) {
    card(ui, "Части", |ui| {
        ui.spacing_mut().item_spacing.y = GAP_S;
        let mut removed: Option<(&'static str, usize)> = None;
        let mut part = |ui: &mut Ui,
                        field: &'static str,
                        index: usize,
                        name: &str,
                        add: &mut dyn FnMut(&mut Ui, &mut Part)| {
            let mut p = Part {
                path,
                field,
                index,
                editing,
                actions: &mut *actions,
                remove: false,
            };
            p.row(ui, name, |ui, p| add(ui, p));
            if p.remove {
                removed = Some((field, index));
            }
        };
        for (i, s) in node.shapes.iter().enumerate() {
            part(ui, "shapes", i, shape_name(s), &mut |ui, p| match s {
                ShapeDesc::Rect { size, color, .. } => {
                    p.color(ui, color);
                    p.size(ui, "size", *size);
                }
                ShapeDesc::Circle { r, color, .. } => {
                    p.color(ui, color);
                    p.number(ui, "r", *r, 0.002, "радиус");
                }
                ShapeDesc::Polygon { color, .. } | ShapeDesc::Path { color, .. } => {
                    p.color(ui, color);
                }
                ShapeDesc::Line { width, color, .. } => {
                    p.color(ui, color);
                    p.number(ui, "width", *width, 0.001, "толщина");
                }
            });
        }
        for (i, l) in node.lights.iter().enumerate() {
            let (name, color, power, reach) = match l {
                LightDesc::Point {
                    color,
                    power,
                    reach,
                    ..
                } => ("Лампа", color, power, reach),
                LightDesc::Tube {
                    color,
                    power,
                    reach,
                    ..
                } => ("Неоновая трубка", color, power, reach),
                LightDesc::Sun {
                    color,
                    power,
                    reach,
                    ..
                } => ("Солнце", color, power, reach),
            };
            part(ui, "lights", i, name, &mut |ui, p| {
                p.color(ui, color);
                p.number(ui, "power", f64::from(*power), 0.02, "сила");
                p.number(ui, "reach", f64::from(*reach), 0.01, "докуда");
            });
        }
        for (i, o) in node.objects.iter().enumerate() {
            part(ui, "objects", i, "SDF-объект", &mut |ui, p| {
                p.color(ui, &o.color);
                p.number(ui, "power", f64::from(o.power), 0.02, "свечение");
            });
        }
        for (i, pic) in node.pictures.iter().enumerate() {
            part(ui, "pictures", i, "Рисунок", &mut |ui, p| {
                ui.label(RichText::new(&pic.svg).small().color(TEXT_DIM));
                p.number(ui, "height", pic.height, 0.005, "высота");
            });
        }
        for (i, m) in node.morphs.iter().enumerate() {
            part(ui, "morphs", i, "Фигура Motif", &mut |ui, p| {
                ui.label(RichText::new(&m.motif).small().color(TEXT_DIM));
                p.number(ui, "scale", m.scale, 0.005, "размер");
            });
        }
        for (i, s) in node.solids.iter().enumerate() {
            part(ui, "solids", i, "Опора", &mut |ui, p| {
                p.size(ui, "size", s.size);
            });
        }
        if let Some((field, index)) = removed {
            actions.push(Action::RemovePart {
                path: path.to_vec(),
                field,
                index,
            });
        }

        // Поля-части: тело, касание, скрипт.
        let field = |field: &'static str, value: Option<String>| Action::Edit {
            edit: Edit {
                path: path.to_vec(),
                change: Change::Field { field, value },
            },
            commit: true,
        };
        if let Some(b) = &node.body {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Физическое тело").color(TEXT));
                let mut fixed = b.fixed;
                if ui
                    .add_enabled(editing, egui::Checkbox::new(&mut fixed, "опора"))
                    .on_hover_text("Неподвижное: на него можно встать, его не сдвинуть")
                    .changed()
                {
                    let mut t = body_text(b);
                    t.fixed = fixed;
                    actions.push(field("body", Some(t.to_ron())));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(editing, egui::Button::new("×").small())
                        .clicked()
                    {
                        actions.push(field("body", None));
                    }
                });
            });
        }
        if let Some(t) = node.touch {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Зона касания").color(TEXT));
                ui.label(
                    RichText::new(format!("{:.2} × {:.2}", t.0, t.1))
                        .small()
                        .color(TEXT_DIM),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(editing, egui::Button::new("×").small())
                        .clicked()
                    {
                        actions.push(field("touch", None));
                    }
                });
            });
        }
        if let Some(s) = &node.script {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Скрипт").color(TEXT));
                ui.label(RichText::new(s).small().color(TEXT_DIM));
            });
        }
        if node.shapes.is_empty()
            && node.lights.is_empty()
            && node.objects.is_empty()
            && node.pictures.is_empty()
            && node.morphs.is_empty()
            && node.solids.is_empty()
            && node.body.is_none()
        {
            ui.label(RichText::new("Пока пусто").color(MUTED));
        }
        ui.add_space(GAP_S);
        ui.add_enabled_ui(editing, |ui| {
            ui.menu_button(RichText::new("+ Добавить").color(ACCENT), |ui| {
                ui.set_min_width(220.0);
                for (name, list, item) in PART_TEMPLATES {
                    if ui.button(*name).clicked() {
                        actions.push(Action::AddPart {
                            path: path.to_vec(),
                            field: list,
                            item: (*item).to_owned(),
                        });
                        ui.close();
                    }
                }
                ui.separator();
                let top = path.len() == 1;
                let special = node.player.is_none() && node.dummy.is_none();
                if ui
                    .add_enabled(
                        top && special && node.body.is_none(),
                        egui::Button::new("Физическое тело"),
                    )
                    .on_hover_text("Предмет будет падать и катиться (только у верхнего уровня)")
                    .clicked()
                {
                    actions.push(field("body", Some("(size: (0.1, 0.1))".into())));
                    ui.close();
                }
                if ui
                    .add_enabled(node.touch.is_none(), egui::Button::new("Зона касания"))
                    .on_hover_text("Скрипт узнает, когда герой её касается")
                    .clicked()
                {
                    actions.push(field("touch", Some("(0.1, 0.1)".into())));
                    ui.close();
                }
            });
        });
    });
}

/// Тело в виде, который можно записать обратно (только то, что меняют в инспекторе;
/// остальное не трогаем — пишется весь текст тела).
struct BodyText {
    text: String,
    fixed: bool,
}

impl BodyText {
    fn to_ron(&self) -> String {
        if self.fixed {
            format!("({}, fixed: true)", self.text)
        } else {
            format!("({})", self.text)
        }
    }
}

fn body_text(b: &reyvec_scene::BodyDesc) -> BodyText {
    let d = reyvec_scene::BodyDesc::default();
    let mut f = vec![format!("size: {}", pair([b.size.0, b.size.1]))];
    let mut add = |on: bool, s: String| {
        if on {
            f.push(s);
        }
    };
    add(b.round, "round: true".into());
    add(b.capsule, "capsule: true".into());
    add(
        !b.points.is_empty(),
        format!(
            "points: [{}]",
            b.points
                .iter()
                .map(|p| pair([p.0, p.1]))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    add(
        b.density != d.density,
        format!("density: {}", num(b.density)),
    );
    add(b.bounce != d.bounce, format!("bounce: {}", num(b.bounce)));
    add(
        b.friction != d.friction,
        format!("friction: {}", num(b.friction)),
    );
    add(
        b.damping != d.damping,
        format!("damping: {}", num(b.damping)),
    );
    add(
        b.gravity != d.gravity,
        format!("gravity: {}", num(b.gravity)),
    );
    add(b.bullet, "bullet: true".into());
    BodyText {
        text: f.join(", "),
        fixed: b.fixed,
    }
}

/// Части, которые можно добавить: название, список, текст для файла.
const PART_TEMPLATES: &[(&str, &str, &str)] = &[
    (
        "Прямоугольник",
        "shapes",
        r##"Rect(at: (0.0, 0.0), size: (0.1, 0.1), color: "#8a9bb0")"##,
    ),
    (
        "Круг",
        "shapes",
        r##"Circle(at: (0.0, 0.0), r: 0.05, color: "#e0b070")"##,
    ),
    (
        "Лампа",
        "lights",
        r##"Point(at: (0.0, 0.0), color: "#ffd9a0", power: 1.0, reach: 0.6, size: 0.02)"##,
    ),
    (
        "Неоновая трубка",
        "lights",
        r##"Tube(from: (-0.1, 0.0), to: (0.1, 0.0), color: "#ff5ab4", power: 1.0, reach: 0.6, size: 0.01)"##,
    ),
    (
        "SDF-капля",
        "objects",
        r##"(parts: [Circle(at: (0.0, 0.0), r: 0.05)], color: "#5affb8", power: 1.0)"##,
    ),
    (
        "Невидимая опора",
        "solids",
        "(at: (0.0, 0.0), size: (0.2, 0.02))",
    ),
];

/// Новые предметы для каталога «+»: название, подсказка, текст без места (`at`
/// подставляется туда, куда смотрит камера).
pub(crate) const CATALOG: &[(&str, &str, &str)] = &[
    (
        "Пустой предмет",
        "Только место: в него можно вложить другие",
        r#"(name: "предмет")"#,
    ),
    (
        "Прямоугольник",
        "Цветной прямоугольник",
        r##"(name: "прямоугольник", shapes: [Rect(at: (0.0, 0.0), size: (0.15, 0.1), color: "#8a9bb0")])"##,
    ),
    (
        "Круг",
        "Цветной круг",
        r##"(name: "круг", shapes: [Circle(at: (0.0, 0.0), r: 0.05, color: "#e0b070")])"##,
    ),
    (
        "Лампа",
        "Точечный свет с тенями",
        r##"(name: "лампа", lights: [Point(at: (0.0, 0.0), color: "#ffd9a0", power: 1.2, reach: 0.8, size: 0.02)])"##,
    ),
    (
        "Неоновая трубка",
        "Светящийся отрезок",
        r##"(name: "неон", lights: [Tube(from: (-0.15, 0.0), to: (0.15, 0.0), color: "#ff5ab4", power: 1.0, reach: 0.7, size: 0.01)])"##,
    ),
    (
        "Платформа",
        "Доска: держит сверху, снизу сквозь неё запрыгивают",
        r##"(name: "платформа", shapes: [Rect(at: (0.0, 0.0), size: (0.3, 0.025), color: "#6b4a2a", oneway: true)])"##,
    ),
    (
        "Стена",
        "Твёрдый прямоугольник",
        r##"(name: "стена", shapes: [Rect(at: (0.0, 0.0), size: (0.05, 0.3), color: "#4a4e58", solid: true)])"##,
    ),
    (
        "Ящик",
        "Физическое тело: падает, его толкает герой",
        r##"(name: "ящик", shapes: [Rect(at: (0.0, 0.0), size: (0.12, 0.12), corner: 0.01, color: "#a0784a")], body: (size: (0.12, 0.12)))"##,
    ),
    (
        "Мяч",
        "Круглое тело, прыгает",
        r##"(name: "мяч", shapes: [Circle(at: (0.0, 0.0), r: 0.05, color: "#e05050")], body: (size: (0.1, 0.1), round: true, bounce: 0.6, density: 0.4))"##,
    ),
    (
        "Манекен",
        "Его можно бить",
        r##"(name: "манекен", dummy: (size: (0.16, 0.3), hp: 5, respawn: 2.0), shapes: [Rect(at: (0.0, 0.0), size: (0.15, 0.28), corner: 0.05, color: "#b08850")])"##,
    ),
    (
        "SDF-капля",
        "Мягкая светящаяся капля",
        r##"(name: "капля", objects: [(parts: [Circle(at: (0.0, 0.0), r: 0.06)], color: "#5affb8", power: 2.0, glow: true)])"##,
    ),
];

/// Текст нового предмета из каталога с местом `at`.
pub(crate) fn catalog_item(template: &str, at: [f64; 2]) -> String {
    // `(name: …` → `(at: (x, y), name: …`
    format!("(at: {}, {}", pair(at), &template[1..])
}

/// Новый предмет с рисунком SVG `file`, брошенным в сцену из папки `scene_dir`.
pub(crate) fn picture_item(scene_dir: &Path, file: &Path, at: [f64; 2]) -> String {
    let name = file.file_stem().map_or_else(
        || "рисунок".to_owned(),
        |s| s.to_string_lossy().into_owned(),
    );
    format!(
        "(at: {}, name: {}, pictures: [(svg: {}, height: 0.3)])",
        pair(at),
        string(&name),
        string(&relative(scene_dir, file)),
    )
}

/// Предмет из заготовки (текст предмета в скобках) на месте `at`.
pub(crate) fn prefab_item(text: &str, at: [f64; 2]) -> Result<String, super::ron_edit::EditError> {
    use super::ron_edit;
    let body = text.trim();
    // Обёртка-сцена: поле `at` ставится так же, как в настоящей сцене.
    let wrapped = format!("(nodes: [\n{body}\n])");
    let t = ron_edit::set_field(&wrapped, &[0], "at", &pair(at))?;
    ron_edit::node_text(&t, &[0])
}

/// Имя файла заготовки из имени предмета: только буквы, цифры, пробел, `-` и `_`.
pub(crate) fn prefab_file_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = clean.trim();
    if clean.is_empty() {
        "предмет".to_owned()
    } else {
        clean.to_owned()
    }
}

/// Путь от папки `from` до файла `to` через `/` (так его пишут в файле сцены).
fn relative(from: &Path, to: &Path) -> String {
    let full = |p: &Path| {
        p.canonicalize()
            .or_else(|_| std::path::absolute(p))
            .unwrap_or_else(|_| p.to_owned())
    };
    let (from, to) = (full(from), full(to));
    let a: Vec<_> = from.components().collect();
    let b: Vec<_> = to.components().collect();
    let same = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut out: Vec<String> = vec!["..".into(); a.len() - same];
    out.extend(
        b[same..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    out.join("/")
}

/// Свойства сцены (когда ничего не выбрано): фон и общий свет меняются.
pub(crate) fn scene_settings(
    ui: &mut Ui,
    file: &SceneFile,
    name: &str,
    editing: bool,
    actions: &mut Vec<Action>,
) {
    ui.label(
        RichText::new("Свойства сцены")
            .heading()
            .color(Color32::WHITE),
    );
    ui.label(RichText::new(name).small().color(MUTED));
    ui.add_space(GAP_S);
    let scene = |field: &'static str, value: String| Action::Edit {
        edit: Edit {
            path: Vec::new(),
            change: Change::Scene { field, value },
        },
        commit: true,
    };
    card(ui, "Сцена", |ui| {
        egui::Grid::new("scene-props")
            .num_columns(2)
            .spacing(vec2(GAP_XL, GAP_M))
            .show(ui, |ui| {
                prop(ui, "Фон");
                ui.add_enabled_ui(editing, |ui| {
                    let mut bg = file.background;
                    egui::ComboBox::from_id_salt("scene-background")
                        .selected_text(bg_name(bg))
                        .show_ui(ui, |ui| {
                            for b in [Background::Plain, Background::Lab] {
                                ui.selectable_value(&mut bg, b, bg_name(b));
                            }
                        });
                    if bg != file.background {
                        let v = match bg {
                            Background::Plain => "Plain",
                            Background::Lab => "Lab",
                        };
                        actions.push(scene("background", v.into()));
                    }
                });
                ui.end_row();
                prop(ui, "Общий свет");
                ui.horizontal(|ui| {
                    if let Some(v) = color_field(ui, &file.ambient, editing) {
                        actions.push(scene("ambient", v));
                    }
                    let c = color_of(&file.ambient);
                    ui.label(
                        RichText::new(format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).monospace(),
                    );
                });
                ui.end_row();
                prop(ui, "Предметов");
                ui.label(super::outline::count(file).to_string());
                ui.end_row();
                prop(ui, "Слоёв");
                ui.label(file.layers.len().to_string());
                ui.end_row();
            });
    });
    ui.label(
        RichText::new(
            "Выберите предмет в иерархии или щелчком по сцене, чтобы увидеть его свойства. \
             Новый предмет — кнопка «+» над иерархией.",
        )
        .color(TEXT_DIM),
    );
}

fn bg_name(b: Background) -> &'static str {
    match b {
        Background::Plain => "светлый пол",
        Background::Lab => "лаборатория",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Каждая заготовка каталога и частей читается как сцена.
    #[test]
    fn templates_are_valid_scene_text() {
        let base = "(format: \"reyvec-scene\", version: 1, nodes: [(name: \"x\")])";
        for (name, _, t) in CATALOG {
            let text =
                super::super::ron_edit::insert_node(base, &[], 9, &catalog_item(t, [0.5, -0.25]))
                    .unwrap();
            let f = SceneFile::parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(f.nodes[1].at, (0.5, -0.25), "{name}");
        }
        for (name, list, t) in PART_TEMPLATES {
            let text = super::super::ron_edit::insert_item(base, &[0], list, 0, t).unwrap();
            SceneFile::parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        let b = reyvec_scene::BodyDesc {
            fixed: true,
            round: true,
            ..Default::default()
        };
        let t = body_text(&b).to_ron();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let pic = picture_item(
            &root.join("scenes"),
            &root.join("assets/svg/shapes.svg"),
            [0.1, 0.2],
        );
        assert!(pic.contains(r#"svg: "../assets/svg/shapes.svg""#), "{pic}");
        let f = SceneFile::parse(&super::super::ron_edit::insert_node(base, &[], 9, &pic).unwrap())
            .unwrap();
        assert_eq!(f.nodes[1].name, "shapes");
        let item =
            prefab_item("(name: \"ящик\", at: (9.0, 9.0), shapes: [])", [0.5, 0.25]).unwrap();
        let f =
            SceneFile::parse(&super::super::ron_edit::insert_node(base, &[], 9, &item).unwrap())
                .unwrap();
        assert_eq!(f.nodes[1].at, (0.5, 0.25));
        assert_eq!(prefab_file_name("ящик/2?"), "ящик_2_");
        assert_eq!(prefab_file_name(" "), "предмет");
        // Заготовки проекта встают в сцену.
        for e in std::fs::read_dir(root.join("assets/prefabs")).unwrap() {
            let path = e.unwrap().path();
            let t = std::fs::read_to_string(&path).unwrap();
            let item = prefab_item(&t, [0.1, 0.1]).unwrap();
            let text = super::super::ron_edit::insert_node(base, &[], 9, &item).unwrap();
            SceneFile::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
        let text = super::super::ron_edit::set_field(base, &[0], "body", &t).unwrap();
        let f = SceneFile::parse(&text).unwrap();
        let got = f.nodes[0].body.as_ref().unwrap();
        assert!(got.fixed && got.round);
    }
}
