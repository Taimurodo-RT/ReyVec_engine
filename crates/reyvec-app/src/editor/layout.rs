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

/// Рабочее пространство: вкладка верхней полосы, к какому этапу работы над игрой она
/// относится, и вкладки трёх областей. Размеры областей у всех пространств одинаковые
/// (токены в `theme.rs`), поэтому при переходе между вкладками панели не прыгают.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Workspace {
    pub name: &'static str,
    /// Что в этом пространстве делают (подсказка над вкладкой).
    pub hint: &'static str,
    pub left: &'static [Tab],
    pub right: &'static [Tab],
    pub bottom: &'static [Tab],
}

impl Workspace {
    pub(crate) fn tabs(&self, side: Side) -> &'static [Tab] {
        match side {
            Side::Left => self.left,
            Side::Right => self.right,
            Side::Bottom => self.bottom,
        }
    }
}

const fn soon(title: &'static str, text: &'static str) -> Tab {
    Tab::Soon { title, text }
}

/// Все рабочие пространства в порядке работы над игрой: замысел и сюжет, ассеты, мир,
/// логика, интерфейс игры, сборка.
pub(crate) const WORKSPACES: &[Workspace] = &[
    Workspace {
        name: "Сюжет",
        hint: "Замысел и история: главы, персонажи, диалоги, задания",
        left: &[soon(
            "Главы",
            "Структура истории: главы, сцены и как они связаны (ветвления, условия).",
        )],
        right: &[soon(
            "Персонажи",
            "Карточки персонажей: имя, роль, внешний вид, связи с другими.",
        )],
        bottom: &[
            soon(
                "Диалоги",
                "Реплики и выборы игрока; кто говорит, что меняется после ответа.",
            ),
            soon(
                "Задания",
                "Цели игрока, шаги и награды; что открывает задание.",
            ),
            Tab::Console,
        ],
    },
    Workspace {
        name: "Ассеты",
        hint: "Графика, анимация, звук и синтез ассетов",
        left: &[soon(
            "Категории",
            "Графика, анимация, звук, синтез, шрифты: фильтр библиотеки ассетов.",
        )],
        right: &[soon(
            "Свойства ассета",
            "Размер, формат, где ассет используется; предпросмотр под светом сцены.",
        )],
        bottom: &[
            Tab::Assets,
            soon(
                "Анимация",
                "Клипы персонажа, шкала времени с ключами, кривые и переходы.",
            ),
            soon(
                "Звук",
                "Звуки и музыка, громкость, вариации; к каким событиям игры привязаны.",
            ),
            soon(
                "Синтез",
                "Рецепты ассетов из узлов (форма, шум, материал, звук) и варианты по зерну.",
            ),
            Tab::Console,
        ],
    },
    Workspace {
        name: "Мир",
        hint: "Уровни и сцены: что где стоит",
        left: &[Tab::Hierarchy],
        right: &[Tab::Inspector],
        bottom: &[Tab::Assets, Tab::Console, Tab::Profiler],
    },
    Workspace {
        name: "Логика",
        hint: "Поведение: скрипты, физика, управление",
        left: &[
            Tab::Hierarchy,
            soon(
                "Скрипты",
                "Скрипты Luau проекта и к каким предметам они привязаны.",
            ),
        ],
        right: &[
            Tab::Inspector,
            soon(
                "Физика",
                "Тело, коллайдер и сочленения выбранного предмета (Rapier).",
            ),
        ],
        bottom: &[
            soon(
                "Управление",
                "Схемы (Консоль, Меню, Игра), действия героя, их кнопки и поведение.",
            ),
            soon("Текст скрипта", "Редактор скрипта с подсветкой и ошибками."),
            Tab::Console,
        ],
    },
    Workspace {
        name: "Интерфейс",
        hint: "Меню, здоровье и другие экраны игры",
        left: &[soon(
            "Экраны",
            "Главное меню, пауза, настройки, полоса здоровья, окна диалогов.",
        )],
        right: &[soon(
            "Свойства элемента",
            "Место, размер, текст, цвета и что делает кнопка.",
        )],
        bottom: &[
            soon("Стили", "Шрифты, цвета и рамки интерфейса игры."),
            Tab::Console,
        ],
    },
    Workspace {
        name: "Сборка",
        hint: "Проверка и выпуск: замеры, настройки игры, сборка для игроков",
        left: &[soon(
            "Настройки игры",
            "Название, значок, первая сцена, языки, качество графики по умолчанию.",
        )],
        right: &[Tab::Profiler],
        bottom: &[
            Tab::Console,
            soon("Выпуск", "Сборка игры для Windows в папку или установщик."),
        ],
    },
];

/// С какого пространства редактор открывается: «Мир».
pub(crate) const START_WORKSPACE: usize = 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_workspace_has_tabs_everywhere() {
        assert!(!WORKSPACES.is_empty());
        for w in WORKSPACES {
            for side in Side::ALL {
                assert!(!w.tabs(side).is_empty(), "{}: пустая область", w.name);
            }
        }
    }

    #[test]
    fn starts_in_world() {
        assert_eq!(WORKSPACES[START_WORKSPACE].name, "Мир");
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = WORKSPACES.iter().map(|w| w.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), WORKSPACES.len());
    }
}
