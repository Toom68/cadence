//! Mixer: one channel strip per track, the four fixed subgroup bus strips,
//! plus the master strip.
//!
//! The desk is painted entirely from the active theme's [`MixerPalette`]
//! (`pal.mixer`): a layered console in the dark themes, a light console with
//! dark meter wells in Paper. Strips are placed by hand at fixed widths, so
//! only the strips inside the horizontal viewport are built each frame, and a
//! strip copies just the scalars and short labels it paints.
//!
//! Meters show live post-fader levels from the audio engine
//! ([`crate::audio::MixerPeaks`]) with UI-side ballistics: instant attack, a
//! smooth release, a short peak-hold tick and a clip LED that latches until it
//! is clicked. The ballistics live in egui temp data for the session.

use std::collections::HashMap;
use std::f32::consts::PI;
use std::time::Duration;

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, Layout, Painter, Pos2, Rect, Response,
    RichText, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, WidgetInfo, WidgetType,
};

use super::theme::{self, MixerPalette, Palette, CONTROL_S, GAP_XS};
use super::{ops, sidebar, CadenceApp};
use crate::audio::MixerPeaks;
use crate::model::{Effects, Project, MAX_CUTOFF_HZ, MIN_CUTOFF_HZ};

const STRIP_W: f32 = 168.0;
/// Narrower channel strip used by the compact mixer density.
const STRIP_W_COMPACT: f32 = 112.0;
/// Width of one subgroup bus strip.
const BUS_W: f32 = 140.0;
const BUS_W_COMPACT: f32 = 112.0;
/// Master strip widths (a touch wider than a channel's inner column).
const MASTER_W: f32 = 160.0;
const MASTER_W_COMPACT: f32 = 120.0;
/// Interaction column of a fader.
const FADER_W: f32 = 34.0;
/// Height of the fader cap.
const CAP_H: f32 = 12.0;
/// Room above and below the fader groove for the cap to travel past its ends.
const FADER_PAD: f32 = 8.0;
/// Width of the segmented level meter beside each fader.
const METER_W: f32 = 10.0;
/// Height of the dB value box above a fader.
const READOUT_H: f32 = 18.0;
/// Height of the clip LED above a meter.
const LED_H: f32 = 5.0;
/// Thickness of the colored bar on top of a strip.
const COLOR_BAR: f32 = 4.0;
/// Narrowest a knob's footprint gets (`mixer_knob`).
const KNOB_MIN_W: f32 = 40.0;
/// Width reserved for the pan knob when the pedalboard is open.
const KNOB_COL_W: f32 = 44.0;
/// Horizontal slot one FX pedal occupies in the strip's pedalboard row.
const PEDAL_CELL_W: f32 = 74.0;
/// Painted pedal body width (the cell is a little wider for spacing).
const PEDAL_W: f32 = 66.0;
/// Tallest the pedal body gets; shorter panels get shorter pedals.
const PEDAL_MAX_H: f32 = 116.0;
/// Width of the chevron painted between two pedals.
const ARROW_W: f32 = 10.0;
/// Total width of the four pedals and the arrows between them.
const PEDAL_CHAIN_W: f32 = 4.0 * PEDAL_CELL_W + 3.0 * ARROW_W;
/// Thickness of the strip row's horizontal scrollbar.
const SCROLLBAR_H: f32 = 8.0;
/// Quietest level a meter shows; the bar spans this to 0 dBFS.
const METER_FLOOR_DB: f32 = -60.0;
/// Fader positions below this many dB read as "-inf".
const FADER_FLOOR_DB: f32 = -60.0;
/// Master gain trim (`MASTER_TRIM` in the renderer) in dB: the master fader's
/// readout is the gain it applies, `m² · 1.6`.
const MASTER_TRIM_DB: f32 = 4.082;
/// Linear peak at which a channel or bus meter latches its clip LED.
const CLIP_LEVEL: f32 = 1.0;
/// The master limiter's ceiling (-1 dBFS): reaching it lights the master LED.
const MASTER_CLIP_LEVEL: f32 = 0.885;
/// How long a peak-hold tick stays put before it falls.
const HOLD_SECS: f32 = 1.2;
/// Per-frame (at 60 fps) multiplier of the meter release, matching the
/// master peak decay in `poll_engine`.
const RELEASE_PER_FRAME: f32 = 0.86;
/// egui temp data key for the compact-density toggle. Temp data lives for the
/// session only: it survives tab switches but is gone on restart, and it is
/// never written into the song file.
const COMPACT_KEY: &str = "cadence.mixer.compact";
const SCROLL_KEY: &str = "cadence.mixer.scroll";
const METERS_KEY: &str = "cadence.mixer.meters";

/// The four per-track effects, in the order the engine processes them:
/// drive -> filter -> [fader/pan] -> delay -> reverb. Delay and reverb are
/// post-fader sends to shared buses (the delay feed also hits reverb at 0.2x);
/// see `Strip::process` in `src/audio/renderer.rs`.
#[derive(Clone, Copy)]
enum Pedal {
    Drive,
    Filter,
    Delay,
    Reverb,
}

impl Pedal {
    const ALL: [Pedal; 4] = [Pedal::Drive, Pedal::Filter, Pedal::Delay, Pedal::Reverb];

    fn label(self) -> &'static str {
        match self {
            Pedal::Drive => "Drive",
            Pedal::Filter => "Filter",
            Pedal::Delay => "Delay",
            Pedal::Reverb => "Reverb",
        }
    }

    /// `(normalized value, double-click default, audibly engaged)`.
    fn state(self, fx: &Effects) -> (f32, f32, bool) {
        match self {
            Pedal::Filter => (
                cutoff_to_norm(fx.cutoff_hz),
                1.0,
                fx.cutoff_hz < MAX_CUTOFF_HZ - 1.0,
            ),
            Pedal::Drive => (fx.drive, 0.0, fx.drive > 0.004),
            Pedal::Delay => (fx.delay, 0.0, fx.delay > 0.004),
            Pedal::Reverb => (fx.reverb, 0.15, fx.reverb > 0.004),
        }
    }

    fn value_text(self, fx: &Effects) -> String {
        match self {
            Pedal::Filter => hz_text(fx.cutoff_hz),
            Pedal::Drive => format!("{:.0}%", fx.drive * 100.0),
            Pedal::Delay => format!("{:.0}%", fx.delay * 100.0),
            Pedal::Reverb => format!("{:.0}%", fx.reverb * 100.0),
        }
    }
}

/// Everything the mixer paints with, resolved once per frame.
#[derive(Clone, Copy)]
struct Look {
    pal: Palette,
    mx: MixerPalette,
    compact: bool,
}

impl Look {
    fn margin(self) -> f32 {
        if self.compact {
            4.0
        } else {
            8.0
        }
    }

    fn radius(self) -> u8 {
        if self.compact {
            6
        } else {
            8
        }
    }

    fn readout_w(self) -> f32 {
        if self.compact {
            44.0
        } else {
            48.0
        }
    }

    /// Space between two strips in the row.
    fn strip_gap(self) -> f32 {
        if self.compact {
            4.0
        } else {
            8.0
        }
    }

    fn channel_w(self, fx_open: bool) -> f32 {
        if fx_open {
            2.0 * self.margin()
                + self.readout_w()
                + GAP_XS
                + METER_W
                + GAP_XS
                + KNOB_COL_W
                + GAP_XS
                + PEDAL_CHAIN_W
        } else if self.compact {
            STRIP_W_COMPACT
        } else {
            STRIP_W
        }
    }

    fn bus_w(self) -> f32 {
        if self.compact {
            BUS_W_COMPACT
        } else {
            BUS_W
        }
    }

    fn master_w(self) -> f32 {
        if self.compact {
            MASTER_W_COMPACT
        } else {
            MASTER_W
        }
    }
}

/// Session-only mixer flags kept in egui temp data (see [`COMPACT_KEY`]).
fn compact_mode(ui: &Ui) -> bool {
    ui.data(|d| d.get_temp::<bool>(egui::Id::new(COMPACT_KEY)))
        .unwrap_or(false)
}

fn fx_key(id: u64) -> egui::Id {
    egui::Id::new(("cadence.mixer.fx", id))
}

#[cfg(test)]
fn pedals_open(ui: &Ui, id: u64) -> bool {
    ui.data(|d| d.get_temp::<bool>(fx_key(id))).unwrap_or(false)
}

// ----- strip row layout ------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Slot {
    Track(usize),
    Bus(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Placed {
    slot: Slot,
    /// Left edge within the (unscrolled) strip row.
    x: f32,
    w: f32,
}

/// Places every channel strip, then the bus strips after a divider.
/// `track_widths[i]` is the width of track `i`'s strip. Returns the placed
/// strips, the x of the divider and the total row width.
fn place_strips(look: Look, track_widths: &[f32]) -> (Vec<Placed>, f32, f32) {
    let gap = look.strip_gap();
    let mut x = 0.0;
    let mut placed = Vec::with_capacity(track_widths.len() + ops::NUM_BUSES);
    for (i, &w) in track_widths.iter().enumerate() {
        placed.push(Placed {
            slot: Slot::Track(i),
            x,
            w,
        });
        x += w + gap;
    }
    let divider = x + gap * 0.5;
    x += gap * 2.0;
    for i in 0..ops::NUM_BUSES {
        let w = look.bus_w();
        placed.push(Placed {
            slot: Slot::Bus(i),
            x,
            w,
        });
        x += w + gap;
    }
    (placed, divider, (x - gap).max(0.0))
}

// ----- level and fader math ---------------------------------------------------

fn lin_to_db(lin: f32) -> f32 {
    if lin > 1e-6 {
        20.0 * lin.log10()
    } else {
        f32::NEG_INFINITY
    }
}

/// Meter bar position (0..=1) of a linear level on the dB scale.
fn meter_pos(lin: f32) -> f32 {
    ((lin_to_db(lin) - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0)
}

/// Gain in dB a fader position applies. Faders map position `v` to gain
/// `v²` (see `fader_gain`), so `offset_db` shifts the master's extra trim.
/// `None` is silence (below [`FADER_FLOOR_DB`]).
fn fader_db(v: f32, offset_db: f32) -> Option<f32> {
    if v <= 0.0 {
        return None;
    }
    let db = 40.0 * v.log10() + offset_db;
    (db > FADER_FLOOR_DB).then_some(db)
}

/// Inverse of [`fader_db`]: the fader position that applies `db`.
fn db_to_fader(db: f32, offset_db: f32) -> f32 {
    10f32.powf((db - offset_db) / 40.0)
}

fn db_text(db: Option<f32>) -> String {
    match db {
        None => "-\u{221e}".to_owned(),
        Some(db) if db.abs() < 0.05 => "0.0".to_owned(),
        Some(db) => format!("{db:+.1}"),
    }
}

fn level_db_text(lin: f32) -> String {
    let db = lin_to_db(lin);
    db_text((db > METER_FLOOR_DB).then_some(db))
}

/// `color`, moved toward white (on a dark `bg`) or black (on a light one)
/// until it reads against `bg` with at least `min` contrast.
fn legible(color: Color32, bg: Color32, min: f32) -> Color32 {
    if theme::contrast_ratio(color, bg) >= min {
        return color;
    }
    let toward = if theme::relative_luminance(bg) < 0.4 {
        Color32::WHITE
    } else {
        Color32::BLACK
    };
    (1..=10)
        .map(|i| theme::mix(color, toward, i as f32 / 10.0))
        .find(|c| theme::contrast_ratio(*c, bg) >= min)
        .unwrap_or(toward)
}

// ----- meter ballistics --------------------------------------------------------

/// Ballistics of one level meter: the displayed level, its peak-hold tick,
/// the latched clip flag and the highest peak since the last reset.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Gauge {
    level: f32,
    hold: f32,
    hold_age: f32,
    peak_max: f32,
    clip: bool,
}

impl Gauge {
    /// Advances one frame: `input` is the newest linear peak, `fall` the
    /// release multiplier for this frame's `dt`, `clip_at` the clip threshold.
    fn update(&mut self, input: f32, dt: f32, fall: f32, clip_at: f32) {
        let input = if input.is_finite() {
            input.max(0.0)
        } else {
            0.0
        };
        self.level = if input >= self.level {
            input
        } else {
            self.level * fall
        };
        if self.level < 0.0005 {
            self.level = 0.0;
        }
        if self.level >= self.hold {
            self.hold = self.level;
            self.hold_age = 0.0;
        } else {
            self.hold_age += dt;
            if self.hold_age > HOLD_SECS {
                self.hold = (self.hold * fall).max(self.level);
                if self.hold < 0.0005 {
                    self.hold = 0.0;
                }
            }
        }
        self.peak_max = self.peak_max.max(input);
        if input >= clip_at {
            self.clip = true;
        }
    }

    fn active(&self) -> bool {
        self.level > 0.0 || self.hold > 0.0
    }

    /// Clears the hold tick, the clip latch and the peak maximum.
    fn reset(&mut self) {
        self.hold = self.level;
        self.hold_age = 0.0;
        self.peak_max = self.level;
        self.clip = false;
    }
}

/// Ballistics of every meter on the desk, kept across frames.
#[derive(Clone, Debug, Default)]
struct Meters {
    tracks: HashMap<u64, Gauge>,
    buses: [Gauge; ops::NUM_BUSES],
    master: [Gauge; 2],
    last_peaks: Option<MixerPeaks>,
}

impl Meters {
    /// Folds one frame of engine peaks into the gauges. A track missing from
    /// the engine's snapshot (or no engine at all) reads as silence.
    fn advance(&mut self, project: &Project, master: [f32; 2], dt: f32) {
        let fall = RELEASE_PER_FRAME.powf(dt * 60.0);
        let peaks = self.last_peaks;
        for track in &project.tracks {
            let input = peaks.and_then(|p| p.track(track.id)).unwrap_or(0.0);
            self.tracks
                .entry(track.id)
                .or_default()
                .update(input, dt, fall, CLIP_LEVEL);
        }
        self.tracks
            .retain(|id, _| project.tracks.iter().any(|t| t.id == *id));
        for (i, gauge) in self.buses.iter_mut().enumerate() {
            let input = peaks.map_or(0.0, |p| p.buses[i]);
            gauge.update(input, dt, fall, CLIP_LEVEL);
        }
        for (gauge, input) in self.master.iter_mut().zip(master) {
            gauge.update(input, dt, fall, MASTER_CLIP_LEVEL);
        }
    }

    fn any_active(&self) -> bool {
        self.tracks.values().any(Gauge::active)
            || self.buses.iter().any(Gauge::active)
            || self.master.iter().any(Gauge::active)
    }

    fn reset(&mut self) {
        self.tracks.values_mut().for_each(Gauge::reset);
        self.buses.iter_mut().for_each(Gauge::reset);
        self.master.iter_mut().for_each(Gauge::reset);
    }

    fn master_peak(&self) -> f32 {
        self.master[0].peak_max.max(self.master[1].peak_max)
    }
}

// ----- painted mixer widgets -----------------------------------------------------

/// Re-styles egui's own widgets (combos, text edits, their popups) inside the
/// mixer subtree so they sit on the desk in every theme.
fn mixer_widget_style(ui: &mut Ui, look: Look) {
    let mx = look.mx;
    let v = &mut ui.style_mut().visuals;
    v.window_fill = mx.strip_hi;
    v.window_stroke = Stroke::new(1.0_f32, mx.border);
    v.extreme_bg_color = mx.well;
    v.menu_corner_radius = CornerRadius::same(6);
    v.selection.bg_fill = mx.select;
    v.selection.stroke = Stroke::new(1.0_f32, mx.pan);
    v.text_cursor.stroke = Stroke::new(2.0_f32, mx.text);
    let radius = CornerRadius::same(5);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = mx.strip;
    w.noninteractive.weak_bg_fill = mx.strip;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, mx.border_soft);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, mx.text_dim);
    w.noninteractive.corner_radius = radius;
    w.inactive.bg_fill = mx.control;
    w.inactive.weak_bg_fill = mx.control;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, mx.border);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, mx.text);
    w.inactive.corner_radius = radius;
    w.hovered.bg_fill = mx.control_hover;
    w.hovered.weak_bg_fill = mx.control_hover;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, mx.text_faint);
    w.hovered.fg_stroke = Stroke::new(1.5_f32, mx.text);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;
    w.active.bg_fill = mx.control_active;
    w.active.weak_bg_fill = mx.control_active;
    w.active.bg_stroke = Stroke::new(1.0_f32, mx.pan);
    w.active.fg_stroke = Stroke::new(1.5_f32, mx.text);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;
    w.open.bg_fill = mx.control_hover;
    w.open.weak_bg_fill = mx.control_hover;
    w.open.bg_stroke = Stroke::new(1.0_f32, mx.border);
    w.open.fg_stroke = Stroke::new(1.0_f32, mx.text);
    w.open.corner_radius = radius;
}

fn arc_points(center: Pos2, r: f32, from: f32, to: f32, n: usize) -> Vec<Pos2> {
    (0..=n)
        .map(|i| {
            let a = from + (to - from) * i as f32 / n as f32;
            center + vec2(a.cos(), a.sin()) * r
        })
        .collect()
}

/// A tooltip whose text is only built while the pointer actually hovers.
fn lazy_tip(resp: Response, text: impl FnOnce() -> String) -> Response {
    resp.on_hover_ui(|ui| {
        ui.label(text());
    })
}

/// Rotary knob with a value arc: circular body, recessed arc track, a
/// colored value arc from the start to the current value and a hairline
/// pointer. Interaction matches `theme::knob` (drag, Shift for fine,
/// double-click to reset, Alt+scroll).
#[allow(clippy::too_many_arguments)]
fn mixer_knob(
    ui: &mut Ui,
    look: Look,
    norm: &mut f32,
    default: f32,
    bipolar: bool,
    label: &str,
    value_text: &str,
    color: Color32,
    size: f32,
) -> Response {
    let mx = look.mx;
    let total = vec2(size.max(KNOB_MIN_W), size + 30.0);
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
        let r = size * 0.5 - 1.5;
        let a0 = PI * 0.75;
        let sweep = PI * 1.5;
        let active = resp.hovered() || resp.dragged();
        let body = if active { mx.control_hover } else { mx.control };
        p.circle_filled(center, r, body);
        p.circle_stroke(center, r, Stroke::new(1.0_f32, mx.border));
        p.add(Shape::line(
            arc_points(center, r - 4.0, a0, a0 + sweep, 32),
            Stroke::new(2.0_f32, mx.arc),
        ));
        let (from, to) = if bipolar {
            (0.5f32.min(v), 0.5f32.max(v))
        } else {
            (0.0, v)
        };
        if to - from > 0.005 {
            p.add(Shape::line(
                arc_points(center, r - 4.0, a0 + sweep * from, a0 + sweep * to, 24),
                Stroke::new(2.0_f32, legible(color, body, 2.2)),
            ));
        }
        let a = a0 + sweep * v;
        p.line_segment(
            [
                center + vec2(a.cos(), a.sin()) * (r * 0.3),
                center + vec2(a.cos(), a.sin()) * (r - 5.0),
            ],
            Stroke::new(1.5_f32, mx.text),
        );
        p.text(
            Pos2::new(center.x, rect.top() + size + 3.0),
            Align2::CENTER_TOP,
            value_text,
            theme::body_font(11.0),
            if active { mx.text } else { mx.text_dim },
        );
        p.text(
            Pos2::new(center.x, rect.top() + size + 16.0),
            Align2::CENTER_TOP,
            label,
            theme::body_font(10.0),
            mx.text_faint,
        );
    }
    lazy_tip(resp, || {
        format!(
            "{label}: {value_text}\nDrag up/down to change · Shift for fine · Double-click to reset"
        )
    })
}

/// Fader with a recessed groove, dB scale ticks and a cap. Press anywhere on
/// the groove to jump there, then drag (Shift for fine); double-click resets;
/// Alt+scroll nudges. `offset_db` is the extra gain the master applies.
#[allow(clippy::too_many_arguments)]
fn mixer_fader(
    ui: &mut Ui,
    look: Look,
    rect: Rect,
    id: egui::Id,
    value: &mut f32,
    default: f32,
    offset_db: f32,
    color: Color32,
    label: &str,
) -> Response {
    let mx = look.mx;
    let mut resp = ui.interact(rect, id, Sense::click_and_drag());
    let before = *value;
    let track = Rect::from_center_size(rect.center(), vec2(5.0, rect.height() - 2.0 * FADER_PAD));
    let cap_y = |v: f32| track.bottom() - track.height() * v;
    if resp.double_clicked() {
        *value = default;
    } else if resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
        if let Some(pos) = resp.interact_pointer_pos() {
            if (pos.y - cap_y(*value)).abs() > CAP_H * 0.5 {
                *value = ((track.bottom() - pos.y) / track.height()).clamp(0.0, 1.0);
            }
        }
    } else if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let dy = resp.drag_delta().y;
        *value = (*value - dy / track.height() * if fine { 0.2 } else { 1.0 }).clamp(0.0, 1.0);
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 && ui.input(|i| i.modifiers.alt) {
            *value = (*value + scroll.signum() * 0.02).clamp(0.0, 1.0);
        }
    }
    if *value != before {
        resp.mark_changed();
    }
    let v = *value;
    resp.widget_info(|| WidgetInfo::slider(true, v as f64, label));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect(
            track,
            2.5,
            mx.well,
            Stroke::new(1.0_f32, mx.border_soft),
            StrokeKind::Inside,
        );
        let y = cap_y(v);
        p.rect_filled(
            Rect::from_min_max(Pos2::new(track.left(), y), track.right_bottom()),
            2.5,
            legible(color, mx.well, 3.0).gamma_multiply(0.9),
        );
        let tick = Stroke::new(1.0_f32, mx.tick);
        let unity = Stroke::new(1.0_f32, mx.text_dim);
        for db in [0.0, -6.0, -12.0, -24.0, -40.0] {
            let pos = db_to_fader(db, offset_db);
            if pos > 1.0 {
                continue;
            }
            let (len, stroke) = if db == 0.0 { (6.0, unity) } else { (4.0, tick) };
            let ty = cap_y(pos);
            p.hline(rect.left()..=rect.left() + len, ty, stroke);
            p.hline(rect.right() - len..=rect.right(), ty, stroke);
        }
        let cap = Rect::from_center_size(Pos2::new(rect.center().x, y), vec2(FADER_W - 4.0, CAP_H));
        let hot = resp.hovered() || resp.dragged();
        let body = if hot {
            theme::mix(mx.cap_body, Color32::WHITE, 0.15)
        } else {
            mx.cap_body
        };
        p.rect(
            cap,
            3.0,
            body,
            Stroke::new(1.0_f32, mx.cap_edge),
            StrokeKind::Inside,
        );
        p.hline(
            cap.left() + 5.0..=cap.right() - 5.0,
            cap.top() + 2.5,
            Stroke::new(1.0_f32, mx.cap_hi),
        );
        p.hline(
            cap.left() + 5.0..=cap.right() - 5.0,
            cap.center().y,
            Stroke::new(1.0_f32, mx.cap_line),
        );
    }
    lazy_tip(resp, || {
        format!(
            "{label}: {} dB ({:.0}%)\nDrag to change · Shift for fine · Double-click to reset",
            db_text(fader_db(v, offset_db)),
            v * 100.0
        )
    })
}

/// Recessed value box (a fader's dB or the master peak). The unit is dropped
/// when the box is too narrow for it.
fn paint_readout(ui: &Ui, look: Look, rect: Rect, value: &str, unit: &str, ink: Option<Color32>) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let mx = look.mx;
    let p = ui.painter();
    p.rect(
        rect,
        4.0,
        mx.well,
        Stroke::new(1.0_f32, mx.border_soft),
        StrokeKind::Inside,
    );
    let ink = ink.unwrap_or_else(|| theme::ink_on(mx.well));
    let dim = theme::mix(ink, mx.well, 0.4);
    let value = p.layout_no_wrap(value.to_owned(), theme::strong_font(ui, 11.0), ink);
    let unit = p.layout_no_wrap(unit.to_owned(), theme::body_font(10.0), dim);
    let room = rect.width() - 8.0;
    let with_unit = value.size().x + 3.0 + unit.size().x <= room;
    let total = if with_unit {
        value.size().x + 3.0 + unit.size().x
    } else {
        value.size().x.min(room)
    };
    let x = rect.center().x - total * 0.5;
    p.galley(
        pos2(x, rect.center().y - value.size().y * 0.5),
        value.clone(),
        ink,
    );
    if with_unit {
        p.galley(
            pos2(
                x + value.size().x + 3.0,
                rect.center().y - unit.size().y * 0.5 + 0.5,
            ),
            unit,
            dim,
        );
    }
}

/// Vertical segmented level meter on a dB scale, lit bottom-up with the
/// green -> amber -> red ramp, with the peak-hold tick painted as a bright
/// segment. Silent means every segment is dark.
fn paint_meter(p: &Painter, mx: &MixerPalette, rect: Rect, gauge: &Gauge) {
    p.rect_filled(rect, 2.0, mx.well);
    let inner = rect.shrink(1.0);
    let n = (((inner.height() + 1.0) / 4.0).floor() as usize).clamp(8, 64);
    let gap = 1.0;
    let seg_h = (inner.height() - gap * (n - 1) as f32) / n as f32;
    let lit = (meter_pos(gauge.level) * n as f32).round() as usize;
    let hold = (meter_pos(gauge.hold) * n as f32).round() as usize;
    for i in 0..n {
        let top = inner.bottom() - (i as f32 + 1.0) * seg_h - i as f32 * gap;
        let db_top = METER_FLOOR_DB * (1.0 - (i as f32 + 1.0) / n as f32);
        let color = if hold > 0 && i == hold - 1 {
            mx.meter_peak
        } else if i >= lit {
            mx.meter_off
        } else if db_top > -3.0 {
            mx.meter_high
        } else if db_top > -12.0 {
            mx.meter_mid
        } else {
            mx.meter_low
        };
        p.rect_filled(
            Rect::from_min_size(pos2(inner.left(), top), vec2(inner.width(), seg_h)),
            0.5,
            color,
        );
    }
}

fn paint_clip_led(p: &Painter, mx: &MixerPalette, rect: Rect, clip: bool) {
    p.rect_filled(rect, 1.5, if clip { mx.meter_clip } else { mx.meter_off });
}

/// One meter column: clip LED above the level bar. Clicking either clears the
/// hold tick and the latch. `label` names the meter in its tooltip.
fn meter_column(
    ui: &mut Ui,
    look: Look,
    id: egui::Id,
    led: Rect,
    bar: Rect,
    gauge: &mut Gauge,
    label: &str,
) {
    let hit = Rect::from_min_max(led.min, bar.max);
    let resp = ui.interact(hit, id, Sense::click());
    if resp.clicked() {
        gauge.reset();
    }
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, format!("{label} meter")));
    if ui.is_rect_visible(hit) {
        paint_clip_led(ui.painter(), &look.mx, led, gauge.clip);
        paint_meter(ui.painter(), &look.mx, bar, gauge);
    }
    let g = *gauge;
    lazy_tip(resp, || {
        format!(
            "{label}: {} dB   Peak {} dB{}\nClick to clear the peak and clip indicators",
            level_db_text(g.level),
            level_db_text(g.peak_max),
            if g.clip { "   CLIPPED" } else { "" }
        )
    });
}

/// Thin horizontal scrollbar for the strip row.
fn h_scrollbar(ui: &mut Ui, look: Look, rect: Rect, content: f32, viewport: f32, offset: &mut f32) {
    let mx = look.mx;
    let max = (content - viewport).max(0.0);
    *offset = offset.clamp(0.0, max);
    let resp = ui.interact(rect, egui::Id::new(SCROLL_KEY).with("bar"), Sense::drag());
    let thumb_w = (rect.width() * viewport / content.max(1.0)).clamp(32.0, rect.width());
    let travel = (rect.width() - thumb_w).max(1.0);
    if resp.dragged() || resp.is_pointer_button_down_on() {
        if let Some(pos) = resp.interact_pointer_pos() {
            *offset = ((pos.x - rect.left() - thumb_w * 0.5) / travel * max).clamp(0.0, max);
        }
    }
    let start = *offset / max.max(1.0) * travel;
    let thumb = Rect::from_min_size(
        pos2(rect.left() + start, rect.top()),
        vec2(thumb_w, rect.height()),
    );
    let p = ui.painter();
    p.rect_filled(rect, rect.height() * 0.5, mx.border_soft);
    let color = if resp.dragged() {
        mx.text_dim
    } else if resp.hovered() {
        mx.text_faint
    } else {
        mx.tick
    };
    p.rect_filled(thumb, rect.height() * 0.5, color);
}

pub(super) fn cutoff_to_norm(hz: f32) -> f32 {
    ((hz.clamp(MIN_CUTOFF_HZ, MAX_CUTOFF_HZ) / MIN_CUTOFF_HZ).ln()
        / (MAX_CUTOFF_HZ / MIN_CUTOFF_HZ).ln())
    .clamp(0.0, 1.0)
}

pub(super) fn norm_to_cutoff(n: f32) -> f32 {
    (MIN_CUTOFF_HZ * (MAX_CUTOFF_HZ / MIN_CUTOFF_HZ).powf(n.clamp(0.0, 1.0)))
        .clamp(MIN_CUTOFF_HZ, MAX_CUTOFF_HZ)
}

pub(super) fn hz_text(hz: f32) -> String {
    if hz >= MAX_CUTOFF_HZ - 1.0 {
        "Open".into()
    } else if hz >= 1000.0 {
        format!("{:.1}k", hz / 1000.0)
    } else {
        format!("{hz:.0}")
    }
}

/// "Master" or "Bus A": the route's short name.
fn route_label(bus: u8) -> String {
    if bus == 0 {
        "Master".to_owned()
    } else {
        format!("Bus {}", ops::bus_label(bus))
    }
}

/// The scalars and short labels a channel strip paints, copied out of the
/// project so the strip can edit it while painting.
struct ChannelView {
    id: u64,
    name: String,
    instrument: String,
    color: Color32,
    volume: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    bus: u8,
    fx: Effects,
    audible: bool,
    selected: bool,
}

impl CadenceApp {
    /// Filter / reverb / delay / drive knobs for a track (shared by mixer and inspector).
    pub(super) fn fx_knobs(&mut self, ui: &mut Ui, id: u64, size: f32, color: Color32) {
        let pal = self.palette();
        let Some(fx) = ops::track(&self.project, id).map(|t| t.effects.clone()) else {
            return;
        };
        let mut cut = cutoff_to_norm(fx.cutoff_hz);
        if theme::knob(
            ui,
            pal,
            &mut cut,
            1.0,
            false,
            "Filter",
            &hz_text(fx.cutoff_hz),
            color,
            size,
        )
        .changed()
        {
            let hz = norm_to_cutoff(cut);
            self.edit("Filter cutoff", Some(&format!("cut-{id}")), |p| {
                if let Some(t) = ops::track_mut(p, id) {
                    t.effects.cutoff_hz = hz;
                }
            });
        }
        for (label, value, default) in [
            ("Reverb", fx.reverb, 0.15),
            ("Delay", fx.delay, 0.0),
            ("Drive", fx.drive, 0.0),
        ] {
            let mut v = value;
            if theme::knob(
                ui,
                pal,
                &mut v,
                default,
                false,
                label,
                &format!("{:.0}%", value * 100.0),
                color,
                size,
            )
            .changed()
            {
                self.edit(label, Some(&format!("{label}-{id}")), |p| {
                    if let Some(t) = ops::track_mut(p, id) {
                        let v = v.clamp(0.0, 1.0);
                        match label {
                            "Reverb" => t.effects.reverb = v,
                            "Delay" => t.effects.delay = v,
                            _ => t.effects.drive = v,
                        }
                    }
                });
            }
        }
    }

    pub(super) fn mixer_ui(&mut self, ui: &mut Ui) {
        let look = Look {
            pal: self.palette(),
            mx: self.palette().mixer,
            compact: compact_mode(ui),
        };
        let (pal, mx) = (look.pal, look.mx);
        // The desk fills the whole tab below the tab row (the hosting card
        // keeps its rounded bottom corners); strips and toolbar sit inside a
        // 12/10 inset.
        let surface = ui.available_rect_before_wrap();
        let avail = surface.shrink2(vec2(12.0, 10.0));
        ui.painter().rect_filled(
            surface,
            CornerRadius {
                nw: 0,
                ne: 0,
                sw: 9,
                se: 9,
            },
            mx.panel,
        );
        let mut ui = ui.new_child(
            UiBuilder::new()
                .id_salt("mixer.desk")
                .max_rect(avail)
                .layout(Layout::left_to_right(Align::Min)),
        );
        mixer_widget_style(&mut ui, look);
        let ui = &mut ui;

        let meters_id = egui::Id::new(METERS_KEY);
        let mut meters: Meters = ui
            .data_mut(|d| d.remove_temp(meters_id))
            .unwrap_or_default();
        if let Some(engine) = &self.engine {
            meters.last_peaks = engine.mixer_peaks().or(meters.last_peaks);
        } else {
            meters.last_peaks = None;
        }
        let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1);
        meters.advance(&self.project, self.transport.peak, dt);

        // Slim toolbar above the strips: counts on the left, actions right.
        let tool_h = CONTROL_S;
        let tool = Rect::from_min_size(avail.min, vec2(avail.width(), tool_h));
        let mut tb = ui.new_child(
            UiBuilder::new()
                .id_salt("mixer.toolbar")
                .max_rect(tool)
                .layout(Layout::right_to_left(Align::Center)),
        );
        tb.spacing_mut().item_spacing.x = theme::TOOLBAR_GAP;
        if theme::toggle_button(
            &mut tb,
            pal,
            "Compact",
            look.compact,
            tool_h,
            "Narrower strips with tighter spacing",
        )
        .clicked()
        {
            ui.data_mut(|d| d.insert_temp(egui::Id::new(COMPACT_KEY), !look.compact));
        }
        if theme::text_button(
            &mut tb,
            pal,
            "Clear peaks",
            tool_h,
            "Clear every peak-hold tick and clip light",
        )
        .clicked()
        {
            meters.reset();
        }
        let used = tool.right() - tb.min_rect().left();
        let n = self.project.tracks.len();
        let caption = format!(
            "{n} track{} \u{b7} {} buses",
            if n == 1 { "" } else { "s" },
            ops::NUM_BUSES
        );
        theme::paint_elided(
            ui,
            tool.left_center(),
            Align2::LEFT_CENTER,
            &caption,
            theme::body_font(theme::TEXT_SMALL),
            mx.text_faint,
            (tool.width() - used - 8.0).max(0.0),
        );

        // Row geometry.
        let content = Rect::from_min_max(pos2(avail.left(), tool.bottom() + 8.0), avail.max);
        let master_w = look.master_w();
        let strips_rect = Rect::from_min_max(
            content.min,
            pos2(content.right() - master_w - 8.0, content.bottom()),
        );
        let fx_open: Vec<bool> = ui.data(|d| {
            self.project
                .tracks
                .iter()
                .map(|t| d.get_temp::<bool>(fx_key(t.id)).unwrap_or(false))
                .collect()
        });
        let widths: Vec<f32> = fx_open.iter().map(|&o| look.channel_w(o)).collect();
        let (placed, divider_x, total_w) = place_strips(look, &widths);
        let overflow = total_w > strips_rect.width() + 0.5;
        let strip_h = (content.height()
            - if overflow {
                SCROLLBAR_H + theme::GAP_S
            } else {
                0.0
            })
        .max(96.0);

        // Scrolling: the plain wheel (and a horizontal swipe) moves the row;
        // Alt+wheel is left for the faders and knobs.
        let scroll_id = egui::Id::new(SCROLL_KEY);
        let mut offset = ui.data(|d| d.get_temp::<f32>(scroll_id)).unwrap_or(0.0);
        let max_offset = (total_w - strips_rect.width()).max(0.0);
        if ui.rect_contains_pointer(strips_rect) {
            let (d, alt) = ui.input(|i| (i.raw_scroll_delta, i.modifiers.alt));
            if !alt {
                offset -= if d.x != 0.0 { d.x } else { d.y };
            }
        }
        if overflow {
            let bar = Rect::from_min_size(
                pos2(
                    strips_rect.left(),
                    strips_rect.top() + strip_h + theme::GAP_S,
                ),
                vec2(strips_rect.width(), SCROLLBAR_H),
            );
            h_scrollbar(ui, look, bar, total_w, strips_rect.width(), &mut offset);
        }
        offset = offset.clamp(0.0, max_offset);
        ui.data_mut(|d| d.insert_temp(scroll_id, offset));

        // Strips: only those inside the viewport are built.
        let mut strips = ui.new_child(
            UiBuilder::new()
                .id_salt("mixer.strips")
                .max_rect(strips_rect)
                .layout(Layout::left_to_right(Align::Min)),
        );
        strips.set_clip_rect(strips_rect.intersect(ui.clip_rect()));
        let origin = pos2(strips_rect.left() - offset, strips_rect.top());
        let visible = (offset - 8.0)..(offset + strips_rect.width() + 8.0);
        for pl in &placed {
            if pl.x + pl.w < visible.start || pl.x > visible.end {
                continue;
            }
            let rect = Rect::from_min_size(origin + vec2(pl.x, 0.0), vec2(pl.w, strip_h));
            match pl.slot {
                Slot::Track(i) => {
                    let id = self.project.tracks[i].id;
                    let mut gauge = meters.tracks.get(&id).copied().unwrap_or_default();
                    self.channel_strip(&mut strips, look, rect, i, fx_open[i], &mut gauge);
                    meters.tracks.insert(id, gauge);
                }
                Slot::Bus(i) => {
                    let mut gauge = meters.buses[i];
                    self.bus_strip(&mut strips, look, rect, i, &mut gauge);
                    meters.buses[i] = gauge;
                }
            }
        }
        if total_w > 0.0 && placed.len() > ops::NUM_BUSES {
            let x = origin.x + divider_x;
            if x > strips_rect.left() && x < strips_rect.right() {
                strips.painter().vline(
                    x,
                    strips_rect.top() + 8.0..=strips_rect.top() + strip_h - 8.0,
                    Stroke::new(1.0_f32, mx.border),
                );
            }
        }

        let master_rect = Rect::from_min_size(
            pos2(content.right() - master_w, content.top()),
            vec2(master_w, strip_h),
        );
        self.master_strip(ui, look, master_rect, &mut meters);

        if meters.any_active() {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        ui.data_mut(|d| d.insert_temp(meters_id, meters));
    }

    /// Strip body: fill, outline and the colored bar on top.
    fn strip_body(ui: &Ui, look: Look, rect: Rect, fill: Color32, outline: Color32, bar: Color32) {
        let r = look.radius();
        ui.painter().rect(
            rect,
            r,
            fill,
            Stroke::new(1.0_f32, outline),
            StrokeKind::Inside,
        );
        let ir = r.saturating_sub(1);
        ui.painter().rect_filled(
            Rect::from_min_size(
                rect.min + vec2(1.0, 1.0),
                vec2(rect.width() - 2.0, COLOR_BAR - 1.0),
            ),
            CornerRadius {
                nw: ir,
                ne: ir,
                sw: 0,
                se: 0,
            },
            bar,
        );
    }

    fn channel_strip(
        &mut self,
        ui: &mut Ui,
        look: Look,
        rect: Rect,
        index: usize,
        fx_open: bool,
        gauge: &mut Gauge,
    ) {
        let (pal, mx, compact) = (look.pal, look.mx, look.compact);
        let view = {
            let track = &self.project.tracks[index];
            ChannelView {
                id: track.id,
                name: track.name.clone(),
                instrument: ops::instrument_label(track).into_owned(),
                color: theme::rgb(track.color),
                volume: track.volume,
                pan: track.pan,
                mute: track.mute,
                solo: track.solo,
                bus: track.bus,
                fx: track.effects.clone(),
                audible: self.project.is_track_audible(index),
                selected: self.sel.track == Some(track.id),
            }
        };
        let id = view.id;
        let sid = egui::Id::new(("cadence.mixer.strip", id));
        let accent = legible(view.color, mx.strip, 2.0);
        let bg = ui.interact(rect, sid, Sense::click());
        if bg.clicked() {
            self.select_track(id);
        }
        let fill = if view.selected {
            theme::mix(mx.strip, view.color, 0.10)
        } else {
            mx.strip
        };
        Self::strip_body(
            ui,
            look,
            rect,
            fill,
            if view.selected {
                accent
            } else {
                mx.border_soft
            },
            view.color,
        );

        let m = look.margin();
        let inner = Rect::from_min_max(
            pos2(rect.left() + m, rect.top() + m.max(8.0)),
            pos2(rect.right() - m, rect.bottom() - m),
        );
        let mut y = inner.top();

        // Name (+ instrument underneath in the full density).
        let name_h = if compact { 18.0 } else { 20.0 };
        let name_rect = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), name_h));
        let name_resp = ui.interact(name_rect, sid.with("name"), Sense::click());
        if name_resp.clicked() {
            self.select_track(id);
        }
        let ink = if view.audible { mx.text } else { mx.text_faint };
        theme::paint_elided(
            ui,
            name_rect.left_center(),
            Align2::LEFT_CENTER,
            &view.name,
            theme::strong_font(ui, theme::TEXT_BODY),
            ink,
            name_rect.width(),
        );
        lazy_tip(name_resp, || format!("{}\n{}", view.name, view.instrument));
        y += name_h;
        if !compact {
            let inst_rect = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), 14.0));
            let inst_resp = ui.interact(inst_rect, sid.with("inst"), Sense::hover());
            theme::paint_elided(
                ui,
                inst_rect.left_center(),
                Align2::LEFT_CENTER,
                &view.instrument,
                theme::body_font(theme::TEXT_SMALL),
                mx.text_faint,
                inst_rect.width(),
            );
            lazy_tip(inst_resp, || view.instrument.clone());
            y += 14.0;
        }
        y += GAP_XS;

        // M / S / FX chips, then the output route.
        let row = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), CONTROL_S));
        let mut cui = ui.new_child(
            UiBuilder::new()
                .id_salt(("mixer.chips", id))
                .max_rect(row)
                .layout(Layout::left_to_right(Align::Center)),
        );
        cui.spacing_mut().item_spacing.x = GAP_XS;
        cui.spacing_mut().interact_size.y = CONTROL_S;
        let chip = vec2(CONTROL_S, CONTROL_S);
        if theme::chip(&mut cui, pal, "M", view.mute, pal.danger, chip, "Mute").clicked() {
            self.toggle_mute(id);
        }
        if theme::chip(&mut cui, pal, "S", view.solo, pal.warn, chip, "Solo").clicked() {
            self.toggle_solo(id);
        }
        if theme::chip(
            &mut cui,
            pal,
            "FX",
            fx_open,
            mx.pan,
            chip,
            "Effect pedalboard: drive, filter, delay and reverb",
        )
        .clicked()
        {
            ui.data_mut(|d| d.insert_temp(fx_key(id), !fx_open));
        }
        y += CONTROL_S + GAP_XS;
        let route_ui = if compact {
            let r = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), CONTROL_S));
            y += CONTROL_S + GAP_XS;
            let mut c = ui.new_child(
                UiBuilder::new()
                    .id_salt(("mixer.route", id))
                    .max_rect(r)
                    .layout(Layout::left_to_right(Align::Center)),
            );
            c.spacing_mut().interact_size.y = CONTROL_S;
            c
        } else {
            cui
        };
        self.route_combo(route_ui, id, view.bus);

        // Fader zone: value box + fader + meter + pan (+ FX grid / pedals).
        let zone = Rect::from_min_max(pos2(inner.left(), y), inner.max);
        let ro_w = look.readout_w();
        let col2 = inner.left() + ro_w + GAP_XS;
        let fader_rect = Rect::from_min_size(
            pos2(
                inner.left() + (ro_w - FADER_W) * 0.5,
                zone.top() + READOUT_H + GAP_XS,
            ),
            vec2(FADER_W, (zone.height() - READOUT_H - GAP_XS).max(40.0)),
        );
        let readout = Rect::from_min_size(zone.min, vec2(ro_w, READOUT_H));
        paint_readout(
            ui,
            look,
            readout,
            &db_text(fader_db(view.volume, 0.0)),
            "dB",
            None,
        );
        let mut vol = view.volume;
        if mixer_fader(
            ui,
            look,
            fader_rect,
            sid.with("fader"),
            &mut vol,
            0.7,
            0.0,
            view.color,
            &format!("{} volume", view.name),
        )
        .changed()
        {
            self.edit("Track volume", Some(&format!("vol-{id}")), |p| {
                if let Some(t) = ops::track_mut(p, id) {
                    t.volume = vol;
                }
            });
        }
        let bar = Rect::from_min_size(
            pos2(col2, fader_rect.top() + FADER_PAD),
            vec2(METER_W, fader_rect.height() - 2.0 * FADER_PAD),
        );
        let led = Rect::from_min_size(pos2(col2, fader_rect.top()), vec2(METER_W, LED_H));
        meter_column(
            ui,
            look,
            sid.with("meter"),
            led,
            bar,
            gauge,
            &format!("{} level", view.name),
        );

        // Right column: pan knob, then the FX knob grid or the pedalboard.
        let col3 = col2 + METER_W + GAP_XS;
        let col3_w = (inner.right() - col3).max(0.0);
        let ks: f32 = if compact {
            20.0
        } else if zone.height() >= 200.0 {
            34.0
        } else {
            28.0
        };
        let knob_w = ks.max(KNOB_MIN_W);
        let pan_x = if fx_open {
            col3 + (KNOB_COL_W - knob_w) * 0.5
        } else {
            col3 + (col3_w - knob_w) * 0.5
        };
        let mut kui = ui.new_child(
            UiBuilder::new()
                .id_salt(("mixer.pan", id))
                .max_rect(Rect::from_min_size(
                    pos2(pan_x, zone.top()),
                    vec2(knob_w, ks + 30.0),
                ))
                .layout(Layout::top_down(Align::Min)),
        );
        let mut pan = (view.pan + 1.0) / 2.0;
        if mixer_knob(
            &mut kui,
            look,
            &mut pan,
            0.5,
            true,
            "Pan",
            &sidebar::pan_text(view.pan),
            mx.pan,
            ks,
        )
        .changed()
        {
            let v = (pan * 2.0 - 1.0).clamp(-1.0, 1.0);
            self.edit("Track pan", Some(&format!("pan-{id}")), |p| {
                if let Some(t) = ops::track_mut(p, id) {
                    t.pan = if v.abs() < 0.02 { 0.0 } else { v };
                }
            });
        }
        if fx_open {
            let chain = Rect::from_min_size(
                pos2(col3 + KNOB_COL_W + GAP_XS, zone.top()),
                vec2(PEDAL_CHAIN_W, zone.height()),
            );
            self.fx_pedal_chain(ui, look, id, &view.fx, view.color, chain);
        } else if !compact && zone.height() >= 3.0 * (ks + 30.0) + 2.0 * GAP_XS {
            // The knob grid is the expanded-mode FX control; compact strips
            // and the open pedalboard cover effects instead.
            let grid_w = 2.0 * knob_w + GAP_XS;
            let gx = col3 + (col3_w - grid_w) * 0.5;
            let gy = zone.top() + ks + 30.0 + GAP_XS;
            let cells = [
                (Pedal::Filter, 0, 0),
                (Pedal::Delay, 1, 0),
                (Pedal::Reverb, 0, 1),
                (Pedal::Drive, 1, 1),
            ];
            for (pedal, cx, cy) in cells {
                let r = Rect::from_min_size(
                    pos2(
                        gx + cx as f32 * (knob_w + GAP_XS),
                        gy + cy as f32 * (ks + 30.0 + GAP_XS),
                    ),
                    vec2(knob_w, ks + 30.0),
                );
                self.fx_knob(ui, look, id, &view.fx, pedal, view.color, ks, r);
            }
        }

        if !view.audible {
            dim_strip(ui, look, rect);
        }
    }

    /// Output route selector: "Master" / "Bus A"… with the bus name in the list.
    fn route_combo(&mut self, mut ui: Ui, id: u64, bus: u8) {
        let width = ui.available_width().max(48.0);
        let mut choice = bus;
        let buses = &self.project.buses;
        let selected = RichText::new(route_label(bus)).size(theme::TEXT_SMALL);
        let out = theme::combo(("bus", id), selected, width).show_ui(&mut ui, |ui| {
            for b in 0..=ops::NUM_BUSES as u8 {
                let mut label = route_label(b);
                if b > 0 {
                    let name = &buses[(b - 1) as usize].name;
                    if *name != label {
                        label = format!("{label} \u{b7} {name}");
                    }
                }
                ui.selectable_value(&mut choice, b, label);
            }
        });
        let name = (bus > 0)
            .then(|| self.project.buses[(bus - 1) as usize].name.clone())
            .filter(|n| *n != route_label(bus));
        lazy_tip(out.response, || match name {
            Some(n) => format!("Output: {} ({n})", route_label(bus)),
            None => format!("Output: {}", route_label(bus)),
        });
        if choice != bus {
            self.edit("Track bus", Some(&format!("bus-{id}")), |p| {
                ops::set_track_bus(p, id, choice);
            });
        }
    }

    /// One effect knob (inline grid and pedals share this).
    #[allow(clippy::too_many_arguments)]
    fn fx_knob(
        &mut self,
        ui: &mut Ui,
        look: Look,
        id: u64,
        fx: &Effects,
        pedal: Pedal,
        color: Color32,
        size: f32,
        rect: Rect,
    ) {
        let mut kui = ui.new_child(
            UiBuilder::new()
                .id_salt(("mixer.fx", id, pedal as usize))
                .max_rect(rect)
                .layout(Layout::top_down(Align::Min)),
        );
        let (norm, default, _) = pedal.state(fx);
        let mut v = norm;
        let changed = mixer_knob(
            &mut kui,
            look,
            &mut v,
            default,
            false,
            pedal.label(),
            &pedal.value_text(fx),
            color,
            size,
        )
        .changed();
        if !changed {
            return;
        }
        match pedal {
            Pedal::Filter => {
                let hz = norm_to_cutoff(v);
                self.edit("Filter cutoff", Some(&format!("cut-{id}")), |p| {
                    if let Some(t) = ops::track_mut(p, id) {
                        t.effects.cutoff_hz = hz;
                    }
                });
            }
            Pedal::Reverb | Pedal::Delay | Pedal::Drive => {
                let label = pedal.label();
                let v = v.clamp(0.0, 1.0);
                self.edit(label, Some(&format!("{label}-{id}")), |p| {
                    if let Some(t) = ops::track_mut(p, id) {
                        match pedal {
                            Pedal::Reverb => t.effects.reverb = v,
                            Pedal::Delay => t.effects.delay = v,
                            _ => t.effects.drive = v,
                        }
                    }
                });
            }
        }
    }

    /// The strip's FX pedalboard: the four effects as a horizontal chain of
    /// pedal-like blocks in engine order, chevrons between them. `area` is
    /// the chain's rectangle (as tall as the fader zone).
    fn fx_pedal_chain(
        &mut self,
        ui: &mut Ui,
        look: Look,
        id: u64,
        fx: &Effects,
        color: Color32,
        area: Rect,
    ) {
        for (i, pedal) in Pedal::ALL.into_iter().enumerate() {
            let x = area.left() + i as f32 * (PEDAL_CELL_W + ARROW_W);
            if i > 0 {
                let c = pos2(x - ARROW_W * 0.5, area.center().y);
                let tip = pos2(c.x + 2.0, c.y);
                let stroke = Stroke::new(1.5_f32, look.mx.text_faint);
                let p = ui.painter();
                p.line_segment([pos2(c.x - 3.0, c.y - 5.0), tip], stroke);
                p.line_segment([tip, pos2(c.x - 3.0, c.y + 5.0)], stroke);
            }
            let cell = Rect::from_min_size(pos2(x, area.top()), vec2(PEDAL_CELL_W, area.height()));
            self.fx_pedal(ui, look, id, fx, pedal, color, cell);
        }
    }

    /// One pedal: rounded block with the effect name and a centered knob.
    /// Pedals with no audible amount (drive/delay/reverb ~0, filter open)
    /// are dimmed — the engine always runs all four, there is no bypass.
    #[allow(clippy::too_many_arguments)]
    fn fx_pedal(
        &mut self,
        ui: &mut Ui,
        look: Look,
        id: u64,
        fx: &Effects,
        pedal: Pedal,
        color: Color32,
        cell: Rect,
    ) {
        let mx = look.mx;
        let (_, _, active) = pedal.state(fx);
        let ph = (cell.height() - 4.0).clamp(48.0, PEDAL_MAX_H);
        let prect = Rect::from_center_size(cell.center(), vec2(PEDAL_W, ph));
        let (fill, edge) = if active {
            (mx.control, mx.border)
        } else {
            (mx.control.gamma_multiply(0.7), mx.border_soft)
        };
        ui.painter().rect(
            prect,
            8.0,
            fill,
            Stroke::new(1.0_f32, edge),
            StrokeKind::Inside,
        );
        // mixer_knob is at least 40 px wide and `size + 30` tall; pick a size
        // that always fits inside the pedal body.
        let inner = prect.shrink2(vec2(9.0, 6.0));
        let ksize = (inner.height() - 30.0).clamp(6.0, 48.0).min(inner.width());
        let krect = Rect::from_center_size(prect.center(), vec2(ksize.max(44.0), ksize + 30.0));
        self.fx_knob(ui, look, id, fx, pedal, color, ksize, krect);
    }

    /// One fixed subgroup strip: name, solo/mute, fader, meter and pan.
    /// Buses carry no instrument, effects or sends.
    fn bus_strip(&mut self, ui: &mut Ui, look: Look, rect: Rect, index: usize, gauge: &mut Gauge) {
        let (pal, mx, compact) = (look.pal, look.mx, look.compact);
        let num = index as u8 + 1;
        let (name, volume, pan, mute, solo) = {
            let b = &self.project.buses[index];
            (b.name.clone(), b.volume, b.pan, b.mute, b.solo)
        };
        let audible = self.project.is_bus_audible(num);
        let members = ops::bus_members(&self.project, num);
        let sid = egui::Id::new(("cadence.mixer.bus", index));
        Self::strip_body(ui, look, rect, mx.strip, mx.border_soft, mx.pan);

        let m = look.margin();
        let inner = Rect::from_min_max(
            pos2(rect.left() + m, rect.top() + m.max(8.0)),
            pos2(rect.right() - m, rect.bottom() - m),
        );
        let mut y = inner.top();

        // Inline rename; `edit` coalesces keystrokes under one group key.
        let name_rect = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), CONTROL_S));
        let mut nui = ui.new_child(
            UiBuilder::new()
                .id_salt(("mixer.bus.name", index))
                .max_rect(name_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        nui.spacing_mut().interact_size.y = CONTROL_S;
        let mut text = name.clone();
        let r = nui.add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(name_rect.width())
                .font(theme::strong_font(&nui, theme::TEXT_BODY))
                .vertical_align(Align::Center),
        );
        lazy_tip(r.clone(), || {
            format!(
                "{name}\n{members} track{} \u{b7} routed to Master\nEdit to rename",
                if members == 1 { "" } else { "s" }
            )
        });
        if r.changed() {
            self.edit("Bus name", Some(&format!("bname-{index}")), |p| {
                ops::rename_bus(p, index, &text);
            });
        }
        y += CONTROL_S + GAP_XS;

        // M / S chips and the member count.
        let row = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), CONTROL_S));
        let mut cui = ui.new_child(
            UiBuilder::new()
                .id_salt(("mixer.bus.chips", index))
                .max_rect(row)
                .layout(Layout::left_to_right(Align::Center)),
        );
        cui.spacing_mut().item_spacing.x = GAP_XS;
        let chip = vec2(CONTROL_S, CONTROL_S);
        if theme::chip(&mut cui, pal, "M", mute, pal.danger, chip, "Mute bus").clicked() {
            self.edit("Bus mute", Some(&format!("bmute-{index}")), |p| {
                p.buses[index].mute = !p.buses[index].mute;
            });
        }
        if theme::chip(&mut cui, pal, "S", solo, pal.warn, chip, "Solo bus").clicked() {
            self.edit("Bus solo", Some(&format!("bsolo-{index}")), |p| {
                p.buses[index].solo = !p.buses[index].solo;
            });
        }
        let used = cui.min_rect().width() + GAP_XS;
        if inner.width() - used > 36.0 {
            let caption = if members == 0 {
                "Empty".to_owned()
            } else {
                format!("{members} track{}", if members == 1 { "" } else { "s" })
            };
            theme::paint_elided(
                ui,
                pos2(row.right(), row.center().y),
                Align2::RIGHT_CENTER,
                &caption,
                theme::body_font(theme::TEXT_SMALL),
                mx.text_faint,
                inner.width() - used,
            );
        }
        y += CONTROL_S + GAP_XS;

        let zone = Rect::from_min_max(pos2(inner.left(), y), inner.max);
        let ro_w = look.readout_w();
        let col2 = inner.left() + ro_w + GAP_XS;
        let fader_rect = Rect::from_min_size(
            pos2(
                inner.left() + (ro_w - FADER_W) * 0.5,
                zone.top() + READOUT_H + GAP_XS,
            ),
            vec2(FADER_W, (zone.height() - READOUT_H - GAP_XS).max(40.0)),
        );
        paint_readout(
            ui,
            look,
            Rect::from_min_size(zone.min, vec2(ro_w, READOUT_H)),
            &db_text(fader_db(volume, 0.0)),
            "dB",
            None,
        );
        let mut vol = volume;
        if mixer_fader(
            ui,
            look,
            fader_rect,
            sid.with("fader"),
            &mut vol,
            0.8,
            0.0,
            mx.pan,
            &format!("{name} volume"),
        )
        .changed()
        {
            self.edit("Bus volume", Some(&format!("bvol-{index}")), |p| {
                p.buses[index].volume = vol.clamp(0.0, 1.0);
            });
        }
        meter_column(
            ui,
            look,
            sid.with("meter"),
            Rect::from_min_size(pos2(col2, fader_rect.top()), vec2(METER_W, LED_H)),
            Rect::from_min_size(
                pos2(col2, fader_rect.top() + FADER_PAD),
                vec2(METER_W, fader_rect.height() - 2.0 * FADER_PAD),
            ),
            gauge,
            &format!("{name} level"),
        );
        let col3 = col2 + METER_W + GAP_XS;
        let ks: f32 = if compact { 20.0 } else { 28.0 };
        let knob_w = ks.max(KNOB_MIN_W);
        let mut kui = ui.new_child(
            UiBuilder::new()
                .id_salt(("mixer.bus.pan", index))
                .max_rect(Rect::from_min_size(
                    pos2(col3 + ((inner.right() - col3) - knob_w) * 0.5, zone.top()),
                    vec2(knob_w, ks + 30.0),
                ))
                .layout(Layout::top_down(Align::Min)),
        );
        let mut p = (pan + 1.0) / 2.0;
        if mixer_knob(
            &mut kui,
            look,
            &mut p,
            0.5,
            true,
            "Pan",
            &sidebar::pan_text(pan),
            mx.pan,
            ks,
        )
        .changed()
        {
            let v = (p * 2.0 - 1.0).clamp(-1.0, 1.0);
            self.edit("Bus pan", Some(&format!("bpan-{index}")), |q| {
                q.buses[index].pan = if v.abs() < 0.02 { 0.0 } else { v };
            });
        }

        if !audible {
            dim_strip(ui, look, rect);
        }
    }

    /// The master strip: gain fader, stereo meter with a dB scale and the
    /// latched peak readout.
    fn master_strip(&mut self, ui: &mut Ui, look: Look, rect: Rect, meters: &mut Meters) {
        let (pal, mx, compact) = (look.pal, look.mx, look.compact);
        let sid = egui::Id::new("cadence.mixer.master");
        Self::strip_body(ui, look, rect, mx.master, mx.border, mx.text_faint);
        let m = look.margin();
        let inner = Rect::from_min_max(
            pos2(rect.left() + m, rect.top() + m.max(8.0)),
            pos2(rect.right() - m, rect.bottom() - m),
        );
        let mut y = inner.top();
        let title = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), 20.0));
        theme::paint_elided(
            ui,
            title.left_center(),
            Align2::LEFT_CENTER,
            "Master",
            theme::strong_font(ui, theme::TEXT_TITLE),
            mx.text,
            title.width(),
        );
        y += 20.0 + 14.0 + GAP_XS;
        let _ = pal;

        // Group: fader column | dB scale | meter L | meter R, centered.
        let ro_w = look.readout_w();
        let mw = if compact { 12.0 } else { 14.0 };
        let scale_w = 22.0;
        let meters_w = 2.0 * mw + 3.0;
        let group_w = ro_w + GAP_XS + scale_w + 2.0 + meters_w;
        let gx = inner.left() + (inner.width() - group_w) * 0.5;
        let zone = Rect::from_min_max(pos2(inner.left(), y), inner.max);

        let volume = self.project.master_volume;
        paint_readout(
            ui,
            look,
            Rect::from_min_size(pos2(gx, zone.top()), vec2(ro_w, READOUT_H)),
            &db_text(fader_db(volume, MASTER_TRIM_DB)),
            "dB",
            None,
        );
        let fader_rect = Rect::from_min_size(
            pos2(gx + (ro_w - FADER_W) * 0.5, zone.top() + READOUT_H + GAP_XS),
            vec2(FADER_W, (zone.height() - READOUT_H - GAP_XS).max(40.0)),
        );
        let mut vol = volume;
        if mixer_fader(
            ui,
            look,
            fader_rect,
            sid.with("fader"),
            &mut vol,
            0.8,
            MASTER_TRIM_DB,
            mx.meter_peak,
            "Master volume",
        )
        .changed()
        {
            self.edit("Master volume", Some("master"), |p| p.master_volume = vol);
        }

        // Stereo meter with the dB scale between fader and bars.
        let scale_x = gx + ro_w + GAP_XS;
        let bar_x = scale_x + scale_w + 2.0;
        let bar_top = fader_rect.top() + FADER_PAD;
        let bar_h = fader_rect.height() - 2.0 * FADER_PAD;
        for (i, label) in ["L", "R"].into_iter().enumerate() {
            let x = bar_x + i as f32 * (mw + 3.0);
            let mut gauge = meters.master[i];
            meter_column(
                ui,
                look,
                sid.with(("meter", i)),
                Rect::from_min_size(pos2(x, fader_rect.top()), vec2(mw, LED_H)),
                Rect::from_min_size(pos2(x, bar_top), vec2(mw, bar_h)),
                &mut gauge,
                &format!("Master {label}"),
            );
            meters.master[i] = gauge;
        }
        if ui.is_rect_visible(rect) {
            for db in [0.0_f32, -6.0, -12.0, -24.0, -48.0] {
                let ty = bar_top + bar_h * (1.0 - meter_pos(10f32.powf(db / 20.0)));
                let text = if db == 0.0 {
                    "0".to_owned()
                } else {
                    format!("{}", db as i32)
                };
                ui.painter().text(
                    pos2(scale_x + scale_w, ty),
                    Align2::RIGHT_CENTER,
                    text,
                    theme::body_font(theme::TEXT_KICKER),
                    mx.text_faint,
                );
            }
        }

        // Latched peak readout above the bars; click clears everything.
        let peak = meters.master_peak();
        let clipped = meters.master.iter().any(|g| g.clip);
        let peak_rect = Rect::from_min_size(
            pos2(scale_x, zone.top()),
            vec2(scale_w + 2.0 + meters_w, READOUT_H),
        );
        let resp = ui.interact(peak_rect, sid.with("peak"), Sense::click());
        if resp.clicked() {
            meters.reset();
        }
        paint_readout(
            ui,
            look,
            peak_rect,
            &level_db_text(peak),
            "dB",
            clipped.then_some(mx.meter_clip),
        );
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Master peak"));
        lazy_tip(resp, || {
            format!(
                "Master peak: {} dBFS{}\nClick to clear the peak and clip indicators",
                level_db_text(peak),
                if clipped {
                    "\nReached the -1 dBFS limiter ceiling"
                } else {
                    ""
                }
            )
        });
    }
}

/// Greys out the strip of an inaudible (muted or not-soloed) track or bus.
fn dim_strip(ui: &Ui, look: Look, rect: Rect) {
    ui.painter()
        .rect_filled(rect, look.radius(), look.mx.panel.gamma_multiply(0.55));
}

#[cfg(test)]
mod tests {
    use super::{
        compact_mode, db_text, db_to_fader, fader_db, fx_key, legible, lin_to_db, meter_pos,
        pedals_open, place_strips, Gauge, Look, Meters, Pedal, Slot, BUS_W, BUS_W_COMPACT,
        COMPACT_KEY, FADER_FLOOR_DB, HOLD_SECS, MASTER_TRIM_DB, MASTER_W, MASTER_W_COMPACT,
        RELEASE_PER_FRAME, STRIP_W, STRIP_W_COMPACT,
    };
    use crate::gui::theme::{self, ThemeChoice};
    use crate::gui::{ops, CadenceApp, Tab};
    use crate::model::Project;
    use egui::{vec2, Pos2, Rect};

    /// The pedal chain must follow the engine's FX order
    /// (drive -> filter -> delay -> reverb; see `Pedal`'s doc comment).
    #[test]
    fn pedal_order_matches_engine() {
        let order: Vec<&str> = Pedal::ALL.iter().map(|p| p.label()).collect();
        assert_eq!(order, ["Drive", "Filter", "Delay", "Reverb"]);
    }

    /// The density/pedalboard flags round-trip through egui temp data.
    #[test]
    fn temp_flags_round_trip() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(!compact_mode(ui));
                assert!(!pedals_open(ui, 7));
                ui.data_mut(|d| {
                    d.insert_temp(egui::Id::new(COMPACT_KEY), true);
                    d.insert_temp(fx_key(7), true);
                });
                assert!(compact_mode(ui));
                assert!(pedals_open(ui, 7));
                assert!(!pedals_open(ui, 8));
            });
        });
    }

    /// Every theme resolves a layered desk (panel under strips under the
    /// master, wells recessed, text clearly readable on strip bodies) and
    /// distinct meter colors.
    #[test]
    fn every_theme_has_a_layered_readable_desk() {
        for choice in ThemeChoice::ALL {
            let pal = choice.palette();
            let mx = pal.mixer;
            let lum = theme::relative_luminance;
            let dark = lum(mx.strip) < 0.4;
            if dark {
                assert!(
                    lum(mx.panel) < lum(mx.strip),
                    "{choice:?}: strips above panel"
                );
                assert!(lum(mx.strip) < lum(mx.master), "{choice:?}: master lighter");
            } else {
                assert!(lum(mx.panel) < lum(mx.strip), "{choice:?}: strips lighter");
            }
            assert!(
                lum(mx.well) < lum(mx.strip),
                "{choice:?}: wells are recessed below the strip"
            );
            for (name, fg) in [("text", mx.text), ("text_dim", mx.text_dim)] {
                assert!(
                    theme::contrast_ratio(fg, mx.strip) >= 4.5,
                    "{choice:?}: {name} on strip"
                );
            }
            assert!(theme::contrast_ratio(mx.text_faint, mx.strip) >= 3.5);
            assert_ne!(mx.meter_low, mx.meter_mid);
            assert_ne!(mx.meter_mid, mx.meter_high);
            assert!(
                theme::contrast_ratio(mx.meter_low, mx.well) >= 3.0
                    && theme::contrast_ratio(mx.meter_peak, mx.well) >= 3.0,
                "{choice:?}: lit segments read on the well"
            );
            assert!(
                lum(mx.cap_hi) > lum(mx.cap_body) && lum(mx.cap_body) > lum(mx.cap_edge),
                "{choice:?}: the cap reads as lit from above"
            );
        }
    }

    /// The density constants narrow every strip kind in compact mode and
    /// keep channels wider than buses. Compile-time facts.
    #[test]
    fn density_constants_shrink_compact_strips() {
        const {
            assert!(STRIP_W_COMPACT < STRIP_W);
            assert!(BUS_W_COMPACT < BUS_W);
            assert!(MASTER_W_COMPACT < MASTER_W);
            assert!(BUS_W < STRIP_W);
        }
    }

    #[test]
    fn db_math_round_trips() {
        assert_eq!(db_text(fader_db(0.0, 0.0)), "-\u{221e}");
        assert_eq!(db_text(fader_db(1.0, 0.0)), "0.0");
        assert_eq!(db_text(fader_db(0.7, 0.0)), "-6.2");
        // The master default (0.8) applies ~0 dB with the renderer's trim.
        assert_eq!(db_text(fader_db(0.8, MASTER_TRIM_DB)), "+0.2");
        for db in [0.0, -6.0, -12.0, -24.0, -40.0] {
            let back = fader_db(db_to_fader(db, 0.0), 0.0).unwrap();
            assert!((back - db).abs() < 1e-3, "{db} -> {back}");
        }
        assert!(fader_db(db_to_fader(FADER_FLOOR_DB - 1.0, 0.0), 0.0).is_none());
        assert_eq!(meter_pos(0.0), 0.0);
        assert_eq!(meter_pos(1.0), 1.0);
        assert_eq!(meter_pos(4.0), 1.0, "over-range pins to the top");
        assert!((meter_pos(10f32.powf(-30.0 / 20.0)) - 0.5).abs() < 1e-4);
        assert_eq!(lin_to_db(0.0), f32::NEG_INFINITY);
    }

    /// Meters rise instantly, release smoothly to exactly zero, hold a peak
    /// tick, and a clip latches until reset.
    #[test]
    fn gauge_ballistics() {
        let dt = 1.0 / 60.0;
        let fall = RELEASE_PER_FRAME;
        let mut g = Gauge::default();
        g.update(0.5, dt, fall, 1.0);
        assert_eq!(g.level, 0.5, "instant attack");
        assert_eq!(g.hold, 0.5);
        g.update(0.0, dt, fall, 1.0);
        assert!(g.level < 0.5 && g.level > 0.4, "smooth release");
        assert_eq!(g.hold, 0.5, "hold stays put at first");
        // Silence: the level reaches exactly zero and stays there.
        for _ in 0..120 {
            g.update(0.0, dt, fall, 1.0);
        }
        assert_eq!(g.level, 0.0, "falls to empty when silent");
        assert!(g.hold < 0.5, "the hold tick falls after {HOLD_SECS} s");
        for _ in 0..600 {
            g.update(0.0, dt, fall, 1.0);
        }
        assert!(!g.active(), "idle meters stop asking for repaints");
        assert_eq!(g.peak_max, 0.5);
        assert!(!g.clip);
        g.update(1.2, dt, fall, 1.0);
        assert!(g.clip, "over full scale latches the clip LED");
        for _ in 0..600 {
            g.update(0.0, dt, fall, 1.0);
        }
        assert!(g.clip, "the clip LED stays lit until cleared");
        g.reset();
        assert!(!g.clip);
        assert_eq!(g.peak_max, 0.0);
        g.update(f32::NAN, dt, fall, 1.0);
        assert_eq!(g.level, 0.0, "non-finite peaks read as silence");
    }

    /// Peaks are matched to tracks by ID and a track absent from the engine
    /// snapshot reads as silence; meters for deleted tracks are dropped.
    #[test]
    fn meters_follow_track_ids_and_drop_deleted_tracks() {
        let mut project = Project::demo();
        let mut m = Meters::default();
        m.advance(&project, [0.0, 0.0], 1.0 / 60.0);
        assert_eq!(m.tracks.len(), project.tracks.len());
        assert!(!m.any_active(), "no engine peaks: everything is empty");
        project.tracks.pop();
        m.advance(&project, [0.3, 0.2], 1.0 / 60.0);
        assert_eq!(m.tracks.len(), project.tracks.len());
        assert_eq!(m.master[0].level, 0.3);
        assert_eq!(m.master[1].level, 0.2);
        assert!(m.any_active());
        m.reset();
        assert_eq!(m.master_peak(), m.master[0].level.max(m.master[1].level));
    }

    /// `legible` only moves a color when it must, and always reaches the
    /// requested contrast.
    #[test]
    fn legible_reaches_the_contrast_floor() {
        let well = theme::ThemeChoice::Paper.palette().mixer.well;
        let dark_blue = egui::Color32::from_rgb(52, 88, 158);
        let fixed = legible(dark_blue, well, 3.0);
        assert!(theme::contrast_ratio(fixed, well) >= 3.0);
        let white = egui::Color32::WHITE;
        assert_eq!(legible(white, well, 3.0), white);
    }

    /// Strips never overlap, buses follow the channels after the divider and
    /// the row width is the sum of its parts.
    #[test]
    fn strip_layout_is_ordered_and_gapless() {
        for compact in [false, true] {
            let look = Look {
                pal: ThemeChoice::Graphite.palette(),
                mx: ThemeChoice::Graphite.palette().mixer,
                compact,
            };
            let widths = [
                look.channel_w(false),
                look.channel_w(true),
                look.channel_w(false),
            ];
            let (placed, divider, total) = place_strips(look, &widths);
            assert_eq!(placed.len(), 3 + ops::NUM_BUSES);
            for pair in placed.windows(2) {
                assert!(
                    pair[0].x + pair[0].w + look.strip_gap() <= pair[1].x + 0.01,
                    "{pair:?} overlap"
                );
            }
            let last = placed.last().unwrap();
            assert!((last.x + last.w - total).abs() < 0.01);
            assert_eq!(placed[3].slot, Slot::Bus(0));
            assert!(divider > placed[2].x + placed[2].w && divider < placed[3].x);
            assert!(widths[1] > widths[0], "the pedalboard widens its strip");
        }
    }

    fn input_sized(w: f32, h: f32) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w, h))),
            focused: true,
            ..Default::default()
        }
    }

    fn input() -> egui::RawInput {
        input_sized(1360.0, 860.0)
    }

    /// The mixer tab renders strips and the pedalboard for real, in both
    /// densities and every theme — headless frame driver, same as `gui::tests`.
    #[test]
    fn mixer_frames_smoke() {
        for choice in ThemeChoice::ALL {
            let ctx = egui::Context::default();
            let mut app = CadenceApp::headless(Project::demo());
            app.theme = choice;
            app.tab = Tab::Mixer;
            for _ in 0..2 {
                let _ = ctx.run(input(), |ctx| app.frame(ctx));
            }
            ctx.data_mut(|d| {
                d.insert_temp(egui::Id::new(COMPACT_KEY), true);
                for t in &app.project.tracks {
                    d.insert_temp(fx_key(t.id), true);
                }
            });
            for _ in 0..2 {
                let _ = ctx.run(input(), |ctx| app.frame(ctx));
            }
        }
    }

    /// Runs one mixer frame and returns the painted strip-body rects:
    /// `(channel+bus strip bodies, master strip bodies)`.
    fn painted_strips(
        app: &mut CadenceApp,
        ctx: &egui::Context,
        raw: egui::RawInput,
    ) -> (Vec<Rect>, Vec<Rect>) {
        let mx = app.palette().mixer;
        let full = ctx.run(raw, |ctx| app.frame(ctx));
        let mut strips = Vec::new();
        let mut masters = Vec::new();
        for cs in &full.shapes {
            let egui::Shape::Rect(rs) = &cs.shape else {
                continue;
            };
            if rs.fill == mx.strip {
                strips.push(rs.rect);
            } else if rs.fill == mx.master {
                masters.push(rs.rect);
            }
        }
        (strips, masters)
    }

    /// Every visible channel/bus paints exactly one strip body and the
    /// master one — in both densities — and compact strips are narrower.
    #[test]
    fn strips_paint_one_body_per_visible_strip_and_master() {
        let ctx = egui::Context::default();
        let mut app = CadenceApp::headless(Project::empty());
        app.tab = Tab::Mixer;
        // Keep the default first-track selection from tinting a strip body.
        app.sel.track = None;
        let tracks = app.project.tracks.len();

        let (strips, masters) = painted_strips(&mut app, &ctx, input());
        assert_eq!(
            strips.len(),
            tracks + ops::NUM_BUSES,
            "channel + bus bodies"
        );
        assert_eq!(masters.len(), 1, "one master body");
        assert!(
            strips.iter().any(|r| (r.width() - STRIP_W).abs() < 0.5),
            "expanded channels keep their full width"
        );
        let expanded = strips;

        ctx.data_mut(|d| d.insert_temp(egui::Id::new(COMPACT_KEY), true));
        let (strips, masters) = painted_strips(&mut app, &ctx, input());
        assert_eq!(strips.len(), tracks + ops::NUM_BUSES, "compact bodies");
        assert_eq!(masters.len(), 1, "compact master body");
        for (compact, expanded) in strips.iter().zip(&expanded) {
            assert!(
                compact.width() < expanded.width() - 0.5,
                "compact strip {} narrower than its expanded {}",
                compact.width(),
                expanded.width()
            );
        }
    }

    /// At the minimum and a large window, in both densities and every theme,
    /// strips and the master never overlap, never cross the desk's edges and
    /// the master keeps its slot. A long song scrolls instead of squeezing.
    #[test]
    fn strips_never_overlap_or_leave_the_desk() {
        for (w, h) in [(1100.0, 760.0), (1920.0, 1080.0)] {
            for choice in ThemeChoice::ALL {
                for compact in [false, true] {
                    let ctx = egui::Context::default();
                    let mut app = CadenceApp::headless(Project::demo());
                    app.theme = choice;
                    app.tab = Tab::Mixer;
                    app.sel.track = None;
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new(COMPACT_KEY), compact));
                    for _ in 0..2 {
                        let _ = painted_strips(&mut app, &ctx, input_sized(w, h));
                    }
                    let (strips, masters) = painted_strips(&mut app, &ctx, input_sized(w, h));
                    let ctx_desc = format!("{w}x{h} {choice:?} compact={compact}");
                    assert_eq!(masters.len(), 1, "{ctx_desc}: master");
                    let master = masters[0];
                    assert!(master.right() <= w, "{ctx_desc}: master inside the window");
                    assert!(master.height() >= 96.0, "{ctx_desc}: master height");
                    for (i, a) in strips.iter().enumerate() {
                        assert!(
                            a.right() <= master.left() + 1.0,
                            "{ctx_desc}: strip {i} vs master"
                        );
                        assert!(a.height() >= 96.0, "{ctx_desc}: strip {i} height");
                        for b in &strips[i + 1..] {
                            assert!(
                                !a.shrink(0.5).intersects(*b),
                                "{ctx_desc}: {a:?} overlaps {b:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The headless frame at 1100 px wide with a 15-track song builds only
    /// the strips inside the viewport.
    #[test]
    fn offscreen_strips_are_not_built() {
        let ctx = egui::Context::default();
        let song = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/songs/neon-circuits.cadence"
        ))
        .unwrap();
        let project: Project = serde_json::from_str(&song).unwrap();
        let tracks = project.tracks.len();
        let mut app = CadenceApp::headless(project);
        app.tab = Tab::Mixer;
        app.sel.track = None;
        for _ in 0..2 {
            let _ = painted_strips(&mut app, &ctx, input_sized(1100.0, 760.0));
        }
        let (strips, _) = painted_strips(&mut app, &ctx, input_sized(1100.0, 760.0));
        assert!(
            strips.len() < tracks + ops::NUM_BUSES,
            "{} of {} strips built",
            strips.len(),
            tracks + ops::NUM_BUSES
        );
        assert!(strips.len() >= 3);
    }
}
