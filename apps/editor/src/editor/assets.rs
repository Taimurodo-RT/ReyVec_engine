//! Вкладка «Ассеты»: дерево папок проекта слева, строка «крошек» (путь до открытой папки)
//! и плитки файлов справа. Рисунок SVG можно перетащить в сцену — появится предмет с ним.
//! Двойной щелчок по модели формы (`*.shape.ron`) открывает её на подставке «Форм» (М.2).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

use super::shapes::{EXTENSION, is_shape};
use super::theme::*;

/// Папки, которые показываются в дереве: сцены и ассеты проекта.
const ROOTS: [&str; 2] = ["scenes", "assets"];
/// Глубже не заходим: в проекте игры этого хватает, а случайная огромная папка не
/// подвесит редактор.
const MAX_DEPTH: usize = 8;

/// Папка проекта со вложенными папками и файлами.
#[derive(Debug, Default)]
pub(crate) struct Dir {
    pub name: String,
    pub dirs: Vec<Dir>,
    pub files: Vec<String>,
    /// Где папка на диске.
    pub path: PathBuf,
}

impl Dir {
    fn read(path: &Path, name: String, depth: usize) -> Self {
        let mut dir = Dir {
            name,
            path: path.to_owned(),
            ..Default::default()
        };
        let Ok(rd) = std::fs::read_dir(path) else {
            return dir;
        };
        for e in rd.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if e.path().is_dir() {
                if depth < MAX_DEPTH {
                    dir.dirs.push(Dir::read(&e.path(), name, depth + 1));
                }
            } else {
                dir.files.push(name);
            }
        }
        dir.dirs.sort_by(|a, b| a.name.cmp(&b.name));
        dir.files.sort();
        dir
    }

    /// Вложенная папка по номерам: `[1, 0]` — первая папка во второй.
    pub(crate) fn at(&self, path: &[usize]) -> Option<&Dir> {
        path.iter().try_fold(self, |d, &i| d.dirs.get(i))
    }

    /// Имена папок от корня до `path`, корень первым.
    pub(crate) fn crumbs(&self, path: &[usize]) -> Vec<&str> {
        let mut out = vec![self.name.as_str()];
        let mut d = self;
        for &i in path {
            let Some(next) = d.dirs.get(i) else { break };
            out.push(&next.name);
            d = next;
        }
        out
    }
}

/// Состояние вкладки «Ассеты».
pub(crate) struct Assets {
    /// Корень «Проект»; `None`, если папка проекта не нашлась.
    root: Option<Dir>,
    /// Открытая папка (номера от корня).
    current: Vec<usize>,
    /// Раскрытые папки в дереве.
    open: HashSet<Vec<usize>>,
}

impl Assets {
    pub(crate) fn scan(project: Option<&Path>) -> Self {
        let root = project.map(|p| Dir {
            name: "Проект".into(),
            dirs: ROOTS
                .iter()
                .map(|r| Dir::read(&p.join(r), (*r).into(), 1))
                .collect(),
            files: Vec::new(),
            path: p.to_owned(),
        });
        // Сразу открыты сцены: с них начинается работа.
        Self {
            root,
            current: vec![0],
            open: [vec![], vec![0], vec![1]].into(),
        }
    }

    /// Перечитать папки с диска; открытая папка остаётся, если она ещё есть.
    pub(crate) fn rescan(&mut self, project: Option<&Path>) {
        let fresh = Self::scan(project);
        self.root = fresh.root;
        if self
            .root
            .as_ref()
            .and_then(|r| r.at(&self.current))
            .is_none()
        {
            self.current = fresh.current;
        }
    }

    /// Нашлась ли папка проекта и есть ли в ней хоть что-то.
    #[cfg(test)]
    pub(crate) fn root(&self) -> Option<&Dir> {
        self.root.as_ref()
    }

    /// Рисует вкладку; возвращает файл модели формы или сцены 3D, по которому дважды
    /// щёлкнули.
    pub(crate) fn draw(&mut self, ui: &mut Ui) -> Option<PathBuf> {
        let Some(root) = &self.root else {
            ui.label(RichText::new("Папка проекта не найдена").color(MUTED));
            return None;
        };
        let mut go = None;
        let mut opened = None;
        let tree_width = 180.0_f32.min(ui.available_width() * 0.4);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(tree_width);
                egui::ScrollArea::vertical()
                    .id_salt("assets-tree")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        tree(
                            ui,
                            root,
                            &mut Vec::new(),
                            0,
                            &self.current,
                            &mut self.open,
                            &mut go,
                        );
                    });
            });
            ui.separator();
            ui.vertical(|ui| {
                crumbs(ui, root, &self.current, &mut go);
                ui.add_space(GAP_M);
                let dir = root.at(&self.current).unwrap_or(root);
                egui::ScrollArea::vertical()
                    .id_salt("assets-files")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        if dir.dirs.is_empty() && dir.files.is_empty() {
                            ui.label(RichText::new("Пусто").color(MUTED));
                        }
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(GAP_XL, GAP_XL);
                            for (i, d) in dir.dirs.iter().enumerate() {
                                if tile(ui, &d.name, true, false).double_clicked() {
                                    let mut p = self.current.clone();
                                    p.push(i);
                                    go = Some(p);
                                }
                            }
                            let prefabs = dir.name == PREFABS;
                            for f in &dir.files {
                                // Модель формы тащат в открытую сцену 3D (М.8б).
                                let drag = droppable(f) || is_shape(f) || (prefabs && is_prefab(f));
                                let r = tile(ui, f, false, drag);
                                if drag {
                                    r.dnd_set_drag_payload(AssetFile(dir.path.join(f)));
                                }
                                if is_shape(f) {
                                    let r = r.on_hover_text(
                                        "Модель формы: двойной щелчок — открыть на подставке; \
                                         в открытую сцену 3D её можно перетащить",
                                    );
                                    if r.double_clicked() {
                                        opened = Some(dir.path.join(f));
                                    }
                                } else if !prefabs
                                    && f.to_lowercase().ends_with(".ron")
                                    && r.double_clicked()
                                    && is_space_scene(&dir.path.join(f))
                                {
                                    // Сцена 3D (М.8) открывается, как модель: в окне сцены.
                                    opened = Some(dir.path.join(f));
                                }
                            }
                        });
                    });
            });
        });
        if let Some(p) = go {
            // Открыть папку — значит и раскрыть путь к ней в дереве.
            for n in 0..=p.len() {
                self.open.insert(p[..n].to_vec());
            }
            self.current = p;
        }
        opened
    }
}

/// Дерево папок: строка на папку, стрелка раскрывает, щелчок открывает.
fn tree(
    ui: &mut Ui,
    dir: &Dir,
    path: &mut Vec<usize>,
    depth: usize,
    current: &[usize],
    open: &mut HashSet<Vec<usize>>,
    go: &mut Option<Vec<usize>>,
) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let selected = path.as_slice() == current;
    if selected {
        ui.painter().rect_filled(rect, RADIUS_S, SELECT_BG);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, RADIUS_S, HOVER);
    }
    let is_open = open.contains(path.as_slice());
    let x = rect.left() + GAP_S + depth as f32 * INDENT;
    let arrow = Rect::from_min_size(pos2(x, rect.top()), vec2(INDENT, rect.height()));
    if !dir.dirs.is_empty() {
        let c = arrow.center();
        let s = Stroke::new(1.3, MUTED);
        let pts = if is_open {
            [
                c + vec2(-4.0, -2.0),
                c + vec2(0.0, 2.0),
                c + vec2(4.0, -2.0),
            ]
        } else {
            [
                c + vec2(-2.0, -4.0),
                c + vec2(2.0, 0.0),
                c + vec2(-2.0, 4.0),
            ]
        };
        ui.painter().line(pts.to_vec(), s);
    }
    let color = if selected { egui::Color32::WHITE } else { TEXT };
    ui.painter().text(
        pos2(arrow.right() + GAP_XS, rect.center().y),
        Align2::LEFT_CENTER,
        &dir.name,
        FontId::proportional(FONT_BODY),
        color,
    );
    if response.clicked() {
        let on_arrow = response
            .interact_pointer_pos()
            .is_some_and(|p| arrow.contains(p));
        if on_arrow {
            if !open.remove(path.as_slice()) {
                open.insert(path.clone());
            }
        } else {
            *go = Some(path.clone());
        }
    }
    if open.contains(path.as_slice()) {
        for (i, d) in dir.dirs.iter().enumerate() {
            path.push(i);
            tree(ui, d, path, depth + 1, current, open, go);
            path.pop();
        }
    }
}

/// Строка «крошек»: Проект › assets › svg. Щелчок по звену открывает эту папку.
fn crumbs(ui: &mut Ui, root: &Dir, current: &[usize], go: &mut Option<Vec<usize>>) {
    let names = root.crumbs(current);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP_XS;
        let last = names.len() - 1;
        for (n, name) in names.iter().enumerate() {
            if n > 0 {
                ui.label(RichText::new("›").color(MUTED));
            }
            let text = RichText::new(*name).color(if n == last { TEXT } else { TEXT_DIM });
            let button = egui::Button::new(text)
                .fill(egui::Color32::TRANSPARENT)
                .stroke(Stroke::NONE)
                .corner_radius(RADIUS_S);
            if ui.add(button).clicked() && n != last {
                *go = Some(current[..n].to_vec());
            }
        }
    });
}

/// Файл, который тащат из Ассетов в сцену.
#[derive(Clone, Debug)]
pub(crate) struct AssetFile(pub PathBuf);

/// Папка заготовок в `assets`.
pub(crate) const PREFABS: &str = "prefabs";

/// Файл — сцена 3D (формат `reyvec-space`), а не сцена 2D.
pub(crate) fn is_space_scene(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| t.contains("\"reyvec-space\""))
}

/// Заготовка: предмет, записанный в свой файл `.ron` в папке `assets/prefabs`.
fn is_prefab(name: &str) -> bool {
    name.to_lowercase().ends_with(".ron")
}

/// Такой файл можно бросить в сцену: рисунок SVG станет предметом.
fn droppable(name: &str) -> bool {
    name.to_lowercase().ends_with(".svg")
}

/// Вид файла для плитки: подпись и цвет, чтобы файлы разных видов различались с
/// первого взгляда. `prefab` — файл из папки заготовок.
pub(crate) fn kind_of(name: &str, folder: bool, prefab: bool) -> (&'static str, Color32) {
    let lower = name.to_lowercase();
    let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
    match ext {
        _ if folder => ("папка", ACCENT),
        _ if is_shape(&lower) => ("модель формы", FILE_SHAPE),
        "ron" if prefab => ("заготовка", KIND_SCRIPT),
        "ron" => ("сцена", SELECT),
        "svg" => ("рисунок", GOOD),
        "png" | "jpg" | "jpeg" | "webp" | "ktx2" => ("картинка", FILE_IMAGE),
        "glb" | "gltf" | "obj" => ("3D-сетка", KIND_PLAYER),
        "wav" | "ogg" | "mp3" | "flac" => ("звук", FILE_SOUND),
        "ttf" | "otf" => ("шрифт", FILE_FONT),
        "slang" | "rs" | "lua" | "rhai" => ("код", BAD),
        _ => ("файл", MUTED),
    }
}

/// Плитка файла или папки: цветная полоса и подпись по виду файла.
fn tile(ui: &mut Ui, name: &str, folder: bool, drag: bool) -> egui::Response {
    let sense = if drag {
        Sense::click_and_drag()
    } else {
        Sense::click()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(96.0, 92.0), sense);
    let thumb = Rect::from_min_size(rect.min, vec2(rect.width(), 64.0));
    let prefab = drag && !droppable(name);
    let (kind, color) = kind_of(name, folder, prefab);
    let fill = if response.hovered() { HOVER } else { CARD };
    let painter = ui.painter();
    painter.rect(
        thumb,
        RADIUS_M,
        fill,
        Stroke::new(LINE, BORDER_STRONG),
        egui::StrokeKind::Inside,
    );
    // Цвет вида: лёгкая заливка и полоса сверху.
    painter.rect_filled(thumb.shrink(LINE), RADIUS_M, color.gamma_multiply(0.08));
    let band = Rect::from_min_size(
        thumb.min + vec2(LINE, LINE),
        vec2(thumb.width() - 2.0 * LINE, 3.0),
    );
    painter.rect_filled(
        band,
        egui::CornerRadius {
            nw: RADIUS_M - 1,
            ne: RADIUS_M - 1,
            sw: 0,
            se: 0,
        },
        color,
    );
    let ext = if folder {
        "папка".to_owned()
    } else if is_shape(name) {
        EXTENSION.to_owned()
    } else {
        name.rsplit_once('.')
            .map_or_else(String::new, |(_, e)| format!(".{e}"))
    };
    let lift = if folder { 0.0 } else { 6.0 };
    painter.text(
        thumb.center() - vec2(0.0, lift),
        Align2::CENTER_CENTER,
        ext,
        FontId::monospace(FONT_SMALL),
        color,
    );
    if !folder {
        painter.text(
            thumb.center() + vec2(0.0, 10.0),
            Align2::CENTER_CENTER,
            kind,
            FontId::proportional(FONT_SMALL - 1.5),
            MUTED,
        );
    }
    let short: String = if name.chars().count() > 14 {
        name.chars().take(13).chain(['…']).collect()
    } else {
        name.to_owned()
    };
    ui.painter().text(
        pos2(rect.left() + GAP_XS, thumb.bottom() + GAP_L),
        Align2::LEFT_TOP,
        short,
        FontId::proportional(FONT_SMALL),
        TEXT_DIM,
    );
    let hint = if folder {
        format!("{name}\nДвойной щелчок — открыть папку")
    } else if drag && droppable(name) {
        format!("{name}\nПеретащите в сцену — появится предмет с этим рисунком")
    } else if drag {
        format!("{name}\nЗаготовка: перетащите в сцену — появится такой же предмет")
    } else {
        format!("{name}\n{}", capital(kind))
    };
    response.on_hover_text(hint)
}

/// Слово с большой буквы.
fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_kinds_have_their_own_colours() {
        let kinds = [
            kind_of("стеллаж.shape.ron", false, false),
            kind_of("first-room.ron", false, false),
            kind_of("ящик.ron", false, true),
            kind_of("hero.SVG", false, false),
            kind_of("table.glb", false, false),
            kind_of("музыка.ogg", false, false),
            kind_of("scenes", true, false),
        ];
        assert_eq!(kinds[0].0, "модель формы");
        assert_eq!(kinds[1].0, "сцена");
        assert_eq!(kinds[2].0, "заготовка");
        assert_eq!(kinds[3].0, "рисунок");
        for (i, a) in kinds.iter().enumerate() {
            for b in &kinds[i + 1..] {
                assert_ne!(a.1, b.1, "{} и {} одного цвета", a.0, b.0);
            }
        }
        assert_eq!(capital("звук"), "Звук");
    }

    fn project() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn project_tree_has_scenes_and_assets() {
        let a = Assets::scan(Some(&project()));
        let root = a.root().expect("папка проекта");
        let names: Vec<_> = root.dirs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ROOTS);
        assert!(root.dirs[0].files.iter().any(|f| f == "first-room.ron"));
        assert!(Assets::scan(None).root().is_none());
    }

    #[test]
    fn crumbs_follow_path() {
        let root = Dir {
            name: "Проект".into(),
            dirs: vec![Dir {
                name: "assets".into(),
                dirs: vec![Dir {
                    name: "svg".into(),
                    ..Default::default()
                }],
                files: vec![],
                ..Default::default()
            }],
            files: vec![],
            ..Default::default()
        };
        assert_eq!(root.crumbs(&[]), ["Проект"]);
        assert_eq!(root.crumbs(&[0, 0]), ["Проект", "assets", "svg"]);
        // Путь, которого больше нет (папку удалили), обрезается.
        assert_eq!(root.crumbs(&[0, 5]), ["Проект", "assets"]);
        assert_eq!(root.at(&[0, 0]).map(|d| d.name.as_str()), Some("svg"));
        assert!(root.at(&[3]).is_none());
    }
}
