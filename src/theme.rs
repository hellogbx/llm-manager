//! Visual design tokens plus the small painted widgets shared by every page.
//!
//! The whole look lives here: palette, fonts, egui `Style`, and helpers such as
//! cards, pills, badges and buttons. Pages in `ui.rs` only compose these.

use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Layout,
    Margin, Painter, Pos2, Rect, Response, Sense, Shadow, Stroke, StrokeKind, Ui, UiBuilder, Vec2,
};

// ----------------------------------------------------------------- palette

/// App background (behind pages).
pub const BG: Color32 = Color32::from_rgb(13, 15, 22);
/// Sidebar / chrome surface.
pub const PANEL: Color32 = Color32::from_rgb(17, 19, 28);
/// Card surface.
pub const CARD: Color32 = Color32::from_rgb(24, 27, 38);
/// Card surface while hovered.
pub const CARD_HOVER: Color32 = Color32::from_rgb(31, 35, 49);
/// Sunken surface (code blocks, inputs).
pub const SUNKEN: Color32 = Color32::from_rgb(9, 11, 17);
/// Hairline border.
pub const BORDER: Color32 = Color32::from_rgb(38, 43, 58);
/// Border under the cursor.
pub const BORDER_HOVER: Color32 = Color32::from_rgb(62, 71, 96);

/// Primary text.
pub const TEXT: Color32 = Color32::from_rgb(232, 235, 243);
/// Slightly de-emphasized text.
pub const TEXT_DIM: Color32 = Color32::from_rgb(178, 186, 203);
/// Secondary text.
pub const MUTED: Color32 = Color32::from_rgb(140, 149, 168);
/// Tertiary text, hints, timestamps.
pub const FAINT: Color32 = Color32::from_rgb(92, 100, 120);

/// Brand accent.
pub const ACCENT: Color32 = Color32::from_rgb(112, 141, 255);
/// Lighter accent for text/icons on tinted backgrounds.
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(166, 184, 255);
/// Accent at low opacity, for selected rows.
pub const ACCENT_TINT: Color32 = Color32::from_rgba_premultiplied(22, 29, 58, 255);
/// Secondary accent.
pub const TEAL: Color32 = Color32::from_rgb(79, 209, 197);
pub const GREEN: Color32 = Color32::from_rgb(61, 214, 140);
pub const AMBER: Color32 = Color32::from_rgb(242, 179, 64);
pub const RED: Color32 = Color32::from_rgb(255, 107, 107);

pub fn alpha(color: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

/// Linear blend from `a` to `b` (`t` in `0..=1`).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

// ------------------------------------------------------------------ style

/// Install fonts and the global egui style. Call once at startup.
pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);

    let mut style = (*ctx.style()).clone();

    style.text_styles = [
        (egui::TextStyle::Heading, FontId::proportional(20.0)),
        (egui::TextStyle::Body, FontId::proportional(13.5)),
        (egui::TextStyle::Button, FontId::proportional(13.0)),
        (egui::TextStyle::Small, FontId::proportional(11.0)),
        (egui::TextStyle::Monospace, FontId::monospace(12.5)),
    ]
    .into_iter()
    .collect();

    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.interact_size = Vec2::new(40.0, 30.0);
    style.spacing.window_margin = Margin::same(18);
    style.spacing.menu_margin = Margin::same(8);
    style.spacing.slider_width = 190.0;
    style.spacing.text_edit_width = 240.0;
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.floating = true;
    style.spacing.scroll.bar_inner_margin = 2.0;
    style.animation_time = 0.15;
    style.interaction.selectable_labels = true;

    let mut visuals = egui::Visuals::dark();
    visuals.dark_mode = true;
    visuals.panel_fill = BG;
    visuals.window_fill = PANEL;
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.window_corner_radius = CornerRadius::same(14);
    visuals.window_shadow = Shadow {
        offset: [0, 14],
        blur: 36,
        spread: 0,
        color: Color32::from_black_alpha(150),
    };
    visuals.menu_corner_radius = CornerRadius::same(10);
    visuals.extreme_bg_color = SUNKEN;
    visuals.faint_bg_color = CARD;
    visuals.code_bg_color = SUNKEN;
    visuals.hyperlink_color = ACCENT;
    visuals.selection = egui::style::Selection {
        bg_fill: alpha(ACCENT, 70),
        stroke: Stroke::new(1.0_f32, ACCENT),
    };
    visuals.widgets.noninteractive = egui::style::WidgetVisuals {
        bg_fill: CARD,
        weak_bg_fill: CARD,
        bg_stroke: Stroke::new(1.0_f32, BORDER),
        corner_radius: CornerRadius::same(10),
        fg_stroke: Stroke::new(1.0_f32, TEXT_DIM),
        expansion: 0.0,
    };
    visuals.widgets.inactive = egui::style::WidgetVisuals {
        bg_fill: CARD,
        weak_bg_fill: CARD,
        bg_stroke: Stroke::new(1.0_f32, BORDER),
        corner_radius: CornerRadius::same(8),
        fg_stroke: Stroke::new(1.0_f32, TEXT_DIM),
        expansion: 0.0,
    };
    visuals.widgets.hovered = egui::style::WidgetVisuals {
        bg_fill: CARD_HOVER,
        weak_bg_fill: CARD_HOVER,
        bg_stroke: Stroke::new(1.0_f32, BORDER_HOVER),
        corner_radius: CornerRadius::same(8),
        fg_stroke: Stroke::new(1.0_f32, TEXT),
        expansion: 0.0,
    };
    visuals.widgets.active = egui::style::WidgetVisuals {
        bg_fill: CARD_HOVER,
        weak_bg_fill: CARD_HOVER,
        bg_stroke: Stroke::new(1.0_f32, ACCENT),
        corner_radius: CornerRadius::same(8),
        fg_stroke: Stroke::new(1.0_f32, egui::Color32::WHITE),
        expansion: 0.0,
    };
    visuals.widgets.open = visuals.widgets.active;
    style.visuals = visuals;

    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(|s| *s = style.clone());
}

/// macOS ships CJK fonts as `.ttc` collections; use one for both Latin and CJK.
fn install_fonts(ctx: &egui::Context) {
    const CANDIDATES: [&str; 5] = [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/Supplemental/Songti.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
    ];
    let mut fonts = FontDefinitions::default();
    let mut loaded = false;
    for path in CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if bytes.len() < 4 {
            continue;
        }
        fonts
            .font_data
            .insert("cjk".to_owned(), Arc::new(FontData::from_owned(bytes)));
        loaded = true;
        break;
    }
    if !loaded {
        return;
    }
    // PingFang's Latin looks native on macOS, so it leads the proportional list.
    // Monospace stays as-is, with CJK appended as a fallback face.
    if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
        family.insert(0, "cjk".to_owned());
    }
    for family in [&FontFamily::Proportional, &FontFamily::Monospace] {
        fonts
            .families
            .entry(family.clone())
            .or_default()
            .push("cjk".to_owned());
    }
    ctx.set_fonts(fonts);
}

// ----------------------------------------------------------------- frames

/// Standard content card.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(12)
        .inner_margin(Margin::symmetric(16, 14))
}

/// Card variant with tighter padding (list rows).
pub fn row_card() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(11)
        .inner_margin(Margin::symmetric(14, 11))
}

/// Frame that highlights itself while hovered. The closure receives a ui whose
/// width already fills the card.
pub fn hover_card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> (Response, R) {
    let mut prepared = row_card().begin(ui);
    prepared
        .content_ui
        .set_width(prepared.content_ui.available_width());
    let inner = add(&mut prepared.content_ui);
    let response = prepared.allocate_space(ui);
    if response.hovered() {
        prepared.frame.fill = CARD_HOVER;
        prepared.frame.stroke = Stroke::new(1.0_f32, BORDER_HOVER);
    }
    prepared.paint(ui);
    (response, inner)
}

/// Dark, sunken surface used for code and log output.
pub fn sunken() -> egui::Frame {
    egui::Frame::new()
        .fill(SUNKEN)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(10)
        .inner_margin(Margin::same(12))
}

// ------------------------------------------------------------------ text

pub fn section_header(ui: &mut Ui, title: &str, hint: &str) {
    ui.label(egui::RichText::new(title).size(14.5).strong().color(TEXT));
    if !hint.is_empty() {
        ui.add_space(2.0);
        ui.label(egui::RichText::new(hint).size(11.5).color(FAINT));
    }
}

pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(11.5).color(FAINT));
}

pub fn divider(ui: &mut Ui) {
    ui.add_space(6.0);
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0_f32, BORDER));
    ui.add_space(6.0);
}

// --------------------------------------------------------------- controls

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Primary,
    Ghost,
    Quiet,
    Danger,
    DangerSolid,
}

/// A hand-painted button, so hover/active states match the palette exactly.
pub fn button(ui: &mut Ui, label: &str, kind: Kind) -> Response {
    let font = FontId::proportional(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), TEXT);
    let size = Vec2::new(galley.size().x + 26.0, 30.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = response.hovered();
    let radius = CornerRadius::same(8);

    let (bg, fg, stroke) = match kind {
        Kind::Primary => {
            let bg = if hovered {
                mix(ACCENT, Color32::WHITE, 0.16)
            } else {
                ACCENT
            };
            (bg, Color32::WHITE, Stroke::NONE)
        }
        Kind::Ghost => (
            if hovered {
                CARD_HOVER
            } else {
                Color32::TRANSPARENT
            },
            if hovered { TEXT } else { TEXT_DIM },
            Stroke::new(1.0_f32, if hovered { BORDER_HOVER } else { BORDER }),
        ),
        Kind::Quiet => (
            if hovered {
                alpha(Color32::WHITE, 10)
            } else {
                Color32::TRANSPARENT
            },
            if hovered { TEXT } else { MUTED },
            Stroke::NONE,
        ),
        Kind::Danger => (
            if hovered {
                alpha(RED, 26)
            } else {
                Color32::TRANSPARENT
            },
            RED,
            Stroke::NONE,
        ),
        Kind::DangerSolid => (
            if hovered {
                mix(RED, Color32::BLACK, 0.12)
            } else {
                RED
            },
            Color32::from_rgb(28, 10, 12),
            Stroke::NONE,
        ),
    };

    if ui.is_rect_visible(rect) {
        if bg != Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, radius, bg);
        }
        if stroke.width > 0.0 {
            ui.painter()
                .rect_stroke(rect, radius, stroke, StrokeKind::Inside);
        }
        let galley = ui.painter().layout_no_wrap(label.to_owned(), font, fg);
        ui.painter()
            .galley(rect.center() - galley.size() * 0.5, galley, fg);
    }
    response
}

/// Small square button with a single glyph (✕, ↻, …).
pub fn icon_button(ui: &mut Ui, glyph: &str, tooltip: &str, kind: Kind) -> Response {
    let font = FontId::proportional(13.0);
    let size = Vec2::splat(28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let response = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip);
    let hovered = response.hovered();
    let radius = CornerRadius::same(8);
    let (bg, fg) = match kind {
        Kind::Danger => (
            if hovered {
                alpha(RED, 26)
            } else {
                Color32::TRANSPARENT
            },
            if hovered { RED } else { MUTED },
        ),
        _ => (
            if hovered {
                CARD_HOVER
            } else {
                Color32::TRANSPARENT
            },
            if hovered { TEXT } else { MUTED },
        ),
    };
    if ui.is_rect_visible(rect) {
        if bg != Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, radius, bg);
        }
        let color = fg;
        if glyph == "✕" {
            let s = Stroke::new(1.6_f32, color);
            let c = 4.5;
            ui.painter().line_segment(
                [
                    rect.center() + Vec2::new(-c, -c),
                    rect.center() + Vec2::new(c, c),
                ],
                s,
            );
            ui.painter().line_segment(
                [
                    rect.center() + Vec2::new(c, -c),
                    rect.center() + Vec2::new(-c, c),
                ],
                s,
            );
        } else {
            let galley = ui.painter().layout_no_wrap(glyph.to_owned(), font, color);
            ui.painter()
                .galley(rect.center() - galley.size() * 0.5, galley, color);
        }
    }
    response
}

/// Compact tinted label, e.g. quantization or "视觉".
pub fn badge(ui: &mut Ui, text: &str, tone: Color32) -> Response {
    let font = FontId::proportional(11.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), tone);
    let size = Vec2::new(galley.size().x + 16.0, 19.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let radius = CornerRadius::same(6);
        ui.painter().rect_filled(rect, radius, alpha(tone, 22));
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(1.0_f32, alpha(tone, 46)),
            StrokeKind::Inside,
        );
        ui.painter()
            .galley(rect.center() - galley.size() * 0.5, galley, tone);
    }
    response
}

/// Status pill: colored dot plus label.
pub fn pill(ui: &mut Ui, text: &str, tone: Color32) -> Response {
    let font = FontId::proportional(11.5);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), tone);
    let size = Vec2::new(galley.size().x + 30.0, 21.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let radius = CornerRadius::same(11);
        ui.painter().rect_filled(rect, radius, alpha(tone, 20));
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(1.0_f32, alpha(tone, 44)),
            StrokeKind::Inside,
        );
        ui.painter()
            .circle_filled(Pos2::new(rect.left() + 11.0, rect.center().y), 3.0, tone);
        ui.painter().galley(
            rect.center() - galley.size() * 0.5 + Vec2::new(5.0, 0.0),
            galley,
            tone,
        );
    }
    response
}

/// Neutral metadata chip (counts, sizes, hosts).
pub fn chip(ui: &mut Ui, text: &str, dot: Option<Color32>) -> Response {
    let font = FontId::proportional(11.5);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), TEXT_DIM);
    let lead = if dot.is_some() { 12.0 } else { 0.0 };
    let size = Vec2::new(galley.size().x + 20.0 + lead, 24.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let radius = CornerRadius::same(12);
        ui.painter().rect_filled(rect, radius, CARD);
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(1.0_f32, BORDER),
            StrokeKind::Inside,
        );
        let mut x = rect.left() + 10.0;
        if let Some(color) = dot {
            ui.painter()
                .circle_filled(Pos2::new(x + 2.0, rect.center().y), 3.0, color);
            x += lead;
        }
        ui.painter().galley(
            Pos2::new(x, rect.center().y - galley.size().y * 0.5),
            galley,
            TEXT_DIM,
        );
    }
    response
}

/// Status dot with a soft halo.
pub fn status_dot(ui: &mut Ui, tone: Color32, glow: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    dot(ui.painter(), rect.center(), tone, glow);
}

/// Painted status dot with an optional soft halo.
pub fn dot(painter: &Painter, center: Pos2, tone: Color32, glow: bool) {
    if glow {
        painter.circle_filled(center, 5.5, alpha(tone, 34));
    }
    painter.circle_filled(center, 3.0, tone);
}

/// Allocate an exact-size region and run `add` inside a child ui with the given layout.
pub fn region(ui: &mut Ui, size: Vec2, layout: Layout, add: impl FnOnce(&mut Ui)) -> Rect {
    let rect = Rect::from_min_size(ui.cursor().min, size);
    if ui.is_rect_visible(rect) {
        ui.scope_builder(UiBuilder::new().max_rect(rect).layout(layout), add);
        ui.advance_cursor_after_rect(rect);
    }
    rect
}

/// A grid cell of fixed width whose height grows with its content.
pub fn column(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui)) {
    let start = ui.cursor().min;
    let max = Rect::from_min_size(start, Vec2::new(width, ui.available_height().max(1.0)));
    let inner = ui.scope_builder(
        UiBuilder::new()
            .max_rect(max)
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            ui.set_width(width);
            add(ui);
        },
    );
    let used = Rect::from_min_size(
        start,
        Vec2::new(width, inner.response.rect.height().max(1.0)),
    );
    ui.advance_cursor_after_rect(used);
}

/// Dim the whole window while a modal dialog is open.
pub fn scrim(ctx: &egui::Context) {
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("modal_scrim"))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .interactable(true)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
            ui.painter()
                .rect_filled(rect, 0.0, Color32::from_black_alpha(150));
            response.on_hover_cursor(egui::CursorIcon::Default);
        });
}

/// Thin progress bar painted to match the theme.
pub fn progress(ui: &mut Ui, ratio: f32, width: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 6.0), Sense::hover());
    let radius = CornerRadius::same(3);
    ui.painter()
        .rect_filled(rect, radius, alpha(Color32::WHITE, 16));
    let ratio = ratio.clamp(0.0, 1.0);
    if ratio > 0.0 {
        let w = (rect.width() * ratio).max(5.0);
        let bar = Rect::from_min_size(rect.min, Vec2::new(w, rect.height()));
        ui.painter().rect_filled(bar, radius, color);
    }
}

/// Bordered stat tile used on the status page.
pub fn stat_card(
    ui: &mut Ui,
    width: f32,
    label: &str,
    value: &str,
    sub: &str,
    accent: Color32,
    progress_ratio: Option<f32>,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 98.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let radius = CornerRadius::same(12);
    painter.rect_filled(rect, radius, CARD);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0_f32, BORDER),
        StrokeKind::Inside,
    );

    let pad = 16.0;
    let left = rect.left() + pad;

    painter.circle_filled(Pos2::new(left + 3.0, rect.top() + 22.0), 3.0, accent);
    let l = painter.layout_no_wrap(label.to_owned(), FontId::proportional(10.5), MUTED);
    painter.galley(
        Pos2::new(left + 12.0, rect.top() + 22.0 - l.size().y * 0.5),
        l,
        MUTED,
    );

    let v = painter.layout_no_wrap(value.to_owned(), FontId::proportional(21.0), TEXT);
    let vpos = Pos2::new(left, rect.top() + 38.0);
    painter.galley(vpos, v.clone(), TEXT);
    if !sub.is_empty() {
        let s = painter.layout_no_wrap(sub.to_owned(), FontId::proportional(11.0), FAINT);
        painter.galley(
            Pos2::new(
                left + v.size().x + 6.0,
                vpos.y + v.size().y - s.size().y - 3.0,
            ),
            s,
            FAINT,
        );
    }

    let track = Rect::from_min_size(
        Pos2::new(left, rect.bottom() - 23.0),
        Vec2::new(rect.width() - pad * 2.0, 5.0),
    );
    if let Some(ratio) = progress_ratio {
        let bar_radius = CornerRadius::same(3);
        painter.rect_filled(track, bar_radius, alpha(Color32::WHITE, 14));
        let ratio = ratio.clamp(0.0, 1.0);
        if ratio > 0.0 {
            let fill = Rect::from_min_size(
                track.min,
                Vec2::new((track.width() * ratio).max(5.0), track.height()),
            );
            painter.rect_filled(fill, bar_radius, accent);
        }
    }
}

/// Compact horizontal slider with the app look. Returns the response.
pub fn slider_u64(ui: &mut Ui, value: &mut u64, min: u64, max: u64, width: f32) -> Response {
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::click_and_drag());
    let range = max.saturating_sub(min).max(1) as f32;
    let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width() - 20.0, 6.0));
    let radius = CornerRadius::same(3);

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let t = (*value).clamp(min, max).saturating_sub(min) as f32 / range;
        let x = track.left() + track.width() * t;

        painter.rect_filled(track, radius, alpha(Color32::WHITE, 16));
        if x > track.left() + 1.0 {
            painter.rect_filled(
                Rect::from_min_max(track.min, Pos2::new(x, track.max.y)),
                radius,
                ACCENT,
            );
        }
        let center = Pos2::new(x, track.center().y);
        if response.hovered() || response.dragged() {
            painter.circle_filled(center, 12.0, alpha(ACCENT, 40));
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        painter.circle_filled(center, 8.0, Color32::WHITE);
        painter.circle_stroke(center, 8.0, Stroke::new(1.0_f32, alpha(ACCENT, 120)));
    }

    if response.dragged() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let t = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
            let next = min + (t * range).round() as u64;
            if next != *value {
                *value = next.clamp(min, max);
                response.mark_changed();
            }
        }
    }
    response
}

/// Centered placeholder for empty pages. Returns true when the action is clicked.
pub fn empty_state(
    ui: &mut Ui,
    icon: NavIcon,
    title: &str,
    body: &str,
    action: Option<&str>,
) -> bool {
    let mut clicked = false;
    let top = ((ui.available_height() - 210.0) * 0.42).max(24.0);
    let size = ui.available_size();
    region(ui, size, Layout::top_down(Align::Center), |ui| {
        ui.add_space(top);
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(16), alpha(ACCENT, 18));
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(16),
            Stroke::new(1.0_f32, alpha(ACCENT, 40)),
            StrokeKind::Inside,
        );
        paint_nav_icon(ui.painter(), rect.center(), icon, ACCENT_TEXT);
        ui.add_space(16.0);
        ui.label(egui::RichText::new(title).size(15.5).strong().color(TEXT));
        ui.add_space(6.0);
        ui.label(egui::RichText::new(body).size(12.0).color(FAINT));
        if let Some(action) = action {
            ui.add_space(16.0);
            if button(ui, action, Kind::Primary).clicked() {
                clicked = true;
            }
        }
    });
    clicked
}

// ------------------------------------------------------------------ icons

#[derive(Clone, Copy)]
pub enum NavIcon {
    Discover,
    Models,
    Services,
    Status,
    Logs,
    Settings,
}

/// Line icons drawn with primitives, so they stay crisp and themed.
pub fn paint_nav_icon(painter: &Painter, center: Pos2, icon: NavIcon, color: Color32) {
    let stroke = Stroke::new(1.6_f32, color);
    match icon {
        NavIcon::Discover => {
            painter.line_segment(
                [center + Vec2::new(0.0, -6.2), center + Vec2::new(0.0, 1.2)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(-3.3, -2.2), center + Vec2::new(0.0, 1.2)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(3.3, -2.2), center + Vec2::new(0.0, 1.2)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(-6.0, 4.6), center + Vec2::new(6.0, 4.6)],
                Stroke::new(1.5_f32, color),
            );
        }
        NavIcon::Models => {
            for (i, width) in [11.0_f32, 11.0, 7.0].iter().enumerate() {
                let y = center.y - 4.5 + i as f32 * 4.5;
                let rect =
                    Rect::from_min_size(Pos2::new(center.x - 6.0, y - 1.1), Vec2::new(*width, 2.2));
                painter.rect_filled(rect, CornerRadius::same(2), color);
            }
        }
        NavIcon::Services => {
            let outer = Rect::from_center_size(center, Vec2::new(13.0, 11.0));
            painter.rect_stroke(outer, CornerRadius::same(3), stroke, StrokeKind::Inside);
            painter.line_segment(
                [
                    Pos2::new(outer.left() + 2.5, outer.center().y),
                    Pos2::new(outer.right() - 2.5, outer.center().y),
                ],
                Stroke::new(1.2_f32, color),
            );
            painter.circle_filled(Pos2::new(outer.left() + 3.2, outer.top() + 2.8), 1.1, color);
            painter.circle_filled(
                Pos2::new(outer.left() + 3.2, outer.bottom() - 2.8),
                1.1,
                color,
            );
        }
        NavIcon::Status => {
            let points = [
                (-7.0, 0.0),
                (-3.5, 0.0),
                (-1.5, -4.8),
                (0.5, 4.8),
                (2.5, 0.0),
                (7.0, 0.0),
            ];
            for pair in points.windows(2) {
                painter.line_segment(
                    [
                        center + Vec2::new(pair[0].0, pair[0].1),
                        center + Vec2::new(pair[1].0, pair[1].1),
                    ],
                    stroke,
                );
            }
        }
        NavIcon::Logs => {
            painter.line_segment(
                [
                    center + Vec2::new(-6.0, -3.6),
                    center + Vec2::new(-2.4, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(-2.4, 0.0), center + Vec2::new(-6.0, 3.6)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(-0.5, 3.2), center + Vec2::new(6.0, 3.2)],
                Stroke::new(1.4_f32, color),
            );
            painter.line_segment(
                [center + Vec2::new(1.2, -2.6), center + Vec2::new(6.0, -2.6)],
                Stroke::new(1.4_f32, color),
            );
        }
        NavIcon::Settings => {
            let xs = [-2.6_f32, 2.2, -0.8];
            for (i, x) in xs.iter().enumerate() {
                let y = center.y - 4.2 + i as f32 * 4.2;
                painter.line_segment(
                    [Pos2::new(center.x - 6.0, y), Pos2::new(center.x + 6.0, y)],
                    Stroke::new(1.3_f32, color),
                );
                painter.circle_filled(Pos2::new(center.x + x, y), 1.9, color);
            }
        }
    }
}

/// Generic glyph painted as a small icon plate (used by the toast).
pub fn toast_icon(painter: &Painter, rect: Rect, ok: bool) {
    let tone = if ok { GREEN } else { RED };
    painter.rect_filled(rect, CornerRadius::same(7), alpha(tone, 26));
    let s = Stroke::new(1.7_f32, tone);
    let c = rect.center();
    if ok {
        painter.line_segment([c + Vec2::new(-4.0, 0.2), c + Vec2::new(-1.0, 3.2)], s);
        painter.line_segment([c + Vec2::new(-1.0, 3.2), c + Vec2::new(4.2, -3.2)], s);
    } else {
        painter.line_segment([c + Vec2::new(-3.6, -3.6), c + Vec2::new(3.6, 3.6)], s);
        painter.line_segment([c + Vec2::new(3.6, -3.6), c + Vec2::new(-3.6, 3.6)], s);
    }
}

/// Draw a dot with optional label (used for small "healthy" markers).
pub fn dot_label(ui: &mut Ui, tone: Color32, text: &str) -> Response {
    let font = FontId::proportional(11.5);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, TEXT_DIM);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(galley.size().x + 14.0, 18.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .circle_filled(Pos2::new(rect.left() + 3.0, rect.center().y), 3.0, tone);
        ui.painter().galley(
            Pos2::new(rect.left() + 11.0, rect.center().y - galley.size().y * 0.5),
            galley,
            TEXT_DIM,
        );
    }
    response
}
