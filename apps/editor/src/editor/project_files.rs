//! Файлы проекта для проводника «Ассетов» (Э.1): создать, переименовать, переместить,
//! скопировать и удалить — с отменой (Ctrl+Z) и починкой ссылок.
//!
//! Удалённое не пропадает: оно уезжает в скрытую папку `.trash` проекта, откуда его
//! возвращает отмена. Сцены и модели ссылаются на файлы путями в кавычках («scripts/coin.luau»,
//! «завод/конвейер», «../assets/svg/illustration.svg»). Когда файл или папку переименовали
//! или перенесли, такие пути во всех `.ron` проекта переписываются, чтобы сцены не
//! сломались; отмена возвращает и их.

use std::path::{Component, Path, PathBuf};

/// Папки проекта, которые видит проводник.
pub(crate) const ROOTS: [&str; 2] = ["scenes", "assets"];
/// Скрытая папка, куда уезжает удалённое.
const TRASH: &str = ".trash";
/// Записка в ячейке корзины: откуда удалили (путь от проекта, через «/»).
const FROM: &str = ".откуда";
/// Окончание файла модели формы: «имя.shape.ron» — одно имя, а не «имя.shape» + «.ron».
const DOUBLE_EXT: [&str; 1] = [".shape.ron"];
/// Ссылка на модель пишется без окончания: `Use(model: "завод/конвейер")`.
const MODEL_EXT: &str = ".shape.ron";

/// Что сделано с файлами: по этому делается отмена.
#[derive(Debug)]
pub(crate) enum Done {
    /// Перенесены или переименованы (откуда, куда) и переписаны ссылки.
    Moved {
        moves: Vec<(PathBuf, PathBuf)>,
        refs: Vec<RefEdit>,
    },
    /// Созданы (новая папка, новый ассет, вставка копий).
    Created(Vec<PathBuf>),
    /// Удалены: (где были, где лежат в корзине).
    Trashed(Vec<(PathBuf, PathBuf)>),
}

/// Файл, в котором переписали ссылки: текст до и после.
#[derive(Debug)]
pub(crate) struct RefEdit {
    pub file: PathBuf,
    pub before: String,
    pub after: String,
}

/// Имя файла на «основу» и окончание: «стеллаж.shape.ron» → («стеллаж», «.shape.ron»).
pub(crate) fn split_name(name: &str) -> (&str, &str) {
    let lower = name.to_lowercase();
    for ext in DOUBLE_EXT {
        if lower.ends_with(ext) && name.len() > ext.len() {
            return name.split_at(name.len() - ext.len());
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// Свободное имя в папке: «Новая папка», «Новая папка 2»… Окончание сохраняется.
pub(crate) fn free_name(dir: &Path, name: &str) -> PathBuf {
    let (stem, ext) = split_name(name);
    let mut p = dir.join(name);
    let mut k = 2;
    while p.exists() {
        p = dir.join(format!("{stem} {k}{ext}"));
        k += 1;
    }
    p
}

/// Можно ли так назвать файл в Windows. Ошибка — что не так, словами.
pub(crate) fn check_name(name: &str) -> Result<(), String> {
    let name_t = name.trim();
    if name_t.is_empty() {
        return Err("Имя не может быть пустым".into());
    }
    if let Some(c) = name
        .chars()
        .find(|c| r#"\/:*?"<>|"#.contains(*c) || c.is_control())
    {
        return Err(format!("В имени нельзя знак «{c}»"));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err("Имя не может кончаться точкой или пробелом".into());
    }
    if name.starts_with('.') {
        return Err("Имя с точки в начале — скрытый файл, проводник его не покажет".into());
    }
    let (stem, _) = split_name(name);
    let reserved = ["CON", "PRN", "AUX", "NUL"];
    let upper = stem.to_uppercase();
    let numbered = (upper.starts_with("COM") || upper.starts_with("LPT"))
        && upper.len() == 4
        && upper.as_bytes()[3].is_ascii_digit();
    if reserved.contains(&upper.as_str()) || numbered {
        return Err(format!("Имя «{stem}» Windows держит для себя"));
    }
    Ok(())
}

/// Путь без «.» и «..» (на диск не смотрит).
pub(crate) fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Ключ для сравнения путей: в Windows регистр букв не важен.
fn key(p: &Path) -> String {
    let s = normalize(p).to_string_lossy().replace('\\', "/");
    if cfg!(windows) { s.to_lowercase() } else { s }
}

fn same(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}

/// `p` лежит внутри папки `dir` (не сама папка).
pub(crate) fn inside(p: &Path, dir: &Path) -> bool {
    let (p, d) = (key(p), key(dir));
    p.len() > d.len() && p.starts_with(&d) && p.as_bytes()[d.len()] == b'/'
}

/// Путь `to` относительно папки `from`, через «/»: «../assets/svg/x.svg».
fn relative(to: &Path, from: &Path) -> String {
    let (to, from) = (normalize(to), normalize(from));
    let to: Vec<_> = to.components().collect();
    let from: Vec<_> = from.components().collect();
    let common = to
        .iter()
        .zip(&from)
        .take_while(|(a, b)| same(Path::new(a.as_os_str()), Path::new(b.as_os_str())))
        .count();
    let mut parts: Vec<String> = vec!["..".into(); from.len() - common];
    parts.extend(
        to[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

/// Новое место пути после переносов: сам перенесённый файл или что-то внутри перенесённой
/// папки. `None` — путь переносы не задели.
fn moved_to(p: &Path, moves: &[(PathBuf, PathBuf)]) -> Option<PathBuf> {
    for (from, to) in moves {
        if same(p, from) {
            return Some(to.clone());
        }
        if inside(p, from) {
            let rest = normalize(p);
            let rest = rest.strip_prefix(normalize(from)).ok()?;
            return Some(to.join(rest));
        }
    }
    None
}

/// Все текстовые файлы проекта, где бывают ссылки: `.ron` в `scenes` и `assets`.
fn text_files(project: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let path = e.path();
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if depth < 10 {
                    walk(&path, out, depth + 1);
                }
            } else if path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("ron"))
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    for r in ROOTS {
        walk(&project.join(r), &mut out, 0);
    }
    out.sort();
    out
}

/// Откуда считаются пути в файле: его папка и папки выше (до проекта), и у каждой — её
/// `models` (модели сцен 3D: `Use(model: "завод/конвейер")`, глина `"кузница/x.clay"`).
/// Порядок: ближние раньше. Номер — сколько папок вверх, и с `models` ли.
fn bases(file: &Path, project: &Path) -> Vec<((usize, bool), PathBuf)> {
    let mut out = Vec::new();
    let mut dir = file.parent();
    let mut up = 0;
    while let Some(d) = dir {
        out.push(((up, false), d.to_path_buf()));
        out.push(((up, true), d.join("models")));
        if same(d, project) || !inside(d, project) {
            break;
        }
        dir = d.parent();
        up += 1;
    }
    out
}

/// Та же «база», но у файла на новом месте.
fn base_at(file: &Path, (up, models): (usize, bool)) -> Option<PathBuf> {
    let mut d = file.parent()?;
    for _ in 0..up {
        d = d.parent()?;
    }
    Some(if models {
        d.join("models")
    } else {
        d.to_path_buf()
    })
}

/// Строка в кавычках похожа на путь: есть «/» или окончание файла.
fn looks_like_path(s: &str) -> bool {
    if s.is_empty() || s.len() > 400 || s.contains('\n') || s.starts_with('#') {
        return false;
    }
    let last = s.rsplit('/').next().unwrap_or(s);
    s.contains('/') || last.rfind('.').is_some_and(|i| i > 0 && i + 1 < last.len())
}

/// Строки в кавычках текста: (начало, конец) — байты между кавычками.
fn quoted(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            // Комментарий до конца строки: в нём пути не правим.
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' => {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && b[j] != b'"' && b[j] != b'\n' {
                    if b[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                if j < b.len() && b[j] == b'"' {
                    out.push((start, j));
                }
                i = j;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Куда ведёт строка-путь из файла: (база, полный путь, с окончанием модели ли).
/// Берётся первая база, от которой путь существует на диске.
fn resolve(s: &str, bases: &[((usize, bool), PathBuf)]) -> Option<((usize, bool), PathBuf, bool)> {
    for (id, base) in bases {
        let full = normalize(&base.join(s));
        if looks_like_path(s) && full.exists() {
            return Some((*id, full, false));
        }
        let model = normalize(&base.join(format!("{s}{MODEL_EXT}")));
        if model.is_file() {
            return Some((*id, model, true));
        }
    }
    None
}

/// Переписать пути в текстах проекта под переносы `moves` (до самого переноса: файлы
/// ещё на старых местах). Возвращает правки: какой файл (уже на НОВОМ месте) и тексты.
pub(crate) fn plan_refs(project: &Path, moves: &[(PathBuf, PathBuf)]) -> Vec<RefEdit> {
    let mut edits = Vec::new();
    for file in text_files(project) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let new_file = moved_to(&file, moves).unwrap_or_else(|| file.clone());
        let bases = bases(&file, project);
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for (a, b) in quoted(&text) {
            let s = &text[a..b];
            if s.contains('\\') {
                continue;
            }
            let Some((id, target, model)) = resolve(s, &bases) else {
                continue;
            };
            // Ссылка «на саму папку» без «/» — скорее имя материала, чем путь.
            if target.is_dir() && !s.contains('/') {
                continue;
            }
            let new_target = moved_to(&target, moves);
            if new_target.is_none() && same(&new_file, &file) {
                continue;
            }
            let new_target = new_target.unwrap_or(target);
            let Some(new_base) = base_at(&new_file, id) else {
                continue;
            };
            let mut rel = relative(&new_target, &new_base);
            if model && let Some(stripped) = rel.strip_suffix(MODEL_EXT) {
                rel = stripped.to_owned();
            }
            if rel != s {
                out.push_str(&text[last..a]);
                out.push_str(&rel);
                last = b;
            }
        }
        if last > 0 {
            out.push_str(&text[last..]);
            edits.push(RefEdit {
                file: new_file,
                before: text,
                after: out,
            });
        }
    }
    edits
}

/// Кто ссылается на файл или папку: (файл, сколько раз).
pub(crate) fn used_in(project: &Path, target: &Path) -> Vec<(PathBuf, usize)> {
    let mut out = Vec::new();
    for file in text_files(project) {
        if same(&file, target) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let bases = bases(&file, project);
        let n = quoted(&text)
            .into_iter()
            .filter(|&(a, b)| {
                let s = &text[a..b];
                resolve(s, &bases).is_some_and(|(_, t, _)| {
                    same(&t, target) || (inside(&t, target) && s.contains('/'))
                })
            })
            .count();
        if n > 0 {
            out.push((file, n));
        }
    }
    out
}

/// Перенести или переименовать файлы и папки, переписав ссылки на них.
pub(crate) fn move_all(project: &Path, moves: Vec<(PathBuf, PathBuf)>) -> Result<Done, String> {
    for (from, to) in &moves {
        if same(from, to) {
            return Err("Файл уже здесь".into());
        }
        if inside(to, from) {
            return Err(format!(
                "Папку «{}» нельзя положить в саму себя",
                file_name(from)
            ));
        }
        // В Windows «a» и «A» — одно имя: переименовать в другой регистр можно.
        if to.exists() && key(from) != key(to) {
            return Err(format!("Здесь уже есть «{}»", file_name(to)));
        }
    }
    let refs = plan_refs(project, &moves);
    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (from, to) in &moves {
        if let Err(e) = rename(from, to) {
            // Вернуть уже сделанное, чтобы не остаться на полпути.
            for (f, t) in done.iter().rev() {
                let _ = rename(t, f);
            }
            return Err(format!("Не перенести «{}»: {e}", file_name(from)));
        }
        done.push((from.clone(), to.clone()));
    }
    for e in &refs {
        if let Err(err) = std::fs::write(&e.file, &e.after) {
            tracing::warn!("Ссылки в «{}» не переписаны: {err}", e.file.display());
        }
    }
    super::labels::follow(project, &done);
    Ok(Done::Moved { moves: done, refs })
}

/// Переименование, которое понимает смену одного регистра в Windows (через временное имя).
fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    if key(from) == key(to) && from != to {
        let tmp = from.with_file_name(format!(".{}.rename", file_name(from)));
        std::fs::rename(from, &tmp)?;
        return std::fs::rename(&tmp, to);
    }
    std::fs::rename(from, to)
}

/// Скопировать файлы и папки в папку `dir`; имя занято — «имя - копия».
pub(crate) fn copy_into(project: &Path, dir: &Path, items: &[PathBuf]) -> Result<Done, String> {
    let mut made = Vec::new();
    let mut pairs = Vec::new();
    for item in items {
        if same(item, dir) || inside(dir, item) {
            return Err(format!(
                "Папку «{}» нельзя скопировать в саму себя",
                file_name(item)
            ));
        }
        let name = file_name(item);
        let mut to = dir.join(&name);
        if to.exists() {
            let (stem, ext) = split_name(&name);
            to = free_name(dir, &format!("{stem} - копия{ext}"));
        }
        copy_all(item, &to).map_err(|e| format!("Не скопировать «{name}»: {e}"))?;
        pairs.push((item.clone(), to.clone()));
        made.push(to);
    }
    super::labels::copied(project, &pairs);
    Ok(Done::Created(made))
}

fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_all(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Новая папка или новый файл с текстом `text` (`None` — папка).
pub(crate) fn create(
    dir: &Path,
    name: &str,
    text: Option<&str>,
) -> Result<(Done, PathBuf), String> {
    let path = free_name(dir, name);
    let made = match text {
        None => std::fs::create_dir(&path),
        Some(t) => std::fs::write(&path, t),
    };
    made.map_err(|e| format!("Не создать «{}»: {e}", file_name(&path)))?;
    Ok((Done::Created(vec![path.clone()]), path))
}

/// Удалить: унести в скрытую корзину проекта (`.trash/<номер>/имя`).
pub(crate) fn trash(project: &Path, items: &[PathBuf]) -> Result<Done, String> {
    let bin = project.join(TRASH);
    let mut gone = Vec::new();
    for item in items {
        let mut k = 1;
        let mut slot = bin.join(k.to_string());
        while slot.exists() {
            k += 1;
            slot = bin.join(k.to_string());
        }
        let to = slot.join(file_name(item));
        let moved = std::fs::create_dir_all(&slot).and_then(|()| std::fs::rename(item, &to));
        if let Err(e) = moved {
            for (f, t) in gone.iter().rev() {
                let _ = std::fs::rename(t, f);
            }
            return Err(format!("Не удалить «{}»: {e}", file_name(item)));
        }
        if let Ok(rel) = item.strip_prefix(project) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            let _ = std::fs::write(slot.join(FROM), rel);
        }
        gone.push((item.clone(), to));
    }
    super::labels::follow(project, &gone);
    Ok(Done::Trashed(gone))
}

/// Удалённое в корзине: где лежит сейчас, откуда удалили и когда.
#[derive(Clone, Debug)]
pub(crate) struct Binned {
    /// Ячейка корзины (`.trash/<номер>`).
    pub slot: PathBuf,
    /// Сам файл или папка в ячейке.
    pub item: PathBuf,
    /// Откуда удалили; `None` у удалённого до того, как корзина стала это помнить.
    pub from: Option<PathBuf>,
    pub when: Option<std::time::SystemTime>,
    pub folder: bool,
    pub size: u64,
}

/// Что лежит в корзине проекта, недавно удалённое первым.
pub(crate) fn trash_list(project: &Path) -> Vec<Binned> {
    let Ok(slots) = std::fs::read_dir(project.join(TRASH)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for slot in slots.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let note = slot.join(FROM);
        let from = std::fs::read_to_string(&note)
            .ok()
            .map(|rel| project.join(rel.trim()));
        let Ok(inner) = std::fs::read_dir(&slot) else {
            continue;
        };
        for item in inner.flatten().map(|e| e.path()) {
            if file_name(&item) == FROM {
                continue;
            }
            let meta = std::fs::metadata(&item).ok();
            let when = std::fs::metadata(&note)
                .or_else(|_| std::fs::metadata(&slot))
                .and_then(|m| m.modified())
                .ok();
            out.push(Binned {
                folder: item.is_dir(),
                size: size_of(&item),
                slot: slot.clone(),
                from: from.clone().filter(|_| meta.is_some()),
                item,
                when,
            });
        }
    }
    out.sort_by(|a, b| b.when.cmp(&a.when).then_with(|| a.item.cmp(&b.item)));
    out
}

/// Размер файла или всего в папке.
fn size_of(p: &Path) -> u64 {
    if p.is_dir() {
        std::fs::read_dir(p)
            .map(|d| d.flatten().map(|e| size_of(&e.path())).sum())
            .unwrap_or(0)
    } else {
        std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
    }
}

/// Куда вернуть: на старое место; если имя там занято — рядом с номером; если старое
/// место забыто — в `assets`.
fn restore_target(project: &Path, b: &Binned) -> PathBuf {
    let name = file_name(&b.item);
    let want = b
        .from
        .clone()
        .unwrap_or_else(|| project.join(ROOTS[1]).join(&name));
    let dir = want
        .parent()
        .map_or_else(|| project.to_path_buf(), Path::to_path_buf);
    if want.exists() {
        free_name(&dir, &name)
    } else {
        want
    }
}

/// Вернуть из корзины выбранное (отмена унесёт его обратно в корзину).
pub(crate) fn restore_binned(project: &Path, items: &[Binned]) -> Result<Done, String> {
    let mut back = Vec::new();
    for b in items {
        let to = restore_target(project, b);
        let moved = to
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::rename(&b.item, &to));
        if let Err(e) = moved {
            if back.is_empty() {
                return Err(format!("Не вернуть «{}»: {e}", file_name(&b.item)));
            }
            break;
        }
        super::labels::follow(project, &[(b.item.clone(), to.clone())]);
        drop_slot(&b.slot);
        back.push(to);
    }
    Ok(Done::Created(back))
}

/// Пустую ячейку корзины убрать вместе с запиской.
fn drop_slot(slot: &Path) {
    let only_note = std::fs::read_dir(slot)
        .map(|d| d.flatten().all(|e| file_name(&e.path()) == FROM))
        .unwrap_or(false);
    if only_note {
        let _ = std::fs::remove_file(slot.join(FROM));
        let _ = std::fs::remove_dir(slot);
    }
}

/// Стереть насовсем (без отмены). Возвращает, сколько стёрто.
pub(crate) fn erase(items: &[Binned]) -> Result<usize, String> {
    let mut n = 0;
    for b in items {
        let gone = if b.item.is_dir() {
            std::fs::remove_dir_all(&b.item)
        } else {
            std::fs::remove_file(&b.item)
        };
        gone.map_err(|e| format!("Не стереть «{}»: {e}", file_name(&b.item)))?;
        // Ячейка лежит в `<проект>/.trash/<номер>`.
        if let Some(project) = b.slot.parent().and_then(Path::parent) {
            super::labels::erased(project, std::slice::from_ref(&b.item));
        }
        drop_slot(&b.slot);
        n += 1;
    }
    Ok(n)
}

/// Как повторить отменённое (Ctrl+Y).
#[derive(Debug)]
pub(crate) enum Redo {
    /// Перенести снова (ссылки пересчитываются заново).
    Move(Vec<(PathBuf, PathBuf)>),
    /// Достать из корзины созданное, которое отмена туда унесла.
    Restore(Vec<(PathBuf, PathBuf)>),
    /// Удалить снова.
    Trash(Vec<PathBuf>),
}

/// Отменить сделанное. Возвращает, что вернули (пути, которые выбрать), и как повторить.
pub(crate) fn undo(project: &Path, done: Done) -> Result<(Vec<PathBuf>, Redo), String> {
    match done {
        Done::Moved { moves, refs } => {
            for (from, to) in moves.iter().rev() {
                if from.exists() && key(from) != key(to) {
                    return Err(format!(
                        "Не вернуть «{}»: на старом месте уже есть файл",
                        file_name(from)
                    ));
                }
            }
            for e in &refs {
                // Файл после переноса правили руками — его не трогаем.
                if std::fs::read_to_string(&e.file).is_ok_and(|t| t == e.after) {
                    let _ = std::fs::write(&e.file, &e.before);
                }
            }
            for (from, to) in moves.iter().rev() {
                rename(to, from).map_err(|e| format!("Не вернуть «{}»: {e}", file_name(to)))?;
                super::labels::follow(project, &[(to.clone(), from.clone())]);
            }
            let back = moves.iter().map(|(f, _)| f.clone()).collect();
            Ok((back, Redo::Move(moves)))
        }
        Done::Created(paths) => {
            // Созданное не стирается насовсем: вдруг в него уже что-то записали.
            let Done::Trashed(gone) = trash(project, &paths)? else {
                unreachable!("trash возвращает Trashed");
            };
            Ok((Vec::new(), Redo::Restore(gone)))
        }
        Done::Trashed(gone) => {
            let back = restore(project, &gone)?;
            Ok((back.clone(), Redo::Trash(back)))
        }
    }
}

/// Повторить отменённое; возвращает, что сделано (для новой отмены).
pub(crate) fn redo(project: &Path, redo: Redo) -> Result<Done, String> {
    match redo {
        Redo::Move(moves) => move_all(project, moves),
        Redo::Restore(gone) => restore(project, &gone).map(Done::Created),
        Redo::Trash(paths) => trash(project, &paths),
    }
}

/// Вернуть из корзины на старые места.
fn restore(project: &Path, gone: &[(PathBuf, PathBuf)]) -> Result<Vec<PathBuf>, String> {
    let mut back = Vec::new();
    for (from, at) in gone.iter().rev() {
        if from.exists() {
            return Err(format!("Не вернуть «{}»: имя уже занято", file_name(from)));
        }
        std::fs::rename(at, from).map_err(|e| format!("Не вернуть «{}»: {e}", file_name(from)))?;
        super::labels::follow(project, &[(at.clone(), from.clone())]);
        if let Some(slot) = at.parent() {
            drop_slot(slot);
        }
        back.push(from.clone());
    }
    back.reverse();
    Ok(back)
}

pub(crate) fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Новый ассет: что можно создать в папке и с каким текстом.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NewKind {
    Folder,
    Scene2D,
    Scene3D,
    Shape,
    Script,
}

impl NewKind {
    pub(crate) const ALL: [NewKind; 5] = [
        NewKind::Folder,
        NewKind::Scene2D,
        NewKind::Scene3D,
        NewKind::Shape,
        NewKind::Script,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            NewKind::Folder => "Папка",
            NewKind::Scene2D => "Сцена 2D",
            NewKind::Scene3D => "Сцена 3D",
            NewKind::Shape => "Модель",
            NewKind::Script => "Скрипт",
        }
    }

    pub(crate) fn file_name(self) -> &'static str {
        match self {
            NewKind::Folder => "Новая папка",
            NewKind::Scene2D => "новая сцена.ron",
            NewKind::Scene3D => "новая сцена 3D.ron",
            NewKind::Shape => "новая модель.shape.ron",
            NewKind::Script => "новый скрипт.luau",
        }
    }

    /// Окончание имени файла этого вида.
    pub(crate) fn ext(self) -> &'static str {
        split_name(self.file_name()).1
    }

    /// Имя по умолчанию без окончания.
    pub(crate) fn stem(self) -> &'static str {
        split_name(self.file_name()).0
    }

    pub(crate) fn text(self) -> Option<&'static str> {
        match self {
            NewKind::Folder => None,
            NewKind::Scene2D => Some(super::files::NEW_SCENE),
            NewKind::Scene3D => Some(NEW_SPACE),
            NewKind::Shape => Some(NEW_SHAPE),
            NewKind::Script => Some(NEW_SCRIPT),
        }
    }
}

/// Новая сцена 3D: пол, лампа и куб.
const NEW_SPACE: &str = r#"// Новая сцена 3D. Её можно править в редакторе («Ассеты» → двойной щелчок) или здесь.
(
    format: "reyvec-space",
    version: 1,
    camera: Some((eye: (3.0, 2.2, 4.0), look_at: (0.0, 0.5, 0.0))),
    materials: [
        (name: "пол", color: (0.18, 0.19, 0.21), grid: true),
        (name: "глина", color: (0.75, 0.55, 0.42)),
    ],
    lamps: [
        (at: (1.5, 2.5, 2.0), color: (1.0, 0.9, 0.75), power: 6.0, range: 8.0, bulb: 0.08),
    ],
    objects: [
        (name: "пол", material: "пол", at: (0.0, -0.05, 0.0), shape: Box(size: (8.0, 0.1, 8.0))),
        (name: "куб", material: "глина", at: (0.0, 0.4, 0.0), shape: Box(size: (0.8, 0.8, 0.8))),
    ],
)
"#;

/// Новая модель формы: один брусок.
const NEW_SHAPE: &str = r#"// Новая модель формы. Двойной щелчок в «Ассетах» открывает её на подставке.
(
    format: "reyvec-shape",
    version: 1,
    shape: Box(size: (0.5, 0.5, 0.5)),
)
"#;

/// Новый скрипт: пустой `update`.
const NEW_SCRIPT: &str = "-- Новый скрипт. Движок зовёт update каждый кадр.\n\
-- me — предмет со скриптом, hero — герой, dt — сколько секунд прошло.\n\
\n\
function update(me, hero, dt)\n\
end\n";

#[cfg(test)]
mod tests {
    use super::*;

    /// Временный проект: папки и файлы с текстом.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new(files: &[(&str, &str)]) -> Self {
            static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("reyvec-explorer-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            for (path, text) in files {
                let p = root.join(path);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, text).unwrap();
            }
            std::fs::create_dir_all(root.join("assets")).unwrap();
            Tmp(root)
        }

        fn p(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }

        fn read(&self, rel: &str) -> String {
            std::fs::read_to_string(self.p(rel)).unwrap()
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn names_split_and_check() {
        assert_eq!(split_name("стеллаж.shape.ron"), ("стеллаж", ".shape.ron"));
        assert_eq!(split_name("a.svg"), ("a", ".svg"));
        assert_eq!(split_name("Новая папка"), ("Новая папка", ""));
        assert!(check_name("ящик.svg").is_ok());
        assert!(check_name("").is_err());
        assert!(check_name("a/b").is_err());
        assert!(check_name("a?").is_err());
        assert!(check_name("con.txt").is_err());
        assert!(check_name("имя.").is_err());
        assert!(check_name(".скрытый").is_err());
        assert_eq!(
            relative(Path::new("/p/assets/svg/x.svg"), Path::new("/p/scenes")),
            "../assets/svg/x.svg"
        );
        assert_eq!(
            relative(Path::new("/p/s/m/завод/a"), Path::new("/p/s/m")),
            "завод/a"
        );
    }

    #[test]
    fn rename_svg_rewrites_scene_and_undo_restores() {
        let t = Tmp::new(&[
            (
                "scenes/room.ron",
                "// рисунок \"../assets/svg/a.svg\" в комментарии\n(svg: \"../assets/svg/a.svg\", name: \"a\")\n",
            ),
            ("assets/svg/a.svg", "<svg/>"),
        ]);
        let done = move_all(
            &t.0,
            vec![(t.p("assets/svg/a.svg"), t.p("assets/svg/b.svg"))],
        )
        .unwrap();
        assert!(t.p("assets/svg/b.svg").is_file());
        assert_eq!(
            t.read("scenes/room.ron"),
            "// рисунок \"../assets/svg/a.svg\" в комментарии\n(svg: \"../assets/svg/b.svg\", name: \"a\")\n"
        );
        assert_eq!(used_in(&t.0, &t.p("assets/svg/b.svg")).len(), 1);
        let (back, again) = undo(&t.0, done).unwrap();
        assert_eq!(back, vec![t.p("assets/svg/a.svg")]);
        assert!(
            t.read("scenes/room.ron")
                .contains("(svg: \"../assets/svg/a.svg\"")
        );
        assert!(t.p("assets/svg/a.svg").is_file());
        // Повтор переносит снова и снова чинит путь.
        redo(&t.0, again).unwrap();
        assert!(t.p("assets/svg/b.svg").is_file());
        assert!(
            t.read("scenes/room.ron")
                .contains("\"../assets/svg/b.svg\"")
        );
    }

    #[test]
    fn moving_model_folder_fixes_use_and_clay() {
        let t = Tmp::new(&[
            (
                "scenes/3d/f.ron",
                "(objects: [(material: \"завод\", shape: Use(model: \"завод/конвейер\"))])",
            ),
            (
                "scenes/3d/models/завод/конвейер.shape.ron",
                "(shape: Use(model: \"завод/ось\"))",
            ),
            (
                "scenes/3d/models/завод/ось.shape.ron",
                "(shape: Clay(file: \"завод/ось.clay\"))",
            ),
            ("scenes/3d/models/завод/ось.clay", "x"),
        ]);
        let moves = vec![(t.p("scenes/3d/models/завод"), t.p("scenes/3d/models/цех"))];
        move_all(&t.0, moves).unwrap();
        // Имя материала «завод» без «/» — не путь, его не трогаем.
        assert_eq!(
            t.read("scenes/3d/f.ron"),
            "(objects: [(material: \"завод\", shape: Use(model: \"цех/конвейер\"))])"
        );
        assert_eq!(
            t.read("scenes/3d/models/цех/конвейер.shape.ron"),
            "(shape: Use(model: \"цех/ось\"))"
        );
        assert_eq!(
            t.read("scenes/3d/models/цех/ось.shape.ron"),
            "(shape: Clay(file: \"цех/ось.clay\"))"
        );
    }

    #[test]
    fn moving_scene_keeps_its_own_links() {
        let t = Tmp::new(&[
            ("scenes/room.ron", "(script: \"scripts/coin.luau\")"),
            ("scenes/scripts/coin.luau", "--"),
        ]);
        std::fs::create_dir_all(t.p("scenes/старое")).unwrap();
        move_all(
            &t.0,
            vec![(t.p("scenes/room.ron"), t.p("scenes/старое/room.ron"))],
        )
        .unwrap();
        assert_eq!(
            t.read("scenes/старое/room.ron"),
            "(script: \"../scripts/coin.luau\")"
        );
    }

    #[test]
    fn copy_trash_undo_and_conflicts() {
        let t = Tmp::new(&[("assets/a.svg", "1"), ("assets/d/b.svg", "2")]);
        let made = copy_into(&t.0, &t.p("assets"), &[t.p("assets/a.svg")]).unwrap();
        assert!(t.p("assets/a - копия.svg").is_file());
        assert!(copy_into(&t.0, &t.p("assets/d"), &[t.p("assets")]).is_err());
        let (_, again) = undo(&t.0, made).unwrap();
        assert!(!t.p("assets/a - копия.svg").exists());
        redo(&t.0, again).unwrap();
        assert!(t.p("assets/a - копия.svg").is_file());
        let gone = trash(&t.0, &[t.p("assets/d")]).unwrap();
        assert!(!t.p("assets/d").exists());
        let (_, again) = undo(&t.0, gone).unwrap();
        assert_eq!(t.read("assets/d/b.svg"), "2");
        let gone = redo(&t.0, again).unwrap();
        assert!(!t.p("assets/d").exists());
        undo(&t.0, gone).unwrap();
        assert_eq!(t.read("assets/d/b.svg"), "2");
        assert!(move_all(&t.0, vec![(t.p("assets/d"), t.p("assets/d/x"))]).is_err());
        std::fs::write(t.p("assets/c.svg"), "3").unwrap();
        assert!(move_all(&t.0, vec![(t.p("assets/a.svg"), t.p("assets/c.svg"))]).is_err());
        let (_, p) = create(&t.p("assets"), "Новая папка", None).unwrap();
        let (_, p2) = create(&t.p("assets"), "Новая папка", None).unwrap();
        assert_eq!(file_name(&p), "Новая папка");
        assert_eq!(file_name(&p2), "Новая папка 2");
    }

    /// Корзина помнит, откуда удалили; возвращает на место, а если имя занято — рядом.
    #[test]
    fn trash_lists_restores_and_erases() {
        let t = Tmp::new(&[
            ("assets/svg/a.svg", "1"),
            ("assets/d/b.svg", "2"),
            ("scenes/c.ron", "3"),
        ]);
        trash(&t.0, &[t.p("assets/svg/a.svg")]).unwrap();
        trash(&t.0, &[t.p("assets/d"), t.p("scenes/c.ron")]).unwrap();
        let bin = trash_list(&t.0);
        assert_eq!(bin.len(), 3);
        let find = |name: &str| bin.iter().find(|b| file_name(&b.item) == name).unwrap();
        assert_eq!(
            find("a.svg").from.as_deref(),
            Some(t.p("assets/svg/a.svg").as_path())
        );
        assert!(find("d").folder);
        assert_eq!(find("d").size, 1);
        // На месте a.svg уже новый файл: возвращается рядом под другим именем.
        std::fs::write(t.p("assets/svg/a.svg"), "new").unwrap();
        let done = restore_binned(&t.0, &[find("a.svg").clone(), find("d").clone()]).unwrap();
        assert_eq!(t.read("assets/svg/a 2.svg"), "1");
        assert_eq!(t.read("assets/d/b.svg"), "2");
        assert_eq!(trash_list(&t.0).len(), 1);
        // Отмена возвращения уносит обратно в корзину.
        undo(&t.0, done).unwrap();
        assert!(!t.p("assets/d").exists());
        assert_eq!(trash_list(&t.0).len(), 3);
        // Стереть насовсем: в корзине пусто, пустых ячеек не остаётся.
        assert_eq!(erase(&trash_list(&t.0)).unwrap(), 3);
        assert!(trash_list(&t.0).is_empty());
        let left = std::fs::read_dir(t.p(TRASH)).unwrap().count();
        assert_eq!(left, 0);
    }

    #[test]
    fn new_asset_texts_parse() {
        reyvec_scene::SceneFile::parse(NewKind::Scene2D.text().unwrap()).unwrap();
        reyvec_sdf::space_file::SpaceScene::parse(NEW_SPACE).unwrap();
        reyvec_sdf::shape_model::ModelFile::parse(NEW_SHAPE).unwrap();
    }
}
