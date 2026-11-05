//! Открытая сцена как документ: текст файла, его разбор, несохранённые правки, отмена и
//! повтор. Правка меняет текст точечно ([`super::ron_edit`]), поэтому отмена — это просто
//! прошлый текст.

use std::path::{Path, PathBuf};

use reyvec_scene::SceneFile;

use super::ron_edit::{self, EditError};

/// Сколько шагов отмены держать.
const HISTORY: usize = 200;

/// Что поменять у предмета.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Change {
    /// Место в координатах родителя.
    At([f64; 2]),
    /// Поворот, градусы.
    Angle(f64),
    Scale(f64),
    Depth(f64),
    Opacity(f64),
    Name(String),
}

/// Правка предмета `path`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Edit {
    pub path: Vec<usize>,
    pub change: Change,
}

/// Открытая сцена.
pub(crate) struct SceneDoc {
    path: Option<PathBuf>,
    /// Текст, который сейчас в редакторе (с несохранёнными правками).
    text: String,
    /// Текст на диске (что было прочитано или сохранено последним).
    saved: String,
    file: Option<SceneFile>,
    error: Option<String>,
    undo: Vec<String>,
    redo: Vec<String>,
    /// Текст до начала перетаскивания: пока тянут ручку, правки идут от него, а в историю
    /// попадает один шаг.
    preview_base: Option<String>,
}

impl SceneDoc {
    pub(crate) fn new(path: Option<PathBuf>, text: String) -> Self {
        let mut doc = Self {
            path,
            saved: text.clone(),
            text,
            file: None,
            error: None,
            undo: Vec::new(),
            redo: Vec::new(),
            preview_base: None,
        };
        doc.reparse();
        doc
    }

    fn dir(&self) -> Option<&Path> {
        self.path.as_deref().and_then(Path::parent)
    }

    fn reparse(&mut self) {
        match SceneFile::parse_in(&self.text, self.dir()) {
            Ok(f) => {
                self.file = Some(f);
                self.error = None;
            }
            // Прошлый разбор остаётся: панели не пустеют от опечатки в блокноте.
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn file(&self) -> Option<&SceneFile> {
        self.file.as_ref()
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Есть несохранённые правки.
    pub(crate) fn dirty(&self) -> bool {
        self.text != self.saved
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub(crate) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Сколько шагов в истории отмены.
    #[cfg(test)]
    pub(crate) fn history_len(&self) -> usize {
        self.undo.len()
    }

    fn patched(base: &str, edit: &Edit) -> Result<String, EditError> {
        let p = &edit.path;
        match &edit.change {
            Change::At(at) => ron_edit::set_field(base, p, "at", &ron_edit::pair(*at)),
            Change::Angle(a) => ron_edit::set_field(base, p, "angle", &ron_edit::num(*a)),
            Change::Scale(s) => ron_edit::set_field(base, p, "scale", &ron_edit::num(s.max(0.001))),
            Change::Depth(d) => ron_edit::set_field(base, p, "depth", &ron_edit::num(*d)),
            Change::Opacity(o) => {
                ron_edit::set_field(base, p, "opacity", &ron_edit::num(o.clamp(0.0, 1.0)))
            }
            Change::Name(n) => ron_edit::set_field(base, p, "name", &ron_edit::string(n)),
        }
    }

    /// Правка, которую ещё тянут мышью: видна сразу, в историю попадёт одним шагом при
    /// [`SceneDoc::commit`].
    pub(crate) fn preview(&mut self, edit: &Edit) -> Result<(), EditError> {
        let base = self.preview_base.get_or_insert_with(|| self.text.clone());
        self.text = Self::patched(base, edit)?;
        self.reparse();
        Ok(())
    }

    /// Готовая правка: шаг истории. Если перед ней шло перетаскивание — от его начала.
    pub(crate) fn commit(&mut self, edit: &Edit) -> Result<(), EditError> {
        let base = self
            .preview_base
            .take()
            .unwrap_or_else(|| self.text.clone());
        let text = Self::patched(&base, edit)?;
        if text != base {
            self.push_undo(base);
            self.redo.clear();
        }
        self.text = text;
        self.reparse();
        Ok(())
    }

    /// Перетаскивание кончилось на том, что уже видно: один шаг истории.
    pub(crate) fn finish_preview(&mut self) {
        if let Some(base) = self.preview_base.take()
            && base != self.text
        {
            self.push_undo(base);
            self.redo.clear();
        }
    }

    /// Новый текст целиком — один шаг истории. Текст, который не читается как сцена, не
    /// принимается.
    fn replace(&mut self, text: String) -> Result<(), EditError> {
        if let Err(e) = SceneFile::parse_in(&text, self.dir()) {
            return Err(EditError(format!("после правки сцена не читается: {e}")));
        }
        self.preview_base = None;
        if text != self.text {
            let old = std::mem::replace(&mut self.text, text);
            self.push_undo(old);
            self.redo.clear();
            self.reparse();
        }
        Ok(())
    }

    /// Текст предмета для буфера обмена.
    pub(crate) fn node_text(&self, path: &[usize]) -> Result<String, EditError> {
        ron_edit::node_text(&self.text, path)
    }

    /// Убирает предмет вместе с вложенными.
    pub(crate) fn delete(&mut self, path: &[usize]) -> Result<(), EditError> {
        let t = ron_edit::remove_node(&self.text, path)?;
        self.replace(t)
    }

    /// Вставляет предмет из буфера в список `parent` на место `index`. Возвращает его путь.
    pub(crate) fn paste(
        &mut self,
        item: &str,
        parent: &[usize],
        index: usize,
    ) -> Result<Vec<usize>, EditError> {
        let index = index.min(self.list_len(parent));
        let t = ron_edit::insert_node(&self.text, parent, index, item)?;
        self.replace(t)?;
        Ok(child(parent, index))
    }

    /// Копия предмета сразу за ним, с именем «ящик 2». Возвращает путь копии.
    pub(crate) fn duplicate(&mut self, path: &[usize]) -> Result<Vec<usize>, EditError> {
        let (parent, i) = split(path)?;
        let item = ron_edit::node_text(&self.text, path)?;
        let mut t = ron_edit::insert_node(&self.text, parent, i + 1, &item)?;
        let new = child(parent, i + 1);
        let name = self
            .file
            .as_ref()
            .and_then(|f| super::outline::node_at(f, path))
            .map(|n| n.name.clone())
            .unwrap_or_default();
        if !name.is_empty() {
            t = ron_edit::set_field(&t, &new, "name", &ron_edit::string(&next_name(&name)))?;
        }
        self.replace(t)?;
        Ok(new)
    }

    /// Переносит предмет в список `parent` на место `index` (номер — как до переноса).
    /// Возвращает новый путь.
    pub(crate) fn move_node(
        &mut self,
        from: &[usize],
        parent: &[usize],
        index: usize,
    ) -> Result<Vec<usize>, EditError> {
        if parent.starts_with(from) {
            return Err(EditError("предмет нельзя вложить в самого себя".into()));
        }
        let (from_parent, i) = split(from)?;
        let item = ron_edit::node_text(&self.text, from)?;
        let cut = ron_edit::remove_node(&self.text, from)?;
        // После выреза номера в том же списке за ним сдвигаются на один.
        let parent_before = parent;
        let mut parent = parent.to_vec();
        let mut index = index;
        let n = from_parent.len();
        if parent.len() > n && parent[..n] == *from_parent && parent[n] > i {
            parent[n] -= 1;
        }
        if parent == from_parent && index > i {
            index -= 1;
        }
        let len = self.list_len(parent_before) - usize::from(parent_before == from_parent);
        let index = index.min(len);
        let t = ron_edit::insert_node(&cut, &parent, index, &item)?;
        if t == self.text {
            return Ok(from.to_vec());
        }
        self.replace(t)?;
        Ok(child(&parent, index))
    }

    /// Сколько предметов в списке `parent`.
    fn list_len(&self, parent: &[usize]) -> usize {
        let Some(f) = &self.file else {
            return 0;
        };
        if parent.is_empty() {
            f.nodes.len()
        } else {
            super::outline::node_at(f, parent).map_or(0, |n| n.children.len())
        }
    }

    /// Перетаскивание бросили (Esc): текст как до него.
    pub(crate) fn cancel_preview(&mut self) {
        if let Some(base) = self.preview_base.take() {
            self.text = base;
            self.reparse();
        }
    }

    fn push_undo(&mut self, text: String) {
        if self.undo.len() == HISTORY {
            self.undo.remove(0);
        }
        self.undo.push(text);
    }

    pub(crate) fn undo(&mut self) -> bool {
        self.preview_base = None;
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(&mut self.text, prev));
        self.reparse();
        true
    }

    pub(crate) fn redo(&mut self) -> bool {
        self.preview_base = None;
        let Some(next) = self.redo.pop() else {
            return false;
        };
        let prev = std::mem::replace(&mut self.text, next);
        self.push_undo(prev);
        self.reparse();
        true
    }

    /// Пишет текст в файл сцены.
    pub(crate) fn save(&mut self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Err("встроенный пример некуда сохранить".into());
        };
        std::fs::write(path, &self.text).map_err(|e| e.to_string())?;
        self.saved = self.text.clone();
        Ok(())
    }

    /// Файл поменяли на диске (в блокноте). Свой же текст после сохранения — не новость.
    /// Возвращает `true`, если текст в редакторе сменился.
    pub(crate) fn file_changed(&mut self, text: String) -> bool {
        if text == self.text {
            self.saved = text;
            return false;
        }
        let old = std::mem::take(&mut self.text);
        self.push_undo(old);
        self.redo.clear();
        self.preview_base = None;
        self.saved = text.clone();
        self.text = text;
        self.reparse();
        true
    }
}

/// Родитель и номер в его списке.
fn split(path: &[usize]) -> Result<(&[usize], usize), EditError> {
    path.split_last()
        .map(|(i, p)| (p, *i))
        .ok_or_else(|| EditError("пустой путь к предмету".into()))
}

fn child(parent: &[usize], i: usize) -> Vec<usize> {
    let mut p = parent.to_vec();
    p.push(i);
    p
}

/// Имя копии: «ящик» → «ящик 2», «ящик 2» → «ящик 3».
pub(crate) fn next_name(name: &str) -> String {
    let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (head, num) = name.split_at(name.len() - digits);
    match num.parse::<u64>() {
        Ok(n) if head.ends_with(' ') => format!("{head}{}", n + 1),
        _ => format!("{name} 2"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r##"(
    format: "reyvec-scene",
    version: 1,
    nodes: [
        (name: "ящик", at: (0.0, 0.0)),
    ],
)
"##;

    fn moved(x: f64) -> Edit {
        Edit {
            path: vec![0],
            change: Change::At([x, 0.0]),
        }
    }

    #[test]
    fn drag_is_one_undo_step() {
        let mut d = SceneDoc::new(None, TEXT.into());
        assert!(!d.dirty() && !d.can_undo());
        d.preview(&moved(0.1)).unwrap();
        d.preview(&moved(0.2)).unwrap();
        d.commit(&moved(0.3)).unwrap();
        assert_eq!(d.file().unwrap().nodes[0].at, (0.3, 0.0));
        assert!(d.dirty());
        assert_eq!(d.history_len(), 1);
        assert!(d.undo());
        assert_eq!(d.file().unwrap().nodes[0].at, (0.0, 0.0));
        assert!(!d.dirty());
        assert!(d.redo());
        assert_eq!(d.file().unwrap().nodes[0].at, (0.3, 0.0));
    }

    #[test]
    fn cancel_and_rename() {
        let mut d = SceneDoc::new(None, TEXT.into());
        d.preview(&moved(0.5)).unwrap();
        d.cancel_preview();
        assert_eq!(d.text(), TEXT);
        d.commit(&Edit {
            path: vec![0],
            change: Change::Name("большой ящик".into()),
        })
        .unwrap();
        assert_eq!(d.file().unwrap().nodes[0].name, "большой ящик");
        assert!(d.save().is_err(), "встроенный пример некуда сохранить");
    }

    const TREE: &str = r##"(
    format: "reyvec-scene",
    version: 1,
    nodes: [
        (name: "стол", children: [(name: "банка"), (name: "кружка")]),
        (name: "ящик 2"),
        (name: "лампа"),
    ],
)
"##;

    impl SceneDoc {
        fn names_at(&self, i: usize) -> Vec<String> {
            let n = &self.file().unwrap().nodes[i];
            let mut v = vec![n.name.clone()];
            v.extend(n.children.iter().map(|c| format!("-{}", c.name)));
            v
        }
    }

    fn names(d: &SceneDoc) -> Vec<String> {
        fn walk(nodes: &[reyvec_scene::NodeDesc], out: &mut Vec<String>, depth: usize) {
            for n in nodes {
                out.push(format!("{}{}", "-".repeat(depth), n.name));
                walk(&n.children, out, depth + 1);
            }
        }
        let mut out = Vec::new();
        walk(&d.file().unwrap().nodes, &mut out, 0);
        out
    }

    #[test]
    fn copy_paste_duplicate_delete_move() {
        let mut d = SceneDoc::new(None, TREE.into());
        assert_eq!(d.duplicate(&[1]).unwrap(), vec![2]);
        assert_eq!(
            names(&d),
            ["стол", "-банка", "-кружка", "ящик 2", "ящик 3", "лампа"]
        );
        let jar = d.node_text(&[0, 0]).unwrap();
        assert_eq!(d.paste(&jar, &[3], 0).unwrap(), vec![3, 0]);
        assert_eq!(d.names_at(3), ["лампа", "-банка"]);
        d.delete(&[0, 1]).unwrap();
        assert_eq!(names(&d)[..2], ["стол", "-банка"]);
        // Лампу — первой в стол, стол — в конец.
        assert_eq!(d.move_node(&[3], &[0], 0).unwrap(), vec![0, 0]);
        assert_eq!(d.move_node(&[0], &[], 99).unwrap(), vec![2]);
        assert_eq!(
            names(&d),
            ["ящик 2", "ящик 3", "стол", "-лампа", "--банка", "-банка"]
        );
        assert!(
            d.move_node(&[2], &[2, 0], 0).is_err(),
            "в самого себя нельзя"
        );
        assert!(
            d.paste("(name: ", &[], 0).is_err(),
            "сломанный текст не вставляется"
        );
        assert_eq!(d.history_len(), 5);
        while d.undo() {}
        assert_eq!(d.text(), TREE);
        assert_eq!(next_name("ящик"), "ящик 2");
        assert_eq!(next_name("ящик 9"), "ящик 10");
        assert_eq!(next_name("x2"), "x2 2");
    }

    #[test]
    fn saves_and_ignores_own_write() {
        let dir = std::env::temp_dir().join(format!("reyvec-doc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scene.ron");
        std::fs::write(&path, TEXT).unwrap();
        let mut d = SceneDoc::new(Some(path.clone()), TEXT.into());
        d.commit(&moved(1.0)).unwrap();
        d.save().unwrap();
        assert!(!d.dirty());
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(!d.file_changed(on_disk), "свой же текст не перечитывается");
        assert!(d.file_changed(TEXT.into()));
        assert_eq!(d.file().unwrap().nodes[0].at, (0.0, 0.0));
        assert!(d.can_undo(), "правку из блокнота можно отменить");
        std::fs::remove_dir_all(&dir).ok();
    }
}
