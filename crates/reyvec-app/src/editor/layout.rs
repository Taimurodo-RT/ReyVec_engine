//! Рабочие пространства редактора, описанные данными.
//!
//! Каждое пространство («Сцена», «Анимация», …) — это набор вкладок в трёх областях вокруг
//! живой сцены: слева, справа и снизу. Что где лежит, записано здесь таблицей, а не в коде
//! рисования: так раскладку потом можно читать из файла, давать её менять мышью или
//! перерисовать интерфейс другим способом, не трогая сами инструменты.

/// Вкладка панели.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    /// Дерево предметов сцены.
    Hierarchy,
    /// Свойства выбранного предмета или всей сцены.
    Inspector,
    /// Файлы проекта.
    Assets,
    /// Журнал программы.
    Console,
    /// Счётчики кадра.
    Profiler,
    /// Инструмент, который ещё не сделан: название и что в нём будет.
    Soon {
        title: &'static str,
        text: &'static str,
    },
}

impl Tab {
    /// Название вкладки.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Tab::Hierarchy => "Иерархия",
            Tab::Inspector => "Инспектор",
            Tab::Assets => "Ассеты",
            Tab::Console => "Консоль",
            Tab::Profiler => "Профайлер",
            Tab::Soon { title, .. } => title,
        }
    }
}

/// Область вокруг сцены.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Left,
    Right,
    Bottom,
}

impl Side {
    #[cfg(test)]
    pub(crate) const ALL: [Side; 3] = [Side::Left, Side::Right, Side::Bottom];

    pub(crate) fn index(self) -> usize {
        match self {
            Side::Left => 0,
            Side::Right => 1,
            Side::Bottom => 2,
        }
    }
}

/// Вкладки одной области и её начальный размер (ширина слева и справа, высота снизу),
/// в точках интерфейса.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Area {
    pub tabs: &'static [Tab],
    pub size: f32,
}

/// Рабочее пространство: вкладка верхней полосы и её панели.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Workspace {
    pub name: &'static str,
    pub left: Area,
    pub right: Area,
    pub bottom: Area,
}

impl Workspace {
    pub(crate) fn area(&self, side: Side) -> &Area {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
            Side::Bottom => &self.bottom,
        }
    }
}

const BOTTOM: &[Tab] = &[Tab::Assets, Tab::Console, Tab::Profiler];

/// Все рабочие пространства в порядке вкладок.
pub(crate) const WORKSPACES: &[Workspace] = &[
    Workspace {
        name: "Сцена",
        left: Area {
            tabs: &[Tab::Hierarchy],
            size: 264.0,
        },
        right: Area {
            tabs: &[Tab::Inspector],
            size: 320.0,
        },
        bottom: Area {
            tabs: BOTTOM,
            size: 220.0,
        },
    },
    Workspace {
        name: "Анимация",
        left: Area {
            tabs: &[Tab::Soon {
                title: "Клипы",
                text: "Список анимаций персонажа (бег, прыжок, удар) и переходы между ними.",
            }],
            size: 240.0,
        },
        right: Area {
            tabs: &[Tab::Soon {
                title: "Ключ",
                text: "Значение выбранного ключа и кривая сглаживания.",
            }],
            size: 300.0,
        },
        bottom: Area {
            tabs: &[
                Tab::Soon {
                    title: "Шкала времени",
                    text: "Дорожки костей и морфов с ключами, события (шаг, звук).",
                },
                Tab::Console,
            ],
            size: 280.0,
        },
    },
    Workspace {
        name: "Синтез и звук",
        left: Area {
            tabs: &[Tab::Soon {
                title: "Генераторы",
                text: "Рецепты ассетов: графика (камень, кристаллы, лианы) и звуки.",
            }],
            size: 240.0,
        },
        right: Area {
            tabs: &[Tab::Soon {
                title: "Предпросмотр",
                text: "Ассет под светом сцены и варианты по новому зерну.",
            }],
            size: 320.0,
        },
        bottom: Area {
            tabs: &[
                Tab::Soon {
                    title: "Узлы",
                    text: "Граф узлов: форма, шум, материал, выходы (вектор, коллайдер, звук).",
                },
                Tab::Console,
            ],
            size: 280.0,
        },
    },
    Workspace {
        name: "Управление",
        left: Area {
            tabs: &[Tab::Soon {
                title: "Схемы",
                text: "Стопка схем: Консоль, Меню, Игра. Верхняя забирает кнопки у нижних.",
            }],
            size: 260.0,
        },
        right: Area {
            tabs: &[Tab::Soon {
                title: "Поведение",
                text: "Выбор поведения действия (куда бить, что делает удержание).",
            }],
            size: 320.0,
        },
        bottom: Area {
            tabs: &[
                Tab::Soon {
                    title: "Действия",
                    text: "Таблица действий героя и их кнопок (input.ron и controls.ron).",
                },
                Tab::Console,
            ],
            size: 260.0,
        },
    },
    Workspace {
        name: "Скрипты",
        left: Area {
            tabs: &[Tab::Soon {
                title: "Файлы скриптов",
                text: "Скрипты Luau проекта и к каким предметам они привязаны.",
            }],
            size: 240.0,
        },
        right: Area {
            tabs: &[Tab::Inspector],
            size: 300.0,
        },
        bottom: Area {
            tabs: &[
                Tab::Soon {
                    title: "Текст скрипта",
                    text: "Редактор текста скрипта с подсветкой и ошибками.",
                },
                Tab::Console,
            ],
            size: 300.0,
        },
    },
    Workspace {
        name: "Физика",
        left: Area {
            tabs: &[Tab::Hierarchy],
            size: 264.0,
        },
        right: Area {
            tabs: &[Tab::Soon {
                title: "Тело",
                text: "Тело, коллайдер и сочленения выбранного предмета (Rapier).",
            }],
            size: 320.0,
        },
        bottom: Area {
            tabs: &[Tab::Console, Tab::Profiler],
            size: 200.0,
        },
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_workspace_has_tabs_everywhere() {
        assert!(!WORKSPACES.is_empty());
        for w in WORKSPACES {
            for side in Side::ALL {
                let area = w.area(side);
                assert!(!area.tabs.is_empty(), "{}: пустая область", w.name);
                assert!(area.size > 100.0, "{}: слишком узкая область", w.name);
            }
        }
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = WORKSPACES.iter().map(|w| w.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), WORKSPACES.len());
    }
}
