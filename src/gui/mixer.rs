//! Mixer: one channel strip per track, the four fixed subgroup bus strips,
//! plus the master strip.
//!
//! The whole tab paints a fixed dark-grey "Luna" mixing surface: the mixer
//! palette below is deliberately independent of the app theme, so the desk
//! reads the same in Graphite, Midnight and Paper.

use std::f32::consts::PI;

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Painter, Pos2, Rect,
    Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, WidgetInfo, WidgetType,
};

use super::theme;
use super::{ops, sidebar, CadenceApp};
use crate::model::{Track, MAX_CUTOFF_HZ, MIN_CUTOFF_HZ};

const STRIP_W: f32 = 156.0;
/// Narrower channel strip used by the compact mixer density.
const STRIP_W_COMPACT: f32 = 110.0;
/// Width of one subgroup bus strip.
const BUS_W: f32 = 106.0;
const BUS_W_COMPACT: f32 = 96.0;
/// Master strip widths (slightly wider than a bus strip).
const MASTER_W: f32 = 150.0;
const MASTER_W_COMPACT: f32 = 120.0;
/// Interaction column of a fader (`mixer_fader`'s drag width).
const FADER_W: f32 = 34.0;
/// Width of the segmented VU column beside each fader.
const METER_W: f32 = 9.0;
/// Number of segments in a VU column.
const VU_SEGS: usize = 12;
/// Horizontal slot one FX pedal occupies in the strip's pedalboard row.
const PEDAL_CELL_W: f32 = 74.0;
/// Painted pedal body width (the cell is a little wider for spacing).
const PEDAL_W: f32 = 66.0;
/// Tallest the pedal body gets; shorter panels get shorter pedals.
const PEDAL_MAX_H: f32 = 116.0;
/// Width of the chevron painted between two pedals.
const ARROW_W: f32 = 10.0;
/// egui temp-data key for the compact-density toggle. Temp data lives for the
/// session only: it survives tab switches but is gone on restart, and it is
/// never written into the song file.
const COMPACT_KEY: &str = "cadence.mixer.compact";

// ----- Luna-style mixing surface: fixed dark-grey palette ------------------
// The desk is a layered stack of neutral greys (darkest panel, raised strip
// bodies, lightest master), with the app accent colors reserved for state
// (mute red, solo amber, pan blue) and meter/fader details.
const MIX_PANEL: Color32 = Color32::from_gray(30);
const MIX_STRIP: Color32 = Color32::from_gray(41);
const MIX_STRIP_HI: Color32 = Color32::from_gray(46);
const MIX_MASTER: Color32 = Color32::from_gray(48);
const MIX_BORDER: Color32 = Color32::from_gray(58);
const MIX_BORDER_SOFT: Color32 = Color32::from_gray(50);
/// Recessed groove (fader tracks, VU wells).
const MIX_WELL: Color32 = Color32::from_gray(21);
const MIX_CONTROL: Color32 = Color32::from_gray(53);
const MIX_CONTROL_HOVER: Color32 = Color32::from_gray(64);
const MIX_CONTROL_ACTIVE: Color32 = Color32::from_gray(74);
/// Unlit arc segment of a knob (a touch darker than the knob body).
const MIX_ARC: Color32 = Color32::from_gray(38);
/// Fader scale ticks.
const MIX_TICK: Color32 = Color32::from_gray(74);
/// Text selection inside mixer text edits.
const MIX_SELECT: Color32 = Color32::from_gray(86);
const MIX_TEXT: Color32 = Color32::from_gray(228);
const MIX_TEXT_DIM: Color32 = Color32::from_gray(160);
const MIX_TEXT_FAINT: Color32 = Color32::from_gray(112);
/// Logo blue: pan arcs, bus strips, secondary toggles.
const MIX_LINK: Color32 = Color32::from_rgb(148, 170, 255);
/// Record red: mute chips and clip state.
const MIX_DANGER: Color32 = Color32::from_rgb(255, 112, 100);
/// Solo amber.
const MIX_WARN: Color32 = Color32::from_rgb(255, 188, 84);
// Segmented VU: warm green -> amber -> red, like a hardware desk.
const METER_GREEN: Color32 = Color32::from_rgb(84, 196, 104);
const METER_YELLOW: Color32 = Color32::from_rgb(240, 198, 72);
const METER_RED: Color32 = Color32::from_rgb(242, 88, 70);
const METER_OFF: Color32 = Color32::from_gray(34);
/// Bright peak-hold cap painted over the top lit segment.
const METER_PEAK: Color32 = Color32::from_gray(238);
// Silver fader cap: body, lit top edge, centre groove, outer edge.
const CAP_BODY: Color32 = Color32::from_gray(190);
const CAP_HI: Color32 = Color32::from_gray(234);
const CAP_LINE: Color32 = Color32::from_gray(118);
const CAP_EDGE: Color32 = Color32::from_gray(92);

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
}

/// Session-only mixer flags kept in egui temp data (see [`COMPACT_KEY`]).
fn compact_mode(ui: &Ui) -> bool {
    ui.data(|d| d.get_temp::<bool>(egui::Id::new(COMPACT_KEY)))
        .unwrap_or(false)
}

fn fx_key(id: u64) -> egui::Id {
    egui::Id::new(("cadence.mixer.fx", id))
}

fn pedals_open(ui: &Ui, id: u64) -> bool {
    ui.data(|d| d.get_temp::<bool>(fx_key(id))).unwrap_or(false)
}

// ----- painted mixer widgets (self-contained Luna look) --------------------

/// Re-styles egui's own widgets (combos, text edits, their popups) inside the
/// mixer subtree so they sit on the dark surface in every app theme.
fn mixer_widget_style(ui: &mut Ui) {
    let v = &mut ui.style_mut().visuals;
    v.window_fill = MIX_STRIP_HI;
    v.window_stroke = Stroke::new(1.0_f32, MIX_BORDER);
    v.extreme_bg_color = MIX_WELL;
    v.menu_corner_radius = CornerRadius::same(6);
    v.selection.bg_fill = MIX_SELECT;
    v.selection.stroke = Stroke::new(1.0_f32, MIX_LINK);
    v.text_cursor.stroke = Stroke::new(2.0_f32, MIX_TEXT);
    let radius = CornerRadius::same(5);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = MIX_STRIP;
    w.noninteractive.weak_bg_fill = MIX_STRIP;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, MIX_BORDER_SOFT);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, MIX_TEXT_DIM);
    w.noninteractive.corner_radius = radius;
    w.inactive.bg_fill = MIX_CONTROL;
    w.inactive.weak_bg_fill = MIX_CONTROL;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, MIX_BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, MIX_TEXT);
    w.inactive.corner_radius = radius;
    w.hovered.bg_fill = MIX_CONTROL_HOVER;
    w.hovered.weak_bg_fill = MIX_CONTROL_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, MIX_TEXT_FAINT);
    w.hovered.fg_stroke = Stroke::new(1.5_f32, MIX_TEXT);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;
    w.active.bg_fill = MIX_CONTROL_ACTIVE;
    w.active.weak_bg_fill = MIX_CONTROL_ACTIVE;
    w.active.bg_stroke = Stroke::new(1.0_f32, MIX_LINK);
    w.active.fg_stroke = Stroke::new(1.5_f32, MIX_TEXT);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;
    w.open.bg_fill = MIX_CONTROL_HOVER;
    w.open.weak_bg_fill = MIX_CONTROL_HOVER;
    w.open.bg_stroke = Stroke::new(1.0_f32, MIX_BORDER);
    w.open.fg_stroke = Stroke::new(1.0_f32, MIX_TEXT);
    w.open.corner_radius = radius;
}

/// Compact state toggle (M/S/FX/Compact) in the mixer's grey palette.
fn mixer_chip(
    ui: &mut Ui,
    text: &str,
    on: bool,
    color: Color32,
    size: egui::Vec2,
    tooltip: &str,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let fill = if on {
            color
        } else if hovered {
            MIX_CONTROL_HOVER
        } else {
            MIX_CONTROL
        };
        let border = if on {
            color.gamma_multiply(0.85)
        } else if hovered {
            MIX_TEXT_FAINT
        } else {
            MIX_BORDER
        };
        ui.painter().rect(
            rect,
            4.0,
            fill,
            Stroke::new(1.0_f32, border),
            StrokeKind::Inside,
        );
        let fg = if on {
            theme::ink_on(color)
        } else if hovered {
            MIX_TEXT
        } else {
            MIX_TEXT_DIM
        };
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            text,
            FontId::proportional(11.5),
            fg,
        );
    }
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, tooltip));
    resp.on_hover_text(tooltip)
}

fn arc_points(center: Pos2, r: f32, from: f32, to: f32, n: usize) -> Vec<Pos2> {
    (0..=n)
        .map(|i| {
            let a = from + (to - from) * i as f32 / n as f32;
            center + vec2(a.cos(), a.sin()) * r
        })
        .collect()
}

/// Rotary knob with a value arc: dark circular body, recessed arc track, a
/// colored value arc from the start to the current value and a hairline
/// pointer. Interaction matches `theme::knob` (drag, Shift for fine,
/// double-click to reset, Alt+scroll).
#[allow(clippy::too_many_arguments)]
fn mixer_knob(
    ui: &mut Ui,
    norm: &mut f32,
    default: f32,
    bipolar: bool,
    label: &str,
    value_text: &str,
    color: Color32,
    size: f32,
) -> Response {
    let total = vec2(size.max(40.0), size + 30.0);
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
        p.circle_filled(
            center,
            r,
            if active {
                MIX_CONTROL_HOVER
            } else {
                MIX_CONTROL
            },
        );
        p.circle_stroke(center, r, Stroke::new(1.0_f32, MIX_BORDER));
        p.add(Shape::line(
            arc_points(center, r - 4.0, a0, a0 + sweep, 32),
            Stroke::new(2.0_f32, MIX_ARC),
        ));
        let (from, to) = if bipolar {
            (0.5f32.min(v), 0.5f32.max(v))
        } else {
            (0.0, v)
        };
        if to - from > 0.005 {
            p.add(Shape::line(
                arc_points(center, r - 4.0, a0 + sweep * from, a0 + sweep * to, 24),
                Stroke::new(2.0_f32, color),
            ));
        }
        let a = a0 + sweep * v;
        p.line_segment(
            [
                center + vec2(a.cos(), a.sin()) * (r * 0.3),
                center + vec2(a.cos(), a.sin()) * (r - 5.0),
            ],
            Stroke::new(1.5_f32, MIX_TEXT),
        );
        p.text(
            Pos2::new(center.x, rect.top() + size + 3.0),
            Align2::CENTER_TOP,
            value_text,
            FontId::proportional(11.0),
            if active { MIX_TEXT } else { MIX_TEXT_DIM },
        );
        p.text(
            Pos2::new(center.x, rect.top() + size + 16.0),
            Align2::CENTER_TOP,
            label,
            FontId::proportional(10.0),
            MIX_TEXT_FAINT,
        );
    }
    resp.on_hover_text(format!(
        "{label}: {value_text}\nDrag up/down to change · Shift for fine · Double-click to reset"
    ))
}

/// Fader with a recessed groove, scale ticks and a silver cap: rounded rect
/// body, lit top edge and a centre groove line. Drag sets the value,
/// double-click resets.
fn mixer_fader(
    ui: &mut Ui,
    value: &mut f32,
    default: f32,
    height: f32,
    color: Color32,
    label: &str,
) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(FADER_W, height), Sense::click_and_drag());
    let before = *value;
    let track = Rect::from_center_size(rect.center(), vec2(5.0, height - 16.0));
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
        p.rect(
            track,
            2.5,
            MIX_WELL,
            Stroke::new(1.0_f32, MIX_BORDER_SOFT),
            StrokeKind::Inside,
        );
        let y = track.bottom() - track.height() * v;
        p.rect_filled(
            Rect::from_min_max(Pos2::new(track.left(), y), track.right_bottom()),
            2.5,
            color.gamma_multiply(0.9),
        );
        for i in 0..=4 {
            let ty = track.bottom() - track.height() * i as f32 / 4.0;
            p.hline(
                rect.left()..=rect.left() + 4.0,
                ty,
                Stroke::new(1.0_f32, MIX_TICK),
            );
            p.hline(
                rect.right() - 4.0..=rect.right(),
                ty,
                Stroke::new(1.0_f32, MIX_TICK),
            );
        }
        // The silver cap rides the value position.
        let cap = Rect::from_center_size(Pos2::new(rect.center().x, y), vec2(FADER_W - 4.0, 11.0));
        let hot = resp.hovered() || resp.dragged();
        let body = if hot {
            theme::mix(CAP_BODY, Color32::WHITE, 0.15)
        } else {
            CAP_BODY
        };
        p.rect(
            cap,
            3.0,
            body,
            Stroke::new(1.0_f32, CAP_EDGE),
            StrokeKind::Inside,
        );
        p.hline(
            cap.left() + 5.0..=cap.right() - 5.0,
            cap.top() + 2.5,
            Stroke::new(1.0_f32, CAP_HI),
        );
        p.hline(
            cap.left() + 5.0..=cap.right() - 5.0,
            cap.center().y,
            Stroke::new(1.0_f32, CAP_LINE),
        );
    }
    resp.on_hover_text(format!(
        "{label}: {:.0}%\nDrag to change · Double-click to reset",
        v * 100.0
    ))
}

/// Vertical segmented VU meter, lit bottom-up with the warm green → amber →
/// red ramp. `level` is a linear 0..=1 amplitude. With `peak` the top lit
/// segment gets a bright hold cap (the GUI only has live peaks for the
/// master bus; channel strips light their segments from the fader setting).
fn vu_meter(p: &Painter, rect: Rect, level: f32, peak: bool) {
    p.rect_filled(rect, 1.0, MIX_WELL);
    let gap = 1.0;
    let sh = (rect.height() - gap * (VU_SEGS - 1) as f32) / VU_SEGS as f32;
    let lit = (level.clamp(0.0, 1.0) * VU_SEGS as f32).round() as usize;
    for i in 0..VU_SEGS {
        let top = rect.bottom() - (i as f32 + 1.0) * sh - i as f32 * gap;
        let color = if i >= lit {
            METER_OFF
        } else if i >= VU_SEGS - 2 {
            METER_RED
        } else if i >= VU_SEGS - 5 {
            METER_YELLOW
        } else {
            METER_GREEN
        };
        p.rect_filled(
            Rect::from_min_size(pos2(rect.left(), top), vec2(rect.width(), sh)),
            1.0,
            color,
        );
    }
    if peak && lit > 0 {
        let top = rect.bottom() - lit as f32 * sh - (lit as f32 - 1.0) * gap;
        p.rect_filled(
            Rect::from_min_size(pos2(rect.left(), top), vec2(rect.width(), sh)),
            1.0,
            METER_PEAK,
        );
    }
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
        // The mixing surface fills the whole tab (the hosting card keeps its
        // rounded corners); strips and toolbar sit inside a 12/10 inset.
        let surface = ui.available_rect_before_wrap();
        let avail = surface.shrink2(vec2(12.0, 10.0));
        ui.painter()
            .rect_filled(surface, CornerRadius::same(12), MIX_PANEL);
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(avail)
                .layout(Layout::left_to_right(Align::Min)),
        );
        mixer_widget_style(&mut ui);
        let ui = &mut ui;
        let compact = compact_mode(ui);

        // Slim toolbar above the strips: track count + density toggle.
        let tool_h = 24.0;
        {
            let mut tb = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_size(avail.min, vec2(avail.width(), tool_h)))
                    .layout(Layout::left_to_right(Align::Center)),
            );
            tb.spacing_mut().item_spacing.x = 8.0;
            let n = self.project.tracks.len();
            tb.label(
                RichText::new(format!("{n} track{}", if n == 1 { "" } else { "s" }))
                    .small()
                    .color(MIX_TEXT_FAINT),
            );
            tb.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if mixer_chip(
                    ui,
                    "Compact",
                    compact,
                    MIX_LINK,
                    vec2(64.0, 20.0),
                    "Narrower strips with tighter spacing",
                )
                .clicked()
                {
                    ui.data_mut(|d| d.insert_temp(egui::Id::new(COMPACT_KEY), !compact));
                }
            });
        }

        let content = Rect::from_min_max(pos2(avail.left(), avail.top() + tool_h + 6.0), avail.max);
        let h = content.height();
        // Both densities fill the panel edge to edge; compact only narrows
        // the strips and tightens the gaps — a dense rectangle grid.
        let strip_h = (h - 14.0).max(48.0);
        let master_w = if compact { MASTER_W_COMPACT } else { MASTER_W };
        let strips_rect = Rect::from_min_max(
            content.min,
            pos2(content.right() - master_w - 10.0, content.bottom()),
        );
        let mut strips = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(strips_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        egui::ScrollArea::horizontal()
            .id_salt("mixer_scroll")
            .auto_shrink([false, false])
            .show(&mut strips, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = if compact { 6.0 } else { 8.0 };
                    let ids: Vec<u64> = self.project.tracks.iter().map(|t| t.id).collect();
                    for id in ids {
                        self.channel_strip(ui, id, strip_h);
                    }
                    ui.separator();
                    for i in 0..ops::NUM_BUSES {
                        self.bus_strip(ui, i, strip_h);
                    }
                });
            });
        // The master strip tracks the strip height (identical in both modes).
        let master_top = content.top() + (content.height() - strip_h) * 0.5;
        let master_rect = Rect::from_min_max(
            pos2(content.right() - master_w, master_top),
            pos2(content.right(), master_top + strip_h),
        );
        let mut m = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(master_rect)
                .layout(Layout::top_down(Align::Center)),
        );
        self.master_strip(&mut m, strip_h);
    }

    fn channel_strip(&mut self, ui: &mut Ui, id: u64, height: f32) {
        let compact = compact_mode(ui);
        let fx_open = pedals_open(ui, id);
        let Some(track) = ops::track(&self.project, id).cloned() else {
            return;
        };
        let idx = ops::track_index(&self.project, id).unwrap_or(0);
        let audible = self.project.is_track_audible(idx);
        let color = theme::rgb(track.color);
        let selected = self.sel.track == Some(id);
        let margin = if compact { 4.0 } else { 8.0 };
        let gap = if compact { 2.0 } else { 4.0 };
        // The pedalboard grows the strip to fit meter + fader + pan knob +
        // four pedal cells + three arrows + the row gaps, plus frame margins.
        // Compact strips drop the inline FX knob grid (the pedalboard covers
        // effects).
        let strip_w = if fx_open {
            FADER_W
                + 44.0
                + METER_W
                + 4.0 * PEDAL_CELL_W
                + 3.0 * ARROW_W
                + 9.0 * gap
                + 14.0
                + 2.0 * margin
        } else if compact {
            STRIP_W_COMPACT
        } else {
            STRIP_W
        };
        let content_w = strip_w - 2.0 * margin;
        let frame = egui::Frame::new()
            .fill(if selected {
                theme::mix(MIX_STRIP, color, 0.08)
            } else {
                MIX_STRIP
            })
            .stroke(Stroke::new(
                1.0_f32,
                if selected { color } else { MIX_BORDER_SOFT },
            ))
            .corner_radius(if compact { 5 } else { 8 })
            .inner_margin(Margin::same(margin as i8));
        let resp = frame.show(ui, |ui| {
            ui.set_width(content_w);
            ui.set_height(height - 2.0 * margin);
            // The frame inherits the strip row's horizontal layout; force a
            // vertical stack so the header, route row and fader row sit one
            // under the other instead of side by side.
            ui.vertical(|ui| {
                let top = ui.cursor().min;
                ui.painter().rect_filled(
                    Rect::from_min_size(top - vec2(margin, margin), vec2(strip_w, 4.0)),
                    egui::CornerRadius {
                        nw: 8,
                        ne: 8,
                        sw: 0,
                        se: 0,
                    },
                    color,
                );
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    let name = egui::Label::new(
                        RichText::new(&track.name)
                            .size(12.5)
                            .color(if audible { MIX_TEXT } else { MIX_TEXT_FAINT })
                            .strong(),
                    )
                    .truncate()
                    .sense(Sense::click());
                    let name_w = (content_w - if compact { 48.0 } else { 60.0 }).max(24.0);
                    let name_h = if compact { 14.0 } else { 20.0 };
                    ui.allocate_ui(vec2(name_w, name_h), |ui| {
                        if ui
                            .add(name)
                            .on_hover_text(format!(
                                "{}\n{}",
                                track.name,
                                ops::instrument_label(&track)
                            ))
                            .clicked()
                        {
                            self.select_track(id);
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        let (cw, ch) = if compact { (18.0, 15.0) } else { (22.0, 20.0) };
                        if mixer_chip(ui, "S", track.solo, MIX_WARN, vec2(cw, ch), "Solo").clicked()
                        {
                            self.toggle_solo(id);
                        }
                        if mixer_chip(ui, "M", track.mute, MIX_DANGER, vec2(cw, ch), "Mute")
                            .clicked()
                        {
                            self.toggle_mute(id);
                        }
                    });
                });
                // Instrument label + pedalboard toggle + output-bus selector.
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    let (fw, fh) = if compact { (24.0, 14.0) } else { (30.0, 18.0) };
                    // The ComboBox button's real width is max(this, text +
                    // icon + padding) — reserve what it actually occupies so
                    // the row can't push the strip wider than `strip_w`.
                    let bw = if compact { 48.0 } else { 52.0 };
                    // Keep the row one item-spacing short of the strip width
                    // so the trailing cursor can never widen the strip.
                    let inst_w = (ui.available_width() - fw - bw - 2.0 * gap - 4.0).max(24.0);
                    let inst_h = if compact { 12.0 } else { 16.0 };
                    ui.allocate_ui(vec2(inst_w, inst_h), |ui| {
                        let full = ops::instrument_label(&track).into_owned();
                        ui.add(
                            egui::Label::new(
                                RichText::new(full.as_str()).small().color(MIX_TEXT_FAINT),
                            )
                            .truncate(),
                        )
                        .on_hover_text(full);
                    });
                    if mixer_chip(
                        ui,
                        "FX",
                        fx_open,
                        MIX_LINK,
                        vec2(fw, fh),
                        "Show the effect pedalboard",
                    )
                    .clicked()
                    {
                        ui.data_mut(|d| d.insert_temp(fx_key(id), !fx_open));
                    }
                    // Output route: "M" = straight to the master strip, a letter
                    // = that fixed subgroup bus.
                    let mut choice = track.bus;
                    egui::ComboBox::from_id_salt(("bus", id))
                        .width(bw)
                        .selected_text(ops::bus_label(choice))
                        .show_ui(ui, |ui| {
                            for b in 0..=ops::NUM_BUSES as u8 {
                                let label = if b == 0 {
                                    "M  Master".to_string()
                                } else {
                                    format!(
                                        "{}  {}",
                                        ops::bus_label(b),
                                        self.project.buses[(b - 1) as usize].name
                                    )
                                };
                                ui.selectable_value(&mut choice, b, label);
                            }
                        });
                    if choice != track.bus {
                        self.edit("Track bus", Some(&format!("bus-{id}")), |p| {
                            ops::set_track_bus(p, id, choice);
                        });
                    }
                });
                ui.add_space(if compact { 1.0 } else { 2.0 });
                let fader_h = (ui.available_height() - 20.0).clamp(60.0, 260.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.vertical(|ui| {
                        let mut vol = track.volume;
                        if mixer_fader(
                            ui,
                            &mut vol,
                            0.7,
                            fader_h,
                            color,
                            &format!("{} volume", track.name),
                        )
                        .changed()
                        {
                            self.edit("Track volume", Some(&format!("vol-{id}")), |p| {
                                if let Some(t) = ops::track_mut(p, id) {
                                    t.volume = vol;
                                }
                            });
                        }
                        ui.label(
                            RichText::new(format!("{:.0}%", track.volume * 100.0))
                                .small()
                                .color(MIX_TEXT_DIM),
                        );
                    });
                    // Segmented VU beside the fader. There is no per-track level
                    // plumbing from the audio thread, so the channel meter lights
                    // its segments from the strip's set level (0 when inaudible).
                    ui.vertical(|ui| {
                        ui.add_space(8.0);
                        let (mrect, mresp) =
                            ui.allocate_exact_size(vec2(METER_W, fader_h - 16.0), Sense::hover());
                        vu_meter(
                            ui.painter(),
                            mrect,
                            if audible { track.volume } else { 0.0 },
                            false,
                        );
                        mresp.on_hover_text(format!("Channel level {:.0}%", track.volume * 100.0));
                    });
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = if compact { 1.0 } else { 2.0 };
                        let ks = if compact {
                            20.0
                        } else if fader_h > 190.0 {
                            34.0
                        } else {
                            28.0
                        };
                        let mut pan = (track.pan + 1.0) / 2.0;
                        if mixer_knob(
                            ui,
                            &mut pan,
                            0.5,
                            true,
                            "Pan",
                            &sidebar::pan_text(track.pan),
                            MIX_LINK,
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
                        // The knob grid is the expanded-mode FX control; compact
                        // strips and the open pedalboard cover effects instead.
                        if !fx_open && !compact {
                            if fader_h > 150.0 {
                                egui::Grid::new(ui.id().with(("fx", id)))
                                    .spacing(vec2(1.0, 2.0))
                                    .show(ui, |ui| {
                                        ui.vertical(|ui| self.fx_knob_column(ui, id, ks, color, 0));
                                        ui.vertical(|ui| self.fx_knob_column(ui, id, ks, color, 1));
                                    });
                            } else {
                                ui.label(
                                    RichText::new("Enlarge the panel for effects")
                                        .small()
                                        .color(MIX_TEXT_FAINT),
                                );
                            }
                        }
                    });
                    if fx_open {
                        self.fx_pedal_chain(ui, &track, color, fader_h);
                    }
                });
            });
        });
        let r = resp.response.interact(Sense::click());
        if r.clicked() {
            self.select_track(id);
        }
        if !audible {
            ui.painter()
                .rect_filled(resp.response.rect, 8.0, Color32::from_black_alpha(70));
        }
    }

    /// Two effect knobs in a column: column 0 = filter + reverb, 1 = delay + drive.
    fn fx_knob_column(&mut self, ui: &mut Ui, id: u64, size: f32, color: Color32, col: usize) {
        let Some(fx) = ops::track(&self.project, id).map(|t| t.effects.clone()) else {
            return;
        };
        if col == 0 {
            let mut cut = cutoff_to_norm(fx.cutoff_hz);
            if mixer_knob(
                ui,
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
        }
        let items: &[(&str, f32, f32)] = if col == 0 {
            &[("Reverb", fx.reverb, 0.15)]
        } else {
            &[("Delay", fx.delay, 0.0), ("Drive", fx.drive, 0.0)]
        };
        for &(label, value, default) in items {
            let mut v = value;
            if mixer_knob(
                ui,
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

    /// The strip's FX pedalboard: the four effects as a horizontal chain of
    /// pedal-like blocks in engine order, chevrons between them. `h` is the
    /// fader height so pedals center against the channel controls.
    fn fx_pedal_chain(&mut self, ui: &mut Ui, track: &Track, color: Color32, h: f32) {
        for (i, pedal) in Pedal::ALL.into_iter().enumerate() {
            if i > 0 {
                let (rect, _) = ui.allocate_exact_size(vec2(ARROW_W, h), Sense::hover());
                let c = rect.center();
                let tip = pos2(c.x + 2.0, c.y);
                let stroke = Stroke::new(1.5_f32, MIX_TEXT_FAINT);
                let p = ui.painter();
                p.line_segment([pos2(c.x - 3.0, c.y - 5.0), tip], stroke);
                p.line_segment([tip, pos2(c.x - 3.0, c.y + 5.0)], stroke);
            }
            self.fx_pedal(ui, track, pedal, color, h);
        }
    }

    /// One pedal: rounded block with the effect name and a centered knob.
    /// Pedals with no audible amount (drive/delay/reverb ~0, filter open)
    /// are dimmed — the engine always runs all four, there is no bypass.
    fn fx_pedal(&mut self, ui: &mut Ui, track: &Track, pedal: Pedal, color: Color32, h: f32) {
        let id = track.id;
        let fx = &track.effects;
        let (norm, default, active) = match pedal {
            Pedal::Filter => (
                cutoff_to_norm(fx.cutoff_hz),
                1.0,
                fx.cutoff_hz < MAX_CUTOFF_HZ - 1.0,
            ),
            Pedal::Drive => (fx.drive, 0.0, fx.drive > 0.004),
            Pedal::Delay => (fx.delay, 0.0, fx.delay > 0.004),
            Pedal::Reverb => (fx.reverb, 0.15, fx.reverb > 0.004),
        };
        let (cell, _) = ui.allocate_exact_size(vec2(PEDAL_CELL_W, h), Sense::hover());
        // The knob inside carries the tooltip; the cell only reserves space.
        let value_text = match pedal {
            Pedal::Filter => hz_text(fx.cutoff_hz),
            _ => format!("{:.0}%", norm * 100.0),
        };
        let ph = (h - 4.0).clamp(48.0, PEDAL_MAX_H);
        let prect = Rect::from_center_size(cell.center(), vec2(PEDAL_W, ph));
        let (fill, edge) = if active {
            (MIX_CONTROL, MIX_BORDER)
        } else {
            (MIX_CONTROL.gamma_multiply(0.7), MIX_BORDER_SOFT)
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
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(krect)
                .layout(Layout::left_to_right(Align::Min))
                .id_salt(("mixer.fx", id, pedal as usize)),
        );
        let mut v = norm;
        let changed = mixer_knob(
            &mut child,
            &mut v,
            default,
            false,
            pedal.label(),
            &value_text,
            color,
            ksize,
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

    /// One fixed subgroup strip: name, solo/mute, fader + pan. Slimmer than
    /// a track strip — buses carry no instrument, effects or sends.
    fn bus_strip(&mut self, ui: &mut Ui, index: usize, height: f32) {
        let compact = compact_mode(ui);
        let num = index as u8 + 1;
        let bus = self.project.buses[index].clone();
        let audible = self.project.is_bus_audible(num);
        let members = ops::bus_members(&self.project, num);
        let margin = if compact { 4.0 } else { 6.0 };
        let gap = if compact { 2.0 } else { 3.0 };
        let strip_w = if compact { BUS_W_COMPACT } else { BUS_W };
        let content_w = strip_w - 2.0 * margin;
        let color = MIX_LINK;
        let frame = egui::Frame::new()
            .fill(MIX_STRIP)
            .stroke(Stroke::new(1.0_f32, MIX_BORDER_SOFT))
            .corner_radius(if compact { 5 } else { 8 })
            .inner_margin(Margin::same(margin as i8));
        let resp = frame.show(ui, |ui| {
            ui.set_width(content_w);
            ui.set_height(height - 2.0 * margin);
            // Force a vertical stack (the frame inherits the row's
            // horizontal layout — see `channel_strip`).
            ui.vertical(|ui| {
                let top = ui.cursor().min;
                ui.painter().rect_filled(
                    Rect::from_min_size(top - vec2(margin, margin), vec2(strip_w, 3.0)),
                    egui::CornerRadius {
                        nw: 8,
                        ne: 8,
                        sw: 0,
                        se: 0,
                    },
                    color,
                );
                // Inline rename; `edit` coalesces keystrokes under one group key.
                let mut name = bus.name.clone();
                let r = ui
                    .add(
                        egui::TextEdit::singleline(&mut name)
                            .desired_width(content_w - 2.0)
                            .font(egui::FontId::proportional(11.0)),
                    )
                    .on_hover_text(format!(
                        "{}\n{} track{}",
                        bus.name,
                        members,
                        if members == 1 { "" } else { "s" }
                    ));
                if r.changed() {
                    self.edit("Bus name", Some(&format!("bname-{index}")), |p| {
                        ops::rename_bus(p, index, &name);
                    });
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    let (cw, ch) = if compact { (16.0, 13.0) } else { (18.0, 16.0) };
                    if mixer_chip(ui, "S", bus.solo, MIX_WARN, vec2(cw, ch), "Solo bus").clicked() {
                        self.edit("Bus solo", Some(&format!("bsolo-{index}")), |p| {
                            p.buses[index].solo = !p.buses[index].solo;
                        });
                    }
                    if mixer_chip(ui, "M", bus.mute, MIX_DANGER, vec2(cw, ch), "Mute bus").clicked()
                    {
                        self.edit("Bus mute", Some(&format!("bmute-{index}")), |p| {
                            p.buses[index].mute = !p.buses[index].mute;
                        });
                    }
                });
                ui.add_space(1.0);
                let fader_h = (ui.available_height() - 18.0).clamp(56.0, 240.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.vertical(|ui| {
                        let mut vol = bus.volume;
                        if mixer_fader(
                            ui,
                            &mut vol,
                            0.7,
                            fader_h,
                            color,
                            &format!("{} volume", bus.name),
                        )
                        .changed()
                        {
                            self.edit("Bus volume", Some(&format!("bvol-{index}")), |p| {
                                p.buses[index].volume = vol.clamp(0.0, 1.0);
                            });
                        }
                        ui.label(
                            RichText::new(format!("{:.0}%", bus.volume * 100.0))
                                .small()
                                .color(MIX_TEXT_DIM),
                        );
                    });
                    // Bus level meter: segments light from the fader setting
                    // (no per-bus audio plumbing), dark when inaudible.
                    ui.vertical(|ui| {
                        ui.add_space(8.0);
                        let (mrect, mresp) =
                            ui.allocate_exact_size(vec2(METER_W, fader_h - 16.0), Sense::hover());
                        vu_meter(
                            ui.painter(),
                            mrect,
                            if audible { bus.volume } else { 0.0 },
                            false,
                        );
                        mresp.on_hover_text(format!("Bus level {:.0}%", bus.volume * 100.0));
                    });
                    ui.vertical(|ui| {
                        let mut pan = (bus.pan + 1.0) / 2.0;
                        let ks = if compact { 18.0 } else { 24.0 };
                        if mixer_knob(
                            ui,
                            &mut pan,
                            0.5,
                            true,
                            "Pan",
                            &sidebar::pan_text(bus.pan),
                            color,
                            ks,
                        )
                        .changed()
                        {
                            let v = (pan * 2.0 - 1.0).clamp(-1.0, 1.0);
                            self.edit("Bus pan", Some(&format!("bpan-{index}")), |p| {
                                p.buses[index].pan = if v.abs() < 0.02 { 0.0 } else { v };
                            });
                        }
                    });
                });
            });
        });
        if !audible {
            ui.painter()
                .rect_filled(resp.response.rect, 8.0, Color32::from_black_alpha(70));
        }
    }

    fn master_strip(&mut self, ui: &mut Ui, height: f32) {
        let compact = compact_mode(ui);
        let frame = egui::Frame::new()
            .fill(MIX_MASTER)
            .stroke(Stroke::new(1.0_f32, MIX_BORDER))
            .corner_radius(8)
            .inner_margin(Margin::same(8));
        frame.show(ui, |ui| {
            ui.set_width((if compact { MASTER_W_COMPACT } else { MASTER_W }) - 16.0);
            ui.set_height(height - 16.0);
            ui.label(RichText::new("Master").strong().color(MIX_TEXT));
            ui.label(
                RichText::new(format!("{} voices", self.transport.voices))
                    .small()
                    .color(MIX_TEXT_FAINT),
            );
            ui.add_space(4.0);
            let fader_h = (ui.available_height() - 24.0).clamp(60.0, 280.0);
            ui.horizontal_top(|ui| {
                let mut vol = self.project.master_volume;
                if mixer_fader(ui, &mut vol, 0.8, fader_h, MIX_TEXT, "Master volume").changed() {
                    self.edit("Master volume", Some("master"), |p| p.master_volume = vol);
                }
                // The master is the one bus with live audio peaks: two
                // segmented VUs with bright hold caps on the top lit segment.
                let (rect, resp) = ui.allocate_exact_size(vec2(24.0, fader_h), Sense::hover());
                let r = rect.shrink2(vec2(2.0, 7.0));
                let w = (r.width() - 3.0) / 2.0;
                vu_meter(
                    ui.painter(),
                    Rect::from_min_size(r.min, vec2(w, r.height())),
                    self.transport.peak[0],
                    true,
                );
                vu_meter(
                    ui.painter(),
                    Rect::from_min_size(r.min + vec2(w + 3.0, 0.0), vec2(w, r.height())),
                    self.transport.peak[1],
                    true,
                );
                resp.on_hover_text(format!(
                    "Master output peak  L {:.0}%  R {:.0}%",
                    self.transport.peak[0] * 100.0,
                    self.transport.peak[1] * 100.0
                ));
            });
            ui.label(
                RichText::new(format!("{:.0}%", self.project.master_volume * 100.0))
                    .small()
                    .color(MIX_TEXT_DIM),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        compact_mode, fx_key, pedals_open, Pedal, BUS_W, BUS_W_COMPACT, CAP_BODY, CAP_EDGE, CAP_HI,
        COMPACT_KEY, MASTER_W, MASTER_W_COMPACT, METER_GREEN, METER_RED, METER_YELLOW, MIX_BORDER,
        MIX_MASTER, MIX_PANEL, MIX_STRIP, MIX_TEXT, MIX_WELL, STRIP_W, STRIP_W_COMPACT,
    };
    use crate::gui::{ops, CadenceApp, Tab};
    use crate::model::Project;
    use egui::{vec2, Color32, Pos2, Rect};

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

    /// The mixing surface is a layered dark-grey desk: panel darkest, strips
    /// raised above it, master lightest; wells recessed below the panel and
    /// text clearly brighter than any surface.
    #[test]
    fn palette_is_a_layered_dark_surface() {
        let lum = |c: Color32| c.r();
        assert!(
            lum(MIX_PANEL) < lum(MIX_STRIP),
            "strips sit above the panel"
        );
        assert!(
            lum(MIX_STRIP) < lum(MIX_MASTER),
            "master strip reads lighter"
        );
        assert!(lum(MIX_WELL) < lum(MIX_PANEL), "fader wells are recessed");
        assert!(
            lum(MIX_BORDER) > lum(MIX_STRIP) && lum(MIX_BORDER) < lum(MIX_TEXT),
            "borders separate the surface from text"
        );
        assert!(lum(MIX_TEXT) > 200, "labels stay readable on the dark desk");
        assert_ne!(METER_GREEN, METER_YELLOW);
        assert_ne!(METER_YELLOW, METER_RED);
        assert!(
            CAP_HI.r() > CAP_BODY.r() && CAP_BODY.r() > CAP_EDGE.r(),
            "the silver cap reads as lit from above"
        );
    }

    /// The density constants narrow every strip kind in compact mode and
    /// keep the master wider than a channel, channels wider than buses.
    /// Compile-time facts, so they assert at compile time.
    #[test]
    fn density_constants_shrink_compact_strips() {
        const {
            assert!(STRIP_W_COMPACT < STRIP_W);
            assert!(BUS_W_COMPACT < BUS_W);
            assert!(MASTER_W_COMPACT < MASTER_W);
            assert!(BUS_W < STRIP_W);
        }
    }

    fn input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1360.0, 860.0))),
            focused: true,
            ..Default::default()
        }
    }

    /// The mixer tab renders strips and the pedalboard for real, in both
    /// densities — headless frame driver, same as `gui::tests`.
    #[test]
    fn mixer_frames_smoke() {
        let ctx = egui::Context::default();
        let mut app = CadenceApp::headless(Project::demo());
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

    /// Runs one mixer frame and returns the painted strip-body rects:
    /// `(channel+bus strip bodies, master strip bodies)`.
    fn painted_strips(app: &mut CadenceApp, ctx: &egui::Context) -> (Vec<Rect>, Vec<Rect>) {
        let full = ctx.run(input(), |ctx| app.frame(ctx));
        let mut strips = Vec::new();
        let mut masters = Vec::new();
        for cs in &full.shapes {
            let egui::Shape::Rect(rs) = &cs.shape else {
                continue;
            };
            if rs.fill == MIX_STRIP {
                strips.push(rs.rect);
            } else if rs.fill == MIX_MASTER {
                masters.push(rs.rect);
            }
        }
        (strips, masters)
    }

    /// Every channel and bus paints exactly one strip body and the master one
    /// — in both densities — and compact strips are narrower than expanded.
    #[test]
    fn strips_paint_one_body_per_track_bus_and_master() {
        let ctx = egui::Context::default();
        let mut app = CadenceApp::headless(Project::empty());
        app.tab = Tab::Mixer;
        // Keep the default first-track selection from tinting a strip body.
        app.sel.track = None;
        let tracks = app.project.tracks.len();

        let (strips, masters) = painted_strips(&mut app, &ctx);
        assert_eq!(
            strips.len(),
            tracks + ops::NUM_BUSES,
            "channel + bus bodies"
        );
        assert_eq!(masters.len(), 1, "one master body");
        assert!(
            strips.iter().any(|r| r.width() >= STRIP_W - 0.5),
            "expanded channels keep their full width"
        );

        let expanded = strips;

        ctx.data_mut(|d| {
            d.insert_temp(egui::Id::new(COMPACT_KEY), true);
        });
        let (strips, masters) = painted_strips(&mut app, &ctx);
        assert_eq!(
            strips.len(),
            tracks + ops::NUM_BUSES,
            "compact channel + bus bodies"
        );
        assert_eq!(masters.len(), 1, "compact master body");
        // Painted bodies include frame margins and widget chrome (the bus
        // selector's icon + padding), so compare against each strip's own
        // expanded width rather than the raw density constants.
        for (compact, expanded) in strips.iter().zip(&expanded) {
            assert!(
                compact.width() < expanded.width() - 0.5,
                "compact strip {} narrower than its expanded {}",
                compact.width(),
                expanded.width()
            );
        }
        // And a compact channel still fits inside the full-density width.
        assert!(
            strips[0].width() < STRIP_W - 0.5,
            "compact channel {} narrower than a full channel",
            strips[0].width()
        );
    }
}
