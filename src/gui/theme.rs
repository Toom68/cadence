//! Visual design: selectable UI themes (Graphite, Midnight, Paper),
//! typography and painted widgets.
//!
//! Every color the app paints with lives in a [`Palette`], derived from the
//! active [`ThemeChoice`]. GUI code reads the palette once per frame
//! (`CadenceApp::palette`) and passes it to the free helpers in this module —
//! there is no global theme state.

use egui::{
    epaint::Shadow, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Margin, Painter, Pos2,
    Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, TextStyle, Ui, Vec2, WidgetInfo,
    WidgetType,
};

/// The resolved colors of one theme. All fields are `Color32` so a palette is
/// a cheap `Copy` value that can be passed around by value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// App background / clear color.
    pub bg: Color32,
    /// Deepest surface (egui `extreme_bg_color`: text edits, scroll areas).
    pub bg_deep: Color32,
    /// Panel fills (sidebar, transport/status bars, menus).
    pub panel: Color32,
    /// Raised fills (cards, windows, `faint_bg_color`).
    pub raised: Color32,
    /// Resting control fill (buttons, chips, inactive widgets).
    pub control: Color32,
    pub control_hover: Color32,
    pub control_active: Color32,
    pub border: Color32,
    pub border_soft: Color32,
    /// Timeline/editor canvas background.
    pub canvas: Color32,
    /// Alternating lane background.
    pub canvas_alt: Color32,
    pub grid_faint: Color32,
    pub grid_beat: Color32,
    pub grid_bar: Color32,
    /// Sunken groove color (fader/meter tracks, inset displays).
    pub inset: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    /// Primary accent (play button, playhead, selection, meters).
    pub accent: Color32,
    /// Secondary accent (links, marquee, secondary toggles).
    pub link: Color32,
    /// Error / clipping color.
    pub danger: Color32,
    /// Warning color.
    pub warn: Color32,
    /// Selection highlight fill behind `accent` selection strokes.
    pub selection_bg: Color32,
}

const GRAPHITE: Palette = Palette {
    bg: Color32::from_rgb(19, 18, 17),
    bg_deep: Color32::from_rgb(17, 16, 15),
    panel: Color32::from_rgb(29, 27, 25),
    raised: Color32::from_rgb(38, 36, 33),
    control: Color32::from_rgb(49, 46, 42),
    control_hover: Color32::from_rgb(62, 58, 53),
    control_active: Color32::from_rgb(74, 69, 63),
    border: Color32::from_rgb(60, 56, 51),
    border_soft: Color32::from_rgb(44, 41, 38),
    canvas: Color32::from_rgb(23, 22, 20),
    canvas_alt: Color32::from_rgb(27, 26, 24),
    grid_faint: Color32::from_rgb(36, 34, 31),
    grid_beat: Color32::from_rgb(46, 43, 39),
    grid_bar: Color32::from_rgb(68, 63, 57),
    inset: Color32::from_rgb(22, 21, 19),
    text: Color32::from_rgb(237, 232, 224),
    text_dim: Color32::from_rgb(165, 158, 148),
    text_faint: Color32::from_rgb(114, 108, 100),
    accent: Color32::from_rgb(198, 243, 107),
    link: Color32::from_rgb(154, 167, 255),
    danger: Color32::from_rgb(255, 122, 102),
    warn: Color32::from_rgb(255, 196, 92),
    selection_bg: Color32::from_rgb(76, 94, 42),
};

const MIDNIGHT: Palette = Palette {
    bg: Color32::from_rgb(13, 16, 24),
    bg_deep: Color32::from_rgb(9, 11, 17),
    panel: Color32::from_rgb(20, 24, 35),
    raised: Color32::from_rgb(28, 33, 47),
    control: Color32::from_rgb(38, 45, 63),
    control_hover: Color32::from_rgb(51, 59, 81),
    control_active: Color32::from_rgb(63, 73, 98),
    border: Color32::from_rgb(57, 66, 89),
    border_soft: Color32::from_rgb(43, 50, 69),
    canvas: Color32::from_rgb(16, 19, 29),
    canvas_alt: Color32::from_rgb(21, 25, 36),
    grid_faint: Color32::from_rgb(31, 36, 51),
    grid_beat: Color32::from_rgb(43, 50, 69),
    grid_bar: Color32::from_rgb(66, 76, 102),
    inset: Color32::from_rgb(18, 22, 32),
    text: Color32::from_rgb(229, 235, 245),
    text_dim: Color32::from_rgb(148, 158, 180),
    text_faint: Color32::from_rgb(97, 107, 129),
    accent: Color32::from_rgb(190, 242, 120),
    link: Color32::from_rgb(164, 178, 255),
    danger: Color32::from_rgb(255, 128, 108),
    warn: Color32::from_rgb(255, 199, 102),
    selection_bg: Color32::from_rgb(56, 76, 44),
};

const PAPER: Palette = Palette {
    bg: Color32::from_rgb(243, 240, 233),
    bg_deep: Color32::from_rgb(233, 228, 217),
    panel: Color32::from_rgb(250, 247, 240),
    raised: Color32::from_rgb(255, 252, 246),
    control: Color32::from_rgb(233, 227, 215),
    control_hover: Color32::from_rgb(221, 214, 200),
    control_active: Color32::from_rgb(203, 195, 179),
    border: Color32::from_rgb(207, 199, 185),
    border_soft: Color32::from_rgb(221, 214, 201),
    canvas: Color32::from_rgb(236, 231, 220),
    canvas_alt: Color32::from_rgb(229, 223, 210),
    grid_faint: Color32::from_rgb(218, 211, 197),
    grid_beat: Color32::from_rgb(198, 189, 171),
    grid_bar: Color32::from_rgb(165, 154, 132),
    inset: Color32::from_rgb(217, 210, 197),
    text: Color32::from_rgb(43, 38, 31),
    text_dim: Color32::from_rgb(102, 94, 82),
    text_faint: Color32::from_rgb(134, 126, 112),
    accent: Color32::from_rgb(184, 76, 44),
    link: Color32::from_rgb(52, 88, 158),
    danger: Color32::from_rgb(188, 42, 40),
    warn: Color32::from_rgb(168, 108, 18),
    selection_bg: Color32::from_rgb(240, 224, 200),
};

/// The selectable UI themes, in menu order. [`ThemeChoice::Graphite`] is the
/// default and reproduces the original warm-graphite look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeChoice {
    /// Warm near-black with a lime accent (the original Cadence look).
    #[default]
    Graphite,
    /// Cool navy dark theme.
    Midnight,
    /// Warm off-white light theme.
    Paper,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [Self::Graphite, Self::Midnight, Self::Paper];

    /// Stable id persisted in eframe storage.
    pub fn id(self) -> &'static str {
        match self {
            Self::Graphite => "graphite",
            Self::Midnight => "midnight",
            Self::Paper => "paper",
        }
    }

    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Graphite => "Graphite",
            Self::Midnight => "Midnight",
            Self::Paper => "Paper",
        }
    }

    /// One-line description shown in the theme menu.
    pub fn description(self) -> &'static str {
        match self {
            Self::Graphite => "Warm dark · lime accent",
            Self::Midnight => "Cool navy dark",
            Self::Paper => "Warm paper light",
        }
    }

    /// Parses a persisted id. Unknown or missing values fall back to the
    /// default theme so a corrupted/older storage entry can never break the UI.
    pub fn from_id(id: &str) -> Self {
        match id {
            "midnight" => Self::Midnight,
            "paper" => Self::Paper,
            _ => Self::Graphite,
        }
    }

    /// Restores the theme from eframe storage. Missing storage or an unknown
    /// value yields the default.
    pub fn from_storage(storage: Option<&dyn eframe::Storage>) -> Self {
        storage
            .and_then(|s| s.get_string(STORAGE_KEY))
            .map(|id| Self::from_id(&id))
            .unwrap_or_default()
    }

    /// The resolved colors of this theme.
    pub fn palette(self) -> Palette {
        match self {
            Self::Graphite => GRAPHITE,
            Self::Midnight => MIDNIGHT,
            Self::Paper => PAPER,
        }
    }

    /// Whether egui should treat this as a dark theme.
    pub fn dark_mode(self) -> bool {
        !matches!(self, Self::Paper)
    }
}

/// eframe storage key for the selected theme.
pub const STORAGE_KEY: &str = "ui.theme";

/// Accent colors offered for tracks (song data — deliberately theme-independent).
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

/// Readable ink for text/icons painted on top of a saturated `fill`
/// (accent buttons, track-colored clips and notes). Picks near-black or
/// near-white by perceived luminance so it stays legible on every theme's
/// accent and on the fixed pastel track colors alike.
pub fn ink_on(fill: Color32) -> Color32 {
    let luma =
        0.2126 * f32::from(fill.r()) + 0.7152 * f32::from(fill.g()) + 0.0722 * f32::from(fill.b());
    if luma > 165.0 {
        Color32::from_rgb(24, 22, 18)
    } else {
        Color32::from_rgb(248, 246, 240)
    }
}

pub fn kicker() -> TextStyle {
    TextStyle::Name("Kicker".into())
}

pub fn display() -> TextStyle {
    TextStyle::Name("Display".into())
}

pub fn kicker_text(pal: Palette, text: &str) -> RichText {
    RichText::new(text)
        .text_style(kicker())
        .color(pal.text_faint)
}

/// Applies the egui theme and widget visuals for `choice`. Called on startup,
/// whenever the user picks a different theme, and once per headless frame.
pub fn apply(ctx: &egui::Context, choice: ThemeChoice) {
    let pal = choice.palette();
    ctx.set_theme(egui::Theme::from_dark_mode(choice.dark_mode()));
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
        *v = if choice.dark_mode() {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        v.panel_fill = pal.panel;
        v.window_fill = pal.raised;
        v.extreme_bg_color = pal.bg_deep;
        v.faint_bg_color = pal.raised;
        v.code_bg_color = pal.control;
        v.window_stroke = Stroke::new(1.0_f32, pal.border);
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
        v.hyperlink_color = pal.link;
        v.warn_fg_color = pal.warn;
        v.error_fg_color = pal.danger;
        v.selection.bg_fill = pal.selection_bg;
        v.selection.stroke = Stroke::new(1.0_f32, pal.accent);
        v.slider_trailing_fill = true;
        v.text_cursor.stroke = Stroke::new(2.0_f32, pal.accent);
        let radius = CornerRadius::same(6);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = pal.panel;
        w.noninteractive.weak_bg_fill = pal.panel;
        w.noninteractive.bg_stroke = Stroke::new(1.0_f32, pal.border_soft);
        w.noninteractive.fg_stroke = Stroke::new(1.0_f32, pal.text_dim);
        w.noninteractive.corner_radius = radius;
        w.inactive.bg_fill = pal.control;
        w.inactive.weak_bg_fill = pal.control;
        w.inactive.bg_stroke = Stroke::new(1.0_f32, pal.border);
        w.inactive.fg_stroke = Stroke::new(1.0_f32, pal.text);
        w.inactive.corner_radius = radius;
        w.hovered.bg_fill = pal.control_hover;
        w.hovered.weak_bg_fill = pal.control_hover;
        w.hovered.bg_stroke = Stroke::new(1.0_f32, pal.text_faint);
        w.hovered.fg_stroke = Stroke::new(1.5_f32, pal.text);
        w.hovered.corner_radius = radius;
        w.hovered.expansion = 0.0;
        w.active.bg_fill = pal.control_active;
        w.active.weak_bg_fill = pal.control_active;
        w.active.bg_stroke = Stroke::new(1.0_f32, pal.accent);
        w.active.fg_stroke = Stroke::new(1.5_f32, pal.text);
        w.active.corner_radius = radius;
        w.active.expansion = 0.0;
        w.open.bg_fill = pal.control_hover;
        w.open.weak_bg_fill = pal.control_hover;
        w.open.bg_stroke = Stroke::new(1.0_f32, pal.border);
        w.open.fg_stroke = Stroke::new(1.0_f32, pal.text);
        w.open.corner_radius = radius;
    });
}

/// Card-like frame used for panels inside the main layout.
pub fn card(pal: Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(pal.raised)
        .stroke(Stroke::new(1.0_f32, pal.border_soft))
        .corner_radius(10)
        .inner_margin(Margin::same(10))
}

/// Section header: small caps kicker over a title.
pub fn section_header(ui: &mut Ui, pal: Palette, kicker: &str, title: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.label(kicker_text(pal, kicker));
        ui.label(RichText::new(title).size(15.5).color(pal.text));
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
    pal: Palette,
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
            mix(pal.raised, accent, 0.2)
        } else if down {
            pal.control_active
        } else if hovered {
            pal.control_hover
        } else {
            pal.control
        };
        let border = if active {
            accent.gamma_multiply(0.7)
        } else if hovered {
            pal.text_faint
        } else {
            pal.border
        };
        ui.painter().rect(
            rect,
            7.0,
            fill,
            Stroke::new(1.0_f32, border),
            StrokeKind::Inside,
        );
        let fg = if !enabled {
            pal.text_faint
        } else if active {
            accent
        } else {
            pal.text
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
pub fn play_button(ui: &mut Ui, pal: Palette, playing: bool, size: f32) -> Response {
    let label = if playing {
        "Pause (Space)"
    } else {
        "Play (Space)"
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, playing, label));
    let fill = if resp.is_pointer_button_down_on() {
        pal.accent.gamma_multiply(0.78)
    } else if resp.hovered() {
        mix(pal.accent, Color32::WHITE, 0.15)
    } else {
        pal.accent
    };
    ui.painter().circle_filled(rect.center(), size * 0.5, fill);
    let icon = if playing { Icon::Pause } else { Icon::Play };
    let ir = Rect::from_center_size(
        rect.center() + vec2(if playing { 0.0 } else { size * 0.04 }, 0.0),
        Vec2::splat(size * 0.36),
    );
    paint_icon(ui.painter(), ir, icon, ink_on(fill));
    resp.on_hover_text(label)
}

/// Compact text toggle (mute/solo style) of fixed size.
pub fn chip(
    ui: &mut Ui,
    pal: Palette,
    text: &str,
    on: bool,
    color: Color32,
    size: Vec2,
    tooltip: &str,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    paint_chip(ui, pal, rect, &resp, text, on, color);
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, tooltip));
    resp.on_hover_text(tooltip)
}

pub fn paint_chip(
    ui: &Ui,
    pal: Palette,
    rect: Rect,
    resp: &Response,
    text: &str,
    on: bool,
    color: Color32,
) {
    let hovered = resp.hovered();
    let fill = if on {
        color
    } else if hovered {
        pal.control_hover
    } else {
        pal.control
    };
    let border = if on { color } else { pal.border };
    ui.painter().rect(
        rect,
        5.0,
        fill,
        Stroke::new(1.0_f32, border),
        StrokeKind::Inside,
    );
    let fg = if on {
        ink_on(color)
    } else if hovered {
        pal.text
    } else {
        pal.text_dim
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(11.5),
        fg,
    );
}

/// Primary call-to-action button (filled accent).
pub fn primary_button(ui: &mut Ui, pal: Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(ink_on(pal.accent)).strong())
            .fill(pal.accent)
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
    pal: Palette,
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
        p.circle_filled(
            center,
            r,
            if active {
                pal.control_hover
            } else {
                pal.control
            },
        );
        p.add(Shape::line(
            arc_points(center, r - 3.0, a0, a0 + sweep, 32),
            Stroke::new(3.0_f32, pal.border_soft),
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
            Stroke::new(2.0_f32, pal.text),
        );
        p.text(
            Pos2::new(center.x, rect.top() + size + 3.0),
            Align2::CENTER_TOP,
            value_text,
            FontId::proportional(11.0),
            if active { pal.text } else { pal.text_dim },
        );
        p.text(
            Pos2::new(center.x, rect.top() + size + 16.0),
            Align2::CENTER_TOP,
            label,
            FontId::proportional(10.0),
            pal.text_faint,
        );
    }
    resp.on_hover_text(format!(
        "{label}: {value_text}\nDrag up/down to change · Shift for fine · Double-click to reset"
    ))
}

/// Vertical fader for a 0..=1 value. Click/drag sets the value; double-click resets.
pub fn fader(
    ui: &mut Ui,
    pal: Palette,
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
        p.rect_filled(track, 3.0, pal.inset);
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
                Stroke::new(1.0_f32, pal.grid_bar),
            );
            p.hline(
                rect.right() - 5.0..=rect.right(),
                ty,
                Stroke::new(1.0_f32, pal.grid_bar),
            );
        }
        let thumb = Rect::from_center_size(Pos2::new(rect.center().x, y), vec2(28.0, 12.0));
        let hot = resp.hovered() || resp.dragged();
        p.rect(
            thumb,
            3.0,
            if hot {
                pal.text
            } else {
                mix(pal.text, pal.control, 0.2)
            },
            Stroke::new(1.0_f32, pal.bg),
            StrokeKind::Inside,
        );
        p.hline(
            thumb.left() + 6.0..=thumb.right() - 6.0,
            thumb.center().y,
            Stroke::new(1.0_f32, pal.bg),
        );
    }
    resp.on_hover_text(format!(
        "{label}: {:.0}%\nDrag to change · Double-click to reset",
        v * 100.0
    ))
}

/// Level meter (0..=1 amplitude) painted bottom-up or left-right.
pub fn meter(p: &Painter, pal: Palette, rect: Rect, level: f32, vertical: bool) {
    p.rect_filled(rect, 2.0, pal.inset);
    let level = level.clamp(0.0, 1.0);
    if level <= 0.0005 {
        return;
    }
    let color = if level > 0.97 {
        pal.danger
    } else if level > 0.8 {
        pal.warn
    } else {
        pal.accent
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
#[allow(clippy::too_many_arguments)]
pub fn scrollbar(
    ui: &mut Ui,
    pal: Palette,
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
    p.rect_filled(rect, 4.0, pal.canvas);
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
    p.rect_filled(thumb, 3.0, if hot { pal.text_faint } else { pal.border });
    resp.dragged()
}

/// Small rounded color dot.
pub fn color_dot(ui: &mut Ui, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter().circle_filled(rect.center(), size * 0.5, color);
}

pub fn hint(ui: &mut Ui, pal: Palette, text: &str) {
    ui.label(RichText::new(text).small().color(pal.text_faint));
}
