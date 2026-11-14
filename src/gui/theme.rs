//! Visual design: warm graphite palette, typography and painted widgets.

use egui::{
    epaint::Shadow, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Margin, Painter, Pos2,
    Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, TextStyle, Ui, Vec2, WidgetInfo,
    WidgetType,
};

pub const BG: Color32 = Color32::from_rgb(19, 18, 17);
pub const PANEL: Color32 = Color32::from_rgb(29, 27, 25);
pub const RAISED: Color32 = Color32::from_rgb(38, 36, 33);
pub const CONTROL: Color32 = Color32::from_rgb(49, 46, 42);
pub const CONTROL_HOVER: Color32 = Color32::from_rgb(62, 58, 53);
pub const CONTROL_ACTIVE: Color32 = Color32::from_rgb(74, 69, 63);
pub const BORDER: Color32 = Color32::from_rgb(60, 56, 51);
pub const BORDER_SOFT: Color32 = Color32::from_rgb(44, 41, 38);
pub const CANVAS: Color32 = Color32::from_rgb(23, 22, 20);
pub const CANVAS_ALT: Color32 = Color32::from_rgb(27, 26, 24);
pub const GRID_FAINT: Color32 = Color32::from_rgb(36, 34, 31);
pub const GRID_BEAT: Color32 = Color32::from_rgb(46, 43, 39);
pub const GRID_BAR: Color32 = Color32::from_rgb(68, 63, 57);
pub const TEXT: Color32 = Color32::from_rgb(237, 232, 224);
pub const TEXT_DIM: Color32 = Color32::from_rgb(165, 158, 148);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(114, 108, 100);
pub const LIME: Color32 = Color32::from_rgb(198, 243, 107);
pub const PERIWINKLE: Color32 = Color32::from_rgb(154, 167, 255);
pub const CORAL: Color32 = Color32::from_rgb(255, 122, 102);
pub const AMBER: Color32 = Color32::from_rgb(255, 196, 92);
pub const INK: Color32 = Color32::from_rgb(22, 21, 19);

/// Accent colors offered for tracks.
pub const TRACK_PALETTE: [[u8; 3]; 10] = [
    [198, 243, 107],
    [154, 167, 255],
    [255, 122, 102],
    [255, 196, 92],
    [110, 214, 200],
    [232, 132, 214],
    [126, 200, 255],
    [255, 158, 196],
    [180, 150, 255],
    [140, 226, 140],
];

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Mixes two colors (t = 0 → a, t = 1 → b).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

pub fn kicker() -> TextStyle {
    TextStyle::Name("Kicker".into())
}

pub fn display() -> TextStyle {
    TextStyle::Name("Display".into())
}

pub fn kicker_text(text: &str) -> RichText {
    RichText::new(text).text_style(kicker()).color(TEXT_FAINT)
}

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        use FontFamily::{Monospace, Proportional};
        style.text_styles = [
            (TextStyle::Heading, FontId::new(19.0, Proportional)),
            (TextStyle::Body, FontId::new(13.5, Proportional)),
            (TextStyle::Button, FontId::new(13.5, Proportional)),
            (TextStyle::Small, FontId::new(11.0, Proportional)),
            (TextStyle::Monospace, FontId::new(12.5, Monospace)),
            (kicker(), FontId::new(10.5, Proportional)),
            (display(), FontId::new(21.0, Monospace)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = vec2(8.0, 6.0);
        s.button_padding = vec2(10.0, 4.0);
        s.interact_size = vec2(36.0, 26.0);
        s.slider_width = 120.0;
        s.combo_width = 120.0;
        s.menu_margin = Margin::same(6);
        s.window_margin = Margin::same(18);
        s.icon_width = 15.0;
        s.icon_spacing = 6.0;
        style.interaction.tooltip_delay = 0.35;

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = PANEL;
        v.window_fill = RAISED;
        v.extreme_bg_color = Color32::from_rgb(17, 16, 15);
        v.faint_bg_color = RAISED;
        v.code_bg_color = CONTROL;
        v.window_stroke = Stroke::new(1.0_f32, BORDER);
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(8);
        v.window_shadow = Shadow {
            offset: [0, 10],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(140),
        };
        v.popup_shadow = Shadow {
            offset: [0, 6],
            blur: 16,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        v.hyperlink_color = PERIWINKLE;
        v.warn_fg_color = AMBER;
        v.error_fg_color = CORAL;
        v.selection.bg_fill = Color32::from_rgb(76, 94, 42);
        v.selection.stroke = Stroke::new(1.0_f32, LIME);
        v.slider_trailing_fill = true;
        v.text_cursor.stroke = Stroke::new(2.0_f32, LIME);
        let radius = CornerRadius::same(6);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = PANEL;
        w.noninteractive.weak_bg_fill = PANEL;
        w.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER_SOFT);
        w.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_DIM);
        w.noninteractive.corner_radius = radius;
        w.inactive.bg_fill = CONTROL;
        w.inactive.weak_bg_fill = CONTROL;
        w.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
        w.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
        w.inactive.corner_radius = radius;
        w.hovered.bg_fill = CONTROL_HOVER;
        w.hovered.weak_bg_fill = CONTROL_HOVER;
        w.hovered.bg_stroke = Stroke::new(1.0_f32, TEXT_FAINT);
        w.hovered.fg_stroke = Stroke::new(1.5_f32, TEXT);
        w.hovered.corner_radius = radius;
        w.hovered.expansion = 0.0;
        w.active.bg_fill = CONTROL_ACTIVE;
        w.active.weak_bg_fill = CONTROL_ACTIVE;
        w.active.bg_stroke = Stroke::new(1.0_f32, LIME);
        w.active.fg_stroke = Stroke::new(1.5_f32, TEXT);
        w.active.corner_radius = radius;
        w.active.expansion = 0.0;
        w.open.bg_fill = CONTROL_HOVER;
        w.open.weak_bg_fill = CONTROL_HOVER;
        w.open.bg_stroke = Stroke::new(1.0_f32, BORDER);
        w.open.fg_stroke = Stroke::new(1.0_f32, TEXT);
        w.open.corner_radius = radius;
    });
}

/// Card-like frame used for panels inside the main layout.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(RAISED)
        .stroke(Stroke::new(1.0_f32, BORDER_SOFT))
        .corner_radius(10)
        .inner_margin(Margin::same(10))
}

/// Section header: small caps kicker over a title.
pub fn section_header(ui: &mut Ui, kicker: &str, title: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.label(kicker_text(kicker));
        ui.label(RichText::new(title).size(15.5).color(TEXT));
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    ToStart,
    Loop,
    Metronome,
    Undo,
    Redo,
    Plus,
    Minus,
    Duplicate,
    Trash,
    Fit,
}

fn arc_points(center: Pos2, r: f32, from: f32, to: f32, n: usize) -> Vec<Pos2> {
    (0..=n)
        .map(|i| {
            let a = from + (to - from) * i as f32 / n as f32;
            center + vec2(a.cos(), a.sin()) * r
        })
        .collect()
}

pub fn paint_icon(p: &Painter, r: Rect, icon: Icon, color: Color32) {
    let c = r.center();
    let w = r.width();
    let stroke = Stroke::new((w * 0.12).clamp(1.4, 2.2), color);
    let tri = |pts: Vec<Pos2>| Shape::convex_polygon(pts, color, Stroke::NONE);
    match icon {
        Icon::Play => {
            p.add(tri(vec![
                r.left_top() + vec2(w * 0.18, 0.0),
                Pos2::new(r.right(), c.y),
                r.left_bottom() + vec2(w * 0.18, 0.0),
            ]));
        }
        Icon::Pause => {
            let bw = w * 0.28;
            p.rect_filled(
                Rect::from_min_size(r.left_top() + vec2(w * 0.1, 0.0), vec2(bw, r.height())),
                1.5,
                color,
            );
            p.rect_filled(
                Rect::from_min_size(r.left_top() + vec2(w * 0.62, 0.0), vec2(bw, r.height())),
                1.5,
                color,
            );
        }
        Icon::Stop => {
            p.rect_filled(r.shrink(w * 0.06), 2.0, color);
        }
        Icon::ToStart => {
            p.rect_filled(
                Rect::from_min_size(r.left_top(), vec2(w * 0.16, r.height())),
                1.0,
                color,
            );
            p.add(tri(vec![
                Pos2::new(r.left() + w * 0.24, c.y),
                r.right_top(),
                r.right_bottom(),
            ]));
        }
        Icon::Loop => {
            let rr = Rect::from_center_size(c, vec2(w, r.height() * 0.66));
            p.rect_stroke(rr, rr.height() * 0.45, stroke, StrokeKind::Middle);
            let tip = Pos2::new(c.x + w * 0.2, rr.top());
            p.add(tri(vec![
                tip + vec2(-w * 0.2, -w * 0.18),
                tip,
                tip + vec2(-w * 0.2, w * 0.18),
            ]));
            let tip2 = Pos2::new(c.x - w * 0.2, rr.bottom());
            p.add(tri(vec![
                tip2 + vec2(w * 0.2, -w * 0.18),
                tip2,
                tip2 + vec2(w * 0.2, w * 0.18),
            ]));
        }
        Icon::Metronome => {
            let pts = vec![
                Pos2::new(c.x - w * 0.14, r.top()),
                Pos2::new(c.x + w * 0.14, r.top()),
                Pos2::new(r.right() - w * 0.08, r.bottom()),
                Pos2::new(r.left() + w * 0.08, r.bottom()),
            ];
            p.add(Shape::closed_line(pts, stroke));
            p.line_segment(
                [
                    Pos2::new(c.x, r.bottom() - w * 0.2),
                    Pos2::new(r.right(), r.top() + w * 0.1),
                ],
                stroke,
            );
        }
        Icon::Undo | Icon::Redo => {
            let flip = if icon == Icon::Redo { -1.0 } else { 1.0 };
            let center = Pos2::new(c.x + flip * w * 0.05, c.y + w * 0.12);
            let rad = w * 0.4;
            let pts: Vec<Pos2> = arc_points(
                center,
                rad,
                std::f32::consts::PI * 1.05,
                std::f32::consts::PI * 2.1,
                12,
            )
            .into_iter()
            .map(|q| Pos2::new(center.x + (q.x - center.x) * flip, q.y))
            .collect();
            let start = pts[0];
            p.add(Shape::line(pts, stroke));
            let s = w * 0.22;
            p.add(tri(vec![
                start + vec2(-s * 0.9 * flip, -s * 0.2),
                start + vec2(s * 0.7 * flip, -s * 0.3),
                start + vec2(0.0, s),
            ]));
        }
        Icon::Plus => {
            p.line_segment(
                [Pos2::new(r.left(), c.y), Pos2::new(r.right(), c.y)],
                stroke,
            );
            p.line_segment(
                [Pos2::new(c.x, r.top()), Pos2::new(c.x, r.bottom())],
                stroke,
            );
        }
        Icon::Minus => {
            p.line_segment(
                [Pos2::new(r.left(), c.y), Pos2::new(r.right(), c.y)],
                stroke,
            );
        }
        Icon::Duplicate => {
            let a =
                Rect::from_min_size(r.left_top() + vec2(w * 0.28, 0.0), vec2(w * 0.72, w * 0.72));
            let b =
                Rect::from_min_size(r.left_top() + vec2(0.0, w * 0.28), vec2(w * 0.72, w * 0.72));
            p.rect_stroke(a, 2.0, stroke, StrokeKind::Middle);
            p.rect_filled(b, 2.0, color.gamma_multiply(0.25));
            p.rect_stroke(b, 2.0, stroke, StrokeKind::Middle);
        }
        Icon::Trash => {
            p.line_segment(
                [
                    Pos2::new(r.left(), r.top() + w * 0.18),
                    Pos2::new(r.right(), r.top() + w * 0.18),
                ],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - w * 0.15, r.top()),
                    Pos2::new(c.x + w * 0.15, r.top()),
                ],
                stroke,
            );
            let body = Rect::from_min_max(
                Pos2::new(r.left() + w * 0.12, r.top() + w * 0.3),
                Pos2::new(r.right() - w * 0.12, r.bottom()),
            );
            p.rect_stroke(body, 2.0, stroke, StrokeKind::Middle);
        }
        Icon::Fit => {
            p.line_segment(
                [Pos2::new(r.left(), c.y), Pos2::new(r.right(), c.y)],
                stroke,
            );
            p.line_segment([r.left_top(), r.left_bottom()], stroke);
            p.line_segment([r.right_top(), r.right_bottom()], stroke);
        }
    }
}

/// Square button with a painted icon. `active` lights it in `accent`.
pub fn icon_button(
    ui: &mut Ui,
    icon: Icon,
    size: f32,
    active: bool,
    accent: Color32,
    tooltip: &str,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, active, tooltip));
    if ui.is_rect_visible(rect) {
        let down = resp.is_pointer_button_down_on();
        let hovered = resp.hovered() && enabled;
        let fill = if active {
            mix(RAISED, accent, 0.2)
        } else if down {
            CONTROL_ACTIVE
        } else if hovered {
            CONTROL_HOVER
        } else {
            CONTROL
        };
        let border = if active {
            accent.gamma_multiply(0.7)
        } else if hovered {
            TEXT_FAINT
        } else {
            BORDER
        };
        ui.painter().rect(
            rect,
            7.0,
            fill,
            Stroke::new(1.0_f32, border),
            StrokeKind::Inside,
        );
        let fg = if !enabled {
            TEXT_FAINT
        } else if active {
            accent
        } else {
            TEXT
        };
        paint_icon(
            ui.painter(),
            Rect::from_center_size(rect.center(), Vec2::splat((size * 0.4).round())),
            icon,
            fg,
        );
    }
    resp.on_hover_text(tooltip)
}

/// The big round primary transport button.
pub fn play_button(ui: &mut Ui, playing: bool, size: f32) -> Response {
    let label = if playing {
        "Pause (Space)"
    } else {
        "Play (Space)"
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, playing, label));
    let fill = if resp.is_pointer_button_down_on() {
        mix(LIME, INK, 0.25)
    } else if resp.hovered() {
        mix(LIME, Color32::WHITE, 0.15)
    } else {
        LIME
    };
    ui.painter().circle_filled(rect.center(), size * 0.5, fill);
    let icon = if playing { Icon::Pause } else { Icon::Play };
    let ir = Rect::from_center_size(
        rect.center() + vec2(if playing { 0.0 } else { size * 0.04 }, 0.0),
        Vec2::splat(size * 0.36),
    );
    paint_icon(ui.painter(), ir, icon, INK);
    resp.on_hover_text(label)
}

/// Compact text toggle (mute/solo style) of fixed size.
pub fn chip(
    ui: &mut Ui,
    text: &str,
    on: bool,
    color: Color32,
    size: Vec2,
    tooltip: &str,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    paint_chip(ui, rect, &resp, text, on, color);
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, tooltip));
    resp.on_hover_text(tooltip)
}

pub fn paint_chip(ui: &Ui, rect: Rect, resp: &Response, text: &str, on: bool, color: Color32) {
    let hovered = resp.hovered();
    let fill = if on {
        color
    } else if hovered {
        CONTROL_HOVER
    } else {
        CONTROL
    };
    let border = if on { color } else { BORDER };
    ui.painter().rect(
        rect,
        5.0,
        fill,
        Stroke::new(1.0_f32, border),
        StrokeKind::Inside,
    );
    let fg = if on {
        INK
    } else if hovered {
        TEXT
    } else {
        TEXT_DIM
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(11.5),
        fg,
    );
}

/// Primary call-to-action button (filled lime).
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(INK).strong())
            .fill(LIME)
            .stroke(Stroke::NONE)
            .min_size(vec2(88.0, 30.0)),
    )
}

pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Button::new(text).min_size(vec2(88.0, 30.0)))
}

/// Rotary knob editing a normalized 0..=1 value by vertical drag.
/// Double-click resets to `default`; hold Shift for fine control.
#[allow(clippy::too_many_arguments)]
pub fn knob(
    ui: &mut Ui,
    norm: &mut f32,
    default: f32,
    bipolar: bool,
    label: &str,
    value_text: &str,
    color: Color32,
    size: f32,
) -> Response {
    let total = vec2(size.max(44.0), size + 30.0);
    let (rect, mut resp) = ui.allocate_exact_size(total, Sense::click_and_drag());
    let before = *norm;
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let d = resp.drag_delta();
        *norm = (*norm + (d.x - d.y) / if fine { 600.0 } else { 150.0 }).clamp(0.0, 1.0);
    }
    if resp.double_clicked() {
        *norm = default;
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 && ui.input(|i| i.modifiers.alt) {
            *norm = (*norm + scroll.signum() * 0.02).clamp(0.0, 1.0);
        }
    }
    if *norm != before {
        resp.mark_changed();
    }
    let v = *norm;
    resp.widget_info(|| WidgetInfo::slider(true, v as f64, format!("{label} {value_text}")));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let center = Pos2::new(rect.center().x, rect.top() + size * 0.5);
        let r = size * 0.5 - 2.0;
        let a0 = std::f32::consts::PI * 0.75;
        let sweep = std::f32::consts::PI * 1.5;
        let active = resp.hovered() || resp.dragged();
        p.circle_filled(center, r, if active { CONTROL_HOVER } else { CONTROL });
        p.add(Shape::line(
            arc_points(center, r - 3.0, a0, a0 + sweep, 32),
            Stroke::new(3.0_f32, BORDER_SOFT),
        ));
        let (from, to) = if bipolar {
            (0.5f32.min(v), 0.5f32.max(v))
        } else {
            (0.0, v)
        };
        if to - from > 0.005 {
            p.add(Shape::line(
                arc_points(center, r - 3.0, a0 + sweep * from, a0 + sweep * to, 24),
                Stroke::new(3.0_f32, color),
            ));
        }
        let a = a0 + sweep * v;
        p.line_segment(
            [
                center + vec2(a.cos(), a.sin()) * (r * 0.25),
                center + vec2(a.cos(), a.sin()) * (r - 6.0),
            ],
            Stroke::new(2.0_f32, TEXT),
        );
        p.text(
            Pos2::new(center.x, rect.top() + size + 3.0),
            Align2::CENTER_TOP,
            value_text,
            FontId::proportional(11.0),
            if active { TEXT } else { TEXT_DIM },
        );
        p.text(
            Pos2::new(center.x, rect.top() + size + 16.0),
            Align2::CENTER_TOP,
            label,
            FontId::proportional(10.0),
            TEXT_FAINT,
        );
    }
    resp.on_hover_text(format!(
        "{label}: {value_text}\nDrag up/down to change · Shift for fine · Double-click to reset"
    ))
}

/// Vertical fader for a 0..=1 value. Click/drag sets the value; double-click resets.
pub fn fader(
    ui: &mut Ui,
    value: &mut f32,
    default: f32,
    height: f32,
    color: Color32,
    label: &str,
) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(34.0, height), Sense::click_and_drag());
    let before = *value;
    let track = Rect::from_center_size(rect.center(), vec2(6.0, height - 14.0));
    if resp.double_clicked() {
        *value = default;
    } else if resp.is_pointer_button_down_on() {
        if let Some(pos) = resp.interact_pointer_pos() {
            *value = ((track.bottom() - pos.y) / track.height()).clamp(0.0, 1.0);
        }
    }
    if *value != before {
        resp.mark_changed();
    }
    let v = *value;
    resp.widget_info(|| WidgetInfo::slider(true, v as f64, label));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(track, 3.0, INK);
        let y = track.bottom() - track.height() * v;
        p.rect_filled(
            Rect::from_min_max(Pos2::new(track.left(), y), track.right_bottom()),
            3.0,
            color.gamma_multiply(0.85),
        );
        for i in 0..=4 {
            let ty = track.bottom() - track.height() * i as f32 / 4.0;
            p.hline(
                rect.left()..=rect.left() + 5.0,
                ty,
                Stroke::new(1.0_f32, GRID_BAR),
            );
            p.hline(
                rect.right() - 5.0..=rect.right(),
                ty,
                Stroke::new(1.0_f32, GRID_BAR),
            );
        }
        let thumb = Rect::from_center_size(Pos2::new(rect.center().x, y), vec2(28.0, 12.0));
        let hot = resp.hovered() || resp.dragged();
        p.rect(
            thumb,
            3.0,
            if hot { TEXT } else { mix(TEXT, CONTROL, 0.2) },
            Stroke::new(1.0_f32, INK),
            StrokeKind::Inside,
        );
        p.hline(
            thumb.left() + 6.0..=thumb.right() - 6.0,
            thumb.center().y,
            Stroke::new(1.0_f32, INK),
        );
    }
    resp.on_hover_text(format!(
        "{label}: {:.0}%\nDrag to change · Double-click to reset",
        v * 100.0
    ))
}

/// Level meter (0..=1 amplitude) painted bottom-up or left-right.
pub fn meter(p: &Painter, rect: Rect, level: f32, vertical: bool) {
    p.rect_filled(rect, 2.0, INK);
    let level = level.clamp(0.0, 1.0);
    if level <= 0.0005 {
        return;
    }
    let color = if level > 0.97 {
        CORAL
    } else if level > 0.8 {
        AMBER
    } else {
        LIME
    };
    let fill = if vertical {
        Rect::from_min_max(
            Pos2::new(rect.left(), rect.bottom() - rect.height() * level),
            rect.right_bottom(),
        )
    } else {
        Rect::from_min_max(
            rect.left_top(),
            Pos2::new(rect.left() + rect.width() * level, rect.bottom()),
        )
    };
    p.rect_filled(fill, 2.0, color);
}

/// Thin custom scrollbar. Returns true while it is being dragged.
pub fn scrollbar(
    ui: &mut Ui,
    rect: Rect,
    id: egui::Id,
    vertical: bool,
    content: f32,
    viewport: f32,
    offset: &mut f32,
) -> bool {
    let max = (content - viewport).max(0.0);
    *offset = offset.clamp(0.0, max);
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, CANVAS);
    if max <= 0.0 {
        return false;
    }
    let len = if vertical {
        rect.height()
    } else {
        rect.width()
    };
    let thumb_len = (len * viewport / content).clamp(24.0, len);
    let travel = (len - thumb_len).max(1.0);
    if resp.is_pointer_button_down_on() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let along = if vertical {
                pos.y - rect.top()
            } else {
                pos.x - rect.left()
            };
            *offset = ((along - thumb_len * 0.5) / travel * max).clamp(0.0, max);
        }
    }
    let start = *offset / max * travel;
    let thumb = if vertical {
        Rect::from_min_size(
            Pos2::new(rect.left() + 2.0, rect.top() + start),
            vec2(rect.width() - 4.0, thumb_len),
        )
    } else {
        Rect::from_min_size(
            Pos2::new(rect.left() + start, rect.top() + 2.0),
            vec2(thumb_len, rect.height() - 4.0),
        )
    };
    let hot = resp.hovered() || resp.dragged();
    p.rect_filled(thumb, 3.0, if hot { TEXT_FAINT } else { BORDER });
    resp.dragged()
}

/// Small rounded color dot.
pub fn color_dot(ui: &mut Ui, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter().circle_filled(rect.center(), size * 0.5, color);
}

pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(TEXT_FAINT));
}
