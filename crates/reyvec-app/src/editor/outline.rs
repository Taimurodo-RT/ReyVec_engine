//! Дерево предметов сцены для иерархии и инспектора: что за предмет, как его назвать и из
//! чего он состоит. Только чтение файла сцены; правка придёт следующими шагами.

use std::collections::HashSet;

use reyvec_scene::{NodeDesc, SceneFile};

/// Вид предмета: от него цвет метки в иерархии.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Player,
    Dummy,
    Body,
    Script,
    Light,
    Plain,
}

impl Kind {
    pub(crate) fn of(n: &NodeDesc) -> Self {
        if n.player.is_some() {
            Kind::Player
        } else if n.dummy.is_some() {
            Kind::Dummy
        } else if n.body.is_some() || n.soft.is_some() {
            Kind::Body
        } else if n.script.is_some() {
            Kind::Script
        } else if !n.lights.is_empty() && n.shapes.is_empty() {
            Kind::Light
        } else {
            Kind::Plain
        }
    }

    pub(crate) fn color(self) -> egui::Color32 {
        use super::theme::*;
        match self {
            Kind::Player => KIND_PLAYER,
            Kind::Dummy => BAD,
            Kind::Body => KIND_BODY,
            Kind::Script => KIND_SCRIPT,
            Kind::Light => KIND_LIGHT,
            Kind::Plain => KIND_PLAIN,
        }
    }

    /// Название вида для предмета без имени.
    fn fallback(self) -> &'static str {
        match self {
            Kind::Player => "Герой",
            Kind::Dummy => "Манекен",
            Kind::Body => "Тело",
            Kind::Script => "Предмет со скриптом",
            Kind::Light => "Свет",
            Kind::Plain => "Предмет",
        }
    }
}

/// Имя предмета для людей: своё имя, кость героя или вид предмета.
pub(crate) fn display_name(n: &NodeDesc) -> String {
    if !n.name.trim().is_empty() {
        return n.name.clone();
    }
    if let Some(bone) = &n.bone {
        return format!("кость «{bone}»");
    }
    if n.tiles.is_some() {
        return "Уровень из тайлов".into();
    }
    Kind::of(n).fallback().into()
}

/// Строка дерева.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub name: String,
    pub kind: Kind,
    pub depth: usize,
    /// Номера предметов от верхнего уровня: `[2, 0]` — первый вложенный в третий.
    pub path: Vec<usize>,
    pub has_children: bool,
}

/// Предмет по пути из номеров.
pub(crate) fn node_at<'a>(file: &'a SceneFile, path: &[usize]) -> Option<&'a NodeDesc> {
    let (first, rest) = path.split_first()?;
    let mut node = file.nodes.get(*first)?;
    for i in rest {
        node = node.children.get(*i)?;
    }
    Some(node)
}

/// Видимые строки дерева: вложенные показываются только у раскрытых (`open`).
pub(crate) fn rows(file: &SceneFile, open: &HashSet<Vec<usize>>) -> Vec<Row> {
    fn walk(
        nodes: &[NodeDesc],
        depth: usize,
        prefix: &mut Vec<usize>,
        open: &HashSet<Vec<usize>>,
        out: &mut Vec<Row>,
    ) {
        for (i, n) in nodes.iter().enumerate() {
            prefix.push(i);
            out.push(Row {
                name: display_name(n),
                kind: Kind::of(n),
                depth,
                path: prefix.clone(),
                has_children: !n.children.is_empty(),
            });
            if open.contains(prefix.as_slice()) {
                walk(&n.children, depth + 1, prefix, open, out);
            }
            prefix.pop();
        }
    }
    let mut out = Vec::new();
    walk(&file.nodes, 0, &mut Vec::new(), open, &mut out);
    out
}

/// Из чего состоит предмет: короткие метки для инспектора.
pub(crate) fn parts(n: &NodeDesc) -> Vec<String> {
    let mut out = Vec::new();
    let mut count = |what: &str, len: usize| {
        if len > 0 {
            out.push(format!("{what}: {len}"));
        }
    };
    count("фигуры", n.shapes.len());
    count("свет", n.lights.len());
    count("SDF", n.objects.len());
    count("рисунки", n.pictures.len());
    count("морфы", n.morphs.len());
    count("твёрдое", n.solids.len());
    count("вложенные", n.children.len());
    count("сочленения", n.joints.len());
    let mut flag = |on: bool, what: &str| {
        if on {
            out.push(what.to_owned());
        }
    };
    flag(n.player.is_some(), "герой");
    flag(n.dummy.is_some(), "манекен");
    flag(n.body.is_some(), "физическое тело");
    flag(n.soft.is_some(), "мягкое тело");
    flag(n.tiles.is_some(), "тайлы");
    flag(n.repeat.is_some(), "повтор");
    flag(n.spawn.is_some(), "выброс тел");
    flag(n.shadow == Some(true), "отбрасывает тень");
    if let Some(script) = &n.script {
        out.push(format!("скрипт {script}"));
    }
    out
}

/// Сколько всего предметов в сцене, со вложенными.
pub(crate) fn count(file: &SceneFile) -> usize {
    fn walk(nodes: &[NodeDesc]) -> usize {
        nodes.iter().map(|n| 1 + walk(&n.children)).sum()
    }
    walk(&file.nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> SceneFile {
        SceneFile::parse(reyvec_scene::EXAMPLE).expect("пример сцены читается")
    }

    #[test]
    fn rows_follow_open_nodes() {
        let file = example();
        let closed = rows(&file, &HashSet::new());
        assert_eq!(closed.len(), file.nodes.len());
        assert!(closed.iter().all(|r| r.depth == 0));
        let parent = closed
            .iter()
            .find(|r| r.has_children)
            .expect("в примере есть предмет с вложенными");
        let open: HashSet<_> = [parent.path.clone()].into();
        let opened = rows(&file, &open);
        let children = node_at(&file, &parent.path).unwrap().children.len();
        assert_eq!(opened.len(), closed.len() + children);
        assert!(opened.iter().any(|r| r.depth == 1));
    }

    #[test]
    fn node_paths_point_to_nodes() {
        let file = example();
        let all: HashSet<Vec<usize>> = rows(&file, &HashSet::new())
            .into_iter()
            .map(|r| r.path)
            .collect();
        let rows = rows(&file, &all);
        for r in &rows {
            let n = node_at(&file, &r.path).expect("путь ведёт к предмету");
            assert_eq!(display_name(n), r.name);
        }
        assert!(node_at(&file, &[usize::MAX]).is_none());
        assert!(node_at(&file, &[]).is_none());
    }

    #[test]
    fn example_has_hero_with_parts() {
        let file = example();
        let hero = file
            .nodes
            .iter()
            .find(|n| n.player.is_some())
            .expect("в примере есть герой");
        assert_eq!(Kind::of(hero), Kind::Player);
        assert!(parts(hero).iter().any(|p| p == "герой"));
        assert!(count(&file) >= file.nodes.len());
    }
}
