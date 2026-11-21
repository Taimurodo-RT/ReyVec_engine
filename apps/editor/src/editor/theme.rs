//! Тема редактора: тёмный графит, один тёплый акцент (цвет света в движке) и синий цвет
//! выделения. Цвета те же, что на макете.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle};

/// Фон под всем.
pub(crate) const BG: Color32 = Color32::from_rgb(16, 18, 23);
/// Фон панелей.
pub(crate) const PANEL: Color32 = Color32::from_rgb(23, 26, 33);
/// Верхняя полоса и строка состояния.
pub(crate) const BAR: Color32 = Color32::from_rgb(21, 24, 30);
/// Карточки в инспекторе.
pub(crate) const CARD: Color32 = Color32::from_rgb(28, 32, 40);
/// Кнопки и поля.
pub(crate) const RAISED: Color32 = Color32::from_rgb(32, 36, 45);
/// Кнопка под мышью, выбранная вкладка.
pub(crate) const HOVER: Color32 = Color32::from_rgb(38, 43, 54);
/// Линии между панелями.
pub(crate) const BORDER: Color32 = Color32::from_rgb(38, 42, 51);
/// Рамки карточек и кнопок.
pub(crate) const BORDER_STRONG: Color32 = Color32::from_rgb(46, 51, 64);
/// Основной текст.
pub(crate) const TEXT: Color32 = Color32::from_rgb(230, 232, 238);
/// Второстепенный текст.
pub(crate) const TEXT_DIM: Color32 = Color32::from_rgb(174, 181, 196);
/// Подписи и подсказки.
pub(crate) const MUTED: Color32 = Color32::from_rgb(138, 146, 165);
/// Акцент: кнопка «Играть», ползунки.
pub(crate) const ACCENT: Color32 = Color32::from_rgb(245, 165, 74);
/// Текст на акценте.
pub(crate) const ON_ACCENT: Color32 = Color32::from_rgb(26, 18, 6);
/// Выделение: рамка.
pub(crate) const SELECT: Color32 = Color32::from_rgb(91, 140, 255);
/// Выделение: заливка.
pub(crate) const SELECT_BG: Color32 = Color32::from_rgb(42, 53, 80);
/// Кнопка «закрыть окно» под мышью (как в Windows).
pub(crate) const CLOSE_HOVER: Color32 = Color32::from_rgb(196, 43, 28);
/// Всё хорошо.
pub(crate) const GOOD: Color32 = Color32::from_rgb(107, 227, 138);
/// Ошибка.
pub(crate) const BAD: Color32 = Color32::from_rgb(255, 107, 107);

/// Цвета видов предметов в иерархии.
pub(crate) const KIND_PLAYER: Color32 = Color32::from_rgb(79, 199, 168);
pub(crate) const KIND_LIGHT: Color32 = ACCENT;
pub(crate) const KIND_BODY: Color32 = SELECT;
pub(crate) const KIND_SCRIPT: Color32 = Color32::from_rgb(192, 132, 252);
pub(crate) const KIND_PLAIN: Color32 = Color32::from_rgb(111, 120, 145);

// ---- Токены размеров. Все размеры в точках интерфейса (на экране 150 % точка — 1,5
// пикселя). Панели всех рабочих пространств берут одни и те же размеры отсюда, поэтому при
// переходе между вкладками ничего не прыгает.

/// Верхняя полоса (название, вкладки, «Играть», кнопки окна).
pub(crate) const TITLE_HEIGHT: f32 = 44.0;
/// Строка состояния внизу окна.
pub(crate) const STATUS_HEIGHT: f32 = 24.0;
/// Строка вкладок панели.
pub(crate) const TAB_HEIGHT: f32 = 32.0;
/// Строка списка (иерархия, файлы).
pub(crate) const ROW_HEIGHT: f32 = 26.0;
/// Кнопка с подписью в верхней полосе («Играть»).
pub(crate) const BUTTON_HEIGHT: f32 = 30.0;
/// Кнопка окна (свернуть, развернуть, закрыть).
pub(crate) const WINDOW_BUTTON_WIDTH: f32 = 46.0;

/// Левая панель, начальная ширина и пределы.
pub(crate) const LEFT_WIDTH: f32 = 280.0;
/// Правая панель, начальная ширина.
pub(crate) const RIGHT_WIDTH: f32 = 320.0;
/// Нижняя панель, начальная высота.
pub(crate) const BOTTOM_HEIGHT: f32 = 220.0;
/// Пределы ширины боковых панелей, когда их тянут мышью.
pub(crate) const SIDE_RANGE: std::ops::RangeInclusive<f32> = 200.0..=560.0;
/// Пределы высоты нижней панели.
pub(crate) const BOTTOM_RANGE: std::ops::RangeInclusive<f32> = 120.0..=560.0;

/// Промежутки: от самого маленького к большому.
pub(crate) const GAP_XS: f32 = 2.0;
pub(crate) const GAP_S: f32 = 4.0;
pub(crate) const GAP_M: f32 = 6.0;
pub(crate) const GAP_L: f32 = 8.0;
pub(crate) const GAP_XL: f32 = 12.0;
pub(crate) const GAP_XXL: f32 = 16.0;
/// Сдвиг вложенного уровня в дереве.
pub(crate) const INDENT: f32 = 16.0;

/// Поля внутри панели.
pub(crate) const PAD_PANEL: Margin = Margin::symmetric(10, 8);
/// Поля внутри карточки инспектора.
pub(crate) const PAD_CARD: Margin = Margin::symmetric(12, 10);
/// Поля верхней полосы и строки состояния.
pub(crate) const PAD_BAR: Margin = Margin::symmetric(8, 0);
/// Поля значения в рамке и метки.
pub(crate) const PAD_FIELD: Margin = Margin::symmetric(8, 3);

/// Скругления: поля и строки, кнопки, карточки и группы.
pub(crate) const RADIUS_S: u8 = 6;
pub(crate) const RADIUS_M: u8 = 8;
pub(crate) const RADIUS_L: u8 = 10;

/// Шрифты: подписи, метки на сцене, основной, заголовок карточки, название движка,
/// заголовок панели, цифры.
pub(crate) const FONT_SMALL: f32 = 11.5;
pub(crate) const FONT_CHIP: f32 = 12.5;
pub(crate) const FONT_BODY: f32 = 13.5;
pub(crate) const FONT_TITLE: f32 = 14.0;
pub(crate) const FONT_BRAND: f32 = 15.0;
pub(crate) const FONT_HEADING: f32 = 16.0;
pub(crate) const FONT_MONO: f32 = 12.5;

/// Толщина линий и рамок; рамка «идёт игра».
pub(crate) const LINE: f32 = 1.0;
pub(crate) const LINE_FOCUS: f32 = 2.0;

/// Ставит тему в `ctx`.
pub(crate) fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = PANEL;
        v.window_fill = CARD;
        v.window_stroke = Stroke::new(LINE, BORDER_STRONG);
        v.window_corner_radius = CornerRadius::same(RADIUS_L);
        v.menu_corner_radius = CornerRadius::same(RADIUS_M);
        v.extreme_bg_color = BG;
        v.faint_bg_color = CARD;
        v.code_bg_color = BG;
        v.hyperlink_color = ACCENT;
        v.warn_fg_color = ACCENT;
        v.error_fg_color = BAD;
        v.selection.bg_fill = SELECT_BG;
        v.selection.stroke = Stroke::new(LINE, SELECT);
        v.override_text_color = None;

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = PANEL;
        w.noninteractive.weak_bg_fill = PANEL;
        w.noninteractive.bg_stroke = Stroke::new(LINE, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(LINE, TEXT_DIM);
        w.noninteractive.corner_radius = CornerRadius::same(RADIUS_S);
        for (wv, fill, stroke) in [
            (&mut w.inactive, RAISED, BORDER_STRONG),
            (&mut w.hovered, HOVER, BORDER_STRONG),
            (&mut w.active, BORDER_STRONG, SELECT),
            (&mut w.open, HOVER, BORDER_STRONG),
        ] {
            wv.bg_fill = fill;
            wv.weak_bg_fill = fill;
            wv.bg_stroke = Stroke::new(LINE, stroke);
            wv.corner_radius = CornerRadius::same(RADIUS_S);
            wv.expansion = 0.0;
        }
        w.inactive.fg_stroke = Stroke::new(LINE, TEXT);
        w.hovered.fg_stroke = Stroke::new(LINE, Color32::WHITE);
        w.active.fg_stroke = Stroke::new(LINE, Color32::WHITE);
        w.open.fg_stroke = Stroke::new(LINE, TEXT);

        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(GAP_L, GAP_M);
        s.button_padding = egui::vec2(10.0, 5.0);
        s.interact_size.y = ROW_HEIGHT;
        s.window_margin = egui::Margin::same(GAP_XL as i8);
        s.menu_margin = egui::Margin::same(GAP_M as i8);

        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new(FONT_SMALL, FontFamily::Proportional),
            ),
            (
                TextStyle::Body,
                FontId::new(FONT_BODY, FontFamily::Proportional),
            ),
            (
                TextStyle::Button,
                FontId::new(FONT_BODY, FontFamily::Proportional),
            ),
            (
                TextStyle::Heading,
                FontId::new(FONT_HEADING, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(FONT_MONO, FontFamily::Monospace),
            ),
        ]
        .into();
    });
}
