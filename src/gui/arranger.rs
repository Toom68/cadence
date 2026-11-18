//! Multi-track clip arranger: custom-painted timeline with its own
//! horizontal zoom/scroll, track headers, ruler seeking and clip editing.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use egui::{
    pos2, vec2, Align2, Color32, CursorIcon, FontId, Painter, PointerButton, Pos2, Rect, RichText,
    Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder,
};

use super::theme::{self, Palette};
use super::{ops, sidebar, CadenceApp, Focus, StatusKind, Tab};
use crate::model::{
    next_id, AudioClip, Automation, AutomationLane, ChordEvent, ChordQuality, Clip, Marker,
    Pattern, Project, Track, NOTE_NAMES,
};

pub(super) const HEADER_W: f32 = 200.0;
/// The ruler is three horizontal strips: the loop lane on top, the bar
/// numbers and section-marker flags in the middle, and the chord lane
/// along the bottom.
pub(super) const RULER_H: f32 = 40.0;
/// Thin strip at the top of the ruler where dragging defines the loop
/// region; the rest of the ruler keeps its click/drag-to-seek behavior.
pub(super) const LOOP_LANE_H: f32 = 10.0;
/// The middle strip of the ruler — bar numbers, marker flags and the
/// playhead tip. Sized so its bottom edge is exactly where the ruler's
/// bottom was before the chord lane existed.
const NUMBER_STRIP_H: f32 = 16.0;
/// Height of a section-marker flag chip, painted just under the loop lane.
const MARKER_CHIP_H: f32 = 12.5;
/// Height of a chord chip pill on the chord lane.
const CHORD_CHIP_H: f32 = 11.5;
pub(super) const LANE_H: f32 = 52.0;
/// Height of the automation strip a track gains below its clip lane while
/// its "Show automation" toggle is on.
const AUTO_LANE_H: f32 = 28.0;
const SCROLLBAR: f32 = 10.0;
const MIN_PPS: f32 = 0.5;
const MAX_PPS: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Snap {
    Bar,
    Beat,
    Step,
}

impl Snap {
    pub fn steps(self) -> f64 {
        match self {
            Snap::Bar => 16.0,
            Snap::Beat => 4.0,
            Snap::Step => 1.0,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Snap::Bar => "Bar",
            Snap::Beat => "Beat",
            Snap::Step => "1/16",
        }
    }
}

/// Maps song steps to screen x for the visible timeline.
#[derive(Clone, Copy, Debug)]
pub(super) struct Timeline {
    pub left: f32,
    pub pps: f32,
    pub scroll: f32,
}

impl Timeline {
    pub fn x(&self, step: f64) -> f32 {
        self.left + step as f32 * self.pps - self.scroll
    }
    pub fn step(&self, x: f32) -> f64 {
        ((x - self.left + self.scroll) / self.pps) as f64
    }
}

pub(super) fn snap_floor(step: f64, grid: f64) -> f64 {
    (step / grid).floor() * grid
}

pub(super) fn snap_round(step: f64, grid: f64) -> f64 {
    (step / grid).round() * grid
}

fn snap_ceil(step: f64, grid: f64) -> f64 {
    (step / grid).ceil() * grid
}

/// Track-list index a library drag-and-drop would insert at: the lane the
/// pointer is over (its track shifts down), clamped to `0..=track_count` —
/// one past the last lane is a plain append, above the first lane inserts
/// at 0.
fn drop_lane_index(y: f32, lanes_top: f32, scroll_y: f32, track_count: usize) -> usize {
    (((y - lanes_top + scroll_y) / LANE_H).floor() as i64).clamp(0, track_count as i64) as usize
}

/// Loop region from a ruler-lane drag: the touched span snaps outward to
/// `grid` (floor on the left, ceil on the right) and clamps into
/// `[0, total]`. `None` when the span is smaller than one grid cell, so a
/// plain click in the lane never creates a region.
fn loop_region_from_drag(anchor: f64, current: f64, grid: f64, total: f64) -> Option<(f64, f64)> {
    if grid <= 0.0 || !anchor.is_finite() || !current.is_finite() || !total.is_finite() {
        return None;
    }
    if (current - anchor).abs() < 1e-9 {
        return None;
    }
    let lo = snap_floor(anchor.min(current).max(0.0), grid);
    let hi = snap_ceil(anchor.max(current).min(total.max(0.0)), grid);
    (hi - lo >= grid).then_some((lo, hi))
}

/// The ✕ click target inside a painted loop band, right-aligned.
fn loop_clear_rect(band: Rect) -> Rect {
    let s = band.height().min(11.0);
    Rect::from_center_size(pos2(band.right() - 7.0, band.center().y), vec2(s, s))
}

/// Subtle tints cycled across the section bands between markers: four hues
/// at ~4% alpha, translucent enough to read as a wash on both themes.
fn section_band_tints() -> [Color32; 4] {
    [
        Color32::from_rgba_unmultiplied(255, 196, 90, 10), // amber
        Color32::from_rgba_unmultiplied(110, 185, 255, 10), // blue
        Color32::from_rgba_unmultiplied(140, 235, 160, 10), // green
        Color32::from_rgba_unmultiplied(228, 140, 235, 10), // violet
    ]
}

/// Step ranges of the section bands painted between consecutive markers:
/// song start to the first marker, marker to marker, then the last marker
/// to the song end. Markers are sorted by step (then id) after
/// [`Project::normalize`], so the caller's list is already in order; steps
/// are clamped into the song and near-duplicates collapse so no zero-width
/// band is produced.
fn section_band_ranges(markers: &[Marker], song_steps: f64) -> Vec<(f64, f64)> {
    let total = song_steps.max(0.0);
    if markers.is_empty() {
        return Vec::new();
    }
    let mut bounds: Vec<f64> = markers.iter().map(|m| m.step.clamp(0.0, total)).collect();
    bounds.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let mut ranges = Vec::with_capacity(bounds.len() + 1);
    let mut prev = 0.0;
    for b in bounds {
        if b - prev > 1e-9 {
            ranges.push((prev, b));
        }
        prev = b.max(prev);
    }
    if total - prev > 1e-9 {
        ranges.push((prev, total));
    }
    ranges
}

/// 1-indexed bar count needed for the song to contain `step` (the bar the
/// step lands in) — the "set song end here" math.
fn bars_holding_step(step: f64) -> u32 {
    (step.max(0.0) / ops::STEPS_PER_BAR as f64).ceil().max(1.0) as u32
}

/// The clip color for an inaudible track: same hue at reduced opacity so
/// the lane shows through. RGB is preserved, so text ink stays readable.
fn dimmed(color: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (color.a() as f32 * alpha.clamp(0.0, 1.0)).round() as u8,
    )
}

/// Screen y for an automation `value` inside `rect`: volume multipliers run
/// [`crate::model::MAX_AUTOMATION_GAIN`]..0 top-to-bottom like a fader, pan
/// offsets +1..-1. `rect` is the strip's *inner* area (its top/bottom map to
/// the value range's ends).
fn auto_value_to_y(lane: AutomationLane, value: f32, rect: Rect) -> f32 {
    let (lo, hi) = Automation::value_range(lane);
    let span = (hi - lo).max(f32::EPSILON);
    let t = ((value - lo) / span).clamp(0.0, 1.0);
    rect.bottom() - t * rect.height()
}

/// Inverse of [`auto_value_to_y`]: the lane value a pointer y writes.
fn auto_y_to_value(lane: AutomationLane, y: f32, rect: Rect) -> f32 {
    let (lo, hi) = Automation::value_range(lane);
    if rect.height() <= 0.0 {
        return Automation::neutral(lane);
    }
    let t = ((rect.bottom() - y) / rect.height()).clamp(0.0, 1.0);
    lo + (hi - lo) * t
}

/// The floating drag label for an automation point — "Vol ×0.85",
/// "Pan +0.30".
fn auto_point_label(lane: AutomationLane, value: f32) -> String {
    match lane {
        AutomationLane::Volume => format!("Vol ×{value:.2}"),
        AutomationLane::Pan => format!("Pan {value:+.2}"),
    }
}

#[derive(Debug, Clone)]
struct ClipDrag {
    track: u64,
    clip: u64,
    grab: f64,
    origin: f64,
    copy: bool,
    copied: bool,
    copy_failed: bool,
    moved: bool,
    originals: Vec<(u64, Clip)>,
    /// The selection's imported-WAV clips, riding the same gesture
    /// ([`ops::mixed_group_delta`] clamps the whole group's move together).
    originals_audio: Vec<(u64, AudioClip)>,
    /// Undo coalescing key captured at press: `clip` is reassigned to the new
    /// copy's id on the first Alt-copy frame, so the key cannot be derived
    /// from it per frame without splitting one gesture into two undo steps.
    group: String,
}

#[derive(Debug, Clone)]
struct ArrangerMarquee {
    start: Pos2,
    current: Pos2,
    base: BTreeSet<u64>,
    additive: bool,
}

/// Which end of a clip a corner fade handle belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FadeEdge {
    /// Fade-in handle at the clip's left edge.
    In,
    /// Fade-out handle at the clip's right edge.
    Out,
}

/// An in-progress pointer gesture on the arranger grid.
#[derive(Debug, Clone)]
enum ArrangerDrag {
    /// Dragging a whole clip to a new start (Alt copies it).
    Clip(ClipDrag),
    /// Dragging a corner fade handle; the fade length follows the pointer.
    Fade {
        track: u64,
        clip: u64,
        edge: FadeEdge,
    },
    /// Dragging the right-edge loop handle to change the loop count.
    Loop {
        track: u64,
        clip: u64,
        /// Pointer step when the press landed (song steps, scroll-independent).
        grab_step: f64,
        origin_loops: u32,
    },
    /// Dragging a section-marker flag along the ruler. Owned by the ruler
    /// input handler (the grid handler ignores it).
    Marker {
        id: u64,
        /// Undo coalescing key captured at press.
        group: String,
    },
    /// Dragging a chord chip along the ruler's chord lane. Owned by the
    /// ruler input handler (the grid handler ignores it).
    Chord {
        id: u64,
        /// Undo coalescing key captured at press.
        group: String,
        /// True once the chip actually changed bar — a press that never
        /// moved reads as a click and cycles the chord instead.
        moved: bool,
    },
    /// Dragging (or click-placing) an automation point on a track's open
    /// automation strip. `index` is the point's slot inside its sorted lane
    /// and shifts when a move re-sorts it; `group` coalesces the whole
    /// gesture — including the initial add — into one undo step.
    Automation {
        track: u64,
        param: AutomationLane,
        index: usize,
        group: String,
    },
}

impl ArrangerDrag {
    /// True once the gesture has actually changed something (or, for the
    /// corner/edge handles, while the button is held on them).
    fn moved(&self) -> bool {
        match self {
            Self::Clip(d) => d.moved,
            Self::Chord { moved, .. } => *moved,
            Self::Fade { .. }
            | Self::Loop { .. }
            | Self::Marker { .. }
            | Self::Automation { .. } => true,
        }
    }
}

/// Inline rename of a section marker: the flag was double-clicked and a small
/// popup text field edits `text` until Enter commits or Escape cancels.
#[derive(Debug, Clone)]
struct MarkerRename {
    id: u64,
    text: String,
}

#[derive(Debug, Clone, Copy)]
enum ContextTarget {
    /// `step` is the raw pointer step where the menu was opened — the
    /// "Split here" cut position. `audio` marks an imported-WAV clip, whose
    /// menu drops the pattern-only entries (edit pattern, split, unique copy).
    Clip {
        track: u64,
        clip: u64,
        step: f64,
        audio: bool,
    },
    Lane {
        track: u64,
        step: f64,
    },
    /// Right-click on the ruler's chord lane at bar-aligned `step` — opens
    /// the "Add chord" menu via the ruler's own `context_menu` call.
    ChordLane {
        step: f64,
    },
    /// Right-click on the ruler's bare seek strip (neither a marker chip nor
    /// the chord lane): opens the structure menu at the clicked step.
    Ruler {
        step: f64,
    },
    /// Right-click on an open automation strip's empty area (a point under
    /// the pointer is deleted outright, like a marker) — opens the strip's
    /// clear/hide menu.
    AutoStrip {
        track: u64,
        param: AutomationLane,
    },
}

#[derive(Debug)]
pub(super) struct ArrangerView {
    pub px_per_step: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub snap: Snap,
    pub follow: bool,
    pub fit_request: bool,
    pub reveal_track: Option<u64>,
    drag: Option<ArrangerDrag>,
    marquee: Option<ArrangerMarquee>,
    last_seek: Option<f64>,
    /// Anchor step of an in-progress loop-region drag on the ruler's top
    /// lane (`None` = no lane gesture active).
    region_press: Option<f64>,
    context: Option<ContextTarget>,
    marker_rename: Option<MarkerRename>,
    grid_width: f32,
    /// Last frame's timeline grid rect (also used by headless tests).
    pub(super) grid: Rect,
    /// Tracks with their automation strip open, mapped to the lane the
    /// strip edits. View state only — it lives in the arranger, not the
    /// project, so opening a lane never dirties the song.
    pub(super) auto_lanes: HashMap<u64, AutomationLane>,
}

impl Default for ArrangerView {
    fn default() -> Self {
        Self {
            px_per_step: 4.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            snap: Snap::Bar,
            follow: true,
            fit_request: true,
            reveal_track: None,
            drag: None,
            marquee: None,
            last_seek: None,
            region_press: None,
            context: None,
            marker_rename: None,
            grid_width: 800.0,
            grid: Rect::NOTHING,
            auto_lanes: HashMap::new(),
        }
    }
}

impl ArrangerView {
    /// Zooms horizontally keeping the step under `anchor` (grid-relative x) fixed.
    pub fn zoom_by(&mut self, factor: f32, anchor: Option<f32>) {
        let anchor = anchor.unwrap_or(self.grid_width * 0.5);
        let step_at = (anchor + self.scroll_x) / self.px_per_step;
        self.px_per_step = (self.px_per_step * factor).clamp(MIN_PPS, MAX_PPS);
        self.scroll_x = (step_at * self.px_per_step - anchor).max(0.0);
    }

    pub fn end_gesture(&mut self) {
        self.drag = None;
        self.marquee = None;
        self.last_seek = None;
        self.region_press = None;
    }

    /// Drops gesture and context state without touching zoom/scroll — used
    /// when the whole project is replaced mid-gesture.
    pub fn reset_gesture(&mut self) {
        self.end_gesture();
        self.context = None;
        self.reveal_track = None;
        self.marker_rename = None;
        // Open automation strips name tracks by id: a different project reuses
        // ids for different tracks, so the expansion state cannot carry over.
        self.auto_lanes.clear();
    }

    fn timeline(&self, grid: Rect) -> Timeline {
        Timeline {
            left: grid.left(),
            pps: self.px_per_step,
            scroll: self.scroll_x,
        }
    }
}

enum Hit {
    None,
    Lane {
        track: u64,
        step: f64,
    },
    Clip {
        track: u64,
        clip: u64,
        /// `Some` for pattern clips (which pattern to show in the editor);
        /// `None` for imported-WAV clips, which have no editor content.
        pattern: Option<u64>,
        start: f64,
        step: f64,
    },
    /// A corner fade handle in the clip's top band.
    ClipFade {
        track: u64,
        clip: u64,
        edge: FadeEdge,
    },
    /// The right-edge loop-count handle below the top band.
    ClipLoop {
        track: u64,
        clip: u64,
    },
    /// Inside a track's open automation strip. `point` is the grabbed
    /// breakpoint (the nearest one within a few pixels of the pointer), and
    /// `step`/`value` are the raw pointer's write position — the press
    /// handler adds a point there when `point` is `None`, or drags the hit
    /// point when it is `Some`.
    Automation {
        track: u64,
        param: AutomationLane,
        point: Option<usize>,
        step: f64,
        value: f32,
    },
}

impl CadenceApp {
    pub(super) fn arranger_ui(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        self.arranger_toolbar(ui);
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        if full.width() < 300.0 || full.height() < 60.0 {
            return;
        }
        let grid = Rect::from_min_max(
            pos2(full.left() + HEADER_W, full.top() + RULER_H),
            pos2(full.right() - SCROLLBAR, full.bottom() - SCROLLBAR),
        );
        let ruler = Rect::from_min_max(
            pos2(grid.left(), full.top()),
            pos2(grid.right(), grid.top()),
        );
        let header = Rect::from_min_max(
            pos2(full.left(), grid.top()),
            pos2(grid.left(), grid.bottom()),
        );
        let hbar = Rect::from_min_max(
            pos2(grid.left(), grid.bottom()),
            pos2(grid.right(), full.bottom()),
        );
        let vbar = Rect::from_min_max(
            pos2(grid.right(), grid.top()),
            pos2(full.right(), grid.bottom()),
        );
        self.arranger.grid_width = grid.width();
        self.arranger.grid = grid;

        let song_steps = self.project.total_steps();
        let content_steps = (song_steps.max(ops::content_end(&self.project)) + 8.0 * 16.0)
            .min(ops::MAX_SONG_STEPS + 16.0);
        if self.arranger.fit_request {
            self.arranger.fit_request = false;
            self.arranger.px_per_step =
                ((grid.width() - 24.0) / song_steps.max(16.0) as f32).clamp(MIN_PPS, MAX_PPS);
            self.arranger.scroll_x = 0.0;
        }
        let n = self.project.tracks.len();
        let content_w = content_steps as f32 * self.arranger.px_per_step;
        let content_h = self.lane_top(n) + 44.0;

        // Wheel: scroll lanes vertically, Shift/trackpad horizontally, Cmd/Ctrl zooms.
        if ui.rect_contains_pointer(full) {
            let (scroll, zoom, hover) =
                ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
            if zoom != 1.0 {
                let anchor = hover.map(|h| (h.x - grid.left()).max(0.0));
                self.arranger.zoom_by(zoom, anchor);
            }
            if scroll != egui::Vec2::ZERO {
                self.arranger.scroll_x -= scroll.x;
                self.arranger.scroll_y -= scroll.y;
                ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
            }
        }
        if let Some(id) = self.arranger.reveal_track.take() {
            if let Some(i) = ops::track_index(&self.project, id) {
                let top = self.lane_top(i);
                let bottom = top + self.track_row_height(id);
                if top < self.arranger.scroll_y || bottom > self.arranger.scroll_y + grid.height() {
                    self.arranger.scroll_y = bottom - grid.height() + 8.0;
                }
            }
        }
        self.follow_playhead(grid);
        self.arranger.scroll_x = self
            .arranger
            .scroll_x
            .clamp(0.0, (content_w - grid.width()).max(0.0));
        self.arranger.scroll_y = self
            .arranger
            .scroll_y
            .clamp(0.0, (content_h - grid.height()).max(0.0));
        let tl = self.arranger.timeline(grid);

        let grid_resp = ui.interact(grid, ui.id().with("arranger_grid"), Sense::click_and_drag());
        grid_resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Arrangement timeline")
        });
        let ruler_resp = ui.interact(
            ruler,
            ui.id().with("arranger_ruler"),
            Sense::click_and_drag(),
        );
        self.arranger_grid_input(ui, &grid_resp, grid, tl);
        self.arranger_ruler_input(ui, &ruler_resp, ruler, tl, song_steps);

        // Drag-and-drop from the instrument library: the lanes, headers and
        // scrollbars (everything under the ruler) accept a dragged sound.
        // While one hovers here a ghost lane marks the insertion index;
        // releasing drops the track there.
        let drop_area = Rect::from_min_max(
            pos2(full.left(), grid.top()),
            pos2(full.right(), full.bottom()),
        );
        let drop = self.library_drop(ui.ctx(), drop_area);

        let painter = ui.painter_at(grid);
        self.paint_lanes(&painter, grid, tl, song_steps);
        self.paint_section_bands(&painter, grid, tl);
        self.paint_clips(&painter, grid, tl);
        self.paint_automation(&painter, grid, tl);
        self.paint_arranger_marquee(&painter);
        self.paint_loop_region(&painter, grid, tl);
        self.paint_marker_guides(&painter, grid, tl);
        let px = tl.x(self.transport.position);
        if px >= grid.left() - 1.0 && px <= grid.right() + 1.0 {
            painter.vline(px, grid.y_range(), Stroke::new(2.0_f32, pal.accent));
        }
        // Borrowed, not cloned: the drag carries Vec<(u64, Clip)>-sized
        // snapshots that must not deep-copy every frame.
        if let Some(d) = self.arranger.drag.as_ref().filter(|d| d.moved()) {
            // `(start, loops, fade_in, fade_out)` for whichever clip kind the
            // gesture holds — pattern clips and imported-WAV clips share the
            // id space, so the kind resolves inside `any_clip_props`.
            let label_at = |track: u64, clip: u64| {
                ops::track(&self.project, track)
                    .and_then(|t| ops::any_clip_props(t, clip))
                    .map(|(start, _, loops, fi, fo)| (start, loops, fi, fo))
            };
            let (start, label) = match d {
                ArrangerDrag::Clip(cd) => label_at(cd.track, cd.clip).map(|(start, ..)| {
                    (
                        start,
                        format!(
                            "Bar {}{}",
                            ops::format_position(start).trim(),
                            if cd.copy { "  (copy)" } else { "" }
                        ),
                    )
                }),
                ArrangerDrag::Fade { track, clip, edge } => {
                    label_at(*track, *clip).map(|(start, _, fi, fo)| {
                        let v = if *edge == FadeEdge::In { fi } else { fo };
                        (
                            start,
                            format!(
                                "{} fade {:.0} st",
                                if *edge == FadeEdge::In { "In" } else { "Out" },
                                v
                            ),
                        )
                    })
                }
                ArrangerDrag::Loop { track, clip, .. } => label_at(*track, *clip)
                    .map(|(start, loops, ..)| (start, format!("Loops ×{}", loops.max(1)))),
                ArrangerDrag::Automation {
                    track,
                    param,
                    index,
                    ..
                } => ops::track(&self.project, *track)
                    .and_then(|t| t.automation.as_ref())
                    .and_then(|a| a.lane(*param).get(*index))
                    .map(|pt| (pt.step, auto_point_label(*param, pt.value))),
                // The marker flag / chord chip itself is the ghost; no
                // floating label.
                ArrangerDrag::Marker { .. } | ArrangerDrag::Chord { .. } => None,
            }
            .unzip();
            if let (Some(start), Some(label)) = (start, label) {
                painter.text(
                    pos2(tl.x(start) + 4.0, grid.top() + 4.0),
                    Align2::LEFT_TOP,
                    label,
                    FontId::proportional(11.0),
                    pal.text,
                );
            }
        }
        self.paint_ruler(ui, ruler, tl, song_steps);
        self.marker_rename_ui(ui, ruler, tl);
        self.lane_headers(ui, header, grid);
        if let Some((index, sound)) = &drop {
            self.paint_library_drop(ui.painter(), drop_area, *index, sound);
        }

        let mut sx = self.arranger.scroll_x;
        theme::scrollbar(
            ui,
            pal,
            hbar.shrink2(vec2(0.0, 1.0)),
            ui.id().with("arr_hbar"),
            false,
            content_w,
            grid.width(),
            &mut sx,
        );
        self.arranger.scroll_x = sx;
        let mut sy = self.arranger.scroll_y;
        theme::scrollbar(
            ui,
            pal,
            vbar.shrink2(vec2(1.0, 0.0)),
            ui.id().with("arr_vbar"),
            true,
            content_h,
            grid.height(),
            &mut sy,
        );
        self.arranger.scroll_y = sy;
        ui.painter().rect_filled(
            Rect::from_min_max(pos2(full.left(), full.top()), pos2(grid.left(), grid.top())),
            0.0,
            pal.panel,
        );
        ui.painter().text(
            pos2(full.left() + 14.0, full.top() + RULER_H * 0.5),
            Align2::LEFT_CENTER,
            "TRACKS",
            FontId::proportional(10.0),
            pal.text_faint,
        );
        ui.painter()
            .hline(full.x_range(), grid.top(), Stroke::new(1.0_f32, pal.border));
        ui.painter().vline(
            grid.left(),
            full.y_range(),
            Stroke::new(1.0_f32, pal.border),
        );
    }

    /// Scrolls horizontally to keep the playhead visible during playback.
    /// Suppressed while a clip drag or marquee selection owns the grid so the
    /// view does not move underneath an in-progress pointer gesture.
    fn follow_playhead(&mut self, grid: Rect) {
        if !self.transport.playing
            || !self.arranger.follow
            || self.arranger.drag.is_some()
            || self.arranger.marquee.is_some()
            || self.arranger.region_press.is_some()
        {
            return;
        }
        let px =
            self.transport.position as f32 * self.arranger.px_per_step - self.arranger.scroll_x;
        if px > grid.width() - 24.0 || px < 0.0 {
            self.arranger.scroll_x =
                (self.transport.position as f32 * self.arranger.px_per_step - 32.0).max(0.0);
        }
    }

    fn arranger_toolbar(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        egui::Frame::new().inner_margin(egui::Margin { left: 12, right: 12, top: 8, bottom: 6 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::section_header(ui, pal, "ARRANGEMENT", "Song timeline");
                ui.add_space(14.0);
                ui.label(RichText::new("Length").small().color(pal.text_dim));
                let mut bars = self.project.bars;
                let resp = ui.add_sized(vec2(80.0, 24.0), egui::DragValue::new(&mut bars).range(ops::MIN_BARS..=ops::MAX_BARS).speed(0.2).suffix(" bars"));
                if resp.changed() {
                    self.edit("Song length", Some("bars"), |p| p.bars = bars.clamp(ops::MIN_BARS, ops::MAX_BARS));
                }
                resp.on_hover_text(format!("Song length in bars (1–{}). Placing clips past the end grows the song automatically.", ops::MAX_BARS));
                // Quick-adjust steppers next to the drag value, clamped to
                // the model's bar limits like the drag value itself.
                if theme::icon_button(
                    ui,
                    pal,
                    theme::Icon::Minus,
                    24.0,
                    false,
                    pal.accent,
                    "One bar shorter",
                )
                .clicked()
                {
                    let bars = self.project.bars.saturating_sub(1).max(ops::MIN_BARS);
                    self.edit("Song length", Some("bars"), |p| p.bars = bars);
                }
                if theme::icon_button(
                    ui,
                    pal,
                    theme::Icon::Plus,
                    24.0,
                    false,
                    pal.accent,
                    "One bar longer",
                )
                .clicked()
                {
                    let bars = self.project.bars.saturating_add(1).min(ops::MAX_BARS);
                    self.edit("Song length", Some("bars"), |p| p.bars = bars);
                }
                ui.add_space(6.0);
                ui.label(RichText::new("Snap").small().color(pal.text_dim));
                egui::ComboBox::from_id_salt("arr_snap").width(64.0).selected_text(self.arranger.snap.label()).show_ui(ui, |ui| {
                    for s in [Snap::Bar, Snap::Beat, Snap::Step] {
                        ui.selectable_value(&mut self.arranger.snap, s, s.label());
                    }
                });
                ui.add_space(6.0);
                if theme::icon_button(ui, pal, theme::Icon::Minus, 26.0, false, pal.accent, "Zoom out (−, or Cmd/Ctrl + scroll)").clicked() {
                    self.arranger.zoom_by(0.8, None);
                }
                if theme::icon_button(ui, pal, theme::Icon::Plus, 26.0, false, pal.accent, "Zoom in (=, or Cmd/Ctrl + scroll)").clicked() {
                    self.arranger.zoom_by(1.25, None);
                }
                if theme::icon_button(ui, pal, theme::Icon::Fit, 26.0, false, pal.accent, "Fit the whole song").clicked() {
                    self.arranger.fit_request = true;
                }
                ui.checkbox(&mut self.arranger.follow, RichText::new("Follow").small()).on_hover_text("Scroll to keep the playhead visible during playback");
                ui.add_space(6.0);
                let loop_tip = match self.transport.loop_region {
                    Some((a, b)) => format!(
                        "Loop bars {}–{} on repeat (L) · drag the ruler's top strip to move it, ✕ on the band clears it",
                        bar_number(a),
                        bar_number(b),
                    ),
                    None => "Loop the whole song (L) · drag the ruler's top strip to loop a section instead".to_string(),
                };
                if theme::chip(
                    ui,
                    pal,
                    "LOOP",
                    self.transport.looping,
                    pal.accent,
                    vec2(48.0, 24.0),
                    &loop_tip,
                )
                .clicked()
                {
                    self.toggle_loop();
                }
                if let Some((a, b)) = self.transport.loop_region {
                    ui.label(
                        RichText::new(format!("{}–{}", bar_number(a), bar_number(b)))
                            .small()
                            .color(if self.transport.looping {
                                pal.accent
                            } else {
                                pal.text_faint
                            }),
                    );
                }
                ui.add_space(6.0);
                if theme::chip(
                    ui,
                    pal,
                    "+ MARKER",
                    false,
                    pal.warn,
                    vec2(74.0, 24.0),
                    "Add a section marker at the playhead (or double-click the ruler)",
                )
                .clicked()
                {
                    self.add_marker_at(self.transport.position);
                }
                let selected_clips = self.selected_clip_ids().len();
                if selected_clips > 1 {
                    ui.label(RichText::new(format!("{selected_clips} clips selected")).small().color(pal.accent));
                }
                let can_split = self.split_clip_target(ui.ctx()).is_some();
                if ui
                    .add_enabled(can_split, egui::Button::new("Split").small())
                    .on_hover_text("Split the clip under the pointer at the pointer, else the selected clip at the playhead (B)")
                    .on_disabled_hover_text("Hover a clip, or select one the playhead crosses")
                    .clicked()
                {
                    self.split_clip_at_playhead(ui.ctx());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::hint(ui, pal, "Drag empty lanes to box-select clips · drag selected clips as a group · Alt-drag copies · corners fade · right edge loops");
                });
            });
        });
    }

    /// Height of a track's whole arranger row: the clip lane plus the
    /// automation strip while the track's "Show automation" toggle is on.
    fn track_row_height(&self, track_id: u64) -> f32 {
        LANE_H
            + if self.arranger.auto_lanes.contains_key(&track_id) {
                AUTO_LANE_H
            } else {
                0.0
            }
    }

    /// Y offset of `row`'s top edge inside the lane stack, before scroll.
    /// Rows are variable-height (`track_row_height`), so a plain
    /// `row * LANE_H` no longer works.
    fn lane_top(&self, row: usize) -> f32 {
        self.project
            .tracks
            .iter()
            .take(row)
            .map(|t| self.track_row_height(t.id))
            .sum()
    }

    /// First lane index that can be visible at the current `scroll_y`.
    /// Variable rows mean a `scroll / LANE_H` divide is wrong; the lane
    /// stack is at most [`ops::MAX_TRACKS`] entries, so a walk is cheap.
    fn first_visible_lane(&self) -> usize {
        let mut bottom = 0.0f32;
        for (i, t) in self.project.tracks.iter().enumerate() {
            bottom += self.track_row_height(t.id);
            if bottom > self.arranger.scroll_y {
                return i;
            }
        }
        self.project.tracks.len()
    }

    /// The lane row a screen y lands in and whether it is inside that row's
    /// automation strip — `None` outside the lane stack. `lanes_top` is the
    /// grid's top edge (the stack's screen position before scroll).
    fn lane_row_at(&self, lanes_top: f32, y: f32) -> Option<(usize, bool)> {
        let mut edge = lanes_top - self.arranger.scroll_y;
        if y < edge {
            return None;
        }
        for (i, t) in self.project.tracks.iter().enumerate() {
            let h = self.track_row_height(t.id);
            if y < edge + h {
                return Some((i, y - edge >= LANE_H));
            }
            edge += h;
        }
        None
    }

    /// The lane an expanded track edits; tracks never toggled read Volume.
    fn auto_param(&self, track_id: u64) -> AutomationLane {
        self.arranger
            .auto_lanes
            .get(&track_id)
            .copied()
            .unwrap_or(AutomationLane::Volume)
    }

    /// The automation-strip rect of lane `row` inside `grid` (screen space).
    fn auto_lane_rect(&self, grid: Rect, row: usize) -> Rect {
        let top = grid.top() + self.lane_top(row) + LANE_H - self.arranger.scroll_y;
        Rect::from_min_max(
            pos2(grid.left(), top),
            pos2(grid.right(), top + AUTO_LANE_H),
        )
    }

    /// Opens/closes a track's automation strip (the header menu's "Show
    /// automation"). Opening picks the lane that already carries data — a
    /// pan-only track opens on Pan.
    fn toggle_automation_lane(&mut self, track_id: u64) {
        if self.arranger.auto_lanes.remove(&track_id).is_some() {
            return;
        }
        let lane = ops::track(&self.project, track_id)
            .and_then(|t| t.automation.as_ref())
            .filter(|a| a.volume.is_empty() && !a.pan.is_empty())
            .map_or(AutomationLane::Volume, |_| AutomationLane::Pan);
        self.arranger.auto_lanes.insert(track_id, lane);
    }

    fn hit_test(&self, pos: Pos2, grid: Rect, tl: Timeline) -> Hit {
        if !grid.contains(pos) {
            return Hit::None;
        }
        let Some((lane, in_auto)) = self.lane_row_at(grid.top(), pos.y) else {
            return Hit::None;
        };
        let Some(track) = self.project.tracks.get(lane) else {
            return Hit::None;
        };
        let step = tl.step(pos.x).max(0.0);
        if in_auto {
            // The automation strip under the clip lane: a point within grab
            // range (6 px) is the drag target, anywhere else is add-on-press.
            let param = self.auto_param(track.id);
            let strip = self.auto_lane_rect(grid, lane);
            let point = track.automation.as_ref().and_then(|a| {
                a.lane(param)
                    .iter()
                    .enumerate()
                    .filter(|(_, pt)| (tl.x(pt.step) - pos.x).abs() <= 6.0)
                    .min_by(|(_, a), (_, b)| {
                        (tl.x(a.step) - pos.x)
                            .abs()
                            .total_cmp(&(tl.x(b.step) - pos.x).abs())
                    })
                    .map(|(i, _)| i)
            });
            return Hit::Automation {
                track: track.id,
                param,
                point,
                step,
                value: auto_y_to_value(param, pos.y, strip),
            };
        }
        // Clip lookup covers both kinds: pattern clips first, then imported
        // WAV clips — ids share one space so the hit carries just the id.
        let Some((clip_id, cstart, len, rep, pattern)) = track
            .clips
            .iter()
            .rev()
            .find(|c| step >= c.start && step < c.start + ops::clip_len(track, c))
            .map(|c| {
                (
                    c.id,
                    c.start,
                    ops::clip_len(track, c),
                    pattern_steps(track, c),
                    Some(c.pattern_id),
                )
            })
            .or_else(|| {
                track
                    .audio_clips
                    .iter()
                    .rev()
                    .find(|a| step >= a.start && step < a.start + a.span())
                    .map(|a| (a.id, a.start, a.span(), a.length, None))
            })
        else {
            return Hit::Lane {
                track: track.id,
                step,
            };
        };
        // Clip found: corner fade handles live in the 15 px top band, the loop
        // handle hugs the right edge below it. Handles win over the clip body.
        let x0 = tl.x(cstart);
        let x1 = tl.x(cstart + len);
        let lane_top = grid.top() + self.lane_top(lane) - self.arranger.scroll_y;
        let in_band = (lane_top + 4.0..=lane_top + 19.0).contains(&pos.y);
        if in_band {
            // On clips narrower than ~28 px the zones overlap; left wins.
            if pos.x - x0 <= 14.0 {
                return Hit::ClipFade {
                    track: track.id,
                    clip: clip_id,
                    edge: FadeEdge::In,
                };
            }
            if x1 - pos.x <= 14.0 {
                return Hit::ClipFade {
                    track: track.id,
                    clip: clip_id,
                    edge: FadeEdge::Out,
                };
            }
        } else if x1 - pos.x <= 7.0 && x1 - x0 >= 48.0 && rep * f64::from(tl.pps) >= 14.0 {
            return Hit::ClipLoop {
                track: track.id,
                clip: clip_id,
            };
        }
        Hit::Clip {
            track: track.id,
            clip: clip_id,
            pattern,
            start: cstart,
            step,
        }
    }

    fn arranger_grid_input(
        &mut self,
        ui: &mut Ui,
        resp: &egui::Response,
        grid: Rect,
        tl: Timeline,
    ) {
        let (pressed, released, shift, alt, hover) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.modifiers.shift,
                i.modifiers.alt,
                i.pointer.hover_pos(),
            )
        });
        if let Some(h) = hover.filter(|_| resp.hovered()) {
            let hit = self.hit_test(h, grid, tl);
            if self.arranger.drag.as_ref().is_some_and(|d| d.moved()) {
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            } else {
                match hit {
                    Hit::ClipFade { .. } | Hit::ClipLoop { .. } => {
                        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                    }
                    Hit::Clip { .. } => {
                        ui.ctx().set_cursor_icon(if alt {
                            CursorIcon::Copy
                        } else {
                            CursorIcon::Grab
                        });
                    }
                    Hit::Automation { .. } => {
                        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                    }
                    Hit::Lane { .. } | Hit::None => {}
                }
            }
        }
        if pressed && resp.is_pointer_button_down_on() {
            if let Some(pos) = ui.input(|i| i.pointer.press_origin()) {
                self.focus = Focus::Arranger;
                match self.hit_test(pos, grid, tl) {
                    Hit::Clip {
                        track,
                        clip,
                        pattern,
                        start,
                        step,
                    } => {
                        self.sel.track = Some(track);
                        if let Some(pattern) = pattern {
                            self.select_pattern(track, pattern);
                        }
                        if shift {
                            let mut ids = self.selected_clip_ids();
                            if !ids.remove(&clip) {
                                ids.insert(clip);
                            }
                            self.set_clip_selection(ids, Some(clip));
                        } else {
                            if !self.selected_clip_ids().contains(&clip) {
                                self.set_single_clip_selection(clip);
                            } else {
                                self.sel.clip = Some(clip);
                                self.sel.notes.clear();
                            }
                            let originals = self.selected_clip_snapshots();
                            let originals_audio = self.selected_audio_clip_snapshots();
                            self.arranger.drag = Some(ArrangerDrag::Clip(ClipDrag {
                                track,
                                clip,
                                grab: step - start,
                                origin: start,
                                copy: alt,
                                copied: false,
                                copy_failed: false,
                                moved: false,
                                originals,
                                originals_audio,
                                group: format!("clip-drag-{clip}"),
                            }));
                        }
                    }
                    Hit::ClipFade { track, clip, edge } => {
                        self.select_clip(track, clip);
                        self.arranger.drag = Some(ArrangerDrag::Fade { track, clip, edge });
                    }
                    Hit::ClipLoop { track, clip } => {
                        self.select_clip(track, clip);
                        let grab_step = tl.step(pos.x);
                        let origin_loops = ops::track(&self.project, track)
                            .and_then(|t| ops::any_clip_props(t, clip))
                            .map_or(1, |(_, _, loops, _, _)| loops.max(1));
                        self.arranger.drag = Some(ArrangerDrag::Loop {
                            track,
                            clip,
                            grab_step,
                            origin_loops,
                        });
                    }
                    Hit::Automation {
                        track,
                        param,
                        point,
                        step,
                        value,
                    } => {
                        self.select_track(track);
                        let at = if shift {
                            step
                        } else {
                            snap_round(step, self.arranger.snap.steps())
                        };
                        // One coalescing key covers the whole gesture, so a
                        // click-place + drag is a single undo step.
                        let group = format!("auto-drag-{}-{}", track, next_id());
                        let index = match point {
                            Some(i) => Some(i),
                            None => {
                                let mut added = None;
                                self.edit("Add automation point", Some(&group), |p| {
                                    added = ops::add_automation_point(p, track, param, at, value);
                                });
                                if added.is_none() {
                                    self.set_status(
                                        StatusKind::Warning,
                                        format!(
                                            "Automation lanes hold up to {} points.",
                                            ops::MAX_AUTOMATION_POINTS
                                        ),
                                    );
                                }
                                added
                            }
                        };
                        if let Some(index) = index {
                            self.arranger.drag = Some(ArrangerDrag::Automation {
                                track,
                                param,
                                index,
                                group,
                            });
                        }
                    }
                    Hit::Lane { track, .. } => {
                        let base = if shift {
                            self.selected_clip_ids()
                        } else {
                            self.clear_arranger_clip_selection();
                            BTreeSet::new()
                        };
                        self.sel.track = Some(track);
                        self.sel.notes.clear();
                        self.arranger.marquee = Some(ArrangerMarquee {
                            start: pos,
                            current: pos,
                            base,
                            additive: shift,
                        });
                    }
                    Hit::None => {
                        if !shift {
                            self.clear_arranger_clip_selection();
                        }
                        self.arranger.marquee = Some(ArrangerMarquee {
                            start: pos,
                            current: pos,
                            base: if shift {
                                self.selected_clip_ids()
                            } else {
                                BTreeSet::new()
                            },
                            additive: shift,
                        });
                    }
                }
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            let pos = resp
                .interact_pointer_pos()
                .or_else(|| ui.input(|i| i.pointer.interact_pos()));
            if let Some(pos) = pos {
                if let Some(m) = self.arranger.marquee.as_mut() {
                    m.current = pos;
                    self.update_arranger_marquee_selection(grid, tl);
                } else if let Some(d) = self.arranger.drag.take() {
                    // `take` + put-back instead of a per-frame clone: the
                    // drag state owns Vec-sized snapshots that must not be
                    // deep-copied on every pointer move.
                    match d {
                        ArrangerDrag::Clip(mut cd) => {
                            self.drag_clip_group(&mut cd, pos.x, tl);
                            self.arranger.drag = Some(ArrangerDrag::Clip(cd));
                        }
                        ArrangerDrag::Fade { track, clip, edge } => {
                            self.drag_clip_fade(track, clip, edge, pos.x, tl);
                            self.arranger.drag = Some(ArrangerDrag::Fade { track, clip, edge });
                        }
                        ArrangerDrag::Loop {
                            track,
                            clip,
                            grab_step,
                            origin_loops,
                        } => {
                            self.drag_clip_loop(track, clip, grab_step, origin_loops, pos.x, tl);
                            self.arranger.drag = Some(ArrangerDrag::Loop {
                                track,
                                clip,
                                grab_step,
                                origin_loops,
                            });
                        }
                        ArrangerDrag::Automation {
                            track,
                            param,
                            index,
                            group,
                        } => {
                            let step = tl.step(pos.x).max(0.0);
                            let at = if shift {
                                step
                            } else {
                                snap_round(step, self.arranger.snap.steps())
                            };
                            let mut new_index = index;
                            if let Some(row) = ops::track_index(&self.project, track) {
                                let value =
                                    auto_y_to_value(param, pos.y, self.auto_lane_rect(grid, row));
                                self.edit("Move automation point", Some(&group), |p| {
                                    new_index = ops::move_automation_point(
                                        p, track, param, index, at, value,
                                    )
                                    .unwrap_or(index);
                                });
                            }
                            self.arranger.drag = Some(ArrangerDrag::Automation {
                                track,
                                param,
                                index: new_index,
                                group,
                            });
                        }
                        // Marker and chord-chip drags belong to the ruler
                        // input handler; the drag goes back untouched.
                        other @ (ArrangerDrag::Marker { .. } | ArrangerDrag::Chord { .. }) => {
                            self.arranger.drag = Some(other);
                        }
                    }
                }
            }
        }
        if released && self.arranger.marquee.take().is_some() {
            self.fix_selection();
        }
        if resp.double_clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                match self.hit_test(pos, grid, tl) {
                    // Only clips backed by a pattern open the editor —
                    // imported-WAV clips have no note content to edit.
                    Hit::Clip {
                        pattern: Some(_), ..
                    } => {
                        self.tab = Tab::Editor;
                        self.focus = Focus::Editor;
                    }
                    // Clip handles (and audio-clip bodies) don't carry the
                    // pattern id — resolve it; `None` means an audio clip.
                    Hit::Clip { track, clip, .. }
                    | Hit::ClipFade { track, clip, .. }
                    | Hit::ClipLoop { track, clip } => {
                        if clip_pattern_id(&self.project, track, clip).is_some() {
                            self.tab = Tab::Editor;
                            self.focus = Focus::Editor;
                        }
                    }
                    Hit::Lane { track, step } => {
                        self.place_clip(track, None, snap_floor(step, self.arranger.snap.steps()))
                    }
                    // The press already placed (or grabbed) a point; nothing
                    // extra happens on the second click.
                    Hit::Automation { .. } | Hit::None => {}
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let step = tl.step(pos.x).max(0.0);
                self.arranger.context = match self.hit_test(pos, grid, tl) {
                    Hit::Clip {
                        track,
                        clip,
                        pattern,
                        ..
                    } => {
                        self.select_track(track);
                        self.set_single_clip_selection(clip);
                        if let Some(pattern) = pattern {
                            self.select_pattern(track, pattern);
                        }
                        Some(ContextTarget::Clip {
                            track,
                            clip,
                            step,
                            audio: pattern.is_none(),
                        })
                    }
                    Hit::ClipFade { track, clip, .. } | Hit::ClipLoop { track, clip } => {
                        self.select_clip(track, clip);
                        let audio = ops::track(&self.project, track)
                            .is_some_and(|t| ops::is_audio_clip(t, clip));
                        Some(ContextTarget::Clip {
                            track,
                            clip,
                            step,
                            audio,
                        })
                    }
                    Hit::Lane { track, step } => {
                        self.select_track(track);
                        Some(ContextTarget::Lane {
                            track,
                            step: snap_floor(step, self.arranger.snap.steps()),
                        })
                    }
                    Hit::Automation {
                        track,
                        param,
                        point,
                        ..
                    } => match point {
                        // Like a marker flag: right-click deletes the point
                        // outright. Empty strip opens the lane's menu.
                        Some(index) => {
                            self.select_track(track);
                            self.edit("Delete automation point", None, |p| {
                                ops::delete_automation_point(p, track, param, index);
                            });
                            None
                        }
                        None => Some(ContextTarget::AutoStrip { track, param }),
                    },
                    Hit::None => None,
                };
            }
        }
        resp.context_menu(|ui| self.arranger_context_menu(ui));
    }

    fn drag_clip_group(&mut self, cd: &mut ClipDrag, x: f32, tl: Timeline) {
        let g = self.arranger.snap.steps();
        let target = snap_round(tl.step(x) - cd.grab, g);
        let delta = ops::mixed_group_delta(
            &self.project,
            &cd.originals,
            &cd.originals_audio,
            target - cd.origin,
        );
        if (delta.abs() <= f64::EPSILON && !cd.moved) || cd.copy_failed {
            return;
        }
        let group = cd.group.as_str();
        if cd.copy && !cd.copied {
            // Borrowed, not cloned: the snapshots ride the drag state and a
            // per-frame deep copy of every selected clip would stall large
            // selections. `edit` applies its closure immediately, so the
            // borrows only need to last the call.
            let originals = &cd.originals;
            let originals_audio = &cd.originals_audio;
            let primary_old = cd.clip;
            let mut copied = None;
            self.edit(
                if originals.len() + originals_audio.len() == 1 {
                    "Copy clip"
                } else {
                    "Copy clips"
                },
                Some(group),
                |p| copied = ops::copy_mixed_clip_group(p, originals, originals_audio, delta),
            );
            match copied {
                Some(ids) => {
                    let primary = originals
                        .iter()
                        .position(|(_, c)| c.id == primary_old)
                        .and_then(|i| ids.get(i).copied())
                        .or_else(|| {
                            originals_audio
                                .iter()
                                .position(|(_, c)| c.id == primary_old)
                                .and_then(|i| ids.get(originals.len() + i).copied())
                        })
                        .unwrap_or_else(|| ids[0]);
                    let set: BTreeSet<u64> = ids.iter().copied().collect();
                    cd.clip = primary;
                    cd.track = self.clip_track_id(primary).unwrap_or(cd.track);
                    cd.origin += delta;
                    cd.copied = true;
                    cd.originals = ops::collect_clips(&self.project, &set);
                    cd.originals_audio = ops::collect_audio_clips(&self.project, &set);
                    self.set_clip_selection(set, Some(primary));
                }
                None => {
                    cd.copy_failed = true;
                    self.set_status(
                        StatusKind::Warning,
                        format!(
                            "Could not copy selection (tracks hold up to {} clips).",
                            ops::MAX_CLIPS_PER_TRACK
                        ),
                    );
                    return;
                }
            }
        } else {
            let originals = &cd.originals;
            let originals_audio = &cd.originals_audio;
            self.edit(
                if originals.len() + originals_audio.len() == 1 {
                    "Move clip"
                } else {
                    "Move clips"
                },
                Some(group),
                |p| {
                    ops::move_clip_group(p, originals, delta);
                    ops::move_audio_clip_group(p, originals_audio, delta);
                },
            );
        }
        cd.moved = true;
    }

    fn update_arranger_marquee_selection(&mut self, grid: Rect, tl: Timeline) {
        let Some(m) = self.arranger.marquee.clone() else {
            return;
        };
        let rect = Rect::from_two_pos(m.start, m.current);
        let mut ids = if m.additive { m.base } else { BTreeSet::new() };
        ids.extend(self.clip_ids_in_rect(rect, grid, tl));
        let primary = ids.iter().next().copied();
        self.set_clip_selection(ids, primary);
    }

    fn clip_ids_in_rect(&self, rect: Rect, grid: Rect, tl: Timeline) -> BTreeSet<u64> {
        let mut ids = BTreeSet::new();
        let first = self.first_visible_lane();
        for (i, track) in self.project.tracks.iter().enumerate().skip(first) {
            let y = grid.top() + self.lane_top(i) - self.arranger.scroll_y;
            if y > grid.bottom() || y > rect.bottom() {
                break;
            }
            let lane = Rect::from_min_max(pos2(grid.left(), y), pos2(grid.right(), y + LANE_H));
            if !lane.intersects(rect) {
                continue;
            }
            for clip in &track.clips {
                let x0 = tl.x(clip.start);
                let x1 = tl.x(clip.start + ops::clip_len(track, clip));
                let r =
                    Rect::from_min_max(pos2(x0 + 1.0, y + 4.0), pos2(x1 - 1.0, y + LANE_H - 4.0));
                if r.is_positive() && r.intersects(rect) {
                    ids.insert(clip.id);
                }
            }
            for clip in &track.audio_clips {
                let x0 = tl.x(clip.start);
                let x1 = tl.x(clip.start + clip.span());
                let r =
                    Rect::from_min_max(pos2(x0 + 1.0, y + 4.0), pos2(x1 - 1.0, y + LANE_H - 4.0));
                if r.is_positive() && r.intersects(rect) {
                    ids.insert(clip.id);
                }
            }
        }
        ids
    }

    /// Adds a clip of `pattern` (default: the track's current pattern) at `start`.
    pub(super) fn place_clip(&mut self, track_id: u64, pattern: Option<u64>, start: f64) {
        let Some(track) = ops::track(&self.project, track_id) else {
            return;
        };
        let Some(pid) = pattern.or_else(|| self.current_pattern_id(track)) else {
            return;
        };
        let mut id = None;
        let before = self.project.bars;
        self.edit("Place clip", None, |p| {
            id = ops::add_clip(p, track_id, pid, start)
        });
        match id {
            Some(id) => {
                self.set_single_clip_selection(id);
                self.select_pattern(track_id, pid);
                if self.project.bars > before {
                    self.set_status(
                        StatusKind::Info,
                        format!("Song grew to {} bars to fit the clip.", self.project.bars),
                    );
                }
            }
            None => self.set_status(
                StatusKind::Warning,
                format!(
                    "Could not place a clip here (tracks hold up to {} clips).",
                    ops::MAX_CLIPS_PER_TRACK
                ),
            ),
        }
    }

    /// Selects a clip the way a body press does: its track, the clip itself
    /// and the pattern it plays. Used by the corner/edge handle hits, which
    /// only carry ids.
    fn select_clip(&mut self, track_id: u64, clip_id: u64) {
        self.select_track(track_id);
        self.set_single_clip_selection(clip_id);
        if let Some(pid) = clip_pattern_id(&self.project, track_id, clip_id) {
            self.select_pattern(track_id, pid);
        }
    }

    /// Live corner-fade drag: the pointer step maps to a fade length in steps
    /// (snapped to the grid); the untouched fade keeps its value. The whole
    /// gesture coalesces into one undo group.
    fn drag_clip_fade(&mut self, track: u64, clip: u64, edge: FadeEdge, x: f32, tl: Timeline) {
        let Some((start, span, _, fade_in, fade_out)) =
            ops::track(&self.project, track).and_then(|t| ops::any_clip_props(t, clip))
        else {
            return;
        };
        let g = self.arranger.snap.steps();
        let pointer = tl.step(x);
        let (new_in, new_out) = match edge {
            FadeEdge::In => (snap_round(pointer - start, g).clamp(0.0, span), fade_out),
            FadeEdge::Out => (
                fade_in,
                snap_round(start + span - pointer, g).clamp(0.0, span),
            ),
        };
        self.edit("Fade clip", Some(&format!("clip-fade-{clip}")), |p| {
            ops::set_any_clip_fades(p, track, clip, new_in, new_out);
        });
    }

    /// Live loop-handle drag: each pattern length of pointer travel changes the
    /// loop count by one, clamped to the event-budget cap.
    fn drag_clip_loop(
        &mut self,
        track: u64,
        clip: u64,
        grab_step: f64,
        origin_loops: u32,
        x: f32,
        tl: Timeline,
    ) {
        let Some((rep_len, max_loops)) = ops::track(&self.project, track)
            .and_then(|t| ops::any_clip_rep_len(t, clip).zip(ops::max_any_clip_loops(t, clip)))
        else {
            return;
        };
        if rep_len <= 0.0 {
            return;
        }
        let delta = ((tl.step(x) - grab_step) / rep_len).round() as i64;
        let loops = (i64::from(origin_loops) + delta).clamp(1, i64::from(max_loops)) as u32;
        self.edit("Clip loops", Some(&format!("clip-loops-{clip}")), |p| {
            ops::set_any_clip_loops(p, track, clip, loops);
        });
    }

    /// Sets a clip's loop count (context menu and tests); one undo step.
    /// Works on pattern and imported-WAV clips alike.
    pub(super) fn set_clip_loops(&mut self, track_id: u64, clip_id: u64, loops: u32) {
        self.edit("Clip loops", None, |p| {
            ops::set_any_clip_loops(p, track_id, clip_id, loops);
        });
    }

    /// Sets a clip's fade-in/fade-out lengths in steps; one undo step.
    pub(super) fn set_clip_fades(
        &mut self,
        track_id: u64,
        clip_id: u64,
        fade_in: f64,
        fade_out: f64,
    ) {
        self.edit("Clip fades", None, |p| {
            ops::set_any_clip_fades(p, track_id, clip_id, fade_in, fade_out);
        });
    }

    /// The clip a split command would cut and the step it would cut at: the
    /// clip under the pointer (at the pointer step, grid-snapped) while one
    /// is hovered, else the selected clip containing the playhead.
    pub(super) fn split_clip_target(&self, ctx: &egui::Context) -> Option<(u64, u64, f64)> {
        // No cutting while a drag/marquee gesture or the context menu owns
        // the grid — the pointer means something else then.
        if self.arranger.drag.is_none()
            && self.arranger.marquee.is_none()
            && self.arranger.context.is_none()
        {
            let grid = self.arranger.grid;
            if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                if grid.contains(pos) {
                    let tl = self.arranger.timeline(grid);
                    // Fade and loop handles sit on the clip too — the
                    // pointer over any part of it still targets that clip.
                    let on_clip = match self.hit_test(pos, grid, tl) {
                        Hit::Clip { track, clip, .. }
                        | Hit::ClipFade { track, clip, .. }
                        | Hit::ClipLoop { track, clip } => Some((track, clip)),
                        _ => None,
                    };
                    if let Some((track, clip)) = on_clip {
                        let step = tl.step(pos.x).max(0.0);
                        let g = self.arranger.snap.steps();
                        // The grid-snapped step wins; if it lands on the
                        // clip's edge the raw pointer position still cuts.
                        for at in [snap_round(step, g), step] {
                            if ops::can_split_clip(&self.project, track, clip, at) {
                                return Some((track, clip, at));
                            }
                        }
                    }
                }
            }
        }
        // The primary selected clip first, then the rest of the selection.
        let at = self.transport.position;
        let primary = self.sel.clip;
        let ids = self.selected_clip_ids();
        for clip in primary
            .into_iter()
            .chain(ids.into_iter().filter(|id| Some(*id) != primary))
        {
            let Some(track) = self.clip_track_id(clip) else {
                continue;
            };
            if ops::can_split_clip(&self.project, track, clip, at) {
                return Some((track, clip, at));
            }
        }
        None
    }

    /// Cuts `clip` at `at` in one undo step; the new right-hand clip becomes
    /// the selection (and its pattern the editor's current one).
    fn do_split_clip(&mut self, track_id: u64, clip_id: u64, at: f64) {
        let before: BTreeSet<u64> = ops::track(&self.project, track_id)
            .map(|t| t.clips.iter().map(|c| c.id).collect())
            .unwrap_or_default();
        if !self.edit("Split clip", None, |p| {
            ops::split_clip(p, track_id, clip_id, at);
        }) {
            self.set_status(StatusKind::Warning, "The clip cannot be split there.");
            return;
        }
        self.focus = Focus::Arranger;
        // The right piece is the clip that did not exist before the edit.
        let new = ops::track(&self.project, track_id).and_then(|t| {
            t.clips
                .iter()
                .find(|c| !before.contains(&c.id))
                .map(|c| (c.id, c.pattern_id))
        });
        if let Some((id, pid)) = new {
            self.set_single_clip_selection(id);
            self.select_pattern(track_id, pid);
        }
    }

    /// "Split Clip at Playhead": cuts [`Self::split_clip_target`]'s clip —
    /// the one under the pointer at the pointer step while hovering, else
    /// the selected clip at the transport position.
    pub(super) fn split_clip_at_playhead(&mut self, ctx: &egui::Context) {
        let Some((track, clip, at)) = self.split_clip_target(ctx) else {
            self.set_status(
                StatusKind::Info,
                "Nothing to split — hover a clip, or select one under the playhead.",
            );
            return;
        };
        self.do_split_clip(track, clip, at);
    }

    fn arranger_context_menu(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        match self.arranger.context {
            Some(ContextTarget::Clip {
                track,
                clip,
                step,
                audio,
            }) => {
                if !audio {
                    if ui.button("Edit pattern").clicked() {
                        self.tab = Tab::Editor;
                        self.focus = Focus::Editor;
                    }
                    let cut = [snap_round(step, self.arranger.snap.steps()), step]
                        .into_iter()
                        .find(|at| ops::can_split_clip(&self.project, track, clip, *at));
                    if ui
                        .add_enabled(cut.is_some(), egui::Button::new("Split here"))
                        .on_disabled_hover_text("The pointer sits on the clip's edge")
                        .clicked()
                    {
                        if let Some(at) = cut {
                            self.do_split_clip(track, clip, at);
                        }
                    }
                } else {
                    ui.label(
                        RichText::new(
                            ops::track(&self.project, track)
                                .and_then(|t| ops::audio_clip(t, clip))
                                .map(|c| c.name.clone())
                                .unwrap_or_else(|| "Audio clip".to_string()),
                        )
                        .small()
                        .color(pal.text_faint),
                    );
                }
                ui.separator();
                let repeat = |app: &mut Self, times: usize, label: &str| {
                    let mut last = None;
                    app.edit(label, None, |p| {
                        last = if audio {
                            ops::repeat_audio_clip(p, track, clip, times)
                        } else {
                            ops::repeat_clip(p, track, clip, times)
                        };
                    });
                    match last {
                        Some(id) => app.set_single_clip_selection(id),
                        None => app.set_status(
                            StatusKind::Warning,
                            format!("No room: songs are limited to {} bars.", ops::MAX_BARS),
                        ),
                    }
                };
                if ui
                    .add(
                        egui::Button::new("Duplicate after")
                            .shortcut_text(ui.ctx().format_shortcut(&super::SC_DUPLICATE)),
                    )
                    .clicked()
                {
                    repeat(self, 1, "Duplicate clip");
                }
                if ui.button("Repeat 3 more times").clicked() {
                    repeat(self, 3, "Repeat clip");
                }
                let fill = ops::track(&self.project, track).and_then(|t| {
                    ops::any_clip_props(t, clip).map(|(start, span, _, _, _)| (start, span))
                });
                if let Some((start, len)) = fill {
                    let times = ((self.project.total_steps() - start - len) / len)
                        .floor()
                        .max(0.0) as usize;
                    if ui
                        .add_enabled(times > 0, egui::Button::new("Repeat to song end"))
                        .clicked()
                    {
                        repeat(self, times, "Repeat clip");
                    }
                }
                // Clip properties: loop count and corner fades. Values are
                // fetched up front so `self.edit` calls can't hold borrows.
                let props = ops::track(&self.project, track).and_then(|t| {
                    ops::any_clip_props(t, clip)
                        .map(|(_, span, loops, fade_in, fade_out)| (loops, fade_in, fade_out, span))
                        .zip(ops::max_any_clip_loops(t, clip))
                });
                if let Some(((loops, fade_in, fade_out, span), max_loops)) = props {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Loops").small().color(pal.text_dim));
                        let mut v = loops;
                        let resp = ui.add(
                            egui::DragValue::new(&mut v)
                                .range(1..=max_loops)
                                .speed(0.2)
                                .suffix("×"),
                        );
                        if resp.changed() {
                            self.set_clip_loops(track, clip, v);
                        }
                        resp.on_hover_text("Pattern repetitions — the clip spans pattern × loops");
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Fade in").small().color(pal.text_dim));
                        let mut v = fade_in;
                        let resp = ui.add(
                            egui::DragValue::new(&mut v)
                                .range(0.0..=span)
                                .speed(0.5)
                                .suffix(" st"),
                        );
                        if resp.changed() {
                            self.set_clip_fades(track, clip, v, fade_out);
                        }
                        resp.on_hover_text("Fade length in sixteenth-note steps (0 = off)");
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Fade out").small().color(pal.text_dim));
                        let mut v = fade_out;
                        let resp = ui.add(
                            egui::DragValue::new(&mut v)
                                .range(0.0..=span)
                                .speed(0.5)
                                .suffix(" st"),
                        );
                        if resp.changed() {
                            self.set_clip_fades(track, clip, fade_in, v);
                        }
                        resp.on_hover_text("Fade length in sixteenth-note steps (0 = off)");
                    });
                }
                if !audio && ui.button("Make unique copy").on_hover_text("Give this clip its own copy of the pattern so edits don't change other clips").clicked() {
                    let mut new = None;
                    self.edit("Make clip unique", None, |p| new = ops::make_clip_unique(p, track, clip));
                    match new {
                        Some(pid) => {
                            self.select_pattern(track, pid);
                            self.set_status(StatusKind::Info, "This clip now plays its own pattern copy.");
                        }
                        None => self.set_status(StatusKind::Warning, format!("Tracks hold up to {} patterns.", ops::MAX_PATTERNS_PER_TRACK)),
                    }
                }
                ui.separator();
                if ui
                    .add(
                        egui::Button::new(RichText::new("Delete clip").color(pal.danger))
                            .shortcut_text("Del"),
                    )
                    .clicked()
                {
                    self.edit("Delete clip", None, |p| {
                        ops::delete_any_clip(p, track, clip);
                    });
                    self.clear_arranger_clip_selection();
                }
            }
            Some(ContextTarget::Lane { track, step }) => {
                ui.label(
                    RichText::new(format!("At bar {}", ops::format_position(step).trim()))
                        .small()
                        .color(pal.text_faint),
                );
                let is_audio_lane = ops::track(&self.project, track).is_some_and(|t| t.audio);
                if is_audio_lane && ui.button("Import audio file here…").clicked() {
                    self.import_audio_clip_dialog_at(track, step);
                }
                let patterns: Vec<(u64, String)> = ops::track(&self.project, track)
                    .map(|t| t.patterns.iter().map(|p| (p.id, p.name.clone())).collect())
                    .unwrap_or_default();
                for (pid, name) in patterns {
                    if ui.button(format!("Place \u{201c}{name}\u{201d}")).clicked() {
                        self.place_clip(track, Some(pid), step);
                    }
                }
                if !is_audio_lane {
                    ui.separator();
                }
                if !is_audio_lane && ui.button("New empty pattern here").clicked() {
                    let mut pid = None;
                    self.edit("New pattern", None, |p| {
                        if let Some(t) = ops::track_mut(p, track) {
                            pid = ops::new_pattern(t, 16);
                        }
                        if let Some(pid) = pid {
                            ops::add_clip(p, track, pid, step);
                        }
                    });
                    if let Some(pid) = pid {
                        self.select_pattern(track, pid);
                        self.tab = Tab::Editor;
                        self.focus = Focus::Editor;
                    }
                }
                ui.separator();
                self.structure_menu(ui, step);
            }
            Some(ContextTarget::AutoStrip { track, param }) => {
                let lane_name = match param {
                    AutomationLane::Volume => "volume",
                    AutomationLane::Pan => "pan",
                };
                let count = ops::track(&self.project, track)
                    .and_then(|t| t.automation.as_ref())
                    .map_or(0, |a| a.lane(param).len());
                ui.label(
                    RichText::new(format!("{lane_name} automation · {count} point(s)"))
                        .small()
                        .color(pal.text_faint),
                );
                if ui
                    .add_enabled(
                        count > 0,
                        egui::Button::new(format!("Clear {lane_name} lane")),
                    )
                    .clicked()
                {
                    self.edit("Clear automation", None, |p| {
                        ops::clear_automation_lane(p, track, param);
                    });
                }
                if ui.button("Hide automation lane").clicked() {
                    self.arranger.auto_lanes.remove(&track);
                }
            }
            // The chord-lane menu is owned by the ruler's own context_menu
            // call (`chord_lane_menu`); the grid's menu can't open it.
            Some(ContextTarget::ChordLane { .. }) => {}
            Some(ContextTarget::Ruler { step }) => {
                ui.label(
                    RichText::new(format!("At bar {}", ops::format_position(step).trim()))
                        .small()
                        .color(pal.text_faint),
                );
                ui.separator();
                self.structure_menu(ui, step);
            }
            None => {
                ui.label(
                    RichText::new("Right-click a lane or clip")
                        .small()
                        .color(pal.text_faint),
                );
                ui.separator();
                self.structure_menu(ui, self.transport.position);
            }
        }
    }

    /// The "Structure" submenu shared by the arranger and ruler context
    /// menus: named section markers at `step` (snapped to the arranger
    /// grid), a free-form "Insert marker…", and "Set song end here". Every
    /// choice runs through the undoable [`Self::edit`] path.
    fn structure_menu(&mut self, ui: &mut Ui, step: f64) {
        let at = snap_round(step, self.arranger.snap.steps()).max(0.0);
        ui.menu_button("Structure", |ui| {
            for name in ["Intro", "Verse", "Chorus", "Bridge", "Outro"] {
                if ui.button(name).clicked() {
                    self.add_marker_named(at, name.to_string());
                }
            }
            ui.separator();
            if ui.button("Insert marker…").clicked() {
                // Default name first, then the inline rename popup opens so
                // the name can be typed right away.
                if let Some(id) = self.add_marker_named(at, ops::next_marker_name(&self.project)) {
                    let text = self
                        .project
                        .markers
                        .iter()
                        .find(|m| m.id == id)
                        .map_or_else(String::new, |m| m.name.clone());
                    self.arranger.marker_rename = Some(MarkerRename { id, text });
                }
            }
        });
        let bars = bars_holding_step(step).clamp(ops::MIN_BARS, ops::MAX_BARS);
        if ui
            .add_enabled(
                self.project.bars != bars,
                egui::Button::new(format!("Set song end here (bar {bars})")),
            )
            .on_hover_text(format!(
                "End the song at the bar under the pointer (bars are capped at {}).",
                ops::MAX_BARS
            ))
            .clicked()
        {
            self.set_song_end(step);
        }
    }

    /// Adds a section marker named `name` at `step` (grid-snapped) through
    /// the undoable edit path; returns the new marker's id for the caller
    /// to open the rename popup on, and warns when the cap is reached.
    fn add_marker_named(&mut self, step: f64, name: String) -> Option<u64> {
        let step = snap_round(step, self.arranger.snap.steps()).max(0.0);
        let mut id = None;
        self.edit("Add marker", None, |p| {
            id = ops::add_marker(p, step, Some(name))
        });
        if id.is_none() {
            self.set_status(
                StatusKind::Warning,
                format!("Songs are limited to {} section markers.", ops::MAX_MARKERS),
            );
        }
        id
    }

    /// Sets the song length so it ends on the bar containing `step`, then
    /// grows it back to fit any clips past that point. Clamped to
    /// [`ops::MIN_BARS`]..[`ops::MAX_BARS`].
    fn set_song_end(&mut self, step: f64) {
        let bars = bars_holding_step(step).clamp(ops::MIN_BARS, ops::MAX_BARS);
        self.edit("Set song end", None, |p| {
            p.bars = bars;
            ops::fit_bars_to_content(p);
        });
    }

    fn arranger_ruler_input(
        &mut self,
        ui: &mut Ui,
        resp: &egui::Response,
        ruler: Rect,
        tl: Timeline,
        song_steps: f64,
    ) {
        let lane = Rect::from_min_max(ruler.min, pos2(ruler.right(), ruler.top() + LOOP_LANE_H));
        let chords = chord_lane_rect(ruler);
        if resp.hovered() {
            let in_lane = resp.hover_pos().is_some_and(|p| lane.contains(p));
            ui.ctx().set_cursor_icon(if in_lane {
                CursorIcon::Crosshair
            } else {
                CursorIcon::PointingHand
            });
        }
        // Clicks targeting a flag chip run before the in-progress-drag
        // branch: the second press of a double-click is itself a press and
        // would otherwise be swallowed by the drag it started.
        if resp.double_clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                if let Some(id) = self.marker_at(ui.ctx(), pos, ruler, tl) {
                    self.arranger.drag = None;
                    if let Some(m) = self.project.markers.iter().find(|m| m.id == id) {
                        self.arranger.marker_rename = Some(MarkerRename {
                            id,
                            text: m.name.clone(),
                        });
                    }
                } else if !lane.contains(pos) && !chords.contains(pos) {
                    self.add_marker_at(tl.step(pos.x));
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                if chords.contains(pos) {
                    match self.chord_event_at(ui.ctx(), pos, ruler, tl) {
                        // Right-click deletes a chord outright, like a marker.
                        Some(id) => {
                            self.edit("Delete chord", None, |p| {
                                ops::delete_chord_event(p, id);
                            });
                            self.arranger.context = None;
                        }
                        // On the lane's empty area it opens the "Add chord"
                        // menu for the bar under the pointer.
                        None => {
                            let step =
                                snap_floor(tl.step(pos.x).max(0.0), ops::STEPS_PER_BAR as f64);
                            self.arranger.context = Some(ContextTarget::ChordLane { step });
                        }
                    }
                } else if let Some(id) = self.marker_at(ui.ctx(), pos, ruler, tl) {
                    self.arranger.context = None;
                    self.edit("Delete marker", None, |p| {
                        ops::delete_marker(p, id);
                    });
                    if self
                        .arranger
                        .marker_rename
                        .as_ref()
                        .is_some_and(|r| r.id == id)
                    {
                        self.arranger.marker_rename = None;
                    }
                } else {
                    // Bare seek strip: the ruler's own context menu opens
                    // the structure menu at the clicked step.
                    self.arranger.context = Some(ContextTarget::Ruler {
                        step: tl.step(pos.x).max(0.0),
                    });
                }
            }
        }
        // A marker or chord-chip drag owns the pointer until release and
        // suppresses seeking, so the playhead does not chase the chip.
        // `take` moves the drag out of the view for the frame instead of
        // cloning it; both arms return, so only the fallthrough puts it back.
        match self.arranger.drag.take() {
            Some(ArrangerDrag::Marker { id, group }) => {
                let (primary_down, released) =
                    ui.input(|i| (i.pointer.primary_down(), i.pointer.primary_released()));
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                if primary_down {
                    if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                        let step = snap_round(tl.step(pos.x), self.arranger.snap.steps()).max(0.0);
                        self.edit("Move marker", Some(&group), move |p| {
                            ops::move_marker(p, id, step);
                        });
                    }
                }
                if released || !primary_down {
                    self.arranger.drag = None;
                }
                return;
            }
            Some(ArrangerDrag::Chord { id, group, moved }) => {
                let (primary_down, released, shift) = ui.input(|i| {
                    (
                        i.pointer.primary_down(),
                        i.pointer.primary_released(),
                        i.modifiers.shift,
                    )
                });
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                let mut moved = moved;
                if primary_down {
                    if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                        // Chords are per-bar labels: moves snap to bar lines.
                        let step = snap_round(tl.step(pos.x), ops::STEPS_PER_BAR as f64).max(0.0);
                        if self
                            .project
                            .chord_events
                            .iter()
                            .find(|e| e.id == id)
                            .is_some_and(|e| e.step != step)
                        {
                            moved |= self.edit("Move chord", Some(&group), move |p| {
                                ops::move_chord_event(p, id, step);
                            });
                        }
                    }
                }
                if released || !primary_down {
                    if !moved {
                        // A press that never moved reads as a click: plain
                        // cycles the quality, Shift-click cycles the root.
                        if shift {
                            self.edit("Chord root", None, move |p| {
                                ops::cycle_chord_event_root(p, id, 1);
                            });
                        } else {
                            self.edit("Chord quality", None, move |p| {
                                ops::cycle_chord_event_quality(p, id, 1);
                            });
                        }
                    }
                    self.arranger.drag = None;
                } else {
                    self.arranger.drag = Some(ArrangerDrag::Chord { id, group, moved });
                }
                return;
            }
            // Any other drag (clip fades, automation, …) belongs to the
            // grid handler: put it back untouched.
            other => self.arranger.drag = other,
        }
        if resp.is_pointer_button_down_on() {
            // Where the press *landed* picks the gesture for its whole
            // duration: the top lane draws a loop region, a chord chip
            // starts its drag (an empty-lane press does nothing until
            // release, where `clicked` below writes a chord), a flag chip
            // starts a marker drag, and the rest of the ruler seeks as
            // always — a drag crossing a strip border keeps whichever
            // behavior it started with.
            match ui.input(|i| i.pointer.press_origin()) {
                Some(origin) if lane.contains(origin) => {
                    self.focus = Focus::Arranger;
                    self.loop_lane_drag(origin, resp, lane, tl, song_steps);
                }
                Some(origin) if chords.contains(origin) => {
                    self.focus = Focus::Arranger;
                    if let Some(id) = self.chord_event_at(ui.ctx(), origin, ruler, tl) {
                        self.arranger.drag = Some(ArrangerDrag::Chord {
                            id,
                            group: format!("chord-move-{id}"),
                            moved: false,
                        });
                    }
                }
                Some(origin)
                    if self.arranger.marker_rename.is_none()
                        && self.marker_at(ui.ctx(), origin, ruler, tl).is_some() =>
                {
                    self.focus = Focus::Arranger;
                    let id = self.marker_at(ui.ctx(), origin, ruler, tl).unwrap();
                    self.arranger.drag = Some(ArrangerDrag::Marker {
                        id,
                        group: format!("marker-move-{id}"),
                    });
                }
                _ => {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let exact = ui.input(|i| i.modifiers.shift);
                        let raw = tl.step(pos.x).max(0.0);
                        let step = if exact {
                            raw.floor()
                        } else {
                            snap_round(raw, 4.0)
                        };
                        if self.arranger.last_seek != Some(step) {
                            self.arranger.last_seek = Some(step);
                            self.seek(step);
                        }
                    }
                }
            }
        }
        // A plain click on the chord strip's empty area writes the key's
        // tonic chord on that bar — or cycles the chord already there.
        // Both press and release must land on the strip (a gesture sliding
        // in from elsewhere doesn't write), and the whole thing is skipped
        // while the strip's own context menu is open, so a click that
        // dismisses the menu doesn't also write a chord.
        if resp.clicked() && self.arranger.marker_rename.is_none() && !resp.context_menu_opened() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let origin_ok = ui
                    .input(|i| i.pointer.press_origin())
                    .is_none_or(|o| chords.contains(o));
                if chords.contains(pos) && origin_ok {
                    self.focus = Focus::Arranger;
                    let step = snap_floor(tl.step(pos.x).max(0.0), ops::STEPS_PER_BAR as f64);
                    if let Some(id) = ops::chord_event_at_step(&self.project, step) {
                        self.edit("Chord quality", None, move |p| {
                            ops::cycle_chord_event_quality(p, id, 1);
                        });
                    } else {
                        self.add_tonic_chord_at(step);
                    }
                }
            }
        }
        // Right-clicking the chord strip's empty area opens the "Add chord"
        // menu; keep it attached while that target stays current. A
        // right-click on the bare seek strip opens the structure menu
        // instead.
        if matches!(self.arranger.context, Some(ContextTarget::ChordLane { .. })) {
            resp.context_menu(|ui| self.chord_lane_menu(ui));
        } else if matches!(self.arranger.context, Some(ContextTarget::Ruler { .. })) {
            resp.context_menu(|ui| self.arranger_context_menu(ui));
        }
        let _ = resp.clone().on_hover_text(
            "Click or drag to move the playhead (snaps to beats; hold Shift for 1/16)\nDrag the thin top strip to set a loop region\nDouble-click to add a section marker · flags: drag to move, double-click to rename, right-click to delete\nRight-click the ruler for the Structure menu (markers, song end)\nChord strip (bottom): click adds the key's chord, chip click cycles the quality, Shift-click the root, drag moves, right-click deletes",
        );
    }

    /// The chord strip's right-click menu on an empty spot: the seven
    /// diatonic chords of the song key first (with roman numerals), then
    /// root and quality pickers for anything else. Every choice writes on
    /// the bar the menu was opened on.
    fn chord_lane_menu(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        let Some(ContextTarget::ChordLane { step }) = self.arranger.context else {
            return;
        };
        ui.label(
            RichText::new(format!("Chord at bar {}", bar_number(step)))
                .small()
                .color(pal.text_faint),
        );
        let key = self.project.key;
        for dc in ops::diatonic_chords(key) {
            if ui
                .button(format!(
                    "{} — {}",
                    dc.spec.symbol(),
                    key.degree_numeral(dc.degree)
                ))
                .clicked()
            {
                self.set_or_add_chord(step, dc.spec.root, dc.spec.quality);
            }
        }
        ui.separator();
        // The pickers keep the chord's other half: choosing a root keeps
        // the existing quality (or the key's tonic quality when adding).
        ui.menu_button("Root…", |ui| {
            for root in 0..12u8 {
                if ui.button(NOTE_NAMES[root as usize]).clicked() {
                    let quality = ops::chord_event_at_step(&self.project, step)
                        .and_then(|id| self.project.chord_events.iter().find(|e| e.id == id))
                        .map_or(ops::tonic_chord_spec(key).quality, |e| e.quality);
                    self.set_or_add_chord(step, root, quality);
                }
            }
        });
        ui.menu_button("Quality…", |ui| {
            for q in ChordQuality::ALL {
                if ui
                    .button(format!("{} ({})", q.label(), q.symbol(key.root)))
                    .clicked()
                {
                    let root = ops::chord_event_at_step(&self.project, step)
                        .and_then(|id| self.project.chord_events.iter().find(|e| e.id == id))
                        .map_or(ops::tonic_chord_spec(key).root, |e| e.root);
                    self.set_or_add_chord(step, root, q);
                }
            }
        });
    }

    /// The chord a plain click on the strip's empty area writes on the bar
    /// at `step`: the song key's tonic (I in major, i in minor).
    fn add_tonic_chord_at(&mut self, step: f64) {
        let spec = ops::tonic_chord_spec(self.project.key);
        self.set_or_add_chord(step, spec.root, spec.quality);
    }

    /// Writes `root`/`quality` on the bar at `step`: replaces the chord
    /// already there when one sits on that bar (the strip keeps one label
    /// per bar), else adds an event through [`ops::add_chord_event`].
    fn set_or_add_chord(&mut self, step: f64, root: u8, quality: ChordQuality) {
        if let Some(id) = ops::chord_event_at_step(&self.project, step) {
            self.edit("Set chord", None, move |p| {
                ops::set_chord_event(p, id, root, quality);
            });
            return;
        }
        // `added` is borrowed, not moved, so the cap check below sees the
        // id `add_chord_event` actually returned (None = list is full).
        let mut added = None;
        self.edit("Add chord", None, |p| {
            added = ops::add_chord_event(p, step, root, quality);
        });
        if added.is_none() {
            self.set_status(
                StatusKind::Warning,
                format!(
                    "Songs are limited to {} chord events.",
                    ops::MAX_CHORD_EVENTS
                ),
            );
        }
    }

    /// Drag-and-drop from the instrument library. While a
    /// [`sidebar::QuickSound`] is dragged over `drop_area` (the lanes,
    /// headers and scrollbars — everything under the ruler) this reports
    /// the track-list index a drop would insert at plus the dragged sound,
    /// for the ghost lane, and steals the `Copy` cursor. On the release
    /// frame the payload still lives — egui clears it at end-of-pass — so
    /// this instead creates the track at the release lane and returns
    /// `None`, ending the ghost with the gesture.
    fn library_drop(
        &mut self,
        ctx: &egui::Context,
        drop_area: Rect,
    ) -> Option<(usize, Arc<sidebar::QuickSound>)> {
        let sound = egui::DragAndDrop::payload::<sidebar::QuickSound>(ctx)?;
        let pos = ctx.input(|i| i.pointer.interact_pos())?;
        if !drop_area.contains(pos) {
            return None;
        }
        ctx.set_cursor_icon(CursorIcon::Copy);
        // Fast path: uniform LANE_H rows whenever no automation strip is
        // open; otherwise walk the cumulative (variable) row heights so the
        // insertion slot still lands under the pointer.
        let index = if self.arranger.auto_lanes.is_empty() {
            drop_lane_index(
                pos.y,
                drop_area.top(),
                self.arranger.scroll_y,
                self.project.tracks.len(),
            )
        } else {
            let mut top = drop_area.top() - self.arranger.scroll_y;
            let mut index = self.project.tracks.len();
            for (i, t) in self.project.tracks.iter().enumerate() {
                if pos.y < top + self.track_row_height(t.id) {
                    index = i;
                    break;
                }
                top += self.track_row_height(t.id);
            }
            index
        };
        if ctx.input(|i| i.pointer.any_released()) {
            // The add runs through the same undoable path as the library's
            // + button; then the payload is consumed so nothing else can
            // also take it (egui clears leftovers at end-of-pass anyway).
            self.add_sound_track_at(&sound, index);
            egui::DragAndDrop::take_payload::<sidebar::QuickSound>(ctx);
            return None;
        }
        Some((index, sound))
    }

    /// The ghost lane painted where a dragged library sound would land: a
    /// translucent lane the new track would occupy — spanning the headers
    /// and the grid, named and colored after the sound — with a solid
    /// insertion line along its top edge (tracks below it shift down).
    fn paint_library_drop(
        &self,
        p: &Painter,
        drop_area: Rect,
        index: usize,
        sound: &sidebar::QuickSound,
    ) {
        let pal = self.palette();
        let color = theme::rgb(sound.color());
        let top = drop_area.top() + self.lane_top(index) - self.arranger.scroll_y;
        let lane = Rect::from_min_max(
            pos2(drop_area.left(), top),
            pos2(drop_area.right(), top + LANE_H),
        );
        let clip = lane.intersect(drop_area);
        if clip.is_positive() {
            p.rect_filled(clip, 0.0, color.gamma_multiply(0.14));
            p.text(
                clip.center(),
                Align2::CENTER_CENTER,
                format!("Add {}", sound.name()),
                FontId::proportional(11.5),
                pal.text,
            );
        }
        if (drop_area.top()..=drop_area.bottom()).contains(&lane.top()) {
            p.hline(drop_area.x_range(), lane.top(), Stroke::new(2.0_f32, color));
        }
    }

    /// The flag chip `pos` is over, if any. Chips are hit-tested against
    /// their painted rects so the bare ruler strip between them keeps its
    /// seek behavior and clip/lane input is never disturbed.
    fn marker_at(&self, ctx: &egui::Context, pos: Pos2, ruler: Rect, tl: Timeline) -> Option<u64> {
        self.project
            .markers
            .iter()
            .rev()
            .find(|m| marker_chip(ctx, m, ruler, tl).is_some_and(|r| r.contains(pos)))
            .map(|m| m.id)
    }

    /// The chord chip `pos` is over, if any. Like [`marker_at`], chips are
    /// hit-tested against their painted rects so the bare lane between
    /// them keeps its click-to-add behavior.
    fn chord_event_at(
        &self,
        ctx: &egui::Context,
        pos: Pos2,
        ruler: Rect,
        tl: Timeline,
    ) -> Option<u64> {
        self.project
            .chord_events
            .iter()
            .rev()
            .find(|ev| chord_chip(ctx, ev, ruler, tl).is_some_and(|r| r.contains(pos)))
            .map(|ev| ev.id)
    }

    /// Adds a section marker at `step` (snapped to the arranger grid) through
    /// the undoable edit path; the default section name comes from
    /// [`ops::next_marker_name`].
    fn add_marker_at(&mut self, step: f64) {
        let step = snap_round(step, self.arranger.snap.steps()).max(0.0);
        let mut id = None;
        self.edit("Add marker", None, |p| id = ops::add_marker(p, step, None));
        if id.is_none() {
            self.set_status(
                StatusKind::Warning,
                format!("Songs are limited to {} section markers.", ops::MAX_MARKERS),
            );
        }
    }

    /// The floating rename field opened by double-clicking a marker flag.
    /// Enter (or the field losing focus) commits through `edit`; Escape
    /// cancels. Anchored just under the flag so it floats over the lanes.
    fn marker_rename_ui(&mut self, ui: &mut Ui, ruler: Rect, tl: Timeline) {
        let Some(ed) = self.arranger.marker_rename.clone() else {
            return;
        };
        let Some(m) = self.project.markers.iter().find(|m| m.id == ed.id) else {
            self.arranger.marker_rename = None;
            return;
        };
        let x = tl
            .x(m.step)
            .max(ruler.left())
            .min((ruler.right() - 152.0).max(ruler.left()));
        let mut text = ed.text.clone();
        let mut commit = false;
        let mut cancel = false;
        let area_id = ui.id().with(("marker_rename", ed.id));
        let area = egui::Area::new(area_id)
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(x, ruler.bottom() + 2.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut text)
                            .id(area_id.with("field"))
                            .hint_text("Section name")
                            .desired_width(140.0),
                    );
                    if !resp.has_focus() {
                        resp.request_focus();
                    }
                    commit |= resp.lost_focus()
                        || (resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                    cancel |= ui.input(|i| i.key_pressed(egui::Key::Escape));
                });
            });
        // A click anywhere outside the popup commits, like Enter.
        if ui.input(|i| i.pointer.primary_pressed()) {
            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                commit |= !area.response.rect.contains(pos);
            }
        }
        if let Some(ed) = self.arranger.marker_rename.as_mut() {
            ed.text = text;
        }
        if commit {
            if let Some(ed) = self.arranger.marker_rename.take() {
                self.edit("Rename marker", None, |p| {
                    ops::rename_marker(p, ed.id, &ed.text);
                });
            }
        } else if cancel {
            self.arranger.marker_rename = None;
        }
    }

    /// A gesture on the loop lane: on its first frame the band's ✕ clears
    /// the region (the same drag may still draw a fresh one); every frame
    /// the touched span becomes `transport.loop_region` and a live
    /// [`AudioEngine::set_loop_region`] keeps the wrap bounds in sync while
    /// the song keeps playing.
    fn loop_lane_drag(
        &mut self,
        origin: Pos2,
        resp: &egui::Response,
        lane: Rect,
        tl: Timeline,
        song_steps: f64,
    ) {
        if self.arranger.region_press.is_none() {
            if let Some((a, b)) = self.transport.loop_region {
                let band = loop_band_rect(lane, tl, a, b);
                if band.width() >= 16.0 && loop_clear_rect(band).contains(origin) {
                    self.transport.loop_region = None;
                    if let Some(engine) = &self.engine {
                        engine.set_loop_region(None);
                    }
                }
            }
            self.arranger.region_press = Some(tl.step(origin.x).max(0.0));
        }
        let pos = resp.interact_pointer_pos();
        if let (Some(anchor), Some(pos)) = (self.arranger.region_press, pos) {
            let grid = self.arranger.snap.steps();
            if let Some(region) = loop_region_from_drag(anchor, tl.step(pos.x), grid, song_steps) {
                if self.transport.loop_region != Some(region) {
                    self.transport.loop_region = Some(region);
                    if let Some(engine) = &self.engine {
                        engine.set_loop_region(Some(region));
                    }
                }
            }
        }
    }

    fn paint_lanes(&self, p: &Painter, grid: Rect, tl: Timeline, song_steps: f64) {
        let pal = self.palette();
        p.rect_filled(grid, 0.0, pal.canvas);
        let first = self.first_visible_lane();
        for (i, track) in self.project.tracks.iter().enumerate().skip(first) {
            let y = grid.top() + self.lane_top(i) - self.arranger.scroll_y;
            if y > grid.bottom() {
                break;
            }
            let r = Rect::from_min_size(pos2(grid.left(), y), vec2(grid.width(), LANE_H));
            let selected = self.sel.track == Some(track.id);
            let fill = if selected {
                theme::mix(pal.canvas_alt, theme::rgb(track.color), 0.06)
            } else if i % 2 == 1 {
                pal.canvas_alt
            } else {
                pal.canvas
            };
            p.rect_filled(r, 0.0, fill);
            p.hline(
                grid.x_range(),
                r.bottom(),
                Stroke::new(1.0_f32, pal.border_soft),
            );
        }
        self.paint_bar_lines(p, grid, tl);
        let end = tl.x(song_steps);
        if end < grid.right() {
            p.rect_filled(
                Rect::from_min_max(pos2(end.max(grid.left()), grid.top()), grid.max),
                0.0,
                Color32::from_black_alpha(80),
            );
            p.vline(
                end,
                grid.y_range(),
                Stroke::new(1.5_f32, pal.danger.gamma_multiply(0.6)),
            );
        }
        let rows_end =
            grid.top() + self.lane_top(self.project.tracks.len()) - self.arranger.scroll_y;
        if rows_end < grid.bottom() {
            p.rect_filled(
                Rect::from_min_max(pos2(grid.left(), rows_end), grid.max),
                0.0,
                pal.bg,
            );
        }
    }

    /// Translucent section bands behind the arrangement: one tinted band per
    /// gap between consecutive markers (song start → first marker → … → song
    /// end), cycling [`section_band_tints`]. Pure paint over the lane fills
    /// and under the clips, so the song's structure reads without touching
    /// the model.
    fn paint_section_bands(&self, p: &Painter, grid: Rect, tl: Timeline) {
        let ranges = section_band_ranges(&self.project.markers, self.project.total_steps());
        if ranges.is_empty() {
            return;
        }
        let tints = section_band_tints();
        for (i, (a, b)) in ranges.iter().enumerate() {
            let x0 = tl.x(*a).max(grid.left());
            let x1 = tl.x(*b).min(grid.right());
            if x1 - x0 < 1.0 {
                continue;
            }
            p.rect_filled(
                Rect::from_min_max(pos2(x0, grid.top()), pos2(x1, grid.bottom())),
                0.0,
                tints[i % tints.len()],
            );
        }
    }

    fn paint_bar_lines(&self, p: &Painter, grid: Rect, tl: Timeline) {
        let pal = self.palette();
        let pps = tl.pps;
        let s0 = tl.step(grid.left()).max(0.0).floor() as i64;
        let s1 = tl.step(grid.right()).ceil() as i64;
        let beat_lines = pps * 4.0 >= 9.0;
        let bar_every = bar_label_every(pps);
        let start = s0 - s0 % 4;
        let mut s = start;
        while s <= s1 {
            let x = tl.x(s as f64);
            if s % 16 == 0 {
                let strong = (s / 16) % bar_every as i64 == 0;
                p.vline(
                    x,
                    grid.y_range(),
                    Stroke::new(1.0_f32, if strong { pal.grid_bar } else { pal.grid_beat }),
                );
            } else if beat_lines {
                p.vline(x, grid.y_range(), Stroke::new(1.0_f32, pal.grid_faint));
            }
            s += 4;
        }
    }

    fn paint_clips(&self, p: &Painter, grid: Rect, tl: Timeline) {
        let pal = self.palette();
        let first = self.first_visible_lane();
        for (i, track) in self.project.tracks.iter().enumerate().skip(first) {
            let y = grid.top() + self.lane_top(i) - self.arranger.scroll_y;
            if y > grid.bottom() {
                break;
            }
            let audible = self.project.is_track_audible(i);
            // Inaudible tracks paint at reduced opacity so the arrangement
            // still shows *what* is there without implying it is heard.
            let color = theme::rgb(track.color);
            let color = if audible { color } else { dimmed(color, 0.45) };
            // One id→pattern map per track per frame, so the clip loop does
            // not re-run the linear pattern find for every clip.
            let patterns: HashMap<u64, &Pattern> =
                track.patterns.iter().map(|p| (p.id, p)).collect();
            for clip in &track.clips {
                // Inline clip length from the map (same math as
                // `ops::clip_len`, without a second linear find).
                let pat_len = patterns
                    .get(&clip.pattern_id)
                    .map_or(ops::STEPS_PER_BAR as f64, |p| p.length.max(1) as f64);
                let len = pat_len * clip.loop_count() as f64;
                let (x0, x1) = (tl.x(clip.start), tl.x(clip.start + len));
                if x1 < grid.left() || x0 > grid.right() {
                    continue;
                }
                let r =
                    Rect::from_min_max(pos2(x0 + 1.0, y + 4.0), pos2(x1 - 1.0, y + LANE_H - 4.0));
                let selected = self.selected_clip_ids().contains(&clip.id);
                paint_clip(
                    p,
                    pal,
                    r,
                    track,
                    clip,
                    patterns.get(&clip.pattern_id).copied(),
                    color,
                    selected,
                    audible,
                    tl.pps,
                );
            }
            // Imported-WAV clips share the lane; they paint as a hatched
            // block with the file name rather than a mini piano roll —
            // recognizably a different kind of content at a glance.
            for clip in &track.audio_clips {
                let len = clip.span();
                let (x0, x1) = (tl.x(clip.start), tl.x(clip.start + len));
                if x1 < grid.left() || x0 > grid.right() {
                    continue;
                }
                let r =
                    Rect::from_min_max(pos2(x0 + 1.0, y + 4.0), pos2(x1 - 1.0, y + LANE_H - 4.0));
                let selected = self.selected_clip_ids().contains(&clip.id);
                paint_audio_clip(p, pal, r, clip, color, selected, audible, tl.pps);
            }
        }
    }

    /// The open automation strips under their clip lanes: a recessed band in
    /// the track's tint with the piecewise-linear curve (flat at the ends),
    /// its breakpoints, and a hairline at the lane's neutral value so the
    /// curve reads against "as mixed". Click places a point, drag moves it,
    /// right-click deletes — the gestures live in `arranger_grid_input`.
    fn paint_automation(&self, p: &Painter, grid: Rect, tl: Timeline) {
        if self.arranger.auto_lanes.is_empty() {
            return;
        }
        let pal = self.palette();
        let first = self.first_visible_lane();
        for (i, track) in self.project.tracks.iter().enumerate().skip(first) {
            let Some(param) = self.arranger.auto_lanes.get(&track.id).copied() else {
                continue;
            };
            let y = grid.top() + self.lane_top(i) + LANE_H - self.arranger.scroll_y;
            if y > grid.bottom() {
                break;
            }
            if y + AUTO_LANE_H <= grid.top() {
                continue;
            }
            let r = Rect::from_min_max(pos2(grid.left(), y), pos2(grid.right(), y + AUTO_LANE_H));
            let color = theme::rgb(track.color);
            p.rect_filled(r, 0.0, theme::mix(pal.canvas, color, 0.10));
            p.hline(
                r.x_range(),
                r.bottom(),
                Stroke::new(1.0_f32, pal.border_soft),
            );
            // The neutral value (×1 / centre) gets a hairline: it is both the
            // "as mixed" reading and the empty lane's click-hint.
            let neutral_y = auto_value_to_y(param, Automation::neutral(param), r);
            p.hline(
                r.x_range(),
                neutral_y,
                Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.35)),
            );
            p.text(
                pos2(r.left() + 4.0, r.top() + 2.0),
                Align2::LEFT_TOP,
                match param {
                    AutomationLane::Volume => "VOL",
                    AutomationLane::Pan => "PAN",
                },
                FontId::proportional(9.0),
                pal.text_faint,
            );
            let points = track
                .automation
                .as_ref()
                .map_or(&[][..], |a| a.lane(param).as_slice());
            if let Some((head, rest)) = points.split_first() {
                // Flat at the first point's value to the left edge, then the
                // polyline, then flat at the last point's value to the right.
                let mut path = vec![
                    pos2(grid.left() - 2.0, auto_value_to_y(param, head.value, r)),
                    pos2(tl.x(head.step), auto_value_to_y(param, head.value, r)),
                ];
                for pt in rest {
                    path.push(pos2(tl.x(pt.step), auto_value_to_y(param, pt.value, r)));
                }
                let tail = path.last().unwrap().y;
                path.push(pos2(grid.right() + 2.0, tail));
                p.add(Shape::line(path, Stroke::new(1.5_f32, color)));
                for pt in points {
                    let c = pos2(tl.x(pt.step), auto_value_to_y(param, pt.value, r));
                    if !(r.left() - 4.0..=r.right() + 4.0).contains(&c.x) {
                        continue;
                    }
                    p.circle(c, 3.0, color, Stroke::new(1.0_f32, pal.canvas));
                }
            }
        }
    }

    fn paint_arranger_marquee(&self, p: &Painter) {
        let pal = self.palette();
        if let Some(m) = &self.arranger.marquee {
            let r = Rect::from_two_pos(m.start, m.current).intersect(self.arranger.grid);
            if r.is_positive() {
                p.rect_filled(r, 0.0, pal.link.gamma_multiply(0.12));
                p.rect_stroke(r, 0.0, Stroke::new(1.0_f32, pal.link), StrokeKind::Inside);
            }
        }
    }

    /// Faint wash over the loop region across all lanes plus edge markers;
    /// accent-colored while looping is armed, grey while it is only a marker.
    fn paint_loop_region(&self, p: &Painter, grid: Rect, tl: Timeline) {
        let Some((a, b)) = self.transport.loop_region else {
            return;
        };
        let x0 = tl.x(a).max(grid.left());
        let x1 = tl.x(b).min(grid.right());
        if x1 - x0 < 1.0 {
            return;
        }
        let pal = self.palette();
        let c = if self.transport.looping {
            pal.accent
        } else {
            pal.text_faint
        };
        p.rect_filled(
            Rect::from_min_max(pos2(x0, grid.top()), pos2(x1, grid.bottom())),
            0.0,
            c.gamma_multiply(0.06),
        );
        p.vline(
            x0,
            grid.y_range(),
            Stroke::new(1.0_f32, c.gamma_multiply(0.5)),
        );
        p.vline(
            x1,
            grid.y_range(),
            Stroke::new(1.0_f32, c.gamma_multiply(0.5)),
        );
    }

    /// Hairline section guides dropping from each marker flag through the
    /// lanes, so a marker's boundary reads inside the arrangement too.
    fn paint_marker_guides(&self, p: &Painter, grid: Rect, tl: Timeline) {
        if self.project.markers.is_empty() {
            return;
        }
        let c = self.palette().warn.gamma_multiply(0.28);
        for m in &self.project.markers {
            let x = tl.x(m.step);
            if x >= grid.left() && x <= grid.right() {
                p.vline(x, grid.y_range(), Stroke::new(1.0_f32, c));
            }
        }
    }

    fn paint_ruler(&self, ui: &Ui, ruler: Rect, tl: Timeline, song_steps: f64) {
        let pal = self.palette();
        let p = ui.painter_at(ruler);
        p.rect_filled(ruler, 0.0, pal.panel);
        // The thin top lane is the loop strip: it carries the region band
        // while the rest of the ruler stays the seek surface.
        let lane = Rect::from_min_max(ruler.min, pos2(ruler.right(), ruler.top() + LOOP_LANE_H));
        p.rect_filled(lane, 0.0, theme::mix(pal.panel, pal.canvas, 0.55));
        // The middle strip keeps the pre-chord-lane ruler geometry — bar
        // numbers, marker flags and the playhead tip anchor to it — and
        // the bottom strip is the chord lane, painted slightly sunken.
        let strip = number_strip_rect(ruler);
        let chords = chord_lane_rect(ruler);
        p.rect_filled(chords, 0.0, theme::mix(pal.panel, pal.canvas, 0.65));
        if let Some((a, b)) = self.transport.loop_region {
            let band = loop_band_rect(lane, tl, a, b);
            if band.is_positive() {
                let c = if self.transport.looping {
                    pal.accent
                } else {
                    pal.text_faint
                };
                p.rect_filled(band, 2.0, c.gamma_multiply(0.5));
                p.rect_stroke(band, 2.0, Stroke::new(1.0_f32, c), StrokeKind::Inside);
                if band.width() > 40.0 {
                    p.text(
                        pos2(band.left() + 4.0, band.center().y),
                        Align2::LEFT_CENTER,
                        format!("{}–{}", bar_number(a), bar_number_end(b)),
                        FontId::proportional(9.0),
                        pal.text,
                    );
                }
                if band.width() >= 16.0 {
                    let clear = loop_clear_rect(band);
                    p.text(
                        clear.center(),
                        Align2::CENTER_CENTER,
                        "✕",
                        FontId::proportional(9.0),
                        pal.text,
                    );
                    // Hover-only interact: the ruler's click handler owns
                    // the actual clearing in `loop_lane_drag`.
                    ui.interact(
                        clear,
                        ui.id().with(("loop_clear", a.to_bits(), b.to_bits())),
                        Sense::hover(),
                    )
                    .on_hover_text("Clear loop region");
                }
            }
        }
        p.hline(
            lane.x_range(),
            lane.bottom(),
            Stroke::new(1.0_f32, pal.border_soft),
        );
        // The seam between the number strip and the chord lane.
        p.hline(
            ruler.x_range(),
            chords.top(),
            Stroke::new(1.0_f32, pal.border_soft),
        );
        let every = bar_label_every(tl.pps) as i64;
        let b0 = (tl.step(ruler.left()).max(0.0) / 16.0).floor() as i64;
        let b1 = (tl.step(ruler.right()) / 16.0).ceil() as i64;
        for b in b0..=b1 {
            let x = tl.x(b as f64 * 16.0);
            if b % every == 0 {
                p.vline(
                    x,
                    strip.bottom() - 9.0..=strip.bottom(),
                    Stroke::new(1.0_f32, pal.text_faint),
                );
                p.text(
                    pos2(x + 4.0, strip.center().y - 2.0),
                    Align2::LEFT_CENTER,
                    format!("{}", b + 1),
                    FontId::proportional(11.0),
                    if (b as f64) * 16.0 < song_steps {
                        pal.text_dim
                    } else {
                        pal.text_faint
                    },
                );
                // A matching tick in the chord lane marks the bar line the
                // chips snap to.
                p.vline(
                    x,
                    chords.top() + 3.0..=chords.bottom() - 3.0,
                    Stroke::new(1.0_f32, pal.border_soft),
                );
            } else {
                p.vline(
                    x,
                    strip.bottom() - 5.0..=strip.bottom(),
                    Stroke::new(1.0_f32, pal.border),
                );
            }
        }
        let end = tl.x(song_steps);
        p.text(
            pos2(end + 4.0, strip.bottom() - 3.0),
            Align2::LEFT_BOTTOM,
            "END",
            FontId::proportional(9.0),
            pal.danger,
        );
        let marker = tl.x(self.transport.start_marker);
        if (self.transport.start_marker - self.transport.position).abs() > 0.01 {
            p.add(Shape::convex_polygon(
                vec![
                    pos2(marker - 4.0, strip.bottom() - 7.0),
                    pos2(marker + 4.0, strip.bottom() - 7.0),
                    pos2(marker, strip.bottom()),
                ],
                pal.link,
                Stroke::NONE,
            ));
        }
        self.paint_marker_chips(ui, &p, ruler, tl);
        self.paint_chord_chips(ui, &p, ruler, tl);
        let x = tl.x(self.transport.position);
        p.add(Shape::convex_polygon(
            vec![
                pos2(x - 6.0, ruler.top() + 3.0),
                pos2(x + 6.0, ruler.top() + 3.0),
                pos2(x + 6.0, strip.bottom() - 9.0),
                pos2(x, strip.bottom()),
                pos2(x - 6.0, strip.bottom() - 9.0),
            ],
            pal.accent,
            Stroke::NONE,
        ));
        // A hairline continues the playhead across the chord lane, meeting
        // the playhead line the lanes draw below the ruler.
        p.vline(x, chords.y_range(), Stroke::new(1.5_f32, pal.accent));
        p.hline(
            ruler.x_range(),
            ruler.bottom() - 0.5,
            Stroke::new(1.0_f32, pal.border),
        );
    }

    /// Small colored flag chips naming each section marker, hung just under
    /// the loop lane. Painted after the bar numbers so an opaque chip can
    /// sit on a labeled bar, and before the playhead triangle so the
    /// playhead still wins the overlap. The hovered flag brightens.
    fn paint_marker_chips(&self, ui: &Ui, p: &Painter, ruler: Rect, tl: Timeline) {
        if self.project.markers.is_empty() {
            return;
        }
        let pal = self.palette();
        let col = pal.warn;
        let hovered = ui
            .input(|i| i.pointer.hover_pos())
            .and_then(|pos| self.marker_at(ui.ctx(), pos, ruler, tl));
        for m in &self.project.markers {
            let Some(chip) = marker_chip(ui.ctx(), m, ruler, tl) else {
                continue;
            };
            let lit = hovered == Some(m.id)
                || matches!(&self.arranger.drag, Some(ArrangerDrag::Marker { id, .. }) if *id == m.id);
            let fill = if lit {
                theme::mix(pal.panel, col, 0.72)
            } else {
                theme::mix(pal.panel, col, 0.5)
            };
            let ink = theme::ink_on(fill);
            let x = tl.x(m.step);
            // Flag pole under the pennant, then the rounded name pill.
            p.vline(
                x,
                chip.top() - 1.0..=ruler.bottom() - 1.0,
                Stroke::new(1.0_f32, col),
            );
            p.add(Shape::convex_polygon(
                vec![
                    pos2(x, chip.top() - 1.0),
                    pos2(x + 7.0, chip.top() + 2.0),
                    pos2(x, chip.top() + 5.0),
                ],
                col,
                Stroke::NONE,
            ));
            p.rect_filled(chip, 3.0, fill);
            p.rect_stroke(
                chip,
                3.0,
                Stroke::new(1.0_f32, col.gamma_multiply(0.85)),
                StrokeKind::Inside,
            );
            p.text(
                pos2(chip.left() + 9.0, chip.center().y + 0.5),
                Align2::LEFT_CENTER,
                &m.name,
                FontId::proportional(9.5),
                ink,
            );
            // Hover-only interact so the chip explains itself; clicks stay
            // with the ruler's input handler.
            ui.interact(chip, ui.id().with(("marker_chip", m.id)), Sense::hover())
                .on_hover_text(format!(
                    "{} — drag to move · double-click to rename · right-click to delete",
                    m.name
                ));
        }
    }

    /// Small pills naming each chord event on the ruler's bottom strip —
    /// the same chip idiom as marker flags but tinted `link` instead of
    /// `warn`, with a stubby stem tying the pill to its bar line. Painted
    /// before the playhead hairline so the playhead still wins overlaps;
    /// the hovered or dragged chip brightens. Sparse: bars without a
    /// chord event paint nothing.
    fn paint_chord_chips(&self, ui: &Ui, p: &Painter, ruler: Rect, tl: Timeline) {
        if self.project.chord_events.is_empty() {
            return;
        }
        let pal = self.palette();
        let col = pal.link;
        let hovered = ui
            .input(|i| i.pointer.hover_pos())
            .and_then(|pos| self.chord_event_at(ui.ctx(), pos, ruler, tl));
        for ev in &self.project.chord_events {
            let Some(chip) = chord_chip(ui.ctx(), ev, ruler, tl) else {
                continue;
            };
            let lit = hovered == Some(ev.id)
                || matches!(&self.arranger.drag, Some(ArrangerDrag::Chord { id, .. }) if *id == ev.id);
            let fill = if lit {
                theme::mix(pal.panel, col, 0.72)
            } else {
                theme::mix(pal.panel, col, 0.5)
            };
            let ink = theme::ink_on(fill);
            p.rect_filled(
                Rect::from_min_max(
                    pos2(tl.x(ev.step), chip.top()),
                    pos2(chip.left(), chip.bottom()),
                ),
                1.0,
                col,
            );
            p.rect_filled(chip, 3.0, fill);
            p.rect_stroke(
                chip,
                3.0,
                Stroke::new(1.0_f32, col.gamma_multiply(0.85)),
                StrokeKind::Inside,
            );
            p.text(
                pos2(chip.left() + 6.0, chip.center().y + 0.5),
                Align2::LEFT_CENTER,
                ops::chord_event_label(ev),
                FontId::proportional(10.0),
                ink,
            );
        }
    }

    fn lane_headers(&mut self, ui: &mut Ui, header: Rect, grid: Rect) {
        let pal = self.palette();
        let mut hui = ui.new_child(UiBuilder::new().max_rect(header));
        hui.set_clip_rect(header.intersect(ui.clip_rect()));
        hui.painter().rect_filled(header, 0.0, pal.panel);
        let first = self.first_visible_lane();
        let ids: Vec<u64> = self.project.tracks.iter().map(|t| t.id).collect();
        let mut action: Option<(u64, &str)> = None;
        for (i, id) in ids.iter().copied().enumerate().skip(first) {
            let y = grid.top() + self.lane_top(i) - self.arranger.scroll_y;
            if y > header.bottom() {
                break;
            }
            let Some(track) = ops::track(&self.project, id) else {
                continue;
            };
            let r = Rect::from_min_size(
                pos2(header.left(), y),
                vec2(HEADER_W, self.track_row_height(id)),
            );
            let resp = hui.interact(r, hui.id().with(("lane_header", id)), Sense::click());
            let selected = self.sel.track == Some(id);
            let color = theme::rgb(track.color);
            let audible = self.project.is_track_audible(i);
            let p = hui.painter();
            p.rect_filled(
                r,
                0.0,
                if selected {
                    pal.raised
                } else if resp.hovered() {
                    theme::mix(pal.panel, pal.raised, 0.5)
                } else {
                    pal.panel
                },
            );
            p.rect_filled(
                Rect::from_min_size(
                    r.min + vec2(0.0, 6.0),
                    vec2(if selected { 4.0 } else { 3.0 }, LANE_H - 12.0),
                ),
                2.0,
                if audible {
                    color
                } else {
                    color.gamma_multiply(0.35)
                },
            );
            p.hline(
                r.x_range(),
                r.bottom(),
                Stroke::new(1.0_f32, pal.border_soft),
            );
            let text_clip = Rect::from_min_max(r.min, pos2(r.right() - 64.0, r.bottom()));
            let pc = p.with_clip_rect(text_clip.intersect(hui.clip_rect()));
            pc.text(
                pos2(r.left() + 14.0, r.top() + 9.0),
                Align2::LEFT_TOP,
                &track.name,
                FontId::proportional(13.5),
                if audible { pal.text } else { pal.text_faint },
            );
            pc.text(
                pos2(r.left() + 14.0, r.top() + 29.0),
                Align2::LEFT_TOP,
                ops::instrument_label(track),
                FontId::proportional(10.5),
                pal.text_faint,
            );
            let m = Rect::from_min_size(pos2(r.right() - 58.0, r.top() + 15.0), vec2(24.0, 22.0));
            let s = Rect::from_min_size(pos2(r.right() - 30.0, r.top() + 15.0), vec2(24.0, 22.0));
            let mresp = hui.interact(m, hui.id().with(("mute", id)), Sense::click());
            let sresp = hui.interact(s, hui.id().with(("solo", id)), Sense::click());
            theme::paint_chip(&hui, pal, m, &mresp, "M", track.mute, pal.danger);
            theme::paint_chip(&hui, pal, s, &sresp, "S", track.solo, pal.warn);
            if mresp.on_hover_text("Mute").clicked() {
                action = Some((id, "mute"));
            }
            if sresp.on_hover_text("Solo").clicked() {
                action = Some((id, "solo"));
            }
            // The automation strip's bottom band of the header carries the
            // lane picker (VOL/PAN chips) and a close button — it is only
            // painted while the track's strip is open.
            if let Some(param) = self.arranger.auto_lanes.get(&id).copied() {
                let strip = Rect::from_min_max(
                    pos2(r.left(), r.top() + LANE_H),
                    pos2(r.right(), r.bottom()),
                );
                p.rect_filled(strip, 0.0, theme::mix(pal.panel, pal.canvas, 0.5));
                p.text(
                    pos2(strip.left() + 14.0, strip.center().y),
                    Align2::LEFT_CENTER,
                    "AUTO",
                    FontId::proportional(8.5),
                    pal.text_faint,
                );
                for (chip_i, (kind, label)) in [
                    (AutomationLane::Volume, "VOL"),
                    (AutomationLane::Pan, "PAN"),
                ]
                .into_iter()
                .enumerate()
                {
                    let cr = Rect::from_min_size(
                        pos2(
                            strip.right() - 96.0 + chip_i as f32 * 34.0,
                            strip.top() + 6.0,
                        ),
                        vec2(30.0, 16.0),
                    );
                    let cresp = hui.interact(
                        cr,
                        hui.id().with(("auto_param", id, chip_i)),
                        Sense::click(),
                    );
                    theme::paint_chip(&hui, pal, cr, &cresp, label, param == kind, pal.accent);
                    if cresp
                        .on_hover_text(format!("Edit {label} automation on the strip below"))
                        .clicked()
                    {
                        action = Some((
                            id,
                            if kind == AutomationLane::Volume {
                                "auto-vol"
                            } else {
                                "auto-pan"
                            },
                        ));
                    }
                }
                let x = Rect::from_min_size(
                    pos2(strip.right() - 26.0, strip.top() + 6.0),
                    vec2(16.0, 16.0),
                );
                let xresp = hui.interact(x, hui.id().with(("auto_hide", id)), Sense::click());
                theme::paint_chip(&hui, pal, x, &xresp, "✕", false, pal.text_faint);
                if xresp.on_hover_text("Hide automation lane").clicked() {
                    action = Some((id, "auto-hide"));
                }
            }
            let tip = format!(
                "{} · {}\nClick to select · right-click for track options",
                track.name,
                ops::instrument_label(track)
            );
            let resp = resp.on_hover_text(tip);
            if resp.clicked() {
                action = Some((id, "select"));
            }
            if resp.double_clicked() {
                action = Some((id, "inspect"));
            }
            let auto_open = self.arranger.auto_lanes.contains_key(&id);
            resp.context_menu(|ui| {
                if ui.button("Track settings…").clicked() {
                    action = Some((id, "inspect"));
                }
                if ui.button("Duplicate track").clicked() {
                    action = Some((id, "duplicate"));
                }
                if ui
                    .button(if auto_open {
                        "Hide automation"
                    } else {
                        "Show automation"
                    })
                    .clicked()
                {
                    action = Some((id, "auto-toggle"));
                }
                if ui.button("Move up").clicked() {
                    action = Some((id, "up"));
                }
                if ui.button("Move down").clicked() {
                    action = Some((id, "down"));
                }
                ui.separator();
                if ui
                    .button(RichText::new("Delete track").color(pal.danger))
                    .clicked()
                {
                    action = Some((id, "delete"));
                }
            });
        }
        let add_y = grid.top() + self.lane_top(ids.len()) - self.arranger.scroll_y + 8.0;
        if add_y < header.bottom() - 10.0 {
            let r = Rect::from_min_size(
                pos2(header.left() + 10.0, add_y),
                vec2(HEADER_W - 20.0, 28.0),
            );
            let full = self.project.tracks.len() >= ops::MAX_TRACKS;
            let b = egui::Button::new(
                RichText::new(if full {
                    "Track limit reached"
                } else {
                    "+  Add track"
                })
                .color(if full { pal.text_faint } else { pal.text_dim }),
            )
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0_f32, pal.border));
            if hui
                .put(r, b)
                .on_hover_text("Pick an instrument from the library")
                .clicked()
                && !full
            {
                self.side_tab = sidebar::SideTab::Library;
                self.set_status(
                    StatusKind::Info,
                    "Pick an instrument in the library on the left: + adds it, ▶ previews it.",
                );
            }
        }
        if let Some((id, what)) = action {
            self.focus = Focus::Arranger;
            match what {
                "mute" => self.toggle_mute(id),
                "solo" => self.toggle_solo(id),
                "select" => self.select_track(id),
                "inspect" => {
                    self.select_track(id);
                    self.side_tab = sidebar::SideTab::Track;
                }
                "duplicate" => {
                    let mut new = None;
                    self.edit("Duplicate track", None, |p| {
                        new = ops::duplicate_track(p, id)
                    });
                    match new {
                        Some(n) => self.select_track(n),
                        None => self.set_status(
                            StatusKind::Warning,
                            format!("Track limit reached ({} tracks).", ops::MAX_TRACKS),
                        ),
                    }
                }
                "up" | "down" => {
                    let d = if what == "up" { -1 } else { 1 };
                    self.edit("Move track", None, |p| {
                        ops::move_track(p, id, d);
                    });
                }
                "delete" => self.delete_track(id),
                // The lane chips/✕ and the menu toggle only change view state
                // (which lane an open strip edits, or whether it is open) —
                // never project data, so they bypass `edit` and undo.
                "auto-vol" => {
                    self.arranger.auto_lanes.insert(id, AutomationLane::Volume);
                }
                "auto-pan" => {
                    self.arranger.auto_lanes.insert(id, AutomationLane::Pan);
                }
                "auto-hide" => {
                    self.arranger.auto_lanes.remove(&id);
                }
                "auto-toggle" => self.toggle_automation_lane(id),
                _ => {}
            }
        }
    }
}

/// One pattern's length in steps for loop math (16 when the pattern is missing).
fn pattern_steps(track: &Track, clip: &Clip) -> f64 {
    ops::pattern(track, clip.pattern_id)
        .map_or(ops::STEPS_PER_BAR as f64, |p| p.length.max(1) as f64)
}

/// The pattern a clip plays — for selection when a hit only carried ids.
fn clip_pattern_id(project: &Project, track_id: u64, clip_id: u64) -> Option<u64> {
    ops::track(project, track_id)
        .and_then(|t| t.clips.iter().find(|c| c.id == clip_id))
        .map(|c| c.pattern_id)
}

/// 1-indexed bar holding `step` (for a region's left edge).
pub(super) fn bar_number(step: f64) -> i64 {
    (step.max(0.0) / ops::STEPS_PER_BAR as f64).floor() as i64 + 1
}

/// 1-indexed last bar inside a region whose exclusive end is `step`.
pub(super) fn bar_number_end(step: f64) -> i64 {
    (step.max(0.0) / ops::STEPS_PER_BAR as f64).ceil().max(1.0) as i64
}

/// The painted band rect for a loop region inside the ruler's top lane.
fn loop_band_rect(lane: Rect, tl: Timeline, a: f64, b: f64) -> Rect {
    Rect::from_min_max(
        pos2(tl.x(a).max(lane.left()), lane.top() + 1.0),
        pos2(tl.x(b).min(lane.right()), lane.bottom() - 1.0),
    )
}

/// The ruler's middle strip: bar numbers, section-marker flags and the
/// playhead tip live here. Its bottom edge is the pre-chord-lane
/// `ruler.bottom()`, so everything anchored to it keeps its old position.
fn number_strip_rect(ruler: Rect) -> Rect {
    Rect::from_min_max(
        pos2(ruler.left(), ruler.top() + LOOP_LANE_H),
        pos2(ruler.right(), ruler.top() + LOOP_LANE_H + NUMBER_STRIP_H),
    )
}

/// The ruler's bottom strip — the chord lane the per-bar chips ride on.
fn chord_lane_rect(ruler: Rect) -> Rect {
    Rect::from_min_max(
        pos2(ruler.left(), ruler.top() + LOOP_LANE_H + NUMBER_STRIP_H),
        pos2(ruler.right(), ruler.bottom()),
    )
}

/// The rounded flag chip painted for a marker on the ruler (flag pole plus
/// the name pill under the loop lane). `None` when the marker's step is
/// outside the visible ruler span — the pill extends right of the pole, so
/// near-edge markers still get a hit rect while partially visible.
fn marker_chip(ctx: &egui::Context, m: &Marker, ruler: Rect, tl: Timeline) -> Option<Rect> {
    let x = tl.x(m.step);
    if x < ruler.left() - 2.0 || x > ruler.right() + 2.0 {
        return None;
    }
    let name_w = ctx
        .fonts_mut(|f| f.layout_no_wrap(m.name.clone(), FontId::proportional(9.5), Color32::WHITE))
        .size()
        .x;
    let w = (9.0 + name_w + 5.0).max(16.0);
    Some(Rect::from_min_size(
        pos2(x - 1.0, ruler.top() + LOOP_LANE_H + 1.0),
        vec2(w, MARKER_CHIP_H),
    ))
}

/// The rounded pill painted for a chord event on the chord lane: it hangs
/// just right of the event's bar line, sized to the chord symbol (a 2 px
/// stem on the left ties it to the line). `None` when the event's step is
/// outside the visible ruler span — same edge convention as
/// [`marker_chip`].
fn chord_chip(ctx: &egui::Context, ev: &ChordEvent, ruler: Rect, tl: Timeline) -> Option<Rect> {
    let x = tl.x(ev.step);
    if x < ruler.left() - 2.0 || x > ruler.right() + 2.0 {
        return None;
    }
    let lane = chord_lane_rect(ruler);
    let name_w = ctx
        .fonts_mut(|f| {
            f.layout_no_wrap(
                ops::chord_event_label(ev),
                FontId::proportional(10.0),
                Color32::WHITE,
            )
        })
        .size()
        .x;
    let w = (name_w + 12.0).max(20.0);
    Some(Rect::from_min_size(
        pos2(x + 2.0, lane.top() + (lane.height() - CHORD_CHIP_H) * 0.5),
        vec2(w, CHORD_CHIP_H),
    ))
}

/// Bars between labelled bar lines so labels stay at least ~40 px apart.
fn bar_label_every(pps: f32) -> u32 {
    [1u32, 2, 4, 8, 16, 32]
        .into_iter()
        .find(|k| *k as f32 * 16.0 * pps >= 40.0)
        .unwrap_or(32)
}

#[allow(clippy::too_many_arguments)]
fn paint_clip(
    p: &Painter,
    pal: Palette,
    r: Rect,
    track: &Track,
    clip: &Clip,
    pattern: Option<&Pattern>,
    color: Color32,
    selected: bool,
    audible: bool,
    pps: f32,
) {
    if r.width() < 1.0 {
        return;
    }
    // Text/icons sit on the pastel track-color band: pick ink by luminance.
    let ink = theme::ink_on(color);
    let body = theme::mix(pal.canvas, color, if selected { 0.36 } else { 0.24 });
    p.rect_filled(r, 5.0, body);
    let band = Rect::from_min_max(r.min, pos2(r.right(), r.top() + 15.0));
    p.rect_filled(
        band,
        egui::CornerRadius {
            nw: 5,
            ne: 5,
            sw: 0,
            se: 0,
        },
        if audible {
            color
        } else {
            color.gamma_multiply(0.5)
        },
    );
    let clip_painter = p.with_clip_rect(r.intersect(p.clip_rect()));
    let loops = clip.loop_count();
    // One pattern's length in steps; looped repetitions tile across the clip.
    let pat_len = pattern.map_or(ops::STEPS_PER_BAR as f32, |p| p.length.max(1) as f32);
    let inner = Rect::from_min_max(
        pos2(r.left() + 2.0, band.bottom() + 3.0),
        pos2(r.right() - 2.0, r.bottom() - 3.0),
    );
    if let Some(pat) = pattern {
        if r.width() > 18.0 {
            clip_painter.text(
                pos2(r.left() + 5.0, band.center().y),
                Align2::LEFT_CENTER,
                &pat.name,
                FontId::proportional(10.5),
                ink,
            );
        }
        if pat.notes.is_empty() {
            if r.width() > 60.0 {
                clip_painter.text(
                    inner.center(),
                    Align2::CENTER_CENTER,
                    "empty",
                    FontId::proportional(10.0),
                    pal.text_faint,
                );
            }
        } else if inner.width() > 4.0 {
            let drums = track.shows_drum_grid();
            let rows = track.drum_rows();
            let (lo, hi) = if drums {
                (0.0, rows.len() as f32 - 1.0)
            } else {
                let lo = pat.notes.iter().map(|n| n.pitch).min().unwrap_or(60) as f32;
                let hi = pat.notes.iter().map(|n| n.pitch).max().unwrap_or(60) as f32;
                let mid = (lo + hi) * 0.5;
                let span = (hi - lo).max(12.0);
                (mid - span * 0.5, mid + span * 0.5)
            };
            let row = |pitch: u8| -> f32 {
                let v = if drums {
                    // Stray pitches park on the third row (clamped for small kits).
                    rows.iter()
                        .position(|(d, _)| *d == pitch)
                        .unwrap_or(2.min(rows.len().saturating_sub(1))) as f32
                } else {
                    pitch as f32
                };
                let t = if hi > lo { (v - lo) / (hi - lo) } else { 0.5 };
                if drums {
                    inner.top() + t * inner.height()
                } else {
                    inner.bottom() - t * inner.height()
                }
            };
            let note_color = if audible {
                theme::mix(color, pal.text, 0.35)
            } else {
                pal.text_faint
            };
            // The pattern's mini-notes repeat once per loop; the rect budget
            // is shared across all repetitions.
            let mut drawn = 0usize;
            'reps: for rep in 0..loops {
                let base = r.left() + rep as f32 * pat_len * pps;
                if base > r.right() {
                    break;
                }
                for n in &pat.notes {
                    if drawn >= 3000 {
                        break 'reps;
                    }
                    let x = base + n.start as f32 * pps;
                    if x > r.right() {
                        continue;
                    }
                    let w = (n.duration as f32 * pps).max(1.5).min(r.right() - x);
                    if w <= 0.0 {
                        continue;
                    }
                    let y = row(n.pitch);
                    clip_painter.rect_filled(
                        Rect::from_min_size(pos2(x, y - 1.0), vec2(w, 2.5)),
                        0.5,
                        note_color,
                    );
                    drawn += 1;
                }
            }
        }
    }
    // Faint separators at each loop boundary inside the clip body.
    if loops > 1 {
        let sep = Color32::from_rgba_unmultiplied(
            pal.text_faint.r(),
            pal.text_faint.g(),
            pal.text_faint.b(),
            110,
        );
        for rep in 1..loops {
            let x = r.left() + rep as f32 * pat_len * pps;
            if x <= r.left() || x >= r.right() {
                continue;
            }
            clip_painter.vline(x, r.top() + 15.0..=r.bottom(), Stroke::new(1.0_f32, sep));
        }
    }
    // Soundtrap-style fades: a translucent wedge over the note area plus a
    // thin bright guide line along its slope, anchored at the clip's corners.
    let fade_fill = Color32::from_rgba_unmultiplied(pal.text.r(), pal.text.g(), pal.text.b(), 72);
    let fade_guide = Stroke::new(1.0_f32, pal.text);
    let (top, bottom) = (inner.top(), inner.bottom());
    if clip.fade_in > 0.0 {
        let fx = (clip.fade_in as f32 * pps).clamp(0.0, inner.width());
        if fx > 0.5 {
            let (a, b, c) = (
                pos2(r.left(), bottom),
                pos2(r.left() + fx, bottom),
                pos2(r.left() + fx, top),
            );
            clip_painter.add(Shape::convex_polygon(
                vec![a, b, c],
                fade_fill,
                Stroke::NONE,
            ));
            clip_painter.line_segment([a, c], fade_guide);
        }
    }
    if clip.fade_out > 0.0 {
        let fx = (clip.fade_out as f32 * pps).clamp(0.0, inner.width());
        if fx > 0.5 {
            let (a, b, c) = (
                pos2(r.right(), bottom),
                pos2(r.right() - fx, bottom),
                pos2(r.right() - fx, top),
            );
            clip_painter.add(Shape::convex_polygon(
                vec![a, b, c],
                fade_fill,
                Stroke::NONE,
            ));
            clip_painter.line_segment([a, c], fade_guide);
        }
    }
    // Loop badge in the top band, clear of the corner fade zones.
    if loops > 1 && r.width() > 56.0 {
        let label = format!("×{loops}");
        let font = FontId::proportional(10.0);
        let galley = clip_painter.layout_no_wrap(label.clone(), font.clone(), ink);
        let text_w = galley.size().x;
        let icon = 10.0;
        let end = r.right() - 24.0;
        let x = end - text_w - icon - 3.0;
        theme::paint_icon(
            &clip_painter,
            Rect::from_center_size(pos2(x + icon * 0.5, band.center().y), vec2(icon, icon)),
            theme::Icon::Loop,
            ink,
        );
        clip_painter.text(
            pos2(x + icon + 3.0, band.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            ink,
        );
    }
    // Small grip ticks at the top corners mark the fade handles.
    if r.width() > 20.0 {
        let tick = Color32::from_rgba_unmultiplied(ink.r(), ink.g(), ink.b(), 170);
        clip_painter.add(Shape::convex_polygon(
            vec![
                pos2(r.left() + 1.0, r.top() + 1.0),
                pos2(r.left() + 7.0, r.top() + 1.0),
                pos2(r.left() + 1.0, r.top() + 7.0),
            ],
            tick,
            Stroke::NONE,
        ));
        clip_painter.add(Shape::convex_polygon(
            vec![
                pos2(r.right() - 1.0, r.top() + 1.0),
                pos2(r.right() - 7.0, r.top() + 1.0),
                pos2(r.right() - 1.0, r.top() + 7.0),
            ],
            tick,
            Stroke::NONE,
        ));
    }
    let stroke = if selected {
        Stroke::new(1.5_f32, pal.text)
    } else {
        Stroke::new(1.0_f32, color.gamma_multiply(0.55))
    };
    p.rect_stroke(r, 5.0, stroke, StrokeKind::Inside);
}

/// Paints an imported-WAV clip: same frame/band/fade/loop grammar as
/// [`paint_clip`], but the body shows diagonal hatching and the file name —
/// a waveform is out of scope for the MVP, so the clip reads as audio
/// content without pretending to show one.
#[allow(clippy::too_many_arguments)]
fn paint_audio_clip(
    p: &Painter,
    pal: Palette,
    r: Rect,
    clip: &AudioClip,
    color: Color32,
    selected: bool,
    audible: bool,
    pps: f32,
) {
    if r.width() < 1.0 {
        return;
    }
    let ink = theme::ink_on(color);
    let body = theme::mix(pal.canvas, color, if selected { 0.36 } else { 0.24 });
    p.rect_filled(r, 5.0, body);
    let band = Rect::from_min_max(r.min, pos2(r.right(), r.top() + 15.0));
    p.rect_filled(
        band,
        egui::CornerRadius {
            nw: 5,
            ne: 5,
            sw: 0,
            se: 0,
        },
        if audible {
            color
        } else {
            color.gamma_multiply(0.5)
        },
    );
    let clip_painter = p.with_clip_rect(r.intersect(p.clip_rect()));
    let loops = clip.loop_count();
    let rep_len = clip.length.max(0.0) as f32;
    let inner = Rect::from_min_max(
        pos2(r.left() + 2.0, band.bottom() + 3.0),
        pos2(r.right() - 2.0, r.bottom() - 3.0),
    );
    // Diagonal hatch marks — sparse enough to stay readable at any zoom.
    if inner.is_positive() && inner.width() > 6.0 {
        let hatch = if audible {
            theme::mix(color, pal.text, 0.45).gamma_multiply(0.55)
        } else {
            pal.text_faint
        };
        let mut stripes = Vec::new();
        let step = 10.0_f32;
        let h = inner.height();
        let mut x = inner.left() - h;
        while x < inner.right() {
            stripes.push(pos2(x, inner.bottom()));
            stripes.push(pos2(x + h, inner.top()));
            x += step;
        }
        clip_painter.add(Shape::line(stripes, Stroke::new(1.0_f32, hatch)));
    }
    if r.width() > 18.0 {
        clip_painter.text(
            pos2(r.left() + 5.0, band.center().y),
            Align2::LEFT_CENTER,
            &clip.name,
            FontId::proportional(10.5),
            ink,
        );
    }
    // Loop boundary separators + badge, same rhythm as pattern clips.
    if loops > 1 && rep_len > 0.0 {
        let sep = Color32::from_rgba_unmultiplied(
            pal.text_faint.r(),
            pal.text_faint.g(),
            pal.text_faint.b(),
            110,
        );
        for rep in 1..loops {
            let x = r.left() + rep as f32 * rep_len * pps;
            if x <= r.left() || x >= r.right() {
                continue;
            }
            clip_painter.vline(x, r.top() + 15.0..=r.bottom(), Stroke::new(1.0_f32, sep));
        }
    }
    // Fade wedges, identical to the pattern-clip treatment.
    let fade_fill = Color32::from_rgba_unmultiplied(pal.text.r(), pal.text.g(), pal.text.b(), 72);
    let fade_guide = Stroke::new(1.0_f32, pal.text);
    let (top, bottom) = (inner.top(), inner.bottom());
    if clip.fade_in > 0.0 {
        let fx = (clip.fade_in as f32 * pps).clamp(0.0, inner.width());
        if fx > 0.5 {
            let (a, b, c) = (
                pos2(r.left(), bottom),
                pos2(r.left() + fx, bottom),
                pos2(r.left() + fx, top),
            );
            clip_painter.add(Shape::convex_polygon(
                vec![a, b, c],
                fade_fill,
                Stroke::NONE,
            ));
            clip_painter.line_segment([a, c], fade_guide);
        }
    }
    if clip.fade_out > 0.0 {
        let fx = (clip.fade_out as f32 * pps).clamp(0.0, inner.width());
        if fx > 0.5 {
            let (a, b, c) = (
                pos2(r.right(), bottom),
                pos2(r.right() - fx, bottom),
                pos2(r.right() - fx, top),
            );
            clip_painter.add(Shape::convex_polygon(
                vec![a, b, c],
                fade_fill,
                Stroke::NONE,
            ));
            clip_painter.line_segment([a, c], fade_guide);
        }
    }
    if loops > 1 && r.width() > 56.0 {
        let label = format!("×{loops}");
        clip_painter.text(
            pos2(r.right() - 24.0, band.center().y),
            Align2::RIGHT_CENTER,
            label,
            FontId::proportional(10.0),
            ink,
        );
    }
    // Corner fade-handle ticks.
    if r.width() > 20.0 {
        let tick = Color32::from_rgba_unmultiplied(ink.r(), ink.g(), ink.b(), 170);
        clip_painter.add(Shape::convex_polygon(
            vec![
                pos2(r.left() + 1.0, r.top() + 1.0),
                pos2(r.left() + 7.0, r.top() + 1.0),
                pos2(r.left() + 1.0, r.top() + 7.0),
            ],
            tick,
            Stroke::NONE,
        ));
        clip_painter.add(Shape::convex_polygon(
            vec![
                pos2(r.right() - 1.0, r.top() + 1.0),
                pos2(r.right() - 7.0, r.top() + 1.0),
                pos2(r.right() - 1.0, r.top() + 7.0),
            ],
            tick,
            Stroke::NONE,
        ));
    }
    let stroke = if selected {
        Stroke::new(1.5_f32, pal.text)
    } else {
        Stroke::new(1.0_f32, color.gamma_multiply(0.55))
    };
    p.rect_stroke(r, 5.0, stroke, StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Instrument;
    use crate::packs::InstrumentPreset;

    #[test]
    fn timeline_round_trips_and_snaps() {
        let tl = Timeline {
            left: 200.0,
            pps: 4.0,
            scroll: 64.0,
        };
        assert_eq!(tl.x(16.0), 200.0);
        assert!((tl.step(tl.x(37.5)) - 37.5).abs() < 1e-4);
        assert_eq!(snap_floor(37.0, 16.0), 32.0);
        assert_eq!(snap_round(25.0, 16.0), 32.0);
        assert_eq!(snap_round(5.0, 4.0), 4.0);
    }

    #[test]
    fn zoom_keeps_anchor_step_fixed_and_is_bounded() {
        let mut v = ArrangerView {
            scroll_x: 100.0,
            px_per_step: 4.0,
            ..Default::default()
        };
        let anchor = 300.0;
        let before = (anchor + v.scroll_x) / v.px_per_step;
        v.zoom_by(2.0, Some(anchor));
        let after = (anchor + v.scroll_x) / v.px_per_step;
        assert!((before - after).abs() < 1e-3);
        for _ in 0..50 {
            v.zoom_by(2.0, None);
        }
        assert_eq!(v.px_per_step, MAX_PPS);
        for _ in 0..80 {
            v.zoom_by(0.5, None);
        }
        assert_eq!(v.px_per_step, MIN_PPS);
    }

    #[test]
    fn bar_labels_stay_readable() {
        assert_eq!(bar_label_every(4.0), 1);
        assert_eq!(bar_label_every(1.0), 4);
        assert!(bar_label_every(0.5) >= 8);
    }

    #[test]
    fn loop_region_drag_snaps_outward_and_clamps() {
        let total = 128.0;
        // Dragging 40 → 20 on a bar grid covers bars 2–3.
        assert_eq!(
            loop_region_from_drag(40.0, 20.0, 16.0, total),
            Some((16.0, 48.0))
        );
        // Order does not matter; the touched span expands to whole cells.
        assert_eq!(
            loop_region_from_drag(20.0, 40.0, 16.0, total),
            Some((16.0, 48.0))
        );
        // A beat grid gives finer regions.
        assert_eq!(
            loop_region_from_drag(20.0, 30.0, 4.0, total),
            Some((20.0, 32.0))
        );
        // Clamped into the song and above zero.
        assert_eq!(
            loop_region_from_drag(100.0, 200.0, 16.0, total),
            Some((96.0, 128.0))
        );
        assert_eq!(
            loop_region_from_drag(-10.0, 20.0, 16.0, total),
            Some((0.0, 32.0))
        );
        // A click — press and release on the same spot — never creates one.
        assert_eq!(loop_region_from_drag(20.0, 20.0, 16.0, total), None);
        assert_eq!(loop_region_from_drag(16.0, 16.0, 16.0, total), None);
        // But even a tiny drag inside one cell selects that cell.
        assert_eq!(
            loop_region_from_drag(20.0, 20.5, 16.0, total),
            Some((16.0, 32.0))
        );
        // Degenerate inputs.
        assert_eq!(loop_region_from_drag(0.0, 10.0, 0.0, total), None);
        assert_eq!(loop_region_from_drag(f64::NAN, 10.0, 16.0, total), None);
    }

    #[test]
    fn loop_clear_rect_sits_inside_the_band_end() {
        let band = Rect::from_min_max(pos2(100.0, 0.0), pos2(200.0, 10.0));
        let x = loop_clear_rect(band);
        assert!(band.contains_rect(x));
        assert_eq!(x.center().y, band.center().y);
        assert!(x.right() <= band.right());
    }

    #[test]
    fn follow_playhead_holds_while_a_gesture_owns_the_grid() {
        let mut app = CadenceApp::headless(Project::empty());
        let grid = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 400.0));
        app.arranger.px_per_step = 4.0;
        app.arranger.scroll_x = 0.0;
        app.transport.playing = true;
        app.transport.position = 200.0;

        app.follow_playhead(grid);
        assert!(app.arranger.scroll_x > 0.0, "playhead scrolls into view");

        app.arranger.scroll_x = 0.0;
        app.arranger.marquee = Some(ArrangerMarquee {
            start: Pos2::ZERO,
            current: Pos2::ZERO,
            base: BTreeSet::new(),
            additive: false,
        });
        app.follow_playhead(grid);
        assert_eq!(app.arranger.scroll_x, 0.0, "marquee suppresses follow");

        app.arranger.marquee = None;
        app.arranger.drag = Some(ArrangerDrag::Fade {
            track: 1,
            clip: 2,
            edge: FadeEdge::In,
        });
        app.follow_playhead(grid);
        assert_eq!(app.arranger.scroll_x, 0.0, "clip drag suppresses follow");

        app.arranger.drag = None;
        app.arranger.follow = false;
        app.follow_playhead(grid);
        assert_eq!(
            app.arranger.scroll_x, 0.0,
            "follow toggle off suppresses scroll"
        );
    }

    // ----- split clip targeting ------------------------------------------------

    /// Runs `f` with the pointer hovering `pos`.
    fn with_pointer_at(pos: Pos2, f: impl FnMut(&egui::Context)) {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::PointerMoved(pos));
        let _ = ctx.run(input, f);
    }

    /// Headless app with the default clip selected and a fixed timeline:
    /// the grid starts at x=0, 4 px per step, no scroll.
    fn app_with_grid() -> CadenceApp {
        let mut app = CadenceApp::headless(Project::empty());
        app.arranger.grid = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 200.0));
        app.arranger.px_per_step = 4.0;
        app.arranger.scroll_x = 0.0;
        app.arranger.scroll_y = 0.0;
        app.arranger.snap = Snap::Step;
        app.set_single_clip_selection(app.project.tracks[0].clips[0].id);
        app
    }

    #[test]
    fn split_target_prefers_the_clip_under_the_pointer() {
        let mut app = app_with_grid();
        // A second clip is selected with the playhead inside it, but the
        // pointer over the first clip still wins and cuts at the pointer.
        let pid = app.project.tracks[0].patterns[0].id;
        let tid = app.project.tracks[0].id;
        let second = ops::add_clip(&mut app.project, tid, pid, 32.0).unwrap();
        app.set_single_clip_selection(second);
        app.transport.position = 40.0;
        let first = app.project.tracks[0].clips[0].id;

        // Step 8.4 on the step grid snaps to 8.
        with_pointer_at(pos2(8.4 * 4.0, 20.0), |ctx| {
            assert_eq!(
                app.split_clip_target(ctx),
                Some((tid, first, 8.0)),
                "the hovered clip splits at the snapped pointer step"
            );
        });
        // Hovering empty lane / outside the grid falls back to the
        // selection at the playhead.
        with_pointer_at(pos2(90.0 * 4.0, 20.0), |ctx| {
            assert_eq!(app.split_clip_target(ctx), Some((tid, second, 40.0)));
        });
        with_pointer_at(pos2(8.0 * 4.0, -40.0), |ctx| {
            assert_eq!(app.split_clip_target(ctx), Some((tid, second, 40.0)));
        });
        // A pointer step on the clip's edge still reports the clip as
        // unsplittable rather than borrowing the playhead target.
        with_pointer_at(pos2(0.0, 20.0), |ctx| {
            assert_eq!(
                app.split_clip_target(ctx),
                Some((tid, second, 40.0)),
                "edge hover cannot cut, so the selected clip is used"
            );
        });
    }

    #[test]
    fn split_target_falls_back_to_the_selected_clip_at_the_playhead() {
        let mut app = app_with_grid();
        let tid = app.project.tracks[0].id;
        let clip = app.project.tracks[0].clips[0].id;
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.transport.position = 8.0;
            assert_eq!(app.split_clip_target(ctx), Some((tid, clip, 8.0)));
            // The playhead on the clip's edge or outside it finds no cut.
            app.transport.position = 0.0;
            assert_eq!(app.split_clip_target(ctx), None);
            app.transport.position = 16.0;
            assert_eq!(app.split_clip_target(ctx), None);
            app.transport.position = 64.0;
            assert_eq!(app.split_clip_target(ctx), None);
            // Nothing selected: no target at all.
            app.transport.position = 8.0;
            app.clear_arranger_clip_selection();
            assert_eq!(app.split_clip_target(ctx), None);
        });
    }

    #[test]
    fn split_clip_at_playhead_cuts_selects_the_right_piece_and_undoes() {
        let mut app = app_with_grid();
        let clip = app.project.tracks[0].clips[0].id;
        app.transport.position = 8.0;
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.split_clip_at_playhead(ctx);
        });
        let t = &app.project.tracks[0];
        assert_eq!(t.clips.len(), 2);
        let right = t.clips[1].id;
        assert_ne!(right, clip);
        assert_eq!(app.sel.clip, Some(right), "the new right clip is selected");
        assert!(app.sel.clips.contains(&right));
        assert_eq!(app.focus, Focus::Arranger);
        app.project.validate().unwrap();
        // The whole split is one undo step.
        assert!(app.history.can_undo());
        app.undo();
        assert_eq!(app.project.tracks[0].clips.len(), 1);
    }

    #[test]
    fn split_clip_at_playhead_explains_when_nothing_can_cut() {
        let mut app = app_with_grid();
        app.transport.position = 64.0; // past the only clip
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.split_clip_at_playhead(ctx);
        });
        let status = app.status.as_ref().expect("a status was set");
        assert_eq!(status.kind, StatusKind::Info);
        assert!(status.text.contains("split"), "{}", status.text);
        assert_eq!(app.project.tracks[0].clips.len(), 1);
    }

    // ----- library drag-and-drop -------------------------------------------

    /// The rect the arranger accepts library drops on in these tests:
    /// lanes/headers/scrollbars under the ruler — same shape `arranger_ui`
    /// computes.
    fn drop_area() -> Rect {
        Rect::from_min_max(pos2(0.0, 26.0), pos2(800.0, 400.0))
    }

    #[test]
    fn drop_lane_index_maps_pointer_y_to_the_insert_slot() {
        // Index = the hovered lane; its track would shift down on drop.
        assert_eq!(drop_lane_index(0.0, 0.0, 0.0, 3), 0);
        assert_eq!(drop_lane_index(LANE_H - 1.0, 0.0, 0.0, 3), 0);
        assert_eq!(drop_lane_index(LANE_H, 0.0, 0.0, 3), 1);
        assert_eq!(drop_lane_index(2.5 * LANE_H, 0.0, 0.0, 3), 2);
        // Below the last lane clamps to len (append); above clamps to 0.
        assert_eq!(drop_lane_index(400.0, 0.0, 0.0, 3), 3);
        assert_eq!(drop_lane_index(-5.0, 0.0, 0.0, 3), 0);
        // Lane geometry accounts for the vertical scroll.
        assert_eq!(drop_lane_index(10.0, 0.0, LANE_H, 3), 1);
        assert_eq!(drop_lane_index(10.0, 40.0, 0.0, 3), 0);
        // An empty song only ever inserts at 0.
        assert_eq!(drop_lane_index(300.0, 0.0, 0.0, 0), 0);
    }

    #[test]
    fn library_drop_reports_the_ghost_while_dragging() {
        let mut app = CadenceApp::headless(Project::empty());
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input
            .events
            .push(egui::Event::PointerMoved(pos2(100.0, 30.0 + LANE_H)));
        egui::DragAndDrop::set_payload(&ctx, sidebar::QuickSound::BuiltIn(Instrument::Piano));
        let _ = ctx.run(input, |ctx| {
            let (index, sound) = app
                .library_drop(ctx, drop_area())
                .expect("a dragged sound over a lane has a target");
            // Lane 1 (y = 26 + LANE_H … +2·LANE_H): a drop inserts there.
            assert_eq!(index, 1);
            assert!(matches!(
                *sound,
                sidebar::QuickSound::BuiltIn(Instrument::Piano)
            ));
            assert_eq!(app.project.tracks.len(), 1, "nothing is added yet");
        });
    }

    #[test]
    fn library_drop_release_creates_the_track_at_the_lane() {
        let mut app = CadenceApp::headless(Project::empty());
        let drums = app.project.tracks[0].id;
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::PointerButton {
            pos: pos2(100.0, 30.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        // The release frame still carries the payload (egui clears it at
        // end-of-pass), exactly like a real drag ending over the lane.
        egui::DragAndDrop::set_payload(&ctx, sidebar::QuickSound::BuiltIn(Instrument::Piano));
        let _ = ctx.run(input, |ctx| {
            assert!(
                app.library_drop(ctx, drop_area()).is_none(),
                "the release ends the ghost"
            );
        });
        assert_eq!(app.project.tracks.len(), 2);
        let added = &app.project.tracks[0];
        assert_eq!(added.instrument, Instrument::Piano);
        assert_eq!(
            app.project.tracks[1].id, drums,
            "the existing track shifted down"
        );
        assert_eq!(app.sel.track, Some(added.id), "the new track is selected");
        assert_eq!(app.arranger.reveal_track, Some(added.id));
        app.project.validate().unwrap();
        // The whole drop — add plus reposition — is one undo step.
        assert!(app.history.can_undo());
        app.undo();
        assert_eq!(app.project.tracks.len(), 1);
        assert_eq!(app.project.tracks[0].id, drums);
        // And the payload is consumed, so a stray release can't re-add.
        assert!(!egui::DragAndDrop::has_any_payload(&ctx));
    }

    #[test]
    fn library_drop_ignores_releases_outside_the_lanes() {
        let mut app = CadenceApp::headless(Project::empty());
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::PointerButton {
            pos: pos2(100.0, 10.0), // on the ruler, above the drop area
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        egui::DragAndDrop::set_payload(&ctx, sidebar::QuickSound::BuiltIn(Instrument::Piano));
        let _ = ctx.run(input, |ctx| {
            assert!(app.library_drop(ctx, drop_area()).is_none());
        });
        assert_eq!(app.project.tracks.len(), 1, "no track was added");
        assert!(!app.history.can_undo(), "no edit was recorded");
    }

    #[test]
    fn add_sound_track_at_places_presets_and_kits_correctly() {
        let mut app = CadenceApp::headless(Project::empty());
        // A melodic pack preset embeds itself and stays a Lead track.
        let lead: InstrumentPreset =
            serde_json::from_str(r#"{"id":"t.lead","name":"Lead","oscillators":[{}]}"#).unwrap();
        app.add_sound_track_at(&sidebar::QuickSound::Preset(Box::new(lead)), 1);
        assert_eq!(app.project.tracks.len(), 2);
        assert_eq!(app.project.tracks[1].instrument, Instrument::Lead);
        assert!(app.project.tracks[1].preset.is_some());
        // A kit preset becomes a Drums track (drum grid), like the library's
        // add button.
        let kit: InstrumentPreset = serde_json::from_str(
            r#"{"id":"t.kit","name":"Kit","kit":{"pieces":[
                {"name":"Kick","pitch":36,"voice":{"oscillators":[{}]}}
            ]}}"#,
        )
        .unwrap();
        app.add_sound_track_at(&sidebar::QuickSound::Preset(Box::new(kit)), 1);
        assert_eq!(app.project.tracks.len(), 3);
        let kit_track = &app.project.tracks[1];
        assert_eq!(kit_track.instrument, Instrument::Drums);
        assert!(kit_track.shows_drum_grid());
        // An out-of-range index clamps to the end.
        app.add_sound_track_at(&sidebar::QuickSound::BuiltIn(Instrument::Bass), 99);
        assert_eq!(
            app.project.tracks.last().unwrap().instrument,
            Instrument::Bass
        );
        app.project.validate().unwrap();
    }

    // ----- chord strip ---------------------------------------------------------

    #[test]
    fn chord_lane_sits_under_the_number_strip() {
        let ruler = Rect::from_min_size(Pos2::ZERO, vec2(400.0, RULER_H));
        let strip = number_strip_rect(ruler);
        let lane = chord_lane_rect(ruler);
        // The three ruler strips stack without overlap: the loop lane on
        // top, then the number strip, then the chord lane.
        assert_eq!(strip.top(), ruler.top() + LOOP_LANE_H);
        assert_eq!(lane.top(), strip.bottom());
        assert_eq!(lane.bottom(), ruler.bottom());
        assert_eq!(
            lane.height(),
            RULER_H - LOOP_LANE_H - NUMBER_STRIP_H,
            "the chord strip keeps the thin bottom share"
        );
    }

    #[test]
    fn chord_chip_is_sized_to_its_label_and_culled_offscreen() {
        let ruler = Rect::from_min_size(Pos2::ZERO, vec2(400.0, RULER_H));
        let tl = Timeline {
            left: 0.0,
            pps: 4.0,
            scroll: 0.0,
        };
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let ev = ChordEvent::new(9, ChordQuality::Minor7, 16.0);
        let chip = chord_chip(&ctx, &ev, ruler, tl).expect("on screen");
        let lane = chord_lane_rect(ruler);
        // The pill rides inside the lane, just right of its bar line.
        assert!(lane.contains(chip.center()));
        assert!(chip.left() >= tl.x(16.0) + 1.0);
        assert_eq!(chip.height(), CHORD_CHIP_H);
        // Longer symbols get wider chips.
        let wide = ChordEvent::new(6, ChordQuality::HalfDiminished7, 16.0);
        let wide_chip = chord_chip(&ctx, &wide, ruler, tl).unwrap();
        assert!(wide_chip.width() > chip.width(), "F#m7b5 beats Am7");
        // Off-screen events have no chip and no hit target.
        let off_left = ChordEvent::new(0, ChordQuality::Major, -32.0);
        assert_eq!(chord_chip(&ctx, &off_left, ruler, tl), None);
        let off_right = ChordEvent::new(0, ChordQuality::Major, 1_000_000.0);
        assert_eq!(chord_chip(&ctx, &off_right, ruler, tl), None);
    }

    #[test]
    fn chord_strip_writes_replaces_and_undoes_bar_chords() {
        let mut app = CadenceApp::headless(Project::empty());
        app.project.key = crate::model::SongKey::new(9, crate::model::KeyMode::Minor);
        // A plain click on the strip writes the key's tonic.
        app.add_tonic_chord_at(16.0);
        assert_eq!(app.project.chord_events.len(), 1);
        let ev = &app.project.chord_events[0];
        assert_eq!(ops::chord_event_label(ev), "Am");
        assert_eq!(ev.step, 16.0);
        // Writing on the same bar replaces the label instead of stacking.
        let id = ev.id;
        app.set_or_add_chord(16.0, 7, ChordQuality::Dominant7);
        assert_eq!(app.project.chord_events.len(), 1);
        assert_eq!(app.project.chord_events[0].id, id);
        assert_eq!(ops::chord_event_label(&app.project.chord_events[0]), "G7");
        // A different bar adds a second event, kept sorted.
        app.add_tonic_chord_at(0.0);
        assert_eq!(app.project.chord_events.len(), 2);
        assert_eq!(app.project.chord_events[0].step, 0.0);
        app.project.validate().unwrap();
        // Every write is one undo step.
        app.undo();
        assert_eq!(app.project.chord_events.len(), 1);
    }

    // ----- automation lanes ----------------------------------------------------

    /// A project with three bare tracks for row-geometry tests.
    fn three_track_app() -> CadenceApp {
        let mut app = app_with_grid();
        app.project.tracks = vec![
            crate::model::create_track(Instrument::Drums),
            crate::model::create_track(Instrument::Bass),
            crate::model::create_track(Instrument::Piano),
        ];
        app
    }

    #[test]
    fn lane_row_at_accounts_for_expanded_rows() {
        let mut app = three_track_app();
        let t1 = app.project.tracks[1].id;
        let top = 40.0;
        // Closed: every row is LANE_H tall.
        assert_eq!(app.lane_row_at(top, top + 5.0), Some((0, false)));
        assert_eq!(app.lane_row_at(top, top + LANE_H + 5.0), Some((1, false)));
        assert_eq!(
            app.lane_row_at(top, top + 2.0 * LANE_H + 5.0),
            Some((2, false))
        );
        assert_eq!(app.lane_row_at(top, top - 1.0), None);
        assert_eq!(app.lane_row_at(top, top + 3.0 * LANE_H + 5.0), None);

        // Open track 1's strip: it gets AUTO_LANE_H taller and rows below
        // shift down; the strip band itself reports `in_auto`.
        app.arranger.auto_lanes.insert(t1, AutomationLane::Volume);
        let strip_top = top + LANE_H + LANE_H;
        assert_eq!(app.lane_row_at(top, strip_top - 1.0), Some((1, false)));
        assert_eq!(app.lane_row_at(top, strip_top + 2.0), Some((1, true)));
        assert_eq!(
            app.lane_row_at(top, strip_top + AUTO_LANE_H + 2.0),
            Some((2, false))
        );
        assert_eq!(app.lane_top(2), 2.0 * LANE_H + AUTO_LANE_H);
        // Scrolling lifts the whole stack the same way.
        app.arranger.scroll_y = LANE_H;
        assert_eq!(
            app.lane_row_at(top, top + 4.0),
            Some((1, false)),
            "scroll shifts rows up"
        );
        assert_eq!(app.lane_row_at(top, top + LANE_H + 4.0), Some((1, true)));
    }

    #[test]
    fn first_visible_lane_skips_whole_rows_not_fixed_heights() {
        let mut app = three_track_app();
        let t0 = app.project.tracks[0].id;
        app.arranger.auto_lanes.insert(t0, AutomationLane::Pan);
        // Row 0 is LANE_H + AUTO_LANE_H tall; scrolling just past it leaves
        // lane 1 visible first — a /LANE_H divide would name lane 1 too
        // early or late.
        app.arranger.scroll_y = LANE_H + AUTO_LANE_H - 1.0;
        assert_eq!(app.first_visible_lane(), 0);
        app.arranger.scroll_y = LANE_H + AUTO_LANE_H + 1.0;
        assert_eq!(app.first_visible_lane(), 1);
    }

    #[test]
    fn automation_strip_maps_y_to_lane_values() {
        let r = Rect::from_min_max(pos2(0.0, 100.0), pos2(400.0, 128.0));
        // Volume: top is the 1.5 boost ceiling, bottom is silence, neutral
        // (×1) sits at two-thirds height.
        assert_eq!(auto_y_to_value(AutomationLane::Volume, 100.0, r), 1.5);
        assert_eq!(auto_y_to_value(AutomationLane::Volume, 128.0, r), 0.0);
        let neutral_y = auto_value_to_y(AutomationLane::Volume, 1.0, r);
        assert!((auto_y_to_value(AutomationLane::Volume, neutral_y, r) - 1.0).abs() < 0.01);
        // Pan: top hard right, bottom hard left, centre is zero offset.
        assert_eq!(auto_y_to_value(AutomationLane::Pan, 100.0, r), 1.0);
        assert_eq!(auto_y_to_value(AutomationLane::Pan, 128.0, r), -1.0);
        assert!((auto_y_to_value(AutomationLane::Pan, 114.0, r)).abs() < 0.01);
        // Round trips land on the same spot.
        for v in [0.0_f32, 0.25, 0.9, 1.5] {
            let y = auto_value_to_y(AutomationLane::Volume, v, r);
            assert!((auto_y_to_value(AutomationLane::Volume, y, r) - v).abs() < 1e-4);
        }
        // A degenerate rect reads as the lane's neutral value, never NaN.
        let flat = Rect::from_min_max(pos2(0.0, 10.0), pos2(4.0, 10.0));
        assert_eq!(auto_y_to_value(AutomationLane::Pan, 10.0, flat), 0.0);
    }

    #[test]
    fn hit_test_targets_the_automation_strip_and_its_points() {
        let mut app = app_with_grid();
        let tid = app.project.tracks[0].id;
        app.arranger.auto_lanes.insert(tid, AutomationLane::Volume);
        let grid = app.arranger.grid;
        let tl = app.arranger.timeline(grid);
        // Inside the strip: an Automation hit with no grabbed point.
        let hit = app.hit_test(pos2(100.0, LANE_H + 8.0), grid, tl);
        assert!(
            matches!(hit, Hit::Automation { track, param: AutomationLane::Volume, point: None, .. } if track == tid),
            "empty strip is an add-target"
        );
        // Above the strip the clip lane keeps its usual hits (the demo
        // clip spans steps 0..16 — x 0..64 at 4 px/step).
        assert!(matches!(
            app.hit_test(pos2(40.0, 30.0), grid, tl),
            Hit::Clip { .. }
        ));
        // A drawn point within grab range is reported as the drag target.
        assert_eq!(
            ops::add_automation_point(&mut app.project, tid, AutomationLane::Volume, 4.0, 0.5),
            Some(0)
        );
        let hit = app.hit_test(pos2(4.0 * 4.0 + 2.0, LANE_H + 8.0), grid, tl);
        assert!(
            matches!(hit, Hit::Automation { point: Some(0), .. }),
            "the point at step 4 wins within 6 px"
        );
        // Far from it the strip is an add target again.
        assert!(matches!(
            app.hit_test(pos2(200.0, LANE_H + 8.0), grid, tl),
            Hit::Automation { point: None, .. }
        ));
    }

    #[test]
    fn toggle_automation_lane_picks_the_lane_with_data() {
        let mut app = app_with_grid();
        let tid = app.project.tracks[0].id;
        // No data: opens on Volume.
        app.toggle_automation_lane(tid);
        assert_eq!(app.auto_param(tid), AutomationLane::Volume);
        app.toggle_automation_lane(tid);
        assert!(!app.arranger.auto_lanes.contains_key(&tid));
        // Pan-only data opens on Pan so the points are immediately visible.
        assert_eq!(
            ops::add_automation_point(&mut app.project, tid, AutomationLane::Pan, 0.0, 0.5),
            Some(0)
        );
        app.toggle_automation_lane(tid);
        assert_eq!(app.auto_param(tid), AutomationLane::Pan);
        // Reset clears expansion state like a project swap would require.
        app.arranger.reset_gesture();
        assert!(app.arranger.auto_lanes.is_empty());
    }

    // ----- section bands and structure menu ------------------------------------

    #[test]
    fn section_band_ranges_split_the_song_between_markers() {
        // No markers: no bands.
        assert!(section_band_ranges(&[], 128.0).is_empty());
        // One marker: song start → marker, marker → song end.
        let m = Marker::new("Verse", 48.0);
        assert_eq!(
            section_band_ranges(&[m], 128.0),
            vec![(0.0, 48.0), (48.0, 128.0)]
        );
        // Sorted markers (the model keeps them sorted) produce consecutive
        // gaps; a marker past the song end clamps into it.
        let a = Marker::new("Intro", 0.0);
        let b = Marker::new("Verse", 32.0);
        let c = Marker::new("Chorus", 64.0);
        let d = Marker::new("Outro", 999.0);
        assert_eq!(
            section_band_ranges(&[a, b, c, d], 96.0),
            vec![(0.0, 32.0), (32.0, 64.0), (64.0, 96.0)]
        );
        // Duplicate steps collapse so no zero-width band is produced.
        let e = Marker::new("A", 16.0);
        let f = Marker::new("B", 16.0);
        let g = Marker::new("C", 64.0);
        assert_eq!(
            section_band_ranges(&[e, f, g], 128.0),
            vec![(0.0, 16.0), (16.0, 64.0), (64.0, 128.0)]
        );
    }

    #[test]
    fn bars_holding_step_rounds_up_to_the_clicked_bar() {
        assert_eq!(bars_holding_step(0.0), 1);
        assert_eq!(bars_holding_step(15.9), 1);
        assert_eq!(bars_holding_step(16.0), 1);
        assert_eq!(bars_holding_step(16.1), 2);
        assert_eq!(bars_holding_step(64.0), 4);
        assert_eq!(bars_holding_step(-5.0), 1);
        assert_eq!(bars_holding_step(9999.0), 625);
    }

    #[test]
    fn dimmed_keeps_hue_and_cuts_opacity() {
        let c = Color32::from_rgb(200, 100, 50);
        let d = dimmed(c, 0.45);
        assert_eq!(d.a(), (255.0_f32 * 0.45).round() as u8);
        // egui stores premultiplied RGBA, so the hue is preserved when the
        // channel/alpha ratio stays the original channel/255 ratio.
        for (cd, cc) in [(d.r(), c.r()), (d.g(), c.g()), (d.b(), c.b())] {
            assert!(
                (cd as f32 / d.a() as f32 - cc as f32 / 255.0).abs() < 0.02,
                "channel {cd}/{d} drifts from {cc}/255",
                d = d.a()
            );
        }
    }

    #[test]
    fn structure_menu_edits_markers_and_song_end_undo_cleanly() {
        let mut app = CadenceApp::headless(Project::empty());
        app.arranger.snap = Snap::Step;
        let before = app.project.bars;
        // Named marker at the snapped step.
        let id = app.add_marker_named(34.0, "Verse".to_string());
        let id = id.expect("marker added");
        let m = app.project.markers.iter().find(|m| m.id == id).unwrap();
        assert_eq!(m.name, "Verse");
        assert_eq!(m.step, 34.0, "snap stays on the step grid");
        // Setting the song end rewrites bars; content still fits after it.
        app.set_song_end(80.0);
        assert_eq!(app.project.bars, 5);
        app.project.validate().unwrap();
        // One undo step per edit.
        assert!(app.history.can_undo());
        app.undo();
        assert_eq!(app.project.bars, before);
        app.undo();
        assert!(app.project.markers.is_empty());
    }
}
