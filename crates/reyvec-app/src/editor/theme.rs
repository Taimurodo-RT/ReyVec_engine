//! Тема редактора: тёмный графит, один тёплый акцент (цвет света в движке) и синий цвет
//! выделения. Цвета те же, что на макете.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle};

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

/// Высота верхней полосы, точек.
pub(crate) const TITLE_HEIGHT: f32 = 44.0;
/// Высота строки состояния, точек.
pub(crate) const STATUS_HEIGHT: f32 = 24.0;
/// Высота строки вкладок панели, точек.
pub(crate) const TAB_HEIGHT: f32 = 32.0;

/// Ставит тему в `ctx`.
pub(crate) fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = PANEL;
        v.window_fill = CARD;
        v.window_stroke = Stroke::new(1.0, BORDER_STRONG);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.extreme_bg_color = BG;
        v.faint_bg_color = CARD;
        v.code_bg_color = BG;
        v.hyperlink_color = ACCENT;
        v.warn_fg_color = ACCENT;
        v.error_fg_color = BAD;
        v.selection.bg_fill = SELECT_BG;
        v.selection.stroke = Stroke::new(1.0, SELECT);
        v.override_text_color = None;

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = PANEL;
        w.noninteractive.weak_bg_fill = PANEL;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
        w.noninteractive.corner_radius = CornerRadius::same(6);
        for (wv, fill, stroke) in [
            (&mut w.inactive, RAISED, BORDER_STRONG),
            (&mut w.hovered, HOVER, BORDER_STRONG),
            (&mut w.active, BORDER_STRONG, SELECT),
            (&mut w.open, HOVER, BORDER_STRONG),
        ] {
            wv.bg_fill = fill;
            wv.weak_bg_fill = fill;
            wv.bg_stroke = Stroke::new(1.0, stroke);
            wv.corner_radius = CornerRadius::same(6);
            wv.expansion = 0.0;
        }
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        w.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        w.open.fg_stroke = Stroke::new(1.0, TEXT);

        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(8.0, 6.0);
        s.button_padding = egui::vec2(10.0, 5.0);
        s.interact_size.y = 26.0;
        s.window_margin = egui::Margin::same(12);
        s.menu_margin = egui::Margin::same(6);

        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new(11.5, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(13.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Heading,
                FontId::new(16.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(12.5, FontFamily::Monospace),
            ),
        ]
        .into();
    });
}
