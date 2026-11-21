//! Cadence design system: themes, typography, icons and widget primitives.
//!
//! Every screen paints with the pieces in this module so the app keeps one
//! look. There is no global theme state: GUI code reads the [`Palette`] once
//! per frame (`CadenceApp::palette`) and passes it to the free helpers below.
//!
//! # Themes and color tokens
//!
//! [`ThemeChoice`] (Graphite — default, Midnight, Paper) resolves to a
//! [`Palette`]. Use tokens, never literal colors:
//!
//! | token | use |
//! |---|---|
//! | `bg`, `bg_deep`, `panel`, `raised` | surfaces, darkest to most raised |
//! | `canvas`, `canvas_alt`, `grid_faint`, `grid_beat`, `grid_bar` | timeline / editor grids |
//! | `inset` | sunken grooves and readouts |
//! | `control`, `control_hover`, `control_active`, `control_border` | button bodies |
//! | `border`, `border_soft` | separators and card outlines |
//! | `text`, `text_dim`, `text_faint` | primary / secondary / caption text, all ≥ 4.5:1 |
//! | `text_disabled` | disabled ink, ≥ 3:1 on `panel`, `raised` and `control` |
//! | `accent`, `link`, `danger`, `warn`, `selection_bg` | state colors |
//! | `mixer.*` ([`MixerPalette`]) | the mixing desk: strips, wells, meters, fader caps |
//!
//! Text on a saturated fill uses [`ink_on`]; [`mix`], [`rgb`],
//! [`relative_luminance`] and [`contrast_ratio`] are the color helpers. The
//! contrast floors are enforced by the tests at the bottom of this file.
//!
//! # Size and spacing tokens (4 px grid)
//!
//! - control heights: [`CONTROL_S`] 24, [`CONTROL_M`] 28 (default), [`CONTROL_L`] 32
//! - icon boxes: [`ICON_S`] 14, [`ICON_M`] 16, [`ICON_L`] 18
//! - gaps: [`GAP_XS`] 4, [`GAP_S`] 8, [`GAP_M`] 12, [`GAP_L`] 16, [`GAP_XL`] 24
//! - radii: [`RADIUS_S`] 4 (chips), [`RADIUS_M`] 6 (controls), [`RADIUS_L`] 10 (cards)
//! - toolbars: [`TOOLBAR_H`] 40 tall, [`TOOLBAR_GAP`] 4 between items
//! - text sizes: [`TEXT_HEADING`] 18, [`TEXT_TITLE`] 14, [`TEXT_BODY`] 13,
//!   [`TEXT_SMALL`] 11, [`TEXT_KICKER`] 10
//!
//! # Typography
//!
//! The UI font is Inter 4.1 (`assets/fonts`, SIL OFL 1.1): Regular for the
//! proportional family and SemiBold as the named family [`strong_family`].
//! egui's default fonts stay behind both as fallbacks (emoji), then DejaVu
//! Sans 2.37 as the last resort for symbols none of them has (▸ ▾ ♭ ♯ ✕ ⋯);
//! monospace stays Hack (with the same last resort). [`apply`] installs the fonts and the
//! text styles and returns `true` once the fonts are live (they activate one
//! pass after they are requested — until then every "strong" style quietly
//! uses the regular family, so nothing can panic on a fresh `Context`).
//!
//! - `ui.label(theme::strong_text("Song timeline"))` — SemiBold body text
//! - `ui.label(theme::kicker_text(pal, "TEMPO"))` — small tracked caps label
//! - `theme::strong_font(ui, 12.0)` / `theme::body_font(12.0)` — `FontId`s for painters
//! - `theme::kicker()`, `theme::display()`, `theme::strong()` — named `TextStyle`s
//!
//! # Icons
//!
//! [`Icon`] glyphs are painted vectors on a 16-unit grid: one stroke weight
//! that scales with size, pixel-snapped so they stay crisp from 12 to 28 px.
//! `theme::paint_icon(ui.painter(), rect, Icon::Snap, pal.text)`.
//!
//! Transport: `Play` `Pause` `Stop` `ToStart` `Record` `Loop` `Metronome`
//! `Follow`. Edit: `Undo` `Redo` `Plus` `Minus` `Close` `Check` `Duplicate`
//! `Trash` `Pencil` `Pointer` `Eraser` `Split` `Wand` `Quantize` `Dice`.
//! Navigation: `ChevronDown` `ChevronRight` `ChevronLeft` `ChevronUp` `Menu`
//! `Grip` `SidebarCollapse` `SidebarExpand` `Search` `ZoomIn` `ZoomOut` `Fit`.
//! Music: `Marker` `Snap` `Chords` `Piano` `Pads` `Mixer` `Automation` `Arp`
//! `Note` `Wave` `Keyboard` `Speaker` `SpeakerOff` `Headphones`. App:
//! `Settings` `Folder` `Save` `Export` `Import` `Info` `Warning`.
//! ([`Icon::ALL`] lists them; [`Icon::name`] gives the label.)
//!
//! # Widgets
//!
//! Buttons (`size`/`height` are the tokens above; disabled states stay legible):
//! - `theme::icon_button(ui, pal, Icon::Loop, CONTROL_M, looping, pal.accent, "Loop (L)")`
//! - `theme::ghost_icon_button(ui, pal, Icon::Close, CONTROL_S, "Close")` — no body until hovered
//! - `theme::icon_label_button(ui, pal, Icon::Marker, "Marker", CONTROL_M, "Add a marker")`
//! - `theme::text_button(ui, pal, "Fill", CONTROL_M, "Fill the pattern")`
//! - `theme::toggle_button(ui, pal, "Chords", on, CONTROL_M, "Show the chord lane")`
//! - `theme::icon_toggle_button(ui, pal, Icon::Follow, "Follow", on, CONTROL_M, "Follow the playhead")`
//! - `theme::chip(ui, pal, "M", muted, pal.danger, vec2(24.0, 24.0), "Mute")` — fixed size, own ON color
//! - `theme::primary_button(ui, pal, "Save")`, `theme::secondary_button(ui, "Cancel")`
//! - `theme::play_button(ui, pal, playing, 44.0)`
//! - `theme::shortcut_tip(ui.ctx(), "Undo", &SC_UNDO)` — "Undo (Ctrl+Z)" / "Undo (⌘Z)"
//!
//! Choices and values:
//! - `theme::segmented(ui, pal, &mut tab, &[Segment::new(Tab::Editor, "Pattern Editor"),
//!   Segment::new(Tab::Mixer, "Mixer")], CONTROL_M, None)` — tab strips and small enums
//! - `theme::slider(ui, pal, &mut swing, 0.0..=0.6, 120.0)` — thin track, accent fill, round handle
//!   (`slider_with` adds a custom color and a double-click default)
//! - `theme::combo("snap", "Bar", 96.0).show_ui(ui, |ui| { .. })` — combo box with a chevron
//! - `theme::knob(..)`, `theme::meter(..)`, `theme::scrollbar(..)`
//!
//! Text:
//! - `theme::truncated_label(ui, name, 140.0)` — ellipsis, full text on hover
//! - `theme::elide(ui, text, font, color, max_w)` / `theme::paint_elided(..)` — the same for painters
//! - `theme::hint(ui, pal, "No instruments match.")`
//!
//! Structure:
//! - `theme::toolbar(ui, pal, "arranger", |tb| { .. })` — overflow-safe toolbar row, see [`Toolbar`]
//! - `theme::panel_header(ui, pal, "Mixer", |ui| { /* trailing controls, right to left */ })`
//! - `theme::section_header(ui, pal, "ARRANGEMENT", "Song timeline")`
//! - `theme::empty_state(ui, pal, Icon::Note, "No clips yet", "Double-click a lane to add one.", Some("Add clip"))`
//! - `theme::card(pal)`, `theme::divider_v(ui, pal, 24.0)`, `theme::color_dot(ui, color, 8.0)`
//!
//! # Developer gallery
//!
//! Launch with `CADENCE_DESIGN_GALLERY=1` (or `=controls`, `=layout`,
//! `=colors` to pick the first tab) to open a window showing every icon,
//! widget state and palette token in the active theme.

// The primitives here are a toolkit for the screen modules; not every helper
// has a caller yet.
#![allow(dead_code)]

use std::hash::Hash;
use std::ops::RangeInclusive;
use std::sync::{Arc, LazyLock};

use egui::{
    emath::Numeric,
    epaint::{
        text::{LayoutJob, TextWrapping},
        Shadow,
    },
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily,
    FontId, Galley, Key, KeyboardShortcut, Layout, Margin, Painter, Pos2, Rect, Response, RichText,
    Sense, Shape, Stroke, StrokeKind, TextStyle, Ui, UiBuilder, Vec2, WidgetInfo, WidgetText,
    WidgetType,
};

// ----- palette ---------------------------------------------------------------

/// The resolved colors of one theme: a cheap `Copy` value passed around by
/// value. Every field is a `Color32` (the mixer's own set lives in
/// [`MixerPalette`]).
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
    /// Outline of a control body; stronger than `border` so a resting button
    /// still reads as a button on `panel` and `raised`.
    pub control_border: Color32,
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
    /// Primary text.
    pub text: Color32,
    /// Secondary text (≥ 4.5:1 on every surface and on `control`).
    pub text_dim: Color32,
    /// Captions and kickers (≥ 4.5:1 on `bg`, `panel`, `raised`, canvases).
    pub text_faint: Color32,
    /// Disabled text and icons (≥ 3:1 on `panel`, `raised` and `control`).
    pub text_disabled: Color32,
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
    /// The mixing desk.
    pub mixer: MixerPalette,
}

/// Colors of the mixing desk: a layered stack (desk, strip bodies, master)
/// with recessed wells for faders and meters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixerPalette {
    /// The desk behind the strips.
    pub panel: Color32,
    /// Channel strip body.
    pub strip: Color32,
    /// Selected/hovered strip body and popups opened from a strip.
    pub strip_hi: Color32,
    /// Master strip body.
    pub master: Color32,
    /// Strip outline.
    pub border: Color32,
    /// Dividers inside a strip.
    pub border_soft: Color32,
    /// Recessed groove: fader tracks and meter wells.
    pub well: Color32,
    pub control: Color32,
    pub control_hover: Color32,
    pub control_active: Color32,
    /// Unlit arc segment of a knob.
    pub arc: Color32,
    /// Fader scale ticks.
    pub tick: Color32,
    /// Text selection inside mixer text edits.
    pub select: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    /// Meter segments below the hot zone.
    pub meter_low: Color32,
    /// Meter segments approaching full scale.
    pub meter_mid: Color32,
    /// Meter segments at the top of the scale.
    pub meter_high: Color32,
    /// Latched clip indicator.
    pub meter_clip: Color32,
    /// Unlit meter segment.
    pub meter_off: Color32,
    /// Peak-hold cap over the top lit segment.
    pub meter_peak: Color32,
    /// Fader cap body.
    pub cap_body: Color32,
    /// Fader cap lit top edge.
    pub cap_hi: Color32,
    /// Fader cap centre groove.
    pub cap_line: Color32,
    /// Fader cap outline.
    pub cap_edge: Color32,
    /// Pan arcs, bus strips, secondary toggles.
    pub pan: Color32,
    /// Mute state.
    pub mute: Color32,
    /// Solo state.
    pub solo: Color32,
}

const fn c(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

const GRAPHITE: Palette = Palette {
    bg: c(19, 18, 17),
    bg_deep: c(17, 16, 15),
    panel: c(29, 27, 25),
    raised: c(38, 36, 33),
    control: c(52, 49, 45),
    control_hover: c(64, 60, 55),
    control_active: c(76, 71, 65),
    control_border: c(78, 73, 66),
    border: c(60, 56, 51),
    border_soft: c(44, 41, 38),
    canvas: c(23, 22, 20),
    canvas_alt: c(27, 26, 24),
    grid_faint: c(36, 34, 31),
    grid_beat: c(46, 43, 39),
    grid_bar: c(68, 63, 57),
    inset: c(22, 21, 19),
    text: c(237, 232, 224),
    text_dim: c(178, 171, 161),
    text_faint: c(148, 142, 133),
    text_disabled: c(130, 124, 116),
    accent: c(198, 243, 107),
    link: c(154, 167, 255),
    danger: c(255, 122, 102),
    warn: c(255, 196, 92),
    selection_bg: c(76, 94, 42),
    mixer: MixerPalette {
        panel: c(30, 29, 27),
        strip: c(42, 40, 37),
        strip_hi: c(48, 46, 42),
        master: c(50, 48, 44),
        border: c(64, 61, 56),
        border_soft: c(53, 51, 47),
        well: c(21, 20, 18),
        control: c(57, 54, 50),
        control_hover: c(68, 65, 59),
        control_active: c(80, 76, 69),
        arc: c(36, 34, 31),
        tick: c(92, 88, 81),
        select: c(92, 88, 80),
        text: c(232, 227, 219),
        text_dim: c(178, 172, 162),
        text_faint: c(158, 152, 143),
        meter_low: c(84, 196, 104),
        meter_mid: c(240, 198, 72),
        meter_high: c(242, 88, 70),
        meter_clip: c(255, 84, 72),
        meter_off: c(34, 33, 30),
        meter_peak: c(238, 234, 226),
        cap_body: c(190, 187, 180),
        cap_hi: c(234, 231, 224),
        cap_line: c(118, 115, 108),
        cap_edge: c(92, 89, 83),
        pan: c(148, 170, 255),
        mute: c(255, 112, 100),
        solo: c(255, 188, 84),
    },
};

const MIDNIGHT: Palette = Palette {
    bg: c(13, 16, 24),
    bg_deep: c(9, 11, 17),
    panel: c(20, 24, 35),
    raised: c(28, 33, 47),
    control: c(40, 47, 66),
    control_hover: c(52, 60, 83),
    control_active: c(64, 74, 100),
    control_border: c(70, 80, 107),
    border: c(57, 66, 89),
    border_soft: c(43, 50, 69),
    canvas: c(16, 19, 29),
    canvas_alt: c(21, 25, 36),
    grid_faint: c(31, 36, 51),
    grid_beat: c(43, 50, 69),
    grid_bar: c(66, 76, 102),
    inset: c(18, 22, 32),
    text: c(229, 235, 245),
    text_dim: c(162, 172, 193),
    text_faint: c(133, 143, 165),
    text_disabled: c(113, 123, 146),
    accent: c(190, 242, 120),
    link: c(164, 178, 255),
    danger: c(255, 128, 108),
    warn: c(255, 199, 102),
    selection_bg: c(56, 76, 44),
    mixer: MixerPalette {
        panel: c(18, 22, 32),
        strip: c(30, 35, 50),
        strip_hi: c(36, 42, 59),
        master: c(38, 44, 62),
        border: c(60, 69, 93),
        border_soft: c(47, 55, 75),
        well: c(11, 14, 22),
        control: c(45, 53, 74),
        control_hover: c(57, 66, 90),
        control_active: c(69, 79, 106),
        arc: c(25, 30, 43),
        tick: c(84, 95, 124),
        select: c(80, 92, 124),
        text: c(229, 235, 245),
        text_dim: c(166, 176, 197),
        text_faint: c(142, 152, 174),
        meter_low: c(84, 200, 116),
        meter_mid: c(240, 200, 80),
        meter_high: c(244, 92, 76),
        meter_clip: c(255, 88, 78),
        meter_off: c(24, 29, 41),
        meter_peak: c(232, 238, 248),
        cap_body: c(184, 190, 202),
        cap_hi: c(228, 233, 243),
        cap_line: c(110, 118, 134),
        cap_edge: c(84, 92, 108),
        pan: c(164, 178, 255),
        mute: c(255, 120, 104),
        solo: c(255, 196, 96),
    },
};

const PAPER: Palette = Palette {
    bg: c(243, 240, 233),
    bg_deep: c(233, 228, 217),
    panel: c(250, 247, 240),
    raised: c(255, 252, 246),
    control: c(233, 227, 215),
    control_hover: c(221, 214, 200),
    control_active: c(203, 195, 179),
    control_border: c(186, 177, 161),
    border: c(207, 199, 185),
    border_soft: c(221, 214, 201),
    canvas: c(236, 231, 220),
    canvas_alt: c(229, 223, 210),
    grid_faint: c(218, 211, 197),
    grid_beat: c(198, 189, 171),
    grid_bar: c(165, 154, 132),
    inset: c(217, 210, 197),
    text: c(43, 38, 31),
    text_dim: c(76, 69, 59),
    text_faint: c(94, 87, 76),
    text_disabled: c(128, 120, 107),
    accent: c(184, 76, 44),
    link: c(52, 88, 158),
    danger: c(188, 42, 40),
    warn: c(146, 92, 10),
    selection_bg: c(240, 224, 200),
    // A light desk with dark wells, like a hardware console: the fader slots
    // and meter windows stay dark so lit segments read the same in every theme.
    mixer: MixerPalette {
        panel: c(232, 227, 216),
        strip: c(247, 243, 235),
        strip_hi: c(253, 250, 244),
        master: c(255, 252, 246),
        border: c(196, 188, 173),
        border_soft: c(214, 207, 193),
        well: c(48, 44, 38),
        control: c(233, 227, 215),
        control_hover: c(221, 214, 200),
        control_active: c(203, 195, 179),
        arc: c(212, 205, 192),
        tick: c(150, 142, 128),
        select: c(222, 206, 180),
        text: c(43, 38, 31),
        text_dim: c(76, 69, 59),
        text_faint: c(94, 87, 76),
        meter_low: c(96, 204, 112),
        meter_mid: c(240, 198, 72),
        meter_high: c(242, 88, 70),
        meter_clip: c(255, 84, 72),
        meter_off: c(66, 61, 54),
        meter_peak: c(244, 240, 232),
        cap_body: c(250, 247, 240),
        cap_hi: c(255, 255, 255),
        cap_line: c(120, 113, 102),
        cap_edge: c(96, 89, 78),
        pan: c(52, 88, 158),
        mute: c(188, 42, 40),
        solo: c(150, 96, 12),
    },
};

/// The selectable UI themes, in menu order. [`ThemeChoice::Graphite`] is the
/// default: warm near-black with the lime accent.
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

// ----- size and spacing tokens -----------------------------------------------

/// Small control height (dense rows, chips, inline buttons).
pub const CONTROL_S: f32 = 24.0;
/// Default control height (toolbars, combo boxes, text fields).
pub const CONTROL_M: f32 = 28.0;
/// Large control height (dialog buttons, panel headers).
pub const CONTROL_L: f32 = 32.0;

/// Icon box inside a [`CONTROL_S`] control.
pub const ICON_S: f32 = 14.0;
/// Icon box inside a [`CONTROL_M`] control.
pub const ICON_M: f32 = 16.0;
/// Icon box inside a [`CONTROL_L`] control.
pub const ICON_L: f32 = 18.0;

/// Gaps on the 4 px grid.
pub const GAP_XS: f32 = 4.0;
pub const GAP_S: f32 = 8.0;
pub const GAP_M: f32 = 12.0;
pub const GAP_L: f32 = 16.0;
pub const GAP_XL: f32 = 24.0;

/// Corner radius of chips and other small bodies.
pub const RADIUS_S: f32 = 4.0;
/// Corner radius of controls.
pub const RADIUS_M: f32 = 6.0;
/// Corner radius of cards and panels.
pub const RADIUS_L: f32 = 10.0;

/// Height of a [`toolbar`] row.
pub const TOOLBAR_H: f32 = 40.0;
/// Gap between neighbouring [`toolbar`] items.
pub const TOOLBAR_GAP: f32 = 4.0;

/// Text sizes (points) tuned for Inter.
pub const TEXT_HEADING: f32 = 18.0;
pub const TEXT_TITLE: f32 = 14.0;
pub const TEXT_BODY: f32 = 13.0;
pub const TEXT_SMALL: f32 = 11.0;
pub const TEXT_KICKER: f32 = 10.0;

// ----- color helpers ---------------------------------------------------------

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Mixes two colors (t = 0 → a, t = 1 → b).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// sRGB channel value → linear light, tabulated once.
static LINEAR: LazyLock<[f32; 256]> = LazyLock::new(|| {
    let mut table = [0.0; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        *slot = if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        };
    }
    table
});

/// WCAG relative luminance of an opaque color (0 = black, 1 = white).
pub fn relative_luminance(c: Color32) -> f32 {
    let t = &*LINEAR;
    0.2126 * t[c.r() as usize] + 0.7152 * t[c.g() as usize] + 0.0722 * t[c.b() as usize]
}

/// WCAG contrast ratio between two opaque colors (1.0 ..= 21.0).
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

const INK_DARK: Color32 = c(24, 22, 18);
const INK_LIGHT: Color32 = c(248, 246, 240);

/// Readable ink for text/icons painted on top of a saturated `fill`
/// (accent buttons, track-colored clips and notes): whichever of near-black
/// and near-white has the higher WCAG contrast against it.
pub fn ink_on(fill: Color32) -> Color32 {
    // Dark ink wins exactly when (L + .05)² ≥ (L_dark + .05)(L_light + .05).
    static THRESHOLD: LazyLock<f32> = LazyLock::new(|| {
        ((relative_luminance(INK_DARK) + 0.05) * (relative_luminance(INK_LIGHT) + 0.05)).sqrt()
            - 0.05
    });
    if relative_luminance(fill) >= *THRESHOLD {
        INK_DARK
    } else {
        INK_LIGHT
    }
}

// ----- typography ------------------------------------------------------------

const INTER_REGULAR: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");
/// Symbol coverage none of the other fonts has (geometric shapes, music
/// signs, dingbats, math operators).
const DEJAVU_SANS: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");

static STRONG_FAMILY: LazyLock<FontFamily> = LazyLock::new(|| FontFamily::Name("Strong".into()));
static KICKER_STYLE: LazyLock<TextStyle> = LazyLock::new(|| TextStyle::Name("Kicker".into()));
static DISPLAY_STYLE: LazyLock<TextStyle> = LazyLock::new(|| TextStyle::Name("Display".into()));
static STRONG_STYLE: LazyLock<TextStyle> = LazyLock::new(|| TextStyle::Name("Strong".into()));

/// The SemiBold family (`FontFamily::Name("Strong")`). Only bound once
/// [`apply`] has returned `true`; painters should use [`strong_font`], which
/// falls back to the regular family until then.
pub fn strong_family() -> FontFamily {
    STRONG_FAMILY.clone()
}

/// Small tracked caps label style ("TEMPO").
pub fn kicker() -> TextStyle {
    KICKER_STYLE.clone()
}

/// Large monospace readout style (the transport position).
pub fn display() -> TextStyle {
    DISPLAY_STYLE.clone()
}

/// SemiBold body text style.
pub fn strong() -> TextStyle {
    STRONG_STYLE.clone()
}

/// Kicker label text: small, tracked, in `text_faint`.
pub fn kicker_text(pal: Palette, text: &str) -> RichText {
    RichText::new(text)
        .text_style(kicker())
        .extra_letter_spacing(0.5)
        .color(pal.text_faint)
}

/// SemiBold text for headings, titles and emphasized labels.
pub fn strong_text(text: impl Into<String>) -> RichText {
    RichText::new(text).text_style(strong())
}

/// Regular UI font at `size`.
pub fn body_font(size: f32) -> FontId {
    FontId::proportional(size)
}

/// SemiBold UI font at `size` for painters. Safe on any `Ui`: resolves to the
/// regular family while the bundled fonts are not live yet.
pub fn strong_font(ui: &Ui, size: f32) -> FontId {
    let family = ui
        .style()
        .text_styles
        .get(&*STRONG_STYLE)
        .map_or(FontFamily::Proportional, |font| font.family.clone());
    FontId::new(size, family)
}

/// The font set: Inter first, then every egui default font as a fallback
/// (emoji), then DejaVu Sans for the remaining symbols; plus the SemiBold
/// [`strong_family`]. Monospace keeps Hack first.
pub fn font_definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    for (name, data) in [
        ("Inter", INTER_REGULAR),
        ("Inter-SemiBold", INTER_SEMIBOLD),
        ("DejaVuSans", DEJAVU_SANS),
    ] {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(data)));
    }
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    let fallbacks = proportional.clone();
    proportional.insert(0, "Inter".to_owned());
    proportional.push("DejaVuSans".to_owned());
    let mut strong = vec!["Inter-SemiBold".to_owned(), "Inter".to_owned()];
    strong.extend(fallbacks);
    strong.push("DejaVuSans".to_owned());
    fonts.families.insert(strong_family(), strong);
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .push("DejaVuSans".to_owned());
    fonts
}

/// Requests the bundled fonts on first use and reports whether they are live.
/// egui activates new fonts at the start of the pass after the request, so a
/// request made mid-pass is only usable from the next pass on.
fn fonts_live(ctx: &egui::Context) -> bool {
    let id = egui::Id::new("cadence.theme.fonts-requested");
    let pass = ctx.cumulative_pass_nr();
    match ctx.data(|d| d.get_temp::<u64>(id)) {
        Some(requested) => pass > requested,
        None => {
            ctx.set_fonts(font_definitions());
            ctx.data_mut(|d| d.insert_temp(id, pass));
            false
        }
    }
}

/// Applies the fonts, text styles and widget visuals for `choice`. Called on
/// startup, on the first frames and whenever the user picks a theme.
///
/// Returns `true` once the bundled fonts are live. Until then the pass is
/// styled with the fallback family and discarded, so callers should keep
/// calling `apply` each frame until it returns `true`.
pub fn apply(ctx: &egui::Context, choice: ThemeChoice) -> bool {
    let pal = choice.palette();
    let fonts_live = fonts_live(ctx);
    if !fonts_live {
        ctx.request_discard("Cadence fonts become active on the next pass");
        ctx.request_repaint();
    }
    ctx.set_theme(egui::Theme::from_dark_mode(choice.dark_mode()));
    ctx.all_styles_mut(|style| {
        use FontFamily::{Monospace, Proportional};
        let semibold = if fonts_live {
            strong_family()
        } else {
            Proportional
        };
        style.text_styles = [
            (
                TextStyle::Heading,
                FontId::new(TEXT_HEADING, semibold.clone()),
            ),
            (TextStyle::Body, FontId::new(TEXT_BODY, Proportional)),
            (TextStyle::Button, FontId::new(TEXT_BODY, Proportional)),
            (TextStyle::Small, FontId::new(TEXT_SMALL, Proportional)),
            (TextStyle::Monospace, FontId::new(12.5, Monospace)),
            (kicker(), FontId::new(TEXT_KICKER, semibold.clone())),
            (display(), FontId::new(21.0, Monospace)),
            (strong(), FontId::new(TEXT_BODY, semibold)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = vec2(8.0, 6.0);
        s.button_padding = vec2(10.0, 4.0);
        s.interact_size = vec2(36.0, CONTROL_M);
        s.slider_width = 120.0;
        s.slider_rail_height = 4.0;
        s.combo_width = 120.0;
        s.menu_margin = Margin::same(6);
        s.window_margin = Margin::same(18);
        s.icon_width = 16.0;
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
        v.handle_shape = egui::style::HandleShape::Circle;
        // Disabled egui widgets fade toward the surface; keep their labels
        // at 3:1 or better (see `disabled_fade_keeps_labels_legible`).
        v.disabled_alpha = DISABLED_ALPHA;
        v.text_cursor.stroke = Stroke::new(2.0_f32, pal.accent);
        let radius = CornerRadius::same(RADIUS_M as u8);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = pal.panel;
        w.noninteractive.weak_bg_fill = pal.panel;
        w.noninteractive.bg_stroke = Stroke::new(1.0_f32, pal.border_soft);
        w.noninteractive.fg_stroke = Stroke::new(1.0_f32, pal.text_dim);
        w.noninteractive.corner_radius = radius;
        w.inactive.bg_fill = pal.control;
        w.inactive.weak_bg_fill = pal.control;
        w.inactive.bg_stroke = Stroke::new(1.0_f32, pal.control_border);
        w.inactive.fg_stroke = Stroke::new(1.0_f32, pal.text);
        w.inactive.corner_radius = radius;
        w.hovered.bg_fill = pal.control_hover;
        w.hovered.weak_bg_fill = pal.control_hover;
        w.hovered.bg_stroke = Stroke::new(1.0_f32, mix(pal.control_border, pal.text, 0.3));
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
        w.open.bg_stroke = Stroke::new(1.0_f32, pal.control_border);
        w.open.fg_stroke = Stroke::new(1.0_f32, pal.text);
        w.open.corner_radius = radius;
    });
    fonts_live
}

/// Opacity egui applies to everything inside a disabled `Ui`.
const DISABLED_ALPHA: f32 = 0.65;

/// "Undo (Ctrl+Z)" on Windows, "Undo (⌘Z)" on macOS: a tooltip with its
/// shortcut in the platform's notation.
pub fn shortcut_tip(ctx: &egui::Context, text: &str, shortcut: &KeyboardShortcut) -> String {
    format!("{text} ({})", ctx.format_shortcut(shortcut))
}

/// A painter for custom widgets that ignores the blanket fade of a disabled
/// `Ui`: the widget picks its own disabled colors (`text_disabled` on a dimmed
/// body), which stay legible instead of being faded a second time.
fn unfaded_painter(ui: &Ui) -> Painter {
    let mut painter = ui.painter().clone();
    if !ui.is_enabled() {
        let alpha = ui.visuals().disabled_alpha();
        if alpha > 0.0 {
            painter.set_opacity((painter.opacity() / alpha).min(1.0));
        }
    }
    painter
}

// ----- icons -----------------------------------------------------------------

/// Painted vector icons. See the module docs for the list by area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    ToStart,
    Record,
    Loop,
    Metronome,
    Follow,
    Undo,
    Redo,
    Plus,
    Minus,
    Close,
    Check,
    Duplicate,
    Trash,
    Pencil,
    Pointer,
    Eraser,
    Split,
    Wand,
    Quantize,
    Dice,
    ChevronDown,
    ChevronRight,
    ChevronLeft,
    ChevronUp,
    Menu,
    Grip,
    SidebarCollapse,
    SidebarExpand,
    Search,
    ZoomIn,
    ZoomOut,
    Fit,
    Marker,
    Snap,
    Chords,
    Piano,
    Pads,
    Mixer,
    Automation,
    Arp,
    Note,
    Wave,
    Keyboard,
    Speaker,
    SpeakerOff,
    Headphones,
    Settings,
    Folder,
    Save,
    Export,
    Import,
    Info,
    Warning,
}

impl Icon {
    /// Every icon, in the order of the module docs.
    pub const ALL: [Icon; 56] = [
        Icon::Play,
        Icon::Pause,
        Icon::Stop,
        Icon::ToStart,
        Icon::Record,
        Icon::Loop,
        Icon::Metronome,
        Icon::Follow,
        Icon::Undo,
        Icon::Redo,
        Icon::Plus,
        Icon::Minus,
        Icon::Close,
        Icon::Check,
        Icon::Duplicate,
        Icon::Trash,
        Icon::Pencil,
        Icon::Pointer,
        Icon::Eraser,
        Icon::Split,
        Icon::Wand,
        Icon::Quantize,
        Icon::Dice,
        Icon::ChevronDown,
        Icon::ChevronRight,
        Icon::ChevronLeft,
        Icon::ChevronUp,
        Icon::Menu,
        Icon::Grip,
        Icon::SidebarCollapse,
        Icon::SidebarExpand,
        Icon::Search,
        Icon::ZoomIn,
        Icon::ZoomOut,
        Icon::Fit,
        Icon::Marker,
        Icon::Snap,
        Icon::Chords,
        Icon::Piano,
        Icon::Pads,
        Icon::Mixer,
        Icon::Automation,
        Icon::Arp,
        Icon::Note,
        Icon::Wave,
        Icon::Keyboard,
        Icon::Speaker,
        Icon::SpeakerOff,
        Icon::Headphones,
        Icon::Settings,
        Icon::Folder,
        Icon::Save,
        Icon::Export,
        Icon::Import,
        Icon::Info,
        Icon::Warning,
    ];

    /// The variant name, for galleries and debugging.
    pub fn name(self) -> &'static str {
        match self {
            Icon::Play => "Play",
            Icon::Pause => "Pause",
            Icon::Stop => "Stop",
            Icon::ToStart => "ToStart",
            Icon::Record => "Record",
            Icon::Loop => "Loop",
            Icon::Metronome => "Metronome",
            Icon::Follow => "Follow",
            Icon::Undo => "Undo",
            Icon::Redo => "Redo",
            Icon::Plus => "Plus",
            Icon::Minus => "Minus",
            Icon::Close => "Close",
            Icon::Check => "Check",
            Icon::Duplicate => "Duplicate",
            Icon::Trash => "Trash",
            Icon::Pencil => "Pencil",
            Icon::Pointer => "Pointer",
            Icon::Eraser => "Eraser",
            Icon::Split => "Split",
            Icon::Wand => "Wand",
            Icon::Quantize => "Quantize",
            Icon::Dice => "Dice",
            Icon::ChevronDown => "ChevronDown",
            Icon::ChevronRight => "ChevronRight",
            Icon::ChevronLeft => "ChevronLeft",
            Icon::ChevronUp => "ChevronUp",
            Icon::Menu => "Menu",
            Icon::Grip => "Grip",
            Icon::SidebarCollapse => "SidebarCollapse",
            Icon::SidebarExpand => "SidebarExpand",
            Icon::Search => "Search",
            Icon::ZoomIn => "ZoomIn",
            Icon::ZoomOut => "ZoomOut",
            Icon::Fit => "Fit",
            Icon::Marker => "Marker",
            Icon::Snap => "Snap",
            Icon::Chords => "Chords",
            Icon::Piano => "Piano",
            Icon::Pads => "Pads",
            Icon::Mixer => "Mixer",
            Icon::Automation => "Automation",
            Icon::Arp => "Arp",
            Icon::Note => "Note",
            Icon::Wave => "Wave",
            Icon::Keyboard => "Keyboard",
            Icon::Speaker => "Speaker",
            Icon::SpeakerOff => "SpeakerOff",
            Icon::Headphones => "Headphones",
            Icon::Settings => "Settings",
            Icon::Folder => "Folder",
            Icon::Save => "Save",
            Icon::Export => "Export",
            Icon::Import => "Import",
            Icon::Info => "Info",
            Icon::Warning => "Warning",
        }
    }
}

fn arc_points(center: Pos2, r: f32, from: f32, to: f32, n: usize) -> Vec<Pos2> {
    (0..=n)
        .map(|i| {
            let a = from + (to - from) * i as f32 / n as f32;
            center + vec2(a.cos(), a.sin()) * r
        })
        .collect()
}

/// Stroke weight for an icon box `side_px` physical pixels wide, as
/// `(core, halo)`: a `core`-pixel solid line plus `halo` px of soft edge on
/// each side. One weight per size class keeps every icon in a row equally
/// heavy.
fn icon_stroke(side_px: f32) -> (f32, f32) {
    if side_px < 11.5 {
        (1.0, 0.0)
    } else if side_px < 19.5 {
        (1.0, 0.25)
    } else if side_px < 27.5 {
        (2.0, 0.0)
    } else if side_px < 39.5 {
        (2.0, 0.25)
    } else {
        (3.0, 0.0)
    }
}

/// Icon drawing surface: a 16×16 grid mapped onto a square, pixel-aligned
/// live box inside the target rect.
///
/// The box side has the same parity as the stroke core, so its centre and
/// every snapped grid point sit where a line of that weight is crisp (pixel
/// centres for odd cores, pixel edges for even ones) and mirrored points stay
/// mirrored. All positions are kept in physical pixels relative to the box
/// centre and converted to points on output.
struct IconPen {
    shapes: Vec<Shape>,
    /// Box centre, physical pixels.
    cx: f32,
    cy: f32,
    /// Physical pixels per grid unit.
    unit: f32,
    /// Solid stroke width, physical pixels.
    core: f32,
    /// Painted stroke width (core + soft edge), physical pixels.
    width: f32,
    ppp: f32,
    color: Color32,
}

impl IconPen {
    fn new(rect: Rect, color: Color32, pixels_per_point: f32) -> Self {
        let ppp = if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
            pixels_per_point
        } else {
            1.0
        };
        let lo = (rect.min.to_vec2() * ppp - Vec2::splat(1e-3)).ceil();
        let hi = (rect.max.to_vec2() * ppp + Vec2::splat(1e-3)).floor();
        let room = (hi.x - lo.x).min(hi.y - lo.y).max(1.0);
        let (core, halo) = icon_stroke(room);
        let mut side = room;
        if (side - core) % 2.0 != 0.0 {
            side -= 1.0;
        }
        let side = side.max(core);
        let min = lo + ((hi - lo - Vec2::splat(side)) * 0.5).floor();
        Self {
            shapes: Vec::with_capacity(6),
            cx: min.x + side * 0.5,
            cy: min.y + side * 0.5,
            unit: ((side - core) * 0.5).max(0.5) / 8.0,
            core,
            width: core + 2.0 * halo,
            ppp,
            color,
        }
    }

    fn stroke(&self) -> Stroke {
        Stroke::new(self.width / self.ppp, self.color)
    }

    /// Grid length → whole physical pixels (at least one).
    fn len(&self, grid: f32) -> f32 {
        (grid * self.unit).round().max(1.0)
    }

    /// Grid coordinate → pixel offset from the box centre, snapped to the
    /// line lattice symmetrically around the centre.
    fn off(&self, grid: f32) -> f32 {
        let v = (grid - 8.0) * self.unit;
        v.signum() * (v.abs() + 0.5).floor()
    }

    /// Pixel offsets from the box centre → position in points.
    fn at(&self, dx: f32, dy: f32) -> Pos2 {
        pos2((self.cx + dx) / self.ppp, (self.cy + dy) / self.ppp)
    }

    /// Snapped grid point.
    fn p(&self, x: f32, y: f32) -> Pos2 {
        self.at(self.off(x), self.off(y))
    }

    fn pts(&self, grid: &[(f32, f32)]) -> Vec<Pos2> {
        grid.iter().map(|&(x, y)| self.p(x, y)).collect()
    }

    /// Open stroked polyline through snapped grid points.
    fn line(&mut self, grid: &[(f32, f32)]) {
        let shape = Shape::line(self.pts(grid), self.stroke());
        self.shapes.push(shape);
    }

    /// Closed stroked polygon through snapped grid points.
    fn closed(&mut self, grid: &[(f32, f32)]) {
        let shape = Shape::closed_line(self.pts(grid), self.stroke());
        self.shapes.push(shape);
    }

    /// Stroked path through ready-made points.
    fn path(&mut self, points: Vec<Pos2>, closed: bool) {
        let stroke = self.stroke();
        self.shapes.push(if closed {
            Shape::closed_line(points, stroke)
        } else {
            Shape::line(points, stroke)
        });
    }

    /// Filled convex polygon through snapped grid points.
    fn fill(&mut self, grid: &[(f32, f32)]) {
        let shape = Shape::convex_polygon(self.pts(grid), self.color, Stroke::NONE);
        self.shapes.push(shape);
    }

    /// Filled convex polygon through ready-made points.
    fn fill_path(&mut self, points: Vec<Pos2>) {
        self.shapes
            .push(Shape::convex_polygon(points, self.color, Stroke::NONE));
    }

    /// Stroked circle; centre and radius are snapped so its extremes are crisp.
    fn ring(&mut self, x: f32, y: f32, r: f32) {
        let shape = Shape::circle_stroke(self.p(x, y), self.len(r) / self.ppp, self.stroke());
        self.shapes.push(shape);
    }

    /// Filled circle.
    fn dot(&mut self, x: f32, y: f32, r: f32) {
        let radius = (r * self.unit).max(self.width * 0.5 + 0.25);
        self.shapes.push(Shape::circle_filled(
            self.p(x, y),
            radius / self.ppp,
            self.color,
        ));
    }

    /// Points of an arc around a snapped centre. Angles in degrees, 0 = right,
    /// 90 = down (screen coordinates).
    fn arc_points(&self, x: f32, y: f32, r: f32, from: f32, to: f32) -> Vec<Pos2> {
        let steps = (((to - from).abs() / 12.0).ceil() as usize).max(4);
        arc_points(
            self.p(x, y),
            self.len(r) / self.ppp,
            from.to_radians(),
            to.to_radians(),
            steps,
        )
    }

    /// Stroked arc.
    fn arc(&mut self, x: f32, y: f32, r: f32, from: f32, to: f32) {
        let points = self.arc_points(x, y, r, from, to);
        self.path(points, false);
    }

    /// Stroked rounded rectangle between two snapped grid corners.
    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32) {
        self.shapes.push(Shape::rect_stroke(
            Rect::from_two_pos(self.p(x0, y0), self.p(x1, y1)),
            radius * self.unit / self.ppp,
            self.stroke(),
            StrokeKind::Middle,
        ));
    }

    /// Filled rounded rectangle covering what a core-width stroke between the
    /// two corners would, so its edges land on pixel boundaries.
    fn bar(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32) {
        let half = self.core * 0.5;
        let (ax, ay) = (self.off(x0.min(x1)) - half, self.off(y0.min(y1)) - half);
        let (bx, by) = (self.off(x0.max(x1)) + half, self.off(y0.max(y1)) + half);
        self.shapes.push(Shape::rect_filled(
            Rect::from_min_max(self.at(ax, ay), self.at(bx, by)),
            radius * self.unit / self.ppp,
            self.color,
        ));
    }

    /// A magnifier lens with its handle; `mark` 1 adds a plus, -1 a minus.
    fn magnifier(&mut self, mark: i8) {
        self.ring(6.5, 6.5, 5.0);
        let r = self.len(5.0) * std::f32::consts::FRAC_1_SQRT_2;
        let start = self.at(self.off(6.5) + r, self.off(6.5) + r);
        let end = self.p(15.0, 15.0);
        self.path(vec![start, end], false);
        if mark != 0 {
            self.line(&[(4.0, 6.5), (9.0, 6.5)]);
        }
        if mark > 0 {
            self.line(&[(6.5, 4.0), (6.5, 9.0)]);
        }
    }

    /// The curved undo arrow; `redo` mirrors it.
    fn turn_arrow(&mut self, redo: bool) {
        let m = |x: f32| if redo { 16.0 - x } else { x };
        let dir = if redo { -1.0 } else { 1.0 };
        let (cx, cy) = (self.off(m(10.0)), self.off(9.5));
        let r = self.len(4.0);
        let (top, bottom) = (cy - r, cy + r);
        let tip = self.off(m(1.0));
        let head = self.len(3.5);
        self.path(
            vec![
                self.at(tip + dir * head, top - head),
                self.at(tip, top),
                self.at(tip + dir * head, top + head),
            ],
            false,
        );
        let mut shaft = vec![self.at(tip, top)];
        let centre = self.at(cx, cy);
        let (from, to) = if redo {
            (-90.0_f32, -270.0_f32)
        } else {
            (-90.0_f32, 90.0_f32)
        };
        shaft.extend(arc_points(
            centre,
            r / self.ppp,
            from.to_radians(),
            to.to_radians(),
            14,
        ));
        shaft.push(self.at(self.off(m(6.0)), bottom));
        self.path(shaft, false);
    }

    /// A horseshoe magnet.
    fn magnet(&mut self) {
        let (cx, cy) = (self.off(8.0), self.off(9.0));
        let outer = self.len(5.5);
        let inner = self.len(1.75);
        let top = self.off(1.0);
        let centre = self.at(cx, cy);
        let mut points = vec![self.at(cx - outer, top), self.at(cx - inner, top)];
        points.extend(arc_points(
            centre,
            inner / self.ppp,
            std::f32::consts::PI,
            0.0,
            8,
        ));
        points.push(self.at(cx + inner, top));
        points.push(self.at(cx + outer, top));
        points.extend(arc_points(
            centre,
            outer / self.ppp,
            0.0,
            std::f32::consts::PI,
            16,
        ));
        self.path(points, true);
        let band = self.off(4.5);
        self.path(
            vec![self.at(cx - outer, band), self.at(cx - inner, band)],
            false,
        );
        self.path(
            vec![self.at(cx + inner, band), self.at(cx + outer, band)],
            false,
        );
    }

    /// A gear: a thick ring with eight teeth.
    fn gear(&mut self) {
        let centre = self.p(8.0, 8.0);
        let u = self.unit / self.ppp;
        self.shapes.push(Shape::circle_stroke(
            centre,
            4.0 * u,
            Stroke::new(2.3 * u, self.color),
        ));
        for i in 0..8 {
            let a = i as f32 * std::f32::consts::FRAC_PI_4;
            let (dir, side) = (vec2(a.cos(), a.sin()), vec2(-a.sin(), a.cos()));
            self.fill_path(vec![
                centre + dir * (4.8 * u) - side * (1.5 * u),
                centre + dir * (7.4 * u) - side * (1.05 * u),
                centre + dir * (7.4 * u) + side * (1.05 * u),
                centre + dir * (4.8 * u) + side * (1.5 * u),
            ]);
        }
    }
}

/// The shapes of `icon` inside `rect` (the icon's live box; the glyph is
/// centred in the largest pixel-aligned square that fits). Solid ink stays
/// inside `rect`; the soft edge of the stroke may extend ¼ physical pixel
/// beyond it.
pub fn icon_shapes(rect: Rect, icon: Icon, color: Color32, pixels_per_point: f32) -> Vec<Shape> {
    let mut pen = IconPen::new(rect, color, pixels_per_point);
    let g = &mut pen;
    match icon {
        Icon::Play => g.fill(&[(3.5, 1.0), (15.0, 8.0), (3.5, 15.0)]),
        Icon::Pause => {
            g.bar(2.5, 1.5, 6.0, 14.5, 1.0);
            g.bar(10.0, 1.5, 13.5, 14.5, 1.0);
        }
        Icon::Stop => g.bar(2.0, 2.0, 14.0, 14.0, 1.5),
        Icon::ToStart => {
            g.bar(1.5, 2.0, 3.0, 14.0, 0.5);
            g.fill(&[(15.0, 1.5), (5.5, 8.0), (15.0, 14.5)]);
        }
        Icon::Record => g.dot(8.0, 8.0, 6.5),
        Icon::Loop => {
            g.rect(1.0, 3.5, 15.0, 12.5, 4.5);
            g.fill(&[(8.0, 0.5), (11.5, 3.5), (8.0, 6.5)]);
            g.fill(&[(8.0, 9.5), (4.5, 12.5), (8.0, 15.5)]);
        }
        Icon::Metronome => {
            g.closed(&[(5.5, 1.0), (10.5, 1.0), (13.5, 15.0), (2.5, 15.0)]);
            g.line(&[(3.5, 11.0), (12.5, 11.0)]);
            g.line(&[(8.0, 11.0), (12.5, 3.0)]);
        }
        Icon::Follow => {
            g.fill(&[(1.5, 1.0), (9.5, 1.0), (5.5, 6.0)]);
            g.line(&[(5.5, 5.0), (5.5, 15.0)]);
            g.line(&[(9.0, 10.5), (15.0, 10.5)]);
            g.line(&[(12.0, 7.5), (15.0, 10.5), (12.0, 13.5)]);
        }
        Icon::Undo => g.turn_arrow(false),
        Icon::Redo => g.turn_arrow(true),
        Icon::Plus => {
            g.line(&[(8.0, 1.5), (8.0, 14.5)]);
            g.line(&[(1.5, 8.0), (14.5, 8.0)]);
        }
        Icon::Minus => g.line(&[(1.5, 8.0), (14.5, 8.0)]),
        Icon::Close => {
            g.line(&[(2.0, 2.0), (14.0, 14.0)]);
            g.line(&[(14.0, 2.0), (2.0, 14.0)]);
        }
        Icon::Check => g.line(&[(1.5, 8.5), (6.0, 13.0), (14.5, 3.5)]),
        Icon::Duplicate => {
            g.rect(5.5, 5.5, 15.0, 15.0, 1.5);
            g.line(&[
                (10.5, 3.0),
                (10.5, 1.0),
                (1.0, 1.0),
                (1.0, 10.5),
                (3.0, 10.5),
            ]);
        }
        Icon::Trash => {
            g.line(&[(1.5, 4.0), (14.5, 4.0)]);
            g.line(&[(5.5, 4.0), (5.5, 1.5), (10.5, 1.5), (10.5, 4.0)]);
            g.line(&[(3.0, 4.0), (4.0, 15.0), (12.0, 15.0), (13.0, 4.0)]);
            g.line(&[(6.5, 7.0), (6.5, 12.0)]);
            g.line(&[(9.5, 7.0), (9.5, 12.0)]);
        }
        Icon::Pencil => {
            g.closed(&[
                (11.0, 1.0),
                (15.0, 5.0),
                (5.5, 14.5),
                (1.0, 15.0),
                (1.5, 10.5),
            ]);
            g.line(&[(8.5, 3.5), (12.5, 7.5)]);
        }
        Icon::Pointer => {
            g.fill(&[(3.0, 1.0), (3.0, 12.5), (12.0, 9.0)]);
            g.fill(&[(5.5, 9.5), (8.0, 8.5), (10.5, 14.0), (8.0, 15.0)]);
        }
        Icon::Eraser => {
            g.closed(&[
                (9.5, 1.5),
                (15.0, 7.0),
                (8.5, 13.5),
                (4.5, 13.5),
                (1.0, 10.0),
            ]);
            g.line(&[(5.0, 6.0), (10.5, 11.5)]);
        }
        Icon::Split => {
            g.rect(1.0, 5.0, 5.5, 11.0, 1.0);
            g.rect(10.5, 5.0, 15.0, 11.0, 1.0);
            g.line(&[(8.0, 1.0), (8.0, 15.0)]);
        }
        Icon::Wand => {
            g.line(&[(1.0, 15.0), (9.0, 7.0)]);
            g.line(&[(12.0, 1.0), (12.0, 7.0)]);
            g.line(&[(9.0, 4.0), (15.0, 4.0)]);
            g.line(&[(3.5, 2.0), (3.5, 5.0)]);
            g.line(&[(2.0, 3.5), (5.0, 3.5)]);
            g.line(&[(13.5, 10.5), (13.5, 13.5)]);
            g.line(&[(12.0, 12.0), (15.0, 12.0)]);
        }
        Icon::Quantize => {
            g.line(&[(1.5, 1.0), (1.5, 15.0)]);
            g.line(&[(8.0, 1.0), (8.0, 15.0)]);
            g.line(&[(14.5, 1.0), (14.5, 15.0)]);
            g.bar(1.5, 3.5, 8.0, 6.0, 0.0);
            g.bar(8.0, 10.0, 14.5, 12.5, 0.0);
        }
        Icon::Dice => {
            g.rect(1.0, 1.0, 15.0, 15.0, 2.5);
            for (x, y) in [
                (4.8, 4.8),
                (11.2, 4.8),
                (8.0, 8.0),
                (4.8, 11.2),
                (11.2, 11.2),
            ] {
                g.dot(x, y, 1.0);
            }
        }
        Icon::ChevronDown => g.line(&[(2.0, 5.0), (8.0, 11.0), (14.0, 5.0)]),
        Icon::ChevronUp => g.line(&[(2.0, 11.0), (8.0, 5.0), (14.0, 11.0)]),
        Icon::ChevronRight => g.line(&[(5.0, 2.0), (11.0, 8.0), (5.0, 14.0)]),
        Icon::ChevronLeft => g.line(&[(11.0, 2.0), (5.0, 8.0), (11.0, 14.0)]),
        Icon::Menu => {
            for x in [2.5, 8.0, 13.5] {
                g.dot(x, 8.0, 1.4);
            }
        }
        Icon::Grip => {
            for x in [5.5, 10.5] {
                for y in [3.0, 8.0, 13.0] {
                    g.dot(x, y, 1.2);
                }
            }
        }
        Icon::SidebarCollapse | Icon::SidebarExpand => {
            g.rect(1.0, 2.5, 15.0, 13.5, 1.5);
            g.line(&[(6.0, 2.5), (6.0, 13.5)]);
            if icon == Icon::SidebarCollapse {
                g.line(&[(12.0, 5.5), (9.5, 8.0), (12.0, 10.5)]);
            } else {
                g.line(&[(9.5, 5.5), (12.0, 8.0), (9.5, 10.5)]);
            }
        }
        Icon::Search => g.magnifier(0),
        Icon::ZoomIn => g.magnifier(1),
        Icon::ZoomOut => g.magnifier(-1),
        Icon::Fit => {
            g.line(&[(1.5, 2.5), (1.5, 13.5)]);
            g.line(&[(14.5, 2.5), (14.5, 13.5)]);
            g.line(&[(4.5, 8.0), (11.5, 8.0)]);
            g.line(&[(7.0, 5.5), (4.5, 8.0), (7.0, 10.5)]);
            g.line(&[(9.0, 5.5), (11.5, 8.0), (9.0, 10.5)]);
        }
        Icon::Marker => {
            g.line(&[(3.0, 1.0), (3.0, 15.0)]);
            g.fill(&[(3.0, 2.0), (13.5, 2.0), (10.5, 5.5), (3.0, 5.5)]);
            g.fill(&[(3.0, 5.5), (10.5, 5.5), (13.5, 9.0), (3.0, 9.0)]);
        }
        Icon::Snap => g.magnet(),
        Icon::Chords => {
            g.line(&[(10.0, 1.5), (10.0, 12.5)]);
            for y in [4.0, 8.25, 12.5] {
                g.dot(7.0, y, 1.8);
            }
        }
        Icon::Piano => {
            g.rect(1.0, 2.5, 15.0, 13.5, 1.5);
            g.line(&[(5.7, 8.5), (5.7, 13.5)]);
            g.line(&[(10.3, 8.5), (10.3, 13.5)]);
            g.bar(4.7, 2.5, 6.7, 8.0, 0.0);
            g.bar(9.3, 2.5, 11.3, 8.0, 0.0);
        }
        Icon::Pads => {
            for (x, y) in [(1.0, 1.0), (9.5, 1.0), (1.0, 9.5), (9.5, 9.5)] {
                g.rect(x, y, x + 5.5, y + 5.5, 1.2);
            }
        }
        Icon::Mixer => {
            for (x, y) in [(3.0, 10.5), (8.0, 5.0), (13.0, 9.0)] {
                g.line(&[(x, 1.0), (x, 15.0)]);
                g.bar(x - 1.5, y - 1.0, x + 1.5, y + 1.0, 0.5);
            }
        }
        Icon::Automation => {
            g.line(&[(1.0, 12.5), (5.5, 5.5), (10.5, 10.5), (15.0, 3.0)]);
            g.dot(5.5, 5.5, 1.7);
            g.dot(10.5, 10.5, 1.7);
        }
        Icon::Arp => {
            g.bar(1.0, 12.0, 3.5, 14.0, 0.0);
            g.bar(5.0, 9.0, 7.5, 11.0, 0.0);
            g.bar(8.5, 5.5, 11.0, 7.5, 0.0);
            g.bar(12.5, 2.0, 15.0, 4.0, 0.0);
        }
        Icon::Note => {
            g.dot(5.5, 12.0, 2.7);
            g.line(&[(8.0, 12.0), (8.0, 2.0), (13.0, 4.5)]);
        }
        Icon::Wave => {
            for (x, half) in [
                (1.5, 1.5),
                (4.75, 4.5),
                (8.0, 2.5),
                (11.25, 6.5),
                (14.5, 2.0),
            ] {
                g.line(&[(x, 8.0 - half), (x, 8.0 + half)]);
            }
        }
        Icon::Keyboard => {
            g.rect(1.0, 3.5, 15.0, 12.5, 1.5);
            for x in [4.0, 6.7, 9.3, 12.0] {
                g.dot(x, 6.5, 0.8);
            }
            g.line(&[(5.0, 9.75), (11.0, 9.75)]);
        }
        Icon::Speaker | Icon::SpeakerOff => {
            g.bar(1.5, 6.0, 4.0, 10.0, 0.0);
            g.fill(&[(4.0, 5.5), (8.5, 2.0), (8.5, 14.0), (4.0, 10.5)]);
            if icon == Icon::Speaker {
                g.arc(8.5, 8.0, 2.8, -48.0, 48.0);
                g.arc(8.5, 8.0, 6.0, -48.0, 48.0);
            } else {
                g.line(&[(11.0, 6.0), (15.0, 10.0)]);
                g.line(&[(15.0, 6.0), (11.0, 10.0)]);
            }
        }
        Icon::Headphones => {
            g.arc(8.0, 8.5, 6.0, 180.0, 360.0);
            g.bar(1.5, 8.5, 4.0, 14.0, 1.0);
            g.bar(12.0, 8.5, 14.5, 14.0, 1.0);
        }
        Icon::Settings => g.gear(),
        Icon::Folder => g.closed(&[
            (1.0, 2.5),
            (6.0, 2.5),
            (7.5, 5.0),
            (15.0, 5.0),
            (15.0, 13.5),
            (1.0, 13.5),
        ]),
        Icon::Save => {
            g.closed(&[
                (1.0, 1.0),
                (11.0, 1.0),
                (15.0, 5.0),
                (15.0, 15.0),
                (1.0, 15.0),
            ]);
            g.line(&[(4.5, 1.0), (4.5, 5.5), (10.0, 5.5), (10.0, 1.0)]);
            g.line(&[(4.0, 15.0), (4.0, 9.5), (12.0, 9.5), (12.0, 15.0)]);
        }
        Icon::Export => {
            g.line(&[(1.5, 10.0), (1.5, 15.0), (14.5, 15.0), (14.5, 10.0)]);
            g.line(&[(8.0, 11.0), (8.0, 1.0)]);
            g.line(&[(4.5, 4.5), (8.0, 1.0), (11.5, 4.5)]);
        }
        Icon::Import => {
            g.line(&[(1.5, 10.0), (1.5, 15.0), (14.5, 15.0), (14.5, 10.0)]);
            g.line(&[(8.0, 1.0), (8.0, 11.0)]);
            g.line(&[(4.5, 7.5), (8.0, 11.0), (11.5, 7.5)]);
        }
        Icon::Info => {
            g.ring(8.0, 8.0, 7.0);
            g.dot(8.0, 4.75, 0.9);
            g.line(&[(8.0, 7.25), (8.0, 11.75)]);
        }
        Icon::Warning => {
            g.closed(&[(8.0, 1.5), (15.0, 14.5), (1.0, 14.5)]);
            g.line(&[(8.0, 6.0), (8.0, 10.0)]);
            g.dot(8.0, 12.0, 0.9);
        }
    }
    pen.shapes
}

/// Paints `icon` in `rect` (its live box) in `color`.
pub fn paint_icon(p: &Painter, r: Rect, icon: Icon, color: Color32) {
    p.extend(icon_shapes(r, icon, color, p.pixels_per_point()));
}

/// Icon box side for a square button of `size`: [`ICON_S`]/[`ICON_M`]/[`ICON_L`]
/// at the three control heights, proportional in between.
fn icon_side(size: f32) -> f32 {
    (size * 0.57).round().max(10.0)
}

// ----- buttons ---------------------------------------------------------------

/// Pointer/keyboard state of a button for one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Press {
    enabled: bool,
    hovered: bool,
    down: bool,
    focused: bool,
}

impl Press {
    fn of(resp: &Response, enabled: bool) -> Self {
        Self {
            enabled,
            hovered: enabled && resp.hovered(),
            down: enabled && resp.is_pointer_button_down_on(),
            focused: enabled && resp.has_focus(),
        }
    }
}

/// How a button shows that it is switched on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum OnStyle {
    Off,
    /// Body tinted with the color, icon in the color (icon toggles).
    Tint(Color32),
    /// Body filled with the color, readable ink on top (text toggles, chips).
    Solid(Color32),
}

/// Fill and ink of a solid `color` body for hover/press feedback. The ink is
/// picked for the resting color and the fill moves *away* from it, so the
/// label never loses contrast and never flips between dark and light.
fn solid_fill(color: Color32, hovered: bool, down: bool) -> (Color32, Color32) {
    let ink = ink_on(color);
    let away = if ink == INK_DARK {
        Color32::WHITE
    } else {
        Color32::BLACK
    };
    let amount = if down {
        0.24
    } else if hovered {
        0.12
    } else {
        0.0
    };
    (mix(color, away, amount), ink)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ButtonColors {
    fill: Color32,
    border: Color32,
    ink: Color32,
}

/// The one place button colors are decided, for every state and theme.
fn button_colors(pal: Palette, press: Press, on: OnStyle, ghost: bool) -> ButtonColors {
    if !press.enabled {
        // A dimmed but present body; ink stays ≥ 3:1 against it.
        let fill = mix(pal.control, pal.panel, 0.45);
        let border = mix(pal.control_border, pal.panel, 0.5);
        return match on {
            OnStyle::Off if ghost => ButtonColors {
                fill: Color32::TRANSPARENT,
                border: Color32::TRANSPARENT,
                ink: pal.text_disabled,
            },
            OnStyle::Off => ButtonColors {
                fill,
                border,
                ink: pal.text_disabled,
            },
            OnStyle::Tint(color) | OnStyle::Solid(color) => ButtonColors {
                fill,
                border: mix(border, color, 0.45),
                ink: pal.text_disabled,
            },
        };
    }
    match on {
        OnStyle::Off if ghost => ButtonColors {
            fill: if press.down {
                pal.control_active
            } else if press.hovered {
                pal.control_hover
            } else {
                Color32::TRANSPARENT
            },
            border: Color32::TRANSPARENT,
            ink: if press.hovered || press.down {
                pal.text
            } else {
                pal.text_dim
            },
        },
        OnStyle::Off => ButtonColors {
            fill: if press.down {
                pal.control_active
            } else if press.hovered {
                pal.control_hover
            } else {
                pal.control
            },
            border: if press.hovered || press.down {
                mix(pal.control_border, pal.text, 0.3)
            } else {
                pal.control_border
            },
            ink: pal.text,
        },
        OnStyle::Tint(color) => {
            let amount = if press.down {
                0.34
            } else if press.hovered {
                0.28
            } else {
                0.2
            };
            ButtonColors {
                fill: mix(pal.raised, color, amount),
                border: mix(pal.raised, color, 0.65),
                ink: color,
            }
        }
        OnStyle::Solid(color) => {
            let (fill, ink) = solid_fill(color, press.hovered, press.down);
            ButtonColors {
                fill,
                border: fill,
                ink,
            }
        }
    }
}

fn paint_button_body(
    painter: &Painter,
    pal: Palette,
    rect: Rect,
    radius: f32,
    colors: ButtonColors,
    focused: bool,
) {
    if colors.fill != Color32::TRANSPARENT || colors.border != Color32::TRANSPARENT {
        painter.rect(
            rect,
            radius,
            colors.fill,
            Stroke::new(1.0_f32, colors.border),
            StrokeKind::Inside,
        );
    }
    if focused {
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0_f32, pal.accent),
            StrokeKind::Outside,
        );
    }
}

/// Label font for a control of height `h`.
fn button_font(h: f32) -> FontId {
    body_font(if h < 26.0 { 12.0 } else { TEXT_BODY })
}

/// Horizontal padding around a text label in a control of height `h`.
fn button_pad(h: f32) -> f32 {
    (h * 0.36).round()
}

const ICON_TEXT_GAP: f32 = 6.0;

/// Lays out a button label (uncolored, so one galley serves every state).
fn label_galley(ui: &Ui, text: &str, font: FontId) -> Arc<Galley> {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER)
}

/// Natural width of a button with the given content.
fn button_width(h: f32, icon: bool, label: Option<&Galley>) -> f32 {
    match (icon, label) {
        (_, None) => h,
        (false, Some(g)) => (g.size().x + 2.0 * button_pad(h)).ceil().max(h),
        (true, Some(g)) => {
            (GAP_S + icon_side(h) + ICON_TEXT_GAP + g.size().x + button_pad(h)).ceil()
        }
    }
}

/// Paints icon and/or label centred in `rect`.
fn paint_button_content(
    painter: &Painter,
    rect: Rect,
    icon: Option<Icon>,
    label: Option<Arc<Galley>>,
    ink: Color32,
) {
    let side = icon_side(rect.height());
    match (icon, label) {
        (Some(icon), None) => {
            paint_icon(
                painter,
                Rect::from_center_size(rect.center(), Vec2::splat(side)),
                icon,
                ink,
            );
        }
        (None, Some(galley)) => {
            let pos = rect.center() - galley.size() * 0.5;
            painter.galley(pos, galley, ink);
        }
        (Some(icon), Some(galley)) => {
            let content = side + ICON_TEXT_GAP + galley.size().x;
            let left = rect.center().x - content * 0.5 - (button_pad(rect.height()) - GAP_S) * 0.5;
            paint_icon(
                painter,
                Rect::from_min_size(pos2(left, rect.center().y - side * 0.5), Vec2::splat(side)),
                icon,
                ink,
            );
            let pos = pos2(
                left + side + ICON_TEXT_GAP,
                rect.center().y - galley.size().y * 0.5,
            );
            painter.galley(pos, galley, ink);
        }
        (None, None) => {}
    }
}

fn with_tooltip(resp: Response, tooltip: &str) -> Response {
    if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip).on_disabled_hover_text(tooltip)
    }
}

/// Paints a complete button into an already allocated `rect`.
#[allow(clippy::too_many_arguments)]
fn paint_button(
    ui: &Ui,
    painter: &Painter,
    pal: Palette,
    rect: Rect,
    resp: &Response,
    enabled: bool,
    icon: Option<Icon>,
    label: Option<Arc<Galley>>,
    on: OnStyle,
    ghost: bool,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let press = Press::of(resp, enabled);
    let colors = button_colors(pal, press, on, ghost);
    paint_button_body(painter, pal, rect, RADIUS_M, colors, press.focused);
    paint_button_content(painter, rect, icon, label, colors.ink);
}

/// Square button with a painted icon. `active` lights it in `accent`.
/// Disabled (inside `add_enabled_ui(false, ..)`) it keeps a visible body and a
/// legible icon.
///
/// `theme::icon_button(ui, pal, Icon::Loop, CONTROL_M, looping, pal.accent, "Loop (L)")`
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
    let on = if active {
        OnStyle::Tint(accent)
    } else {
        OnStyle::Off
    };
    let painter = unfaded_painter(ui);
    paint_button(
        ui,
        &painter,
        pal,
        rect,
        &resp,
        enabled,
        Some(icon),
        None,
        on,
        false,
    );
    with_tooltip(resp, tooltip)
}

/// Icon button without a resting body (list rows, panel headers); the body
/// appears on hover.
///
/// `theme::ghost_icon_button(ui, pal, Icon::Close, CONTROL_S, "Close")`
pub fn ghost_icon_button(
    ui: &mut Ui,
    pal: Palette,
    icon: Icon,
    size: f32,
    tooltip: &str,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, tooltip));
    let painter = unfaded_painter(ui);
    paint_button(
        ui,
        &painter,
        pal,
        rect,
        &resp,
        enabled,
        Some(icon),
        None,
        OnStyle::Off,
        true,
    );
    with_tooltip(resp, tooltip)
}

/// Shared body of the labelled buttons.
fn labelled_button(
    ui: &mut Ui,
    pal: Palette,
    icon: Option<Icon>,
    text: &str,
    on: Option<bool>,
    height: f32,
    tooltip: &str,
) -> Response {
    let galley = label_galley(ui, text, button_font(height));
    let width = button_width(height, icon.is_some(), Some(&*galley));
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| match on {
        Some(on) => WidgetInfo::selected(WidgetType::Button, enabled, on, text),
        None => WidgetInfo::labeled(WidgetType::Button, enabled, text),
    });
    let style = if on == Some(true) {
        OnStyle::Solid(pal.accent)
    } else {
        OnStyle::Off
    };
    let painter = unfaded_painter(ui);
    paint_button(
        ui,
        &painter,
        pal,
        rect,
        &resp,
        enabled,
        icon,
        Some(galley),
        style,
        false,
    );
    with_tooltip(resp, tooltip)
}

/// Button with an icon and a text label.
///
/// `theme::icon_label_button(ui, pal, Icon::Marker, "Marker", CONTROL_M, "Add a marker at the playhead")`
pub fn icon_label_button(
    ui: &mut Ui,
    pal: Palette,
    icon: Icon,
    text: &str,
    height: f32,
    tooltip: &str,
) -> Response {
    labelled_button(ui, pal, Some(icon), text, None, height, tooltip)
}

/// Text button in the standard control style (use [`primary_button`] for the
/// one call to action of a dialog).
///
/// `theme::text_button(ui, pal, "Fill", CONTROL_M, "Fill the pattern")`
pub fn text_button(ui: &mut Ui, pal: Palette, text: &str, height: f32, tooltip: &str) -> Response {
    labelled_button(ui, pal, None, text, None, height, tooltip)
}

/// Text toggle. OFF is a normal button body with a legible label; ON fills
/// with the accent.
///
/// `theme::toggle_button(ui, pal, "Chords", show_chords, CONTROL_M, "Show the chord lane")`
pub fn toggle_button(
    ui: &mut Ui,
    pal: Palette,
    text: &str,
    on: bool,
    height: f32,
    tooltip: &str,
) -> Response {
    labelled_button(ui, pal, None, text, Some(on), height, tooltip)
}

/// Icon + text toggle; ON fills with the accent.
///
/// `theme::icon_toggle_button(ui, pal, Icon::Follow, "Follow", follow, CONTROL_M, "Follow the playhead")`
pub fn icon_toggle_button(
    ui: &mut Ui,
    pal: Palette,
    icon: Icon,
    text: &str,
    on: bool,
    height: f32,
    tooltip: &str,
) -> Response {
    labelled_button(ui, pal, Some(icon), text, Some(on), height, tooltip)
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
    let (fill, ink) = solid_fill(pal.accent, resp.hovered(), resp.is_pointer_button_down_on());
    ui.painter().circle_filled(rect.center(), size * 0.5, fill);
    let icon = if playing { Icon::Pause } else { Icon::Play };
    let ir = Rect::from_center_size(rect.center(), Vec2::splat((size * 0.4).round()));
    paint_icon(ui.painter(), ir, icon, ink);
    resp.on_hover_text(label)
}

/// Compact toggle of a fixed `size` with its own ON `color` (mute/solo
/// style). OFF keeps a visible body, border and a legible label.
///
/// `theme::chip(ui, pal, "M", muted, pal.danger, vec2(24.0, 24.0), "Mute")`
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
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, on, tooltip));
    with_tooltip(resp, tooltip)
}

/// Paints a [`chip`] into `rect` for a response the caller already owns.
pub fn paint_chip(
    ui: &Ui,
    pal: Palette,
    rect: Rect,
    resp: &Response,
    text: &str,
    on: bool,
    color: Color32,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let enabled = ui.is_enabled();
    let press = Press::of(resp, enabled);
    let style = if on {
        OnStyle::Solid(color)
    } else {
        OnStyle::Off
    };
    let mut colors = button_colors(pal, press, style, false);
    if !on && enabled && !press.hovered && !press.down {
        colors.ink = pal.text_dim;
    }
    let painter = unfaded_painter(ui);
    paint_button_body(&painter, pal, rect, RADIUS_S, colors, press.focused);
    // Chips have a fixed size: shrink the label (down to 9 pt), then cut it,
    // rather than let it spill out of the body.
    let room = (rect.width() - 6.0).max(4.0);
    let mut galley = label_galley(ui, text, strong_font(ui, TEXT_SMALL));
    if galley.size().x > room {
        let size = (TEXT_SMALL * room / galley.size().x).max(9.0);
        galley = elide(ui, text, strong_font(ui, size), Color32::PLACEHOLDER, room);
    }
    painter.galley(rect.center() - galley.size() * 0.5, galley, colors.ink);
}

/// Primary call-to-action button (filled accent).
pub fn primary_button(ui: &mut Ui, pal: Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(strong_text(text).color(ink_on(pal.accent)))
            .fill(pal.accent)
            .stroke(Stroke::NONE)
            .min_size(vec2(88.0, CONTROL_L)),
    )
}

pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Button::new(text).min_size(vec2(88.0, CONTROL_L)))
}

// ----- segmented control -----------------------------------------------------

/// One option of a [`segmented`] control.
#[derive(Clone, Copy, Debug)]
pub struct Segment<'a, T> {
    pub value: T,
    pub label: &'a str,
    pub icon: Option<Icon>,
    pub tooltip: &'a str,
}

impl<'a, T> Segment<'a, T> {
    pub fn new(value: T, label: &'a str) -> Self {
        Self {
            value,
            label,
            icon: None,
            tooltip: "",
        }
    }

    /// Adds an icon before the label (or alone, with an empty label).
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Hover text for this option (include the shortcut if it has one).
    pub fn tip(mut self, tooltip: &'a str) -> Self {
        self.tooltip = tooltip;
        self
    }
}

/// Segmented control: one option of several, for tab strips and small enums.
/// `width` `None` sizes each segment to its label; `Some(w)` divides `w`
/// equally. The returned response is `changed()` when the selection moved.
///
/// `theme::segmented(ui, pal, &mut tab, &[Segment::new(Tab::Editor, "Pattern Editor"),
/// Segment::new(Tab::Mixer, "Mixer")], CONTROL_M, None)`
pub fn segmented<T: Copy + PartialEq>(
    ui: &mut Ui,
    pal: Palette,
    current: &mut T,
    options: &[Segment<'_, T>],
    height: f32,
    width: Option<f32>,
) -> Response {
    const INSET: f32 = 2.0;
    let font = strong_font(ui, if height < 26.0 { 11.5 } else { 12.5 });
    let pad = button_pad(height) + 2.0;
    let icon = icon_side(height) - 2.0;
    let galleys: Vec<Option<Arc<Galley>>> = options
        .iter()
        .map(|o| (!o.label.is_empty()).then(|| label_galley(ui, o.label, font.clone())))
        .collect();
    let natural = |i: usize| -> f32 {
        let text = galleys[i].as_ref().map_or(0.0, |g| g.size().x);
        let icon_w = match (options[i].icon, text > 0.0) {
            (Some(_), true) => icon + ICON_TEXT_GAP,
            (Some(_), false) => icon,
            (None, _) => 0.0,
        };
        (text + icon_w + 2.0 * pad).ceil().max(height)
    };
    let count = options.len().max(1) as f32;
    let total = match width {
        Some(w) => w,
        None => (0..options.len()).map(natural).sum::<f32>() + 2.0 * INSET,
    };
    let (rect, mut resp) = ui.allocate_exact_size(vec2(total, height), Sense::hover());
    let enabled = ui.is_enabled();
    let visible = ui.is_rect_visible(rect);
    let painter = unfaded_painter(ui);
    if visible {
        painter.rect(
            rect,
            RADIUS_M,
            pal.inset,
            Stroke::new(1.0_f32, pal.control_border),
            StrokeKind::Inside,
        );
    }
    let mut x = rect.left() + INSET;
    for (i, option) in options.iter().enumerate() {
        let w = match width {
            Some(w) => (w - 2.0 * INSET) / count,
            None => natural(i),
        };
        let seg = Rect::from_min_size(pos2(x, rect.top() + INSET), vec2(w, height - 2.0 * INSET));
        x += w;
        let seg_resp = ui.interact(seg, resp.id.with(i), Sense::click());
        let selected = *current == option.value;
        let label = if option.label.is_empty() {
            option.tooltip
        } else {
            option.label
        };
        seg_resp.widget_info(|| {
            WidgetInfo::selected(WidgetType::RadioButton, enabled, selected, label)
        });
        if seg_resp.clicked() && !selected {
            *current = option.value;
            resp.mark_changed();
        }
        if visible {
            let press = Press::of(&seg_resp, enabled);
            let ink = if !enabled {
                pal.text_disabled
            } else if selected {
                let (fill, ink) = solid_fill(pal.accent, press.hovered, press.down);
                painter.rect_filled(seg, RADIUS_M - INSET, fill);
                ink
            } else if press.hovered || press.down {
                painter.rect_filled(seg, RADIUS_M - INSET, pal.control_hover);
                pal.text
            } else {
                pal.text_dim
            };
            if !enabled && selected {
                painter.rect_filled(seg, RADIUS_M - INSET, mix(pal.control, pal.panel, 0.45));
            }
            if press.focused {
                painter.rect_stroke(
                    seg,
                    RADIUS_M - INSET,
                    Stroke::new(1.0_f32, pal.text),
                    StrokeKind::Inside,
                );
            }
            let text_w = galleys[i].as_ref().map_or(0.0, |g| g.size().x);
            let icon_w = if option.icon.is_some() { icon } else { 0.0 };
            let gap = if option.icon.is_some() && text_w > 0.0 {
                ICON_TEXT_GAP
            } else {
                0.0
            };
            let mut cx = seg.center().x - (icon_w + gap + text_w) * 0.5;
            if let Some(glyph) = option.icon {
                paint_icon(
                    &painter,
                    Rect::from_min_size(pos2(cx, seg.center().y - icon * 0.5), Vec2::splat(icon)),
                    glyph,
                    ink,
                );
                cx += icon_w + gap;
            }
            if let Some(galley) = &galleys[i] {
                painter.galley(
                    pos2(cx, seg.center().y - galley.size().y * 0.5),
                    galley.clone(),
                    ink,
                );
            }
        }
        let seg_resp = with_tooltip(seg_resp, option.tooltip);
        resp = resp.union(seg_resp);
    }
    resp
}

/// Width [`segmented`] takes with `width: None` — for reserving a toolbar
/// slot (`tb.item(segmented_width(..), |ui| segmented(..))`).
pub fn segmented_width<T>(ui: &Ui, options: &[Segment<'_, T>], height: f32) -> f32 {
    let font = strong_font(ui, if height < 26.0 { 11.5 } else { 12.5 });
    let pad = button_pad(height) + 2.0;
    let icon = icon_side(height) - 2.0;
    let segments: f32 = options
        .iter()
        .map(|o| {
            let text = if o.label.is_empty() {
                0.0
            } else {
                label_galley(ui, o.label, font.clone()).size().x
            };
            let icon_w = match (o.icon, text > 0.0) {
                (Some(_), true) => icon + ICON_TEXT_GAP,
                (Some(_), false) => icon,
                (None, _) => 0.0,
            };
            (text + icon_w + 2.0 * pad).ceil().max(height)
        })
        .sum();
    segments + 4.0
}

// ----- slider, combo ---------------------------------------------------------

/// Slider with a thin track, accent fill and a round handle. Drag or click to
/// set; hold Shift for fine control; arrow keys nudge while focused.
///
/// `theme::slider(ui, pal, &mut swing, 0.0..=0.6, 120.0)`
pub fn slider<N: Numeric>(
    ui: &mut Ui,
    pal: Palette,
    value: &mut N,
    range: RangeInclusive<N>,
    width: f32,
) -> Response {
    slider_with(ui, pal, value, range, width, pal.accent, None)
}

/// [`slider`] with a custom fill `color` and an optional `default` restored by
/// double-click.
///
/// `theme::slider_with(ui, pal, &mut vol, 0.0..=1.0, 120.0, track_color, Some(0.8))`
pub fn slider_with<N: Numeric>(
    ui: &mut Ui,
    pal: Palette,
    value: &mut N,
    range: RangeInclusive<N>,
    width: f32,
    color: Color32,
    default: Option<N>,
) -> Response {
    const HANDLE: f32 = 6.0;
    let (a, b) = (range.start().to_f64(), range.end().to_f64());
    let (lo, hi) = (a.min(b), a.max(b));
    let span = hi - lo;
    let (rect, mut resp) = ui.allocate_exact_size(
        vec2(width.max(4.0 * HANDLE), CONTROL_S),
        Sense::click_and_drag(),
    );
    let enabled = ui.is_enabled();
    let track = Rect::from_min_max(
        pos2(rect.left() + HANDLE, rect.center().y - 2.0),
        pos2(rect.right() - HANDLE, rect.center().y + 2.0),
    );
    let before = value.to_f64();
    let mut v = before;
    if enabled && span > 0.0 {
        let fine = ui.input(|i| i.modifiers.shift);
        if let (true, Some(default)) = (resp.double_clicked(), default) {
            v = default.to_f64();
        } else if fine && resp.dragged() {
            v += f64::from(resp.drag_delta().x / track.width()) * span * 0.1;
        } else if resp.is_pointer_button_down_on() || resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let t = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
                v = lo + f64::from(t) * span;
            }
        }
        if resp.has_focus() {
            let step = span * if fine { 0.001 } else { 0.01 };
            let step = if N::INTEGRAL { step.max(1.0) } else { step };
            let (left, right) = ui.input(|i| {
                (
                    i.num_presses(Key::ArrowLeft) + i.num_presses(Key::ArrowDown),
                    i.num_presses(Key::ArrowRight) + i.num_presses(Key::ArrowUp),
                )
            });
            v += step * (right as f64 - left as f64);
        }
    }
    v = v.clamp(lo, hi);
    if N::INTEGRAL {
        v = v.round();
    }
    if v != before {
        *value = N::from_f64(v);
        resp.mark_changed();
    }
    resp.widget_info(|| WidgetInfo::slider(enabled, v, ""));
    if ui.is_rect_visible(rect) {
        let painter = unfaded_painter(ui);
        let t = if span > 0.0 {
            ((v - lo) / span) as f32
        } else {
            0.0
        };
        let x = track.left() + track.width() * t;
        let hot = enabled && (resp.hovered() || resp.dragged() || resp.has_focus());
        let (groove, fill, handle) = if enabled {
            (pal.control_border, color, pal.text)
        } else {
            (
                mix(pal.control_border, pal.panel, 0.5),
                pal.text_disabled,
                pal.text_disabled,
            )
        };
        painter.rect_filled(track, 2.0, groove);
        if x > track.left() {
            painter.rect_filled(
                Rect::from_min_max(track.min, pos2(x, track.bottom())),
                2.0,
                fill,
            );
        }
        let centre = pos2(x, rect.center().y);
        if hot {
            painter.circle_filled(centre, HANDLE + 3.0, color.gamma_multiply(0.3));
        }
        painter.circle_filled(centre, HANDLE, handle);
    }
    resp
}

fn combo_chevron(ui: &Ui, rect: Rect, visuals: &egui::style::WidgetVisuals, _open: bool) {
    let side = rect.width().min(rect.height()).min(12.0);
    paint_icon(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(side)),
        Icon::ChevronDown,
        visuals.fg_stroke.color,
    );
}

/// A combo box in the standard control height with a chevron. Finish it with
/// egui's `show_ui` / `show_index`.
///
/// `theme::combo("snap", "Bar", 96.0).show_ui(ui, |ui| { ui.selectable_value(&mut snap, Snap::Bar, "Bar"); })`
pub fn combo(
    id_salt: impl Hash,
    selected_text: impl Into<WidgetText>,
    width: f32,
) -> egui::ComboBox {
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(selected_text)
        .width(width)
        .icon(combo_chevron)
}

// ----- text ------------------------------------------------------------------

/// Label limited to `max_width`: longer text ends in an ellipsis and shows in
/// full on hover.
///
/// `theme::truncated_label(ui, track_name, 140.0)`
pub fn truncated_label(ui: &mut Ui, text: impl Into<WidgetText>, max_width: f32) -> Response {
    ui.scope(|ui| {
        ui.set_max_width(max_width);
        ui.add(egui::Label::new(text).truncate())
    })
    .inner
}

/// Single-line galley limited to `max_width`, ending in an ellipsis when cut
/// (`galley.elided` tells). For painters.
///
/// `let g = theme::elide(ui, &clip.name, theme::body_font(11.0), ink, rect.width() - 8.0);`
pub fn elide(ui: &Ui, text: &str, font: FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping::truncate_at_width(max_width.max(0.0));
    ui.painter().layout_job(job)
}

/// Paints single-line text anchored at `pos`, cut with an ellipsis at
/// `max_width`. Returns the painted rect and whether the text was cut (show
/// the full text on hover when it was).
///
/// `theme::paint_elided(ui, rect.left_center(), Align2::LEFT_CENTER, &name, font, ink, rect.width())`
pub fn paint_elided(
    ui: &Ui,
    pos: Pos2,
    anchor: Align2,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> (Rect, bool) {
    let galley = elide(ui, text, font, color, max_width);
    let rect = anchor.anchor_size(pos, galley.size());
    let elided = galley.elided;
    ui.painter().galley(rect.min, galley, color);
    (rect, elided)
}

/// Small caption line in `text_faint`.
pub fn hint(ui: &mut Ui, pal: Palette, text: &str) {
    ui.label(RichText::new(text).small().color(pal.text_faint));
}

// ----- structure -------------------------------------------------------------

/// Card-like frame used for panels inside the main layout.
pub fn card(pal: Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(pal.raised)
        .stroke(Stroke::new(1.0_f32, pal.border_soft))
        .corner_radius(RADIUS_L as u8)
        .inner_margin(Margin::same(10))
}

/// Section header: small caps kicker over a title.
///
/// `theme::section_header(ui, pal, "ARRANGEMENT", "Song timeline")`
pub fn section_header(ui: &mut Ui, pal: Palette, kicker: &str, title: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.label(kicker_text(pal, kicker));
        ui.label(strong_text(title).size(TEXT_TITLE).color(pal.text));
    });
}

/// Panel header row ([`CONTROL_L`] tall): a title on the left that truncates
/// before it can touch the trailing controls, which `trailing` adds from the
/// right edge inward (right-to-left layout).
///
/// `theme::panel_header(ui, pal, "Mixer", |ui| { theme::ghost_icon_button(ui, pal, Icon::Close, CONTROL_S, "Close"); })`
pub fn panel_header<R>(
    ui: &mut Ui,
    pal: Palette,
    title: &str,
    trailing: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), CONTROL_L),
        Layout::right_to_left(Align::Center),
        |ui| {
            let out = trailing(ui);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add(
                    egui::Label::new(strong_text(title).size(TEXT_TITLE).color(pal.text))
                        .truncate(),
                );
            });
            out
        },
    )
    .inner
}

/// Empty-state block: icon, title, one-line hint and an optional primary
/// action. Returns `true` when the action was clicked.
///
/// `if theme::empty_state(ui, pal, Icon::Note, "No clips yet", "Double-click a lane to add one.", Some("Add clip")) { .. }`
pub fn empty_state(
    ui: &mut Ui,
    pal: Palette,
    icon: Icon,
    title: &str,
    hint: &str,
    action: Option<&str>,
) -> bool {
    let mut clicked = false;
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = GAP_XS;
        let (badge, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
        ui.painter().circle(
            badge.center(),
            22.0,
            pal.control,
            Stroke::new(1.0_f32, pal.control_border),
        );
        paint_icon(
            ui.painter(),
            Rect::from_center_size(badge.center(), Vec2::splat(20.0)),
            icon,
            pal.text_dim,
        );
        ui.add_space(GAP_XS);
        ui.label(strong_text(title).size(TEXT_TITLE).color(pal.text));
        if !hint.is_empty() {
            ui.label(RichText::new(hint).size(12.0).color(pal.text_faint));
        }
        if let Some(action) = action {
            ui.add_space(GAP_S);
            clicked = primary_button(ui, pal, action).clicked();
        }
    });
    clicked
}

/// Thin vertical divider of the given height, for hand-built rows.
pub fn divider_v(ui: &mut Ui, pal: Palette, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, height), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, pal.border);
}

/// Small rounded color dot.
pub fn color_dot(ui: &mut Ui, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter().circle_filled(rect.center(), size * 0.5, color);
}

// ----- knob, meter, scrollbar ------------------------------------------------

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
    p.rect_filled(thumb, 3.0, scrollbar_thumb(pal, hot));
    resp.dragged()
}

/// Scrollbar thumb color: at rest it still stands out from the track
/// (≥ 3:1, a visible UI component in every theme); hovered it brightens.
fn scrollbar_thumb(pal: Palette, hot: bool) -> Color32 {
    if hot {
        pal.text_faint
    } else {
        mix(pal.control_border, pal.text_faint, 0.6)
    }
}

// ----- toolbar ---------------------------------------------------------------

/// Width reserved by a [`Toolbar::separator`]: a 1 px rule with even margins.
const TOOLBAR_SEP: f32 = 16.0;

/// Running width of a row of toolbar items, including the gaps and
/// separators that only materialize between two items.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Tally {
    width: f32,
    any: bool,
    sep: bool,
    gap: f32,
}

impl Tally {
    /// Space inserted before the next item.
    fn lead(&self) -> f32 {
        if !self.any {
            0.0
        } else if self.sep {
            TOOLBAR_SEP + self.gap
        } else {
            TOOLBAR_GAP + self.gap
        }
    }

    fn add(&mut self, width: f32) {
        if width <= 0.0 {
            return;
        }
        self.width += self.lead() + width;
        self.any = true;
        self.sep = false;
        self.gap = 0.0;
    }

    fn separator(&mut self) {
        self.sep = self.any;
    }

    fn add_gap(&mut self, gap: f32) {
        if self.any {
            self.gap += gap;
        }
    }
}

/// Content widths measured on the previous frame, used to pick the layout
/// before the first item of this frame is placed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ToolbarMemo {
    full: f32,
    compact: f32,
}

/// What [`toolbar`] laid out this frame.
#[derive(Clone, Debug)]
pub struct ToolbarOutput {
    /// The whole row.
    pub rect: Rect,
    /// Rect of every item painted in the row, in order.
    pub placed: Vec<Rect>,
    /// Number of items moved into the overflow menu.
    pub overflowed: usize,
    /// Whether labels were collapsed to icons to save room.
    pub compact: bool,
    /// The `⋯` overflow button, when shown.
    pub overflow_button: Option<Rect>,
}

enum Slot {
    /// Paint in the row at this rect.
    Row(Rect),
    /// Paint as a row of the overflow menu.
    Menu,
    /// Not shown in this pass.
    Skip,
}

/// Item builder passed to the [`toolbar`] closure.
///
/// Items are placed left to right in the order they are added:
/// - `tb.icon_button(Icon::Fit, "Fit", "Fit the whole song")` — icon only
/// - `tb.icon_toggle(Icon::Loop, "Loop", on, "Loop (L)")` — icon-only toggle
/// - `tb.button(Some(Icon::Marker), "Marker", "Add a marker")` — icon + label
/// - `tb.toggle(Some(Icon::Split), "Split", on, "Split tool")` — icon + label toggle
/// - `tb.label("Snap")` — dim caption
/// - `tb.labeled("Length", 96.0, |ui| ..)` / `tb.item(96.0, |ui| ..)` — any
///   widget in a fixed-width, clipped slot
/// - `tb.separator()`, `tb.gap(8.0)` — group dividers
/// - `tb.spacer()` — pushes what follows to the right edge
/// - `tb.hint("…")` — optional caption that only uses leftover room
/// - `tb.enabled(false)` — disables the next item
///
/// Every `label` doubles as the row text when the item sits in the overflow
/// menu, and as the accessibility name.
pub struct Toolbar<'a> {
    ui: &'a mut Ui,
    pal: Palette,
    /// Overflow-menu pass (vertical rows) instead of the row pass.
    menu: bool,
    row: Rect,
    /// Right edge items may reach.
    limit: f32,
    compact: bool,
    /// Leftover width for hints and spacers.
    flex: f32,
    index: usize,
    /// Row pass: first item that did not fit. Menu pass: first item to show.
    first_overflow: Option<usize>,
    cursor: Tally,
    full: Tally,
    compact_total: Tally,
    next_enabled: bool,
    placed: Vec<Rect>,
    menu_any: bool,
    menu_sep: bool,
}

impl Toolbar<'_> {
    /// Whether labels are collapsed to icons this frame.
    pub fn is_compact(&self) -> bool {
        self.compact
    }

    /// Whether the closure is currently filling the overflow menu.
    pub fn in_menu(&self) -> bool {
        self.menu
    }

    /// Enables or disables the next item only.
    pub fn enabled(&mut self, enabled: bool) -> &mut Self {
        self.next_enabled = enabled;
        self
    }

    fn take_enabled(&mut self) -> bool {
        std::mem::replace(&mut self.next_enabled, true) && self.ui.is_enabled()
    }

    /// Paints the rule of a pending separator that ends at `start`.
    fn paint_separator(&self, start: f32) {
        if !(self.cursor.sep && self.cursor.any) {
            return;
        }
        let x = (start + TOOLBAR_SEP * 0.5).round() - 0.5;
        let half = (self.row.height() * 0.25).round();
        self.ui.painter().line_segment(
            [
                pos2(x, self.row.center().y - half),
                pos2(x, self.row.center().y + half),
            ],
            Stroke::new(1.0_f32, self.pal.border),
        );
    }

    /// Decides where the next item goes. `full`/`compact` are its widths with
    /// and without labels; a zero width hides it in that layout.
    fn slot(&mut self, full: f32, compact: f32, height: f32) -> Slot {
        let index = self.index;
        self.index += 1;
        if self.menu {
            if self.first_overflow.is_some_and(|first| index >= first) {
                if self.menu_sep && self.menu_any {
                    self.ui.separator();
                }
                self.menu_sep = false;
                self.menu_any = true;
                return Slot::Menu;
            }
            return Slot::Skip;
        }
        self.full.add(full);
        self.compact_total.add(compact);
        if self.first_overflow.is_some() {
            return Slot::Skip;
        }
        let width = if self.compact { compact } else { full };
        if width <= 0.0 {
            return Slot::Skip;
        }
        let start = self.row.left() + self.cursor.width;
        let left = start + self.cursor.lead();
        if left + width > self.limit + 0.5 {
            self.first_overflow = Some(index);
            return Slot::Skip;
        }
        self.paint_separator(start);
        self.cursor.add(width);
        let rect = Rect::from_min_size(
            pos2(left, self.row.center().y - height * 0.5),
            vec2(width, height),
        );
        self.placed.push(rect);
        Slot::Row(rect)
    }

    /// A response that can never be clicked, for items not shown in this pass.
    fn skipped(&mut self, index: usize) -> Response {
        self.ui.interact(
            Rect::from_min_size(self.row.min, Vec2::ZERO),
            self.ui.id().with(("cadence.toolbar.skipped", index)),
            Sense::hover(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn row_button(
        &mut self,
        index: usize,
        rect: Rect,
        icon: Option<Icon>,
        label: Option<Arc<Galley>>,
        on: Option<bool>,
        enabled: bool,
        name: &str,
        tooltip: &str,
    ) -> Response {
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        // Explicit ids keep hover/press state stable when neighbours move to
        // the overflow menu.
        let id = self.ui.id().with(("cadence.toolbar.item", index));
        let resp = self.ui.interact(rect, id, sense);
        resp.widget_info(|| match on {
            Some(on) => WidgetInfo::selected(WidgetType::Button, enabled, on, name),
            None => WidgetInfo::labeled(WidgetType::Button, enabled, name),
        });
        let style = match (on, label.is_some()) {
            (Some(true), true) => OnStyle::Solid(self.pal.accent),
            (Some(true), false) => OnStyle::Tint(self.pal.accent),
            _ => OnStyle::Off,
        };
        let painter = unfaded_painter(self.ui);
        paint_button(
            self.ui, &painter, self.pal, rect, &resp, enabled, icon, label, style, false,
        );
        with_tooltip(resp, tooltip)
    }

    fn menu_button(
        &mut self,
        icon: Option<Icon>,
        label: &str,
        on: Option<bool>,
        enabled: bool,
        tooltip: &str,
    ) -> Response {
        let pal = self.pal;
        let ui = &mut *self.ui;
        let galley = label_galley(ui, label, body_font(TEXT_BODY));
        let natural = GAP_S + ICON_M + GAP_S + galley.size().x + GAP_L + ICON_S + GAP_S;
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, resp) =
            ui.allocate_exact_size(vec2(natural.max(ui.available_width()), CONTROL_M), sense);
        resp.widget_info(|| match on {
            Some(on) => WidgetInfo::selected(WidgetType::Button, enabled, on, label),
            None => WidgetInfo::labeled(WidgetType::Button, enabled, label),
        });
        if ui.is_rect_visible(rect) {
            let press = Press::of(&resp, enabled);
            let painter = ui.painter();
            if press.hovered || press.down {
                painter.rect_filled(rect, RADIUS_S, pal.control_hover);
            }
            let ink = if enabled { pal.text } else { pal.text_disabled };
            let active = on == Some(true) && enabled;
            if let Some(icon) = icon {
                paint_icon(
                    painter,
                    Rect::from_min_size(
                        pos2(rect.left() + GAP_S, rect.center().y - ICON_M * 0.5),
                        Vec2::splat(ICON_M),
                    ),
                    icon,
                    if active { pal.accent } else { ink },
                );
            }
            painter.galley(
                pos2(
                    rect.left() + GAP_S + ICON_M + GAP_S,
                    rect.center().y - galley.size().y * 0.5,
                ),
                galley,
                ink,
            );
            if on == Some(true) {
                paint_icon(
                    painter,
                    Rect::from_min_size(
                        pos2(
                            rect.right() - GAP_S - ICON_S,
                            rect.center().y - ICON_S * 0.5,
                        ),
                        Vec2::splat(ICON_S),
                    ),
                    Icon::Check,
                    if enabled { pal.accent } else { ink },
                );
            }
        }
        if resp.clicked() {
            ui.close();
        }
        with_tooltip(resp, tooltip)
    }

    fn any_button(
        &mut self,
        icon: Option<Icon>,
        label: &str,
        show_label: bool,
        on: Option<bool>,
        tooltip: &str,
    ) -> Response {
        let enabled = self.take_enabled();
        let index = self.index;
        let galley = show_label.then(|| label_galley(self.ui, label, button_font(CONTROL_M)));
        let full = button_width(CONTROL_M, icon.is_some(), galley.as_deref());
        let compact = if icon.is_some() { CONTROL_M } else { full };
        match self.slot(full, compact, CONTROL_M) {
            Slot::Row(rect) => {
                let galley = galley.filter(|_| !(self.compact && icon.is_some()));
                let tip = if tooltip.is_empty() { label } else { tooltip };
                self.row_button(index, rect, icon, galley, on, enabled, label, tip)
            }
            Slot::Menu => self.menu_button(icon, label, on, enabled, tooltip),
            Slot::Skip => self.skipped(index),
        }
    }

    /// Icon-only button. `label` names it in the overflow menu.
    pub fn icon_button(&mut self, icon: Icon, label: &str, tooltip: &str) -> Response {
        self.any_button(Some(icon), label, false, None, tooltip)
    }

    /// Icon-only toggle.
    pub fn icon_toggle(&mut self, icon: Icon, label: &str, on: bool, tooltip: &str) -> Response {
        self.any_button(Some(icon), label, false, Some(on), tooltip)
    }

    /// Button with a label; with an icon, the label collapses when the row
    /// runs out of room.
    pub fn button(&mut self, icon: Option<Icon>, label: &str, tooltip: &str) -> Response {
        self.any_button(icon, label, true, None, tooltip)
    }

    /// Toggle with a label; with an icon, the label collapses when the row
    /// runs out of room.
    pub fn toggle(&mut self, icon: Option<Icon>, label: &str, on: bool, tooltip: &str) -> Response {
        self.any_button(icon, label, true, Some(on), tooltip)
    }

    /// Dim caption. Hidden when labels are collapsed.
    pub fn label(&mut self, text: &str) {
        let galley = label_galley(self.ui, text, body_font(12.0));
        match self.slot(galley.size().x.ceil(), 0.0, CONTROL_M) {
            Slot::Row(rect) => {
                let pos = pos2(rect.left(), rect.center().y - galley.size().y * 0.5);
                self.ui.painter().galley(pos, galley, self.pal.text_dim);
            }
            Slot::Menu => {
                self.ui.label(kicker_text(self.pal, &text.to_uppercase()));
            }
            Slot::Skip => {}
        }
    }

    /// Optional caption that only ever uses room left over by every other
    /// item: hidden while labels are collapsed, never part of the overflow
    /// menu. Place it before a `spacer`.
    pub fn hint(&mut self, text: &str) {
        if self.menu || self.compact || self.first_overflow.is_some() {
            return;
        }
        let lead = self.cursor.lead();
        let room = self.flex - lead;
        if room < 60.0 {
            return;
        }
        let galley = elide(
            self.ui,
            text,
            body_font(12.0),
            self.pal.text_faint,
            room.floor(),
        );
        let width = galley.size().x.ceil().min(room);
        let start = self.row.left() + self.cursor.width;
        self.paint_separator(start);
        let left = start + lead;
        let rect = Rect::from_min_size(
            pos2(left, self.row.center().y - galley.size().y * 0.5),
            vec2(width, galley.size().y),
        );
        self.cursor.add(width);
        self.flex -= lead + width;
        self.placed.push(rect);
        let resp = self.ui.allocate_rect(rect, Sense::hover());
        let elided = galley.elided;
        self.ui
            .painter()
            .galley(rect.min, galley, self.pal.text_faint);
        if elided {
            resp.on_hover_text(text);
        }
    }

    /// Starts a new group: a thin rule before the next item.
    pub fn separator(&mut self) {
        if self.menu {
            self.menu_sep = true;
            return;
        }
        self.cursor.separator();
        self.full.separator();
        self.compact_total.separator();
    }

    /// Extra space before the next item.
    pub fn gap(&mut self, width: f32) {
        if self.menu {
            return;
        }
        self.cursor.add_gap(width);
        self.full.add_gap(width);
        self.compact_total.add_gap(width);
    }

    /// Pushes the following items to the right edge.
    pub fn spacer(&mut self) {
        if !self.menu && self.first_overflow.is_none() {
            self.cursor.width += self.flex;
            self.flex = 0.0;
        }
    }

    fn row_item<R>(&mut self, rect: Rect, enabled: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
        let builder = UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center));
        self.ui
            .scope_builder(builder, |ui| {
                ui.set_clip_rect(rect.intersect(ui.clip_rect()));
                if !enabled {
                    ui.disable();
                }
                add(ui)
            })
            .inner
    }

    /// Any widget in a slot exactly `width` wide; anything it paints outside
    /// the slot is clipped. Returns `None` when the item is not shown in this
    /// pass.
    pub fn item<R>(&mut self, width: f32, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
        let enabled = self.take_enabled();
        match self.slot(width, width, self.row.height()) {
            Slot::Row(rect) => Some(self.row_item(rect, enabled, add)),
            Slot::Menu => Some(
                self.ui
                    .horizontal(|ui| {
                        if !enabled {
                            ui.disable();
                        }
                        add(ui)
                    })
                    .inner,
            ),
            Slot::Skip => None,
        }
    }

    /// [`Toolbar::item`] with a caption that stays attached to it (and
    /// collapses when the row runs out of room).
    pub fn labeled<R>(
        &mut self,
        label: &str,
        width: f32,
        add: impl FnOnce(&mut Ui) -> R,
    ) -> Option<R> {
        let enabled = self.take_enabled();
        let galley = label_galley(self.ui, label, body_font(12.0));
        let label_w = galley.size().x.ceil() + ICON_TEXT_GAP;
        match self.slot(label_w + width, width, self.row.height()) {
            Slot::Row(rect) => {
                let mut body = rect;
                if !self.compact {
                    let pos = pos2(rect.left(), rect.center().y - galley.size().y * 0.5);
                    self.ui.painter().galley(pos, galley, self.pal.text_dim);
                    body.min.x += label_w;
                }
                Some(self.row_item(body, enabled, add))
            }
            Slot::Menu => {
                let ink = self.pal.text_dim;
                Some(
                    self.ui
                        .horizontal(|ui| {
                            ui.label(RichText::new(label).color(ink));
                            if !enabled {
                                ui.disable();
                            }
                            add(ui)
                        })
                        .inner,
                )
            }
            Slot::Skip => None,
        }
    }
}

/// Overflow-safe toolbar row, [`TOOLBAR_H`] tall and as wide as the space
/// available. `add_items` describes the items through a [`Toolbar`].
///
/// Items can never be painted on top of each other: each one gets its own
/// slot, measured before anything is drawn. When the row is too narrow,
/// labels collapse to icons first; items that still do not fit move, from
/// the right, into a `⋯` menu. `add_items` therefore runs once for the row
/// and a second time while that menu is open — keep it free of side effects
/// other than reacting to the returned responses.
///
/// ```ignore
/// theme::toolbar(ui, pal, "arranger", |tb| {
///     if tb.icon_button(Icon::ZoomOut, "Zoom out", &zoom_out_tip).clicked() { .. }
///     tb.separator();
///     if tb.toggle(Some(Icon::Loop), "Loop", looping, "Loop the selection (L)").clicked() { .. }
///     tb.spacer();
///     tb.labeled("Snap", 96.0, |ui| snap_combo(ui));
/// });
/// ```
pub fn toolbar(
    ui: &mut Ui,
    pal: Palette,
    id_salt: impl Hash,
    mut add_items: impl FnMut(&mut Toolbar<'_>),
) -> ToolbarOutput {
    let id = ui.id().with("cadence.toolbar").with(id_salt);
    let memo: Option<ToolbarMemo> = ui.data(|d| d.get_temp(id));
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), TOOLBAR_H), Sense::hover());
    let width = row.width();
    // Without a measurement (first frame) assume the worst: keep room for `⋯`.
    let (compact, reserve) = match memo {
        Some(m) if m.full <= width + 0.5 => (false, false),
        Some(m) => (true, m.compact > width + 0.5),
        None => (false, true),
    };
    let limit = row.right()
        - if reserve {
            CONTROL_M + TOOLBAR_GAP
        } else {
            0.0
        };
    let content = memo.map_or(width, |m| if compact { m.compact } else { m.full });
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(row)
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.set_clip_rect(row.intersect(ui.clip_rect()));
    let mut tb = Toolbar {
        ui: &mut child,
        pal,
        menu: false,
        row,
        limit,
        compact,
        flex: (limit - row.left() - content).max(0.0),
        index: 0,
        first_overflow: None,
        cursor: Tally::default(),
        full: Tally::default(),
        compact_total: Tally::default(),
        next_enabled: true,
        placed: Vec::new(),
        menu_any: false,
        menu_sep: false,
    };
    add_items(&mut tb);
    let Toolbar {
        index,
        first_overflow,
        cursor,
        full,
        compact_total,
        placed,
        ..
    } = tb;

    let measured = ToolbarMemo {
        full: full.width,
        compact: compact_total.width,
    };
    if memo != Some(measured) {
        // The layout was chosen from stale widths; settle on the next frame.
        ui.data_mut(|d| d.insert_temp(id, measured));
        ui.ctx().request_repaint();
    }

    let mut overflow_button = None;
    if let Some(first) = first_overflow {
        let rect = Rect::from_center_size(
            pos2(row.right() - CONTROL_M * 0.5, row.center().y),
            Vec2::splat(CONTROL_M),
        );
        let used = row.left() + cursor.width + if cursor.any { TOOLBAR_GAP } else { 0.0 };
        if rect.left() + 0.5 >= used {
            let resp = child.allocate_rect(rect, Sense::click());
            resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "More"));
            paint_button(
                &child,
                child.painter(),
                pal,
                rect,
                &resp,
                true,
                Some(Icon::Menu),
                None,
                OnStyle::Off,
                false,
            );
            let resp = resp.on_hover_text("More");
            egui::Popup::menu(&resp)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    ui.set_min_width(200.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let row = ui.max_rect();
                    let mut tb = Toolbar {
                        ui,
                        pal,
                        menu: true,
                        row,
                        limit: row.right(),
                        compact: false,
                        flex: 0.0,
                        index: 0,
                        first_overflow: Some(first),
                        cursor: Tally::default(),
                        full: Tally::default(),
                        compact_total: Tally::default(),
                        next_enabled: true,
                        placed: Vec::new(),
                        menu_any: false,
                        menu_sep: false,
                    };
                    add_items(&mut tb);
                });
            overflow_button = Some(rect);
        } else {
            // Stale widths left no room for `⋯` this frame; the items it
            // would hold return next frame, once the row is re-measured.
            ui.ctx().request_repaint();
        }
    }
    ToolbarOutput {
        rect: row,
        placed,
        overflowed: first_overflow.map_or(0, |first| index - first),
        compact,
        overflow_button,
    }
}

// ----- developer gallery -----------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum GalleryTab {
    Icons,
    Controls,
    Layout,
    Colors,
}

#[derive(Clone)]
struct GalleryState {
    tab: GalleryTab,
    toggles: [bool; 4],
    choice: u8,
    amount: f32,
    steps: i32,
    knob: f32,
    combo: usize,
    toolbar_width: f32,
    text: String,
}

impl Default for GalleryState {
    fn default() -> Self {
        Self {
            tab: GalleryTab::Icons,
            toggles: [false, true, false, true],
            choice: 0,
            amount: 0.4,
            steps: 8,
            knob: 0.6,
            combo: 0,
            toolbar_width: 560.0,
            text: "Afterglow".to_owned(),
        }
    }
}

impl GalleryTab {
    /// `CADENCE_DESIGN_GALLERY` value → first tab shown (`None` = gallery off).
    fn from_env(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "0" => None,
            "controls" => Some(Self::Controls),
            "layout" => Some(Self::Layout),
            "colors" => Some(Self::Colors),
            _ => Some(Self::Icons),
        }
    }
}

/// Shows the design gallery window when `CADENCE_DESIGN_GALLERY` is set
/// (developer aid; costs one cached check otherwise). The value picks the
/// first tab: `icons` (or `1`), `controls`, `layout` or `colors`.
pub fn gallery(ctx: &egui::Context, pal: Palette) {
    static FIRST_TAB: LazyLock<Option<GalleryTab>> = LazyLock::new(|| {
        std::env::var("CADENCE_DESIGN_GALLERY")
            .ok()
            .and_then(|v| GalleryTab::from_env(&v))
    });
    if let Some(tab) = *FIRST_TAB {
        gallery_window(ctx, pal, tab);
    }
}

fn gallery_window(ctx: &egui::Context, pal: Palette, first_tab: GalleryTab) {
    let id = egui::Id::new("cadence.theme.gallery");
    let mut state: GalleryState = ctx
        .data(|d| d.get_temp(id))
        .unwrap_or_else(|| GalleryState {
            tab: first_tab,
            ..Default::default()
        });
    egui::Window::new("Design system")
        .default_pos([24.0, 40.0])
        .default_size([900.0, 700.0])
        .show(ctx, |ui| {
            segmented(
                ui,
                pal,
                &mut state.tab,
                &[
                    Segment::new(GalleryTab::Icons, "Icons"),
                    Segment::new(GalleryTab::Controls, "Controls"),
                    Segment::new(GalleryTab::Layout, "Layout"),
                    Segment::new(GalleryTab::Colors, "Colors & type"),
                ],
                CONTROL_M,
                None,
            );
            ui.add_space(GAP_S);
            egui::ScrollArea::vertical().show(ui, |ui| match state.tab {
                GalleryTab::Icons => gallery_icons(ui, pal),
                GalleryTab::Controls => gallery_controls(ui, pal, &mut state),
                GalleryTab::Layout => gallery_layout(ui, pal, &mut state),
                GalleryTab::Colors => gallery_colors(ui, pal),
            });
        });
    ctx.data_mut(|d| d.insert_temp(id, state));
}

fn gallery_icons(ui: &mut Ui, pal: Palette) {
    const SIZES: [f32; 4] = [12.0, 16.0, 20.0, 28.0];
    egui::Grid::new("cadence.theme.gallery.icons")
        .num_columns(4)
        .spacing(vec2(GAP_L, GAP_XS))
        .show(ui, |ui| {
            for (i, icon) in Icon::ALL.into_iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP_S;
                    for size in SIZES {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                        paint_icon(
                            ui.painter(),
                            Rect::from_center_size(rect.center(), Vec2::splat(size)),
                            icon,
                            pal.text,
                        );
                    }
                    ui.label(RichText::new(icon.name()).size(12.0).color(pal.text_dim));
                });
                if i % 4 == 3 {
                    ui.end_row();
                }
            }
        });
}

fn gallery_row(ui: &mut Ui, pal: Palette, title: &str, add: impl FnOnce(&mut Ui)) {
    ui.label(kicker_text(pal, title));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP_S;
        add(ui);
    });
    ui.add_space(GAP_S);
}

fn gallery_controls(ui: &mut Ui, pal: Palette, state: &mut GalleryState) {
    gallery_row(
        ui,
        pal,
        "ICON BUTTONS — S / M / L · ACTIVE · DISABLED",
        |ui| {
            for size in [CONTROL_S, CONTROL_M, CONTROL_L] {
                icon_button(ui, pal, Icon::Undo, size, false, pal.accent, "Undo");
            }
            icon_button(ui, pal, Icon::Loop, CONTROL_M, true, pal.accent, "Loop");
            icon_button(
                ui,
                pal,
                Icon::Metronome,
                CONTROL_M,
                true,
                pal.link,
                "Metronome",
            );
            ui.add_enabled_ui(false, |ui| {
                icon_button(ui, pal, Icon::Undo, CONTROL_M, false, pal.accent, "Undo");
                icon_button(ui, pal, Icon::Redo, CONTROL_M, false, pal.accent, "Redo");
                icon_button(ui, pal, Icon::Loop, CONTROL_M, true, pal.accent, "Loop");
            });
            ghost_icon_button(ui, pal, Icon::Close, CONTROL_M, "Close");
            ghost_icon_button(ui, pal, Icon::Menu, CONTROL_M, "More");
            ui.add_enabled_ui(false, |ui| {
                ghost_icon_button(ui, pal, Icon::Trash, CONTROL_M, "Delete");
            });
            play_button(ui, pal, false, 40.0);
            play_button(ui, pal, true, 40.0);
        },
    );
    gallery_row(ui, pal, "LABEL BUTTONS · TOGGLES", |ui| {
        icon_label_button(ui, pal, Icon::Marker, "Marker", CONTROL_M, "Add a marker");
        text_button(ui, pal, "Fill", CONTROL_M, "Fill the pattern");
        for (i, (label, icon)) in [("Chords", None), ("Follow", Some(Icon::Follow))]
            .into_iter()
            .enumerate()
        {
            let on = state.toggles[i];
            let resp = match icon {
                Some(icon) => icon_toggle_button(ui, pal, icon, label, on, CONTROL_M, label),
                None => toggle_button(ui, pal, label, on, CONTROL_M, label),
            };
            if resp.clicked() {
                state.toggles[i] = !on;
            }
        }
        ui.add_enabled_ui(false, |ui| {
            icon_label_button(ui, pal, Icon::Split, "Split", CONTROL_M, "Split");
            toggle_button(ui, pal, "Loop", false, CONTROL_M, "Loop");
            toggle_button(ui, pal, "Snap", true, CONTROL_M, "Snap");
        });
        primary_button(ui, pal, "Save");
        secondary_button(ui, "Cancel");
    });
    gallery_row(ui, pal, "CHIPS", |ui| {
        let small = vec2(CONTROL_S, CONTROL_S);
        for (i, (label, color)) in [("M", pal.danger), ("S", pal.warn)].into_iter().enumerate() {
            let on = state.toggles[2 + i];
            if chip(ui, pal, label, on, color, small, label).clicked() {
                state.toggles[2 + i] = !on;
            }
        }
        chip(
            ui,
            pal,
            "LOOP",
            false,
            pal.accent,
            vec2(52.0, CONTROL_S),
            "Loop",
        );
        chip(
            ui,
            pal,
            "TAP",
            true,
            pal.accent,
            vec2(44.0, CONTROL_S),
            "Tap tempo",
        );
        ui.add_enabled_ui(false, |ui| {
            chip(ui, pal, "M", false, pal.danger, small, "Mute");
            chip(ui, pal, "S", true, pal.warn, small, "Solo");
        });
    });
    gallery_row(ui, pal, "SEGMENTED", |ui| {
        let options = [
            Segment::new(0u8, "Pattern Editor").tip("Edit the selected pattern"),
            Segment::new(1u8, "Mixer"),
            Segment::new(2u8, "Keyboard"),
        ];
        segmented(ui, pal, &mut state.choice, &options, CONTROL_M, None);
        let icons = [
            Segment::new(0u8, "").icon(Icon::Pointer).tip("Select"),
            Segment::new(1u8, "").icon(Icon::Pencil).tip("Draw"),
            Segment::new(2u8, "").icon(Icon::Eraser).tip("Erase"),
        ];
        segmented(ui, pal, &mut state.choice, &icons, CONTROL_S, None);
        ui.add_enabled_ui(false, |ui| {
            let off = [Segment::new(0u8, "Bar"), Segment::new(1u8, "Beat")];
            segmented(
                ui,
                pal,
                &mut state.choice.min(1),
                &off,
                CONTROL_M,
                Some(120.0),
            );
        });
    });
    gallery_row(ui, pal, "SLIDERS · KNOB · METER", |ui| {
        slider(ui, pal, &mut state.amount, 0.0..=1.0, 140.0);
        slider_with(ui, pal, &mut state.steps, 1..=16, 120.0, pal.link, Some(8));
        ui.label(format!(
            "{:.0}% · {} steps",
            state.amount * 100.0,
            state.steps
        ));
        ui.add_enabled_ui(false, |ui| {
            slider(ui, pal, &mut state.amount.clone(), 0.0..=1.0, 100.0);
        });
        ui.add(egui::Slider::new(&mut state.amount, 0.0..=1.0).show_value(false));
        let text = format!("{:.0}%", state.knob * 100.0);
        knob(
            ui,
            pal,
            &mut state.knob,
            0.6,
            false,
            "Reverb",
            &text,
            pal.accent,
            36.0,
        );
        let (rect, _) = ui.allocate_exact_size(vec2(8.0, 60.0), Sense::hover());
        meter(ui.painter(), pal, rect, state.amount, true);
    });
    gallery_row(ui, pal, "EGUI CONTROLS AT THE SAME HEIGHT", |ui| {
        const SNAPS: [&str; 3] = ["Bar", "Beat", "1/16"];
        combo("cadence.theme.gallery.combo", SNAPS[state.combo], 96.0).show_ui(ui, |ui| {
            for (i, name) in SNAPS.into_iter().enumerate() {
                ui.selectable_value(&mut state.combo, i, name);
            }
        });
        let _ = ui.button("Button");
        ui.add(egui::DragValue::new(&mut state.steps).suffix(" bars"));
        ui.add(egui::TextEdit::singleline(&mut state.text).desired_width(120.0));
        ui.checkbox(&mut state.toggles[0], "Checkbox");
        ui.add_enabled_ui(false, |ui| {
            let _ = ui.button("Disabled");
            ui.checkbox(&mut true, "Disabled");
        });
    });
}

fn gallery_layout(ui: &mut Ui, pal: Palette, state: &mut GalleryState) {
    ui.label(kicker_text(pal, "TOOLBAR — DRAG THE SLIDER TO SQUEEZE IT"));
    slider(ui, pal, &mut state.toolbar_width, 120.0..=820.0, 300.0);
    let toggles = &mut state.toggles;
    card(pal).show(ui, |ui| {
        ui.set_width(state.toolbar_width);
        toolbar(ui, pal, "gallery", |tb| {
            tb.icon_button(Icon::ZoomOut, "Zoom out", "Zoom out (−)");
            tb.icon_button(Icon::ZoomIn, "Zoom in", "Zoom in (+)");
            tb.icon_button(Icon::Fit, "Fit", "Fit the whole song");
            tb.separator();
            tb.labeled("Snap", 80.0, |ui| {
                combo("cadence.theme.gallery.snap", "Bar", 72.0).show_ui(ui, |ui| {
                    let _ = ui.selectable_label(true, "Bar");
                });
            });
            tb.separator();
            if tb
                .toggle(Some(Icon::Loop), "Loop", toggles[0], "Loop the selection")
                .clicked()
            {
                toggles[0] = !toggles[0];
            }
            if tb
                .toggle(
                    Some(Icon::Follow),
                    "Follow",
                    toggles[1],
                    "Follow the playhead",
                )
                .clicked()
            {
                toggles[1] = !toggles[1];
            }
            tb.button(Some(Icon::Marker), "Marker", "Add a marker at the playhead");
            tb.enabled(false)
                .button(Some(Icon::Split), "Split", "Split the clip at the playhead");
            tb.hint("Double-click a lane to add a clip");
            tb.spacer();
            tb.icon_button(Icon::Settings, "Settings", "Timeline settings");
        });
    });
    ui.add_space(GAP_M);
    ui.label(kicker_text(
        pal,
        "PANEL HEADER · SECTION HEADER · TRUNCATED LABEL",
    ));
    card(pal).show(ui, |ui| {
        ui.set_width(360.0);
        panel_header(
            ui,
            pal,
            "Mixer with a title that is far too long to fit",
            |ui| {
                ghost_icon_button(ui, pal, Icon::Close, CONTROL_S, "Close");
                ghost_icon_button(ui, pal, Icon::Settings, CONTROL_S, "Settings");
            },
        );
        ui.separator();
        section_header(ui, pal, "ARRANGEMENT", "Song timeline");
        ui.add_space(GAP_S);
        truncated_label(
            ui,
            "A very long instrument name that has to be cut with an ellipsis",
            220.0,
        );
        hint(ui, pal, "Hover the label above to read all of it.");
    });
    ui.add_space(GAP_M);
    ui.label(kicker_text(pal, "EMPTY STATE"));
    card(pal).show(ui, |ui| {
        ui.set_width(360.0);
        empty_state(
            ui,
            pal,
            Icon::Note,
            "No clips yet",
            "Double-click a lane to add one.",
            Some("Add clip"),
        );
    });
}

fn gallery_swatch(ui: &mut Ui, pal: Palette, name: &str, color: Color32) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::hover());
        ui.painter().rect(
            rect,
            RADIUS_S,
            color,
            Stroke::new(1.0_f32, pal.border),
            StrokeKind::Inside,
        );
        ui.label(RichText::new(name).size(12.0).color(pal.text_dim));
    });
}

fn gallery_colors(ui: &mut Ui, pal: Palette) {
    let m = pal.mixer;
    let groups: [(&str, &[(&str, Color32)]); 5] = [
        (
            "SURFACES",
            &[
                ("bg", pal.bg),
                ("bg_deep", pal.bg_deep),
                ("panel", pal.panel),
                ("raised", pal.raised),
                ("canvas", pal.canvas),
                ("canvas_alt", pal.canvas_alt),
                ("inset", pal.inset),
                ("grid_faint", pal.grid_faint),
                ("grid_beat", pal.grid_beat),
                ("grid_bar", pal.grid_bar),
            ],
        ),
        (
            "CONTROLS",
            &[
                ("control", pal.control),
                ("control_hover", pal.control_hover),
                ("control_active", pal.control_active),
                ("control_border", pal.control_border),
                ("border", pal.border),
                ("border_soft", pal.border_soft),
            ],
        ),
        (
            "TEXT & STATE",
            &[
                ("text", pal.text),
                ("text_dim", pal.text_dim),
                ("text_faint", pal.text_faint),
                ("text_disabled", pal.text_disabled),
                ("accent", pal.accent),
                ("link", pal.link),
                ("danger", pal.danger),
                ("warn", pal.warn),
                ("selection_bg", pal.selection_bg),
            ],
        ),
        (
            "MIXER",
            &[
                ("panel", m.panel),
                ("strip", m.strip),
                ("strip_hi", m.strip_hi),
                ("master", m.master),
                ("border", m.border),
                ("border_soft", m.border_soft),
                ("well", m.well),
                ("control", m.control),
                ("control_hover", m.control_hover),
                ("control_active", m.control_active),
                ("arc", m.arc),
                ("tick", m.tick),
                ("select", m.select),
                ("text", m.text),
                ("text_dim", m.text_dim),
                ("text_faint", m.text_faint),
            ],
        ),
        (
            "MIXER METERS & CAPS",
            &[
                ("meter_low", m.meter_low),
                ("meter_mid", m.meter_mid),
                ("meter_high", m.meter_high),
                ("meter_clip", m.meter_clip),
                ("meter_off", m.meter_off),
                ("meter_peak", m.meter_peak),
                ("cap_body", m.cap_body),
                ("cap_hi", m.cap_hi),
                ("cap_line", m.cap_line),
                ("cap_edge", m.cap_edge),
                ("pan", m.pan),
                ("mute", m.mute),
                ("solo", m.solo),
            ],
        ),
    ];
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = GAP_XL;
        for (title, swatches) in groups {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = GAP_XS;
                ui.label(kicker_text(pal, title));
                for &(name, color) in swatches {
                    gallery_swatch(ui, pal, name, color);
                }
            });
        }
    });
    ui.add_space(GAP_M);
    ui.label(kicker_text(pal, "TYPE"));
    ui.label(
        RichText::new("Heading 18 — Song timeline")
            .heading()
            .color(pal.text),
    );
    ui.label(
        strong_text("Title 14 — Pocket drums")
            .size(TEXT_TITLE)
            .color(pal.text),
    );
    ui.label(strong_text("Strong 13 — Add an instrument").color(pal.text));
    ui.label(RichText::new("Body 13 — Warm, bell-like electric keys with tremolo").color(pal.text));
    ui.label("Body dim 13 — Eight synthesized drum voices, from kick to ride");
    ui.label(RichText::new("Small 11 — Round analog bass with a filter envelope").small());
    ui.label(kicker_text(pal, "KICKER 10 — TEMPO · SWING · MASTER"));
    ui.label(RichText::new("Mono 12.5 — 0:00.0 / 0:18.5").monospace());
    ui.label(RichText::new("1.1.1").text_style(display()).color(pal.text));
    ui.label(
        RichText::new("Symbols — · … → – × ′ ▸ ✕ ± ▶ ° ▾ ♭ ♯ ● « − • » › ‹ ⏵ ⌘ ⇧")
            .color(pal.text_dim),
    );
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::{BTreeMap, HashSet};
    use std::path::{Path, PathBuf};

    use egui::{Context, Event, Modifiers, PointerButton, RawInput};

    use super::*;

    /// A bare egui context with the theme applied and the fonts live.
    struct Harness {
        ctx: Context,
        size: Vec2,
        choice: ThemeChoice,
    }

    impl Harness {
        fn new(width: f32) -> Self {
            Self::themed(width, ThemeChoice::Graphite)
        }

        fn themed(width: f32, choice: ThemeChoice) -> Self {
            let harness = Self {
                ctx: Context::default(),
                size: vec2(width, 700.0),
                choice,
            };
            // The first run requests the fonts (and is discarded once).
            harness.frame(Vec::new(), |_| {});
            harness
        }

        fn pal(&self) -> Palette {
            self.choice.palette()
        }

        /// One frame of a margin-less central panel.
        fn frame(&self, events: Vec<Event>, mut add: impl FnMut(&mut Ui)) -> egui::FullOutput {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                events,
                focused: true,
                ..Default::default()
            };
            self.ctx.run(input, |ctx| {
                apply(ctx, self.choice);
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| add(ui));
            })
        }

        fn click(&self, pos: Pos2, mut add: impl FnMut(&mut Ui)) {
            self.frame(press(pos), &mut add);
            self.frame(release(pos), &mut add);
        }
    }

    fn button_event(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn press(pos: Pos2) -> Vec<Event> {
        vec![Event::PointerMoved(pos), button_event(pos, true)]
    }

    fn release(pos: Pos2) -> Vec<Event> {
        vec![button_event(pos, false)]
    }

    // ----- typography ---------------------------------------------------------

    #[test]
    fn apply_is_safe_before_the_fonts_are_live() {
        let ctx = Context::default();
        let pal = ThemeChoice::Graphite.palette();
        let mut ready = Vec::new();
        for _ in 0..2 {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                ready.push(apply(ctx, ThemeChoice::Graphite));
                egui::CentralPanel::default().show(ctx, |ui| {
                    // Everything that asks for the strong family must work in
                    // the very first pass of a fresh context.
                    ui.heading("Heading");
                    ui.label(strong_text("Strong"));
                    ui.label(kicker_text(pal, "KICKER"));
                    let font = strong_font(ui, 12.0);
                    ui.painter()
                        .text(pos2(10.0, 10.0), Align2::LEFT_TOP, "x", font, pal.text);
                    chip(ui, pal, "M", true, pal.danger, vec2(24.0, 24.0), "Mute");
                });
            });
        }
        // First pass: fonts requested, pass discarded and re-run with them.
        assert_eq!(ready, [false, true, true]);
        let style = ctx.style();
        for text_style in [strong(), kicker(), TextStyle::Heading] {
            assert_eq!(style.text_styles[&text_style].family, strong_family());
        }
        assert_eq!(
            style.text_styles[&TextStyle::Body].family,
            FontFamily::Proportional
        );
    }

    #[test]
    fn startup_order_of_the_real_app_ends_with_live_fonts() {
        // `CadenceApp::new` styles the context before any pass has run;
        // `frame()` keeps calling `apply` until it reports the fonts live.
        let ctx = Context::default();
        assert!(!apply(&ctx, ThemeChoice::Graphite));
        let mut applied = false;
        let mut runs = 0;
        while !applied {
            runs += 1;
            assert!(runs <= 2, "fonts never became live");
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                if !applied {
                    applied = apply(ctx, ThemeChoice::Graphite);
                }
                egui::CentralPanel::default().show(ctx, |ui| ui.heading("Heading"));
            });
        }
        assert_eq!(ctx.style().text_styles[&strong()].family, strong_family());
    }

    #[test]
    fn inter_leads_and_the_default_fonts_stay_as_fallbacks() {
        let defaults = FontDefinitions::default();
        let fonts = font_definitions();
        let with_last_resort = |family: &FontFamily| {
            let mut names = defaults.families[family].clone();
            names.push("DejaVuSans".to_owned());
            names
        };
        let proportional = &fonts.families[&FontFamily::Proportional];
        assert_eq!(proportional[0], "Inter");
        assert_eq!(
            proportional[1..],
            with_last_resort(&FontFamily::Proportional)[..]
        );
        let strong = &fonts.families[&strong_family()];
        assert_eq!(strong[..2], ["Inter-SemiBold", "Inter"]);
        assert_eq!(strong[2..], proportional[1..]);
        assert_eq!(
            fonts.families[&FontFamily::Monospace],
            with_last_resort(&FontFamily::Monospace)
        );
    }

    /// Non-ASCII characters inside Rust string and char literals (comments
    /// are skipped), keyed by character.
    fn literal_chars(source: &str, origin: &str, found: &mut BTreeMap<char, String>) {
        let chars: Vec<char> = source.chars().collect();
        let mut note = |c: char| {
            if !c.is_ascii() && !c.is_control() {
                found.entry(c).or_insert_with(|| origin.to_owned());
            }
        };
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            if c == '/' && next == Some('/') {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            } else if c == '/' && next == Some('*') {
                let mut depth = 1;
                i += 2;
                while i < chars.len() && depth > 0 {
                    if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 1;
                    } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 1;
                    }
                    i += 1;
                }
            } else if c == 'r'
                && matches!(next, Some('"' | '#'))
                && !chars
                    .get(i.wrapping_sub(1))
                    .is_some_and(|p| p.is_alphanumeric() || *p == '_')
            {
                // Raw string: r"..." or r#"..."#.
                let mut j = i + 1;
                let mut hashes = 0;
                while chars.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if chars.get(j) != Some(&'"') {
                    i += 1;
                    continue;
                }
                j += 1;
                loop {
                    match chars.get(j) {
                        None => break,
                        Some('"') if (1..=hashes).all(|k| chars.get(j + k) == Some(&'#')) => {
                            j += hashes + 1;
                            break;
                        }
                        Some(&c) => note(c),
                    }
                    j += 1;
                }
                i = j;
                continue;
            } else if c == '"' {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        if chars.get(i + 1) == Some(&'u') && chars.get(i + 2) == Some(&'{') {
                            let hex: String =
                                chars[i + 3..].iter().take_while(|c| **c != '}').collect();
                            if let Some(c) =
                                u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                            {
                                note(c);
                            }
                        }
                        i += 1;
                    } else {
                        note(chars[i]);
                    }
                    i += 1;
                }
            } else if c == '\'' {
                // A char literal ('x' or '\n'); otherwise a lifetime.
                if next == Some('\\') {
                    i += 2;
                    while i < chars.len() && chars[i] != '\'' {
                        i += 1;
                    }
                } else if chars.get(i + 2) == Some(&'\'') {
                    if let Some(c) = next {
                        note(c);
                    }
                    i += 2;
                }
            }
            i += 1;
        }
    }

    fn files_under(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files_under(&path, extension, out);
            } else if path.extension().is_some_and(|e| e == extension) {
                out.push(path);
            }
        }
    }

    #[test]
    fn literal_scanner_reads_strings_and_skips_comments() {
        let source = r####"
            // comment with ¶ is ignored
            /* block ¶ */
            let a = "dash — and escaped \" quote · \u{2192}";
            let b = r#"raw “quoted” text"#;
            let c = '✕';
            fn lifetime<'a>(x: &'a str) {}
        "####;
        let mut found = BTreeMap::new();
        literal_chars(source, "test", &mut found);
        let found: String = found.keys().collect();
        let mut expected: Vec<char> = "—·→“”✕".chars().collect();
        expected.sort_unstable();
        assert_eq!(found, expected.into_iter().collect::<String>());
    }

    #[test]
    fn bundled_fonts_cover_every_shipped_glyph() {
        // Everything the brief calls out, plus the modifier symbols egui uses
        // for shortcuts on macOS.
        let mut wanted: BTreeMap<char, String> = "—·…→←↑↓–×′▸”“✕±▶°▾♭♯●«−•»›‹⏵⌘⇧⌥"
            .chars()
            .map(|c| (c, "required set".to_owned()))
            .collect();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut sources = Vec::new();
        files_under(&root.join("src"), "rs", &mut sources);
        assert!(sources.len() > 10, "source tree not found");
        for path in sources {
            // The headless UI tests are not shipped.
            if path.ends_with("gui/tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let origin = path.strip_prefix(root).unwrap().display().to_string();
            literal_chars(&text, &origin, &mut wanted);
        }
        // Instrument and pack names come from the bundled JSON packs.
        let mut packs = Vec::new();
        files_under(&root.join("packs"), "json", &mut packs);
        for path in packs {
            let origin = path.strip_prefix(root).unwrap().display().to_string();
            for c in std::fs::read_to_string(&path).unwrap().chars() {
                if !c.is_ascii() && !c.is_control() {
                    wanted.entry(c).or_insert_with(|| origin.clone());
                }
            }
        }

        let harness = Harness::new(400.0);
        let mut missing = Vec::new();
        harness.frame(Vec::new(), |ui| {
            ui.fonts_mut(|fonts| {
                for (&c, origin) in &wanted {
                    for family in [FontFamily::Proportional, strong_family()] {
                        if !fonts.has_glyph(&FontId::new(TEXT_BODY, family.clone()), c) {
                            missing.push(format!(
                                "U+{:04X} {c:?} in {family:?} (first seen in {origin})",
                                c as u32
                            ));
                        }
                    }
                }
            });
        });
        assert!(
            missing.is_empty(),
            "characters without a glyph would render as blank boxes:\n{}",
            missing.join("\n")
        );
    }

    // ----- icons --------------------------------------------------------------

    #[test]
    fn icon_list_is_complete_and_unique() {
        let unique: HashSet<Icon> = Icon::ALL.into_iter().collect();
        assert_eq!(unique.len(), Icon::ALL.len());
        let names: HashSet<&str> = Icon::ALL.into_iter().map(Icon::name).collect();
        assert_eq!(names.len(), Icon::ALL.len());
    }

    #[test]
    fn every_icon_paints_inside_its_rect() {
        let mut problems = Vec::new();
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            for size in [10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 28.0] {
                for origin in [pos2(40.0, 24.0), pos2(40.5, 17.25)] {
                    let rect = Rect::from_min_size(origin, Vec2::splat(size));
                    // Solid ink stays inside; the stroke's soft edge may
                    // reach a quarter of a physical pixel further.
                    let allowed = rect.expand(0.26 / ppp);
                    for icon in Icon::ALL {
                        let shapes = icon_shapes(rect, icon, Color32::WHITE, ppp);
                        let at = format!("{icon:?} at {size} px, ppp {ppp}, origin {origin:?}");
                        if shapes.is_empty() {
                            problems.push(format!("{at}: paints nothing"));
                        }
                        let mut bounds = Rect::NOTHING;
                        for shape in &shapes {
                            let b = shape.visual_bounding_rect();
                            if !(b.width() > 0.0 && b.height() > 0.0) {
                                problems.push(format!("{at}: empty shape"));
                            }
                            bounds = bounds.union(b);
                        }
                        if !allowed.contains_rect(bounds) {
                            problems.push(format!("{at}: leaves its rect: {bounds:?}"));
                        }
                        if bounds.width().max(bounds.height()) < size * 0.6 {
                            problems.push(format!("{at}: too small: {:?}", bounds.size()));
                        }
                    }
                }
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    #[test]
    fn icon_strokes_are_pixel_snapped() {
        // A vertical line is crisp when its edges fall on pixel boundaries.
        for ppp in [1.0, 1.5, 2.0] {
            for size in [12.0, 14.0, 16.0, 18.0, 20.0, 28.0] {
                let rect = Rect::from_min_size(pos2(30.0, 30.0), Vec2::splat(size));
                let (core, _) = icon_stroke(size * ppp);
                let shapes = icon_shapes(rect, Icon::Plus, Color32::WHITE, ppp);
                let Shape::Path(vertical) = &shapes[0] else {
                    panic!("Plus starts with its vertical stroke");
                };
                assert_eq!(vertical.points[0].x, vertical.points[1].x);
                let edge = vertical.points[0].x * ppp - core * 0.5;
                assert!(
                    (edge - edge.round()).abs() < 1e-3,
                    "{size} px at ppp {ppp}: stroke edge at {edge}"
                );
                // Mirrored grid points stay mirrored: the plus is symmetric.
                let Shape::Path(horizontal) = &shapes[1] else {
                    panic!("Plus ends with its horizontal stroke");
                };
                let centre = vertical.points[0].x;
                let (left, right) = (horizontal.points[0].x, horizontal.points[1].x);
                assert!(((centre - left) - (right - centre)).abs() < 1e-3);
            }
        }
    }

    // ----- palettes -----------------------------------------------------------

    #[test]
    fn scrollbar_thumbs_stand_out_from_their_track() {
        for choice in ThemeChoice::ALL {
            let pal = choice.palette();
            for hot in [false, true] {
                let ratio = contrast_ratio(scrollbar_thumb(pal, hot), pal.canvas);
                assert!(ratio >= 3.0, "{choice:?} hot={hot}: {ratio:.2}");
            }
        }
    }

    #[test]
    fn contrast_helpers_match_wcag() {
        assert!((contrast_ratio(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 1e-6);
        // #777 on white is the textbook 4.48:1.
        assert!((contrast_ratio(c(119, 119, 119), Color32::WHITE) - 4.48).abs() < 0.01);
        // ink_on always returns the better of its two inks.
        for v in 0..=255u8 {
            let fill = Color32::from_gray(v);
            let ink = ink_on(fill);
            let other = if ink == INK_DARK { INK_LIGHT } else { INK_DARK };
            assert!(contrast_ratio(ink, fill) >= contrast_ratio(other, fill) - 1e-3);
        }
    }

    #[test]
    fn palettes_meet_wcag_contrast() {
        for choice in ThemeChoice::ALL {
            let p = choice.palette();
            let check = |what: &str, fg: Color32, bg: Color32, min: f32| {
                let ratio = contrast_ratio(fg, bg);
                assert!(
                    ratio >= min,
                    "{choice:?}: {what} is {ratio:.2}:1, needs {min}:1"
                );
            };
            // Text on every surface it sits on.
            for (name, bg) in [
                ("bg", p.bg),
                ("panel", p.panel),
                ("raised", p.raised),
                ("canvas", p.canvas),
                ("canvas_alt", p.canvas_alt),
                ("inset", p.inset),
            ] {
                check(&format!("text on {name}"), p.text, bg, 7.0);
                check(&format!("text_dim on {name}"), p.text_dim, bg, 4.5);
                check(&format!("text_faint on {name}"), p.text_faint, bg, 4.5);
            }
            // Labels on control bodies, resting and hovered.
            for (name, bg) in [("control", p.control), ("control_hover", p.control_hover)] {
                check(&format!("text on {name}"), p.text, bg, 7.0);
                check(&format!("text_dim on {name}"), p.text_dim, bg, 4.5);
            }
            check("text on control_active", p.text, p.control_active, 4.5);
            // Disabled ink stays visible.
            for (name, bg) in [
                ("bg", p.bg),
                ("panel", p.panel),
                ("raised", p.raised),
                ("control", p.control),
                ("canvas", p.canvas),
            ] {
                check(
                    &format!("text_disabled on {name}"),
                    p.text_disabled,
                    bg,
                    3.0,
                );
            }
            // State colors double as text (status line, links, warnings).
            for (name, color) in [
                ("accent", p.accent),
                ("link", p.link),
                ("danger", p.danger),
                ("warn", p.warn),
            ] {
                check(&format!("{name} on panel"), color, p.panel, 4.5);
                check(&format!("{name} on raised"), color, p.raised, 4.5);
                check(&format!("{name} on canvas"), color, p.canvas, 3.0);
                check(&format!("ink on {name}"), ink_on(color), color, 4.5);
            }
            // A resting button reads as a button: its outline stands out
            // from every surface it sits on.
            for (name, bg) in [("bg", p.bg), ("panel", p.panel), ("raised", p.raised)] {
                check(
                    &format!("control_border on {name}"),
                    p.control_border,
                    bg,
                    1.6,
                );
            }

            // The mixing desk.
            let m = p.mixer;
            for (name, bg) in [
                ("mixer.panel", m.panel),
                ("mixer.strip", m.strip),
                ("mixer.strip_hi", m.strip_hi),
                ("mixer.master", m.master),
            ] {
                check(&format!("mixer.text on {name}"), m.text, bg, 7.0);
                check(&format!("mixer.text_dim on {name}"), m.text_dim, bg, 4.5);
                check(
                    &format!("mixer.text_faint on {name}"),
                    m.text_faint,
                    bg,
                    4.5,
                );
            }
            check("mixer.text on mixer.control", m.text, m.control, 7.0);
            for (name, color) in [
                ("meter_low", m.meter_low),
                ("meter_mid", m.meter_mid),
                ("meter_high", m.meter_high),
                ("meter_clip", m.meter_clip),
                ("meter_peak", m.meter_peak),
            ] {
                check(&format!("mixer.{name} in the well"), color, m.well, 3.0);
                check(&format!("mixer.{name} over unlit"), color, m.meter_off, 3.0);
            }
            check("mixer.cap_body in the well", m.cap_body, m.well, 3.0);
            let cap_on_strip =
                contrast_ratio(m.cap_body, m.strip).max(contrast_ratio(m.cap_edge, m.strip));
            assert!(
                cap_on_strip >= 3.0,
                "{choice:?}: fader cap vanishes on the strip ({cap_on_strip:.2}:1)"
            );
            check("mixer.tick on strip", m.tick, m.strip, 1.5);
            check("mixer.border on panel", m.border, m.panel, 1.3);
            for (name, color) in [("pan", m.pan), ("mute", m.mute), ("solo", m.solo)] {
                check(&format!("mixer.{name} on strip"), color, m.strip, 3.0);
                check(&format!("ink on mixer.{name}"), ink_on(color), color, 4.5);
            }
        }
        // Clip and note labels on the fixed track colors.
        for color in TRACK_PALETTE.map(rgb) {
            assert!(contrast_ratio(ink_on(color), color) >= 4.5, "{color:?}");
        }
    }

    #[test]
    fn disabled_fade_keeps_labels_legible() {
        // egui fades everything in a disabled Ui toward the surface behind it.
        for choice in ThemeChoice::ALL {
            let p = choice.palette();
            for surface in [p.panel, p.raised] {
                for ink in [p.text, p.text_dim] {
                    let faded = mix(surface, ink, DISABLED_ALPHA);
                    let ratio = contrast_ratio(faded, surface);
                    assert!(ratio >= 3.0, "{choice:?}: disabled label at {ratio:.2}:1");
                }
            }
        }
    }

    // ----- buttons ------------------------------------------------------------

    #[test]
    fn button_colors_stay_legible_in_every_state() {
        for choice in ThemeChoice::ALL {
            let p = choice.palette();
            let states = [
                Press {
                    enabled: true,
                    ..Default::default()
                },
                Press {
                    enabled: true,
                    hovered: true,
                    ..Default::default()
                },
                Press {
                    enabled: true,
                    hovered: true,
                    down: true,
                    ..Default::default()
                },
            ];
            for press in states {
                let off = button_colors(p, press, OnStyle::Off, false);
                assert!(
                    contrast_ratio(off.ink, off.fill) >= 4.5,
                    "{choice:?} {press:?}"
                );
                // The body is visible: fill or outline differs from the surface.
                assert!(contrast_ratio(off.border, p.panel) >= 1.6);
                for accent in [p.accent, p.link, p.danger, p.warn] {
                    let solid = button_colors(p, press, OnStyle::Solid(accent), false);
                    assert!(
                        contrast_ratio(solid.ink, solid.fill) >= 4.5,
                        "{choice:?} {press:?} solid {accent:?}"
                    );
                    let tint = button_colors(p, press, OnStyle::Tint(accent), false);
                    assert!(
                        contrast_ratio(tint.ink, tint.fill) >= 3.0,
                        "{choice:?} {press:?} tint {accent:?}"
                    );
                }
                let ghost = button_colors(p, press, OnStyle::Off, true);
                let behind = if ghost.fill == Color32::TRANSPARENT {
                    p.panel
                } else {
                    ghost.fill
                };
                assert!(contrast_ratio(ghost.ink, behind) >= 4.5);
            }
            // Disabled: still a button, still readable (never "blank").
            let disabled = Press::default();
            for on in [
                OnStyle::Off,
                OnStyle::Tint(p.accent),
                OnStyle::Solid(p.accent),
            ] {
                let colors = button_colors(p, disabled, on, false);
                assert!(
                    contrast_ratio(colors.ink, colors.fill) >= 3.0,
                    "{choice:?}: disabled {on:?} ink at {:.2}:1",
                    contrast_ratio(colors.ink, colors.fill)
                );
                for surface in [p.panel, p.raised] {
                    let body = contrast_ratio(colors.fill, surface)
                        .max(contrast_ratio(colors.border, surface));
                    assert!(
                        body >= 1.1,
                        "{choice:?}: disabled body vanishes ({body:.2})"
                    );
                }
            }
            let ghost = button_colors(p, disabled, OnStyle::Off, true);
            assert!(contrast_ratio(ghost.ink, p.panel) >= 3.0);
            assert!(contrast_ratio(ghost.ink, p.raised) >= 3.0);
        }
    }

    #[test]
    fn disabled_widgets_undo_the_blanket_fade() {
        let harness = Harness::new(400.0);
        let pal = harness.pal();
        let mut opacities = None;
        harness.frame(Vec::new(), |ui| {
            ui.add_enabled_ui(false, |ui| {
                opacities = Some((ui.painter().opacity(), unfaded_painter(ui).opacity()));
                let resp = icon_button(ui, pal, Icon::Undo, CONTROL_M, false, pal.accent, "Undo");
                assert!(!resp.enabled());
                assert_eq!(resp.rect.size(), Vec2::splat(CONTROL_M));
            });
        });
        let (faded, restored) = opacities.unwrap();
        assert!((faded - DISABLED_ALPHA).abs() < 1e-3);
        assert!((restored - 1.0).abs() < 1e-3);
    }

    #[test]
    fn buttons_use_the_token_sizes() {
        let harness = Harness::new(600.0);
        let pal = harness.pal();
        harness.frame(Vec::new(), |ui| {
            for size in [CONTROL_S, CONTROL_M, CONTROL_L] {
                let icon = icon_button(ui, pal, Icon::Plus, size, false, pal.accent, "Add");
                assert_eq!(icon.rect.size(), Vec2::splat(size));
                let label = icon_label_button(ui, pal, Icon::Marker, "Marker", size, "");
                assert_eq!(label.rect.height(), size);
                let off = toggle_button(ui, pal, "Chords", false, size, "");
                let on = toggle_button(ui, pal, "Chords", true, size, "");
                // Toggling must not shift the layout.
                assert_eq!(off.rect.size(), on.rect.size());
            }
            assert_eq!(
                (icon_side(CONTROL_S), icon_side(CONTROL_M)),
                (ICON_S, ICON_M)
            );
            assert_eq!(icon_side(CONTROL_L), ICON_L);
            // egui's own buttons and combo boxes line up with ours.
            let button = ui.button("Button");
            assert_eq!(button.rect.height(), CONTROL_M);
            let combo = combo("combo", "Bar", 80.0).show_ui(ui, |_| {});
            assert_eq!(combo.response.rect.height(), CONTROL_M);
        });
    }

    /// Bounding rects of the text shapes painted in a frame, by text.
    fn painted_text(output: &egui::FullOutput, text: &str) -> Vec<Rect> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(t) if t.galley.text() == text => Some(t.visual_bounding_rect()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn chip_labels_never_spill_out_of_the_chip() {
        let harness = Harness::new(400.0);
        let pal = harness.pal();
        let mut rects = Vec::new();
        let labels = ["M", "LOOP", "+ MARKER", "A much longer chip label"];
        let output = harness.frame(Vec::new(), |ui| {
            rects.clear();
            for label in labels {
                let size = vec2(44.0, CONTROL_S);
                rects.push(chip(ui, pal, label, false, pal.accent, size, label).rect);
            }
        });
        assert_eq!(painted_text(&output, "M").len(), 1);
        let texts: Vec<Rect> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(t) => Some(t.visual_bounding_rect()),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), labels.len());
        for (label, (rect, text)) in labels.into_iter().zip(rects.iter().zip(&texts)) {
            assert!(
                rect.expand(0.5).contains_rect(*text),
                "{label:?} spills: {text:?} outside {rect:?}"
            );
        }
    }

    #[test]
    fn segmented_selects_on_click() {
        let harness = Harness::new(600.0);
        let pal = harness.pal();
        let value = Cell::new(0u8);
        let rect = Cell::new(Rect::NOTHING);
        let changes = Cell::new(0);
        let add = |ui: &mut Ui| {
            let options = [
                Segment::new(0u8, "Pattern Editor"),
                Segment::new(1u8, "Mixer"),
                Segment::new(2u8, "Keyboard").icon(Icon::Keyboard),
            ];
            let mut v = value.get();
            let resp = segmented(ui, pal, &mut v, &options, CONTROL_M, None);
            value.set(v);
            rect.set(resp.rect);
            changes.set(changes.get() + usize::from(resp.changed()));
        };
        harness.frame(Vec::new(), add);
        let r = rect.get();
        assert_eq!(r.height(), CONTROL_M);
        let state = || (value.get(), changes.get());
        let last = pos2(r.right() - 12.0, r.center().y);
        harness.click(last, add);
        assert_eq!(state(), (2, 1));
        // Clicking the selected segment again changes nothing.
        harness.click(last, add);
        assert_eq!(state(), (2, 1));
        let first = pos2(r.left() + 12.0, r.center().y);
        harness.click(first, add);
        assert_eq!(state(), (0, 2));
    }

    #[test]
    fn segmented_fixed_width_divides_equally() {
        let harness = Harness::new(600.0);
        let pal = harness.pal();
        let value = Cell::new(0u8);
        let add = |ui: &mut Ui| {
            let options = [Segment::new(0u8, "Instruments"), Segment::new(1u8, "Track")];
            let mut v = value.get();
            let resp = segmented(ui, pal, &mut v, &options, CONTROL_M, Some(240.0));
            value.set(v);
            assert_eq!(resp.rect.size(), vec2(240.0, CONTROL_M));
        };
        harness.frame(Vec::new(), add);
        // The midpoint separates the two equal halves.
        harness.click(pos2(125.0, CONTROL_M * 0.5), add);
        assert_eq!(value.get(), 1);
        harness.click(pos2(115.0, CONTROL_M * 0.5), add);
        assert_eq!(value.get(), 0);
    }

    #[test]
    fn segmented_width_predicts_the_natural_width() {
        let harness = Harness::new(600.0);
        let pal = harness.pal();
        harness.frame(Vec::new(), |ui: &mut Ui| {
            for height in [CONTROL_S, CONTROL_M] {
                let options = [
                    Segment::new(0u8, "Diatonic"),
                    Segment::new(1u8, "Progressions").icon(Icon::Chords),
                    Segment::new(2u8, "").icon(Icon::Eraser),
                ];
                let want = segmented_width(ui, &options, height);
                let resp = segmented(ui, pal, &mut 0u8, &options, height, None);
                assert!((resp.rect.width() - want).abs() < 0.01, "{height}: {want}");
            }
        });
    }

    #[test]
    fn slider_follows_the_pointer_and_respects_disabled() {
        let harness = Harness::new(400.0);
        let pal = harness.pal();
        let value = Cell::new(0.0_f32);
        let steps = Cell::new(1_i32);
        let frozen = Cell::new(0.25_f32);
        let rects = Cell::new([Rect::NOTHING; 3]);
        let changed = Cell::new([false; 3]);
        let add = |ui: &mut Ui| {
            let (mut v, mut s, mut f) = (value.get(), steps.get(), frozen.get());
            let a = slider(ui, pal, &mut v, 0.0..=1.0, 212.0);
            let b = slider(ui, pal, &mut s, 1..=16, 212.0);
            let c = ui
                .add_enabled_ui(false, |ui| slider(ui, pal, &mut f, 0.0..=1.0, 212.0))
                .inner;
            value.set(v);
            steps.set(s);
            frozen.set(f);
            rects.set([a.rect, b.rect, c.rect]);
            let mut flags = changed.get();
            for (flag, resp) in flags.iter_mut().zip([a, b, c]) {
                *flag |= resp.changed();
            }
            changed.set(flags);
        };
        harness.frame(Vec::new(), add);
        let r = rects.get();
        assert_eq!(r[0].size(), vec2(212.0, CONTROL_S));
        // The track is inset by the handle radius (6 px) on both sides.
        let at = |rect: Rect, t: f32| pos2(rect.left() + 6.0 + 200.0 * t, rect.center().y);
        harness.click(at(r[0], 0.75), add);
        harness.click(at(r[1], 1.0), add);
        harness.click(at(r[2], 0.9), add);
        assert!((value.get() - 0.75).abs() < 0.01, "value {}", value.get());
        assert_eq!(steps.get(), 16);
        assert_eq!(frozen.get(), 0.25);
        assert_eq!(changed.get(), [true, true, false]);
    }

    // ----- text ---------------------------------------------------------------

    #[test]
    fn long_text_is_elided_within_its_width() {
        let harness = Harness::new(400.0);
        let pal = harness.pal();
        let long = "A very long instrument name that cannot possibly fit";
        harness.frame(Vec::new(), |ui| {
            let cut = elide(ui, long, body_font(TEXT_BODY), pal.text, 90.0);
            assert!(cut.elided);
            assert!(cut.size().x <= 90.0);
            let whole = elide(ui, "Short", body_font(TEXT_BODY), pal.text, 90.0);
            assert!(!whole.elided);
            assert_eq!(whole.text(), "Short");

            let (rect, elided) = paint_elided(
                ui,
                pos2(300.0, 200.0),
                Align2::RIGHT_CENTER,
                long,
                body_font(TEXT_BODY),
                pal.text,
                120.0,
            );
            assert!(elided);
            assert!(rect.width() <= 120.0 && (rect.right() - 300.0).abs() < 0.01);

            ui.horizontal(|ui| {
                let label = truncated_label(ui, long, 140.0);
                assert!(label.rect.width() <= 140.0 + 0.5, "{:?}", label.rect);
                let fits = truncated_label(ui, "Kick", 140.0);
                assert!(fits.rect.width() < 60.0);
            });
        });
    }

    #[test]
    fn panel_header_title_yields_to_its_controls() {
        let harness = Harness::new(220.0);
        let pal = harness.pal();
        let title = "A panel title that is much wider than the panel";
        let mut buttons = Vec::new();
        let output = harness.frame(Vec::new(), |ui| {
            buttons.clear();
            panel_header(ui, pal, title, |ui| {
                buttons.push(ghost_icon_button(ui, pal, Icon::Close, CONTROL_S, "Close").rect);
                buttons.push(ghost_icon_button(ui, pal, Icon::Menu, CONTROL_S, "More").rect);
            });
        });
        let text: Vec<Rect> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(t) if t.galley.text().starts_with("A panel") => {
                    Some(t.visual_bounding_rect())
                }
                _ => None,
            })
            .collect();
        assert_eq!(text.len(), 1);
        for button in &buttons {
            assert!(button.right() <= 220.0);
            assert!(
                text[0].right() <= button.left() + 0.5,
                "title {:?} runs into {button:?}",
                text[0]
            );
        }
    }

    // ----- toolbar ------------------------------------------------------------

    /// A crowded toolbar exercising every item kind. `extra` adds trailing
    /// buttons so tests can change the content between frames.
    fn crowded_toolbar(
        ui: &mut Ui,
        pal: Palette,
        extra: usize,
        clips: &mut Vec<Rect>,
        menu_rows: &mut Vec<(String, Rect, bool)>,
    ) -> ToolbarOutput {
        toolbar(ui, pal, "crowded", |tb| {
            let mut track = |tb: &Toolbar<'_>, name: &str, resp: Response| {
                if tb.in_menu() && resp.rect.width() > 0.0 {
                    menu_rows.push((name.to_owned(), resp.rect, resp.clicked()));
                }
            };
            let r = tb.icon_button(Icon::ZoomOut, "Zoom out", "Zoom out");
            track(tb, "Zoom out", r);
            let r = tb.icon_button(Icon::ZoomIn, "Zoom in", "Zoom in");
            track(tb, "Zoom in", r);
            tb.separator();
            tb.labeled("Length", 72.0, |ui| {
                clips.push(ui.clip_rect());
                // Deliberately wider than its slot: must be clipped, not spill.
                let _ = ui.button("A button far wider than its slot");
            });
            tb.label("Snap");
            tb.item(64.0, |ui| {
                clips.push(ui.clip_rect());
                let _ = ui.button("Bar");
            });
            tb.separator();
            let r = tb.toggle(Some(Icon::Loop), "Loop", true, "Loop the selection");
            track(tb, "Loop", r);
            let r = tb.toggle(Some(Icon::Follow), "Follow", false, "Follow the playhead");
            track(tb, "Follow", r);
            let r = tb.button(Some(Icon::Marker), "Marker", "Add a marker");
            track(tb, "Marker", r);
            let r = tb
                .enabled(false)
                .button(Some(Icon::Split), "Split", "Split the clip");
            track(tb, "Split", r);
            let r = tb.button(None, "Humanize", "Humanize the selection");
            track(tb, "Humanize", r);
            tb.hint("Drag empty lanes to box-select clips · Alt-drag copies · corners fade");
            tb.spacer();
            for i in 0..extra {
                let r = tb.button(Some(Icon::Dice), "Extra", "Extra");
                track(tb, &format!("Extra {i}"), r);
            }
            let r = tb.icon_button(Icon::Settings, "Settings", "Settings");
            track(tb, "Settings", r);
        })
    }

    fn assert_no_overlap(out: &ToolbarOutput, clips: &[Rect], context: &str) {
        let mut rects = out.placed.clone();
        rects.extend(out.overflow_button);
        for (i, a) in rects.iter().enumerate() {
            assert!(
                a.width() > 0.0 && out.rect.expand(0.5).contains_rect(*a),
                "{context}: item {i} {a:?} leaves the row {:?}",
                out.rect
            );
            if let Some(b) = rects.get(i + 1) {
                assert!(
                    a.right() <= b.left() + 0.01,
                    "{context}: item {i} {a:?} overlaps the next one {b:?}"
                );
            }
        }
        // Custom widgets are clipped to their own slot.
        for clip in clips {
            assert!(
                out.placed
                    .iter()
                    .any(|slot| slot.expand(0.01).contains_rect(*clip)),
                "{context}: a custom item may paint outside its slot ({clip:?})"
            );
        }
    }

    #[test]
    fn toolbar_never_overlaps_at_any_width() {
        let pal = ThemeChoice::Graphite.palette();
        let mut widths: Vec<f32> = (40..=1500).step_by(23).map(|w| w as f32).collect();
        widths.extend([1100.0, 1360.0, 1920.0]);
        for width in widths {
            let harness = Harness::new(width);
            let mut settled = None;
            for frame in 0..4 {
                let (mut clips, mut rows) = (Vec::new(), Vec::new());
                let mut out = None;
                harness.frame(Vec::new(), |ui| {
                    clips.clear();
                    out = Some(crowded_toolbar(ui, pal, 0, &mut clips, &mut rows));
                });
                let out = out.unwrap();
                assert_eq!(out.rect.height(), TOOLBAR_H);
                assert_no_overlap(&out, &clips, &format!("width {width} frame {frame}"));
                settled = Some(out);
            }
            // Once measured, the layout adapts instead of clipping.
            let out = settled.unwrap();
            if width >= 900.0 {
                assert!(
                    !out.compact && out.overflowed == 0,
                    "width {width}: {out:?}"
                );
                // The hint fits and the spacer right-aligns the last item.
                assert_eq!(out.placed.len(), 12, "width {width}");
                let last = out.placed.last().unwrap();
                assert!(
                    (last.right() - out.rect.right()).abs() < 0.6,
                    "width {width}"
                );
            }
            if width <= 500.0 {
                assert!(out.compact, "width {width} should collapse labels");
            }
            if out.compact {
                // Labels went first (the "Snap" caption too); the optional
                // hint must not take their room.
                assert!(
                    out.placed.len() <= 10,
                    "width {width}: hint shown while compact"
                );
            }
            if width <= 300.0 {
                assert!(out.overflowed > 0, "width {width} should overflow");
            }
            if out.overflowed > 0 && width >= CONTROL_M {
                let more = out
                    .overflow_button
                    .expect("overflowed items need the ⋯ menu");
                assert!((more.right() - out.rect.right()).abs() < 0.01);
            }
        }
    }

    #[test]
    fn toolbar_stays_safe_when_its_content_changes() {
        // Stale measurements (content or width changed since the last frame)
        // may cost a frame of overflow, never an overlap.
        let pal = ThemeChoice::Graphite.palette();
        let mut harness = Harness::new(760.0);
        for (frame, (width, extra)) in [
            (760.0, 0),
            (760.0, 0),
            (760.0, 6),
            (760.0, 6),
            (420.0, 6),
            (1500.0, 0),
            (200.0, 3),
            (200.0, 3),
        ]
        .into_iter()
        .enumerate()
        {
            harness.size.x = width;
            let (mut clips, mut rows) = (Vec::new(), Vec::new());
            let mut out = None;
            harness.frame(Vec::new(), |ui| {
                clips.clear();
                out = Some(crowded_toolbar(ui, pal, extra, &mut clips, &mut rows));
            });
            assert_no_overlap(&out.unwrap(), &clips, &format!("frame {frame}"));
        }
    }

    #[test]
    fn toolbar_overflow_menu_holds_the_hidden_items() {
        let pal = ThemeChoice::Graphite.palette();
        let harness = Harness::new(260.0);
        let out = RefCell::new(None);
        let rows: RefCell<Vec<(String, Rect, bool)>> = RefCell::new(Vec::new());
        let frame = |events: Vec<Event>| {
            harness.frame(events, |ui| {
                let mut clips = Vec::new();
                let mut fresh = Vec::new();
                *out.borrow_mut() = Some(crowded_toolbar(ui, pal, 0, &mut clips, &mut fresh));
                *rows.borrow_mut() = fresh;
            });
        };
        for _ in 0..3 {
            frame(Vec::new());
        }
        let settled = out.borrow().clone().unwrap();
        assert!(settled.overflowed >= 4, "{settled:?}");
        assert!(
            rows.borrow().is_empty(),
            "the menu is closed until ⋯ is clicked"
        );

        // Open the menu: the hidden buttons appear as rows, in order.
        let more = settled.overflow_button.unwrap().center();
        frame(press(more));
        frame(release(more));
        frame(Vec::new());
        let menu = rows.borrow().clone();
        let names: Vec<&str> = menu.iter().map(|(name, ..)| name.as_str()).collect();
        assert!(names.ends_with(&["Humanize", "Settings"]), "{names:?}");
        assert!(names.contains(&"Marker"), "{names:?}");
        for pair in menu.windows(2) {
            assert!(pair[0].1.bottom() <= pair[1].1.top() + 0.01, "rows overlap");
        }

        // A row in the menu is a working button, and choosing it closes the menu.
        let target = menu.iter().find(|(name, ..)| name == "Settings").unwrap().1;
        frame(press(target.center()));
        frame(release(target.center()));
        let clicked = rows
            .borrow()
            .iter()
            .any(|(name, _, hit)| name == "Settings" && *hit);
        assert!(clicked, "clicking a menu row reaches the item");
        frame(Vec::new());
        frame(Vec::new());
        assert!(rows.borrow().is_empty(), "the menu closes after a choice");
    }

    // ----- gallery ------------------------------------------------------------

    #[test]
    fn gallery_renders_every_tab_in_every_theme() {
        for choice in ThemeChoice::ALL {
            let harness = Harness::themed(1200.0, choice);
            for tab in [
                GalleryTab::Icons,
                GalleryTab::Controls,
                GalleryTab::Layout,
                GalleryTab::Colors,
            ] {
                let state = GalleryState {
                    tab,
                    toolbar_width: 300.0,
                    ..Default::default()
                };
                harness
                    .ctx
                    .data_mut(|d| d.insert_temp(egui::Id::new("cadence.theme.gallery"), state));
                for _ in 0..2 {
                    harness.frame(Vec::new(), |ui| {
                        gallery_window(ui.ctx(), harness.pal(), GalleryTab::Icons)
                    });
                }
            }
        }
    }
}
