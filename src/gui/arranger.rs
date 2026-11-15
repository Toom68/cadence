//! Multi-track clip arranger: custom-painted timeline with its own
//! horizontal zoom/scroll, track headers, ruler seeking and clip editing.

use std::collections::BTreeSet;

use egui::{
    pos2, vec2, Align2, Color32, CursorIcon, FontId, Painter, PointerButton, Pos2, Rect, RichText,
    Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder,
};

use super::theme::{self, Palette};
use super::{ops, sidebar, CadenceApp, Focus, StatusKind, Tab};
use crate::model::{Clip, Pattern, Project, Track};

pub(super) const HEADER_W: f32 = 200.0;
pub(super) const RULER_H: f32 = 26.0;
pub(super) const LANE_H: f32 = 52.0;
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
}

impl ArrangerDrag {
    /// True once the gesture has actually changed something (or, for the
    /// corner/edge handles, while the button is held on them).
    fn moved(&self) -> bool {
        match self {
            Self::Clip(d) => d.moved,
            Self::Fade { .. } | Self::Loop { .. } => true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ContextTarget {
    Clip { track: u64, clip: u64 },
    Lane { track: u64, step: f64 },
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
    context: Option<ContextTarget>,
    grid_width: f32,
    /// Last frame's timeline grid rect (also used by headless tests).
    pub(super) grid: Rect,
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
            context: None,
            grid_width: 800.0,
            grid: Rect::NOTHING,
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
    }

    /// Drops gesture and context state without touching zoom/scroll — used
    /// when the whole project is replaced mid-gesture.
    pub fn reset_gesture(&mut self) {
        self.end_gesture();
        self.context = None;
        self.reveal_track = None;
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
        pattern: u64,
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
        let content_h = n as f32 * LANE_H + 44.0;

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
                let top = i as f32 * LANE_H;
                if top < self.arranger.scroll_y
                    || top + LANE_H > self.arranger.scroll_y + grid.height()
                {
                    self.arranger.scroll_y = top + LANE_H - grid.height() + 8.0;
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
        self.arranger_ruler_input(ui, &ruler_resp, tl);

        let painter = ui.painter_at(grid);
        self.paint_lanes(&painter, grid, tl, song_steps);
        self.paint_clips(&painter, grid, tl);
        self.paint_arranger_marquee(&painter);
        let px = tl.x(self.transport.position);
        if px >= grid.left() - 1.0 && px <= grid.right() + 1.0 {
            painter.vline(px, grid.y_range(), Stroke::new(2.0_f32, pal.accent));
        }
        if let Some(d) = self.arranger.drag.clone().filter(|d| d.moved()) {
            let label_at = |track: u64, clip: u64| {
                ops::track(&self.project, track)
                    .and_then(|t| t.clips.iter().find(|c| c.id == clip))
                    .map(|c| (c.start, c.clone()))
            };
            let (start, label) = match d {
                ArrangerDrag::Clip(cd) => label_at(cd.track, cd.clip).map(|(start, c)| {
                    (
                        start,
                        format!(
                            "Bar {}{}",
                            ops::format_position(c.start).trim(),
                            if cd.copy { "  (copy)" } else { "" }
                        ),
                    )
                }),
                ArrangerDrag::Fade { track, clip, edge } => {
                    label_at(track, clip).map(|(start, c)| {
                        let v = if edge == FadeEdge::In {
                            c.fade_in
                        } else {
                            c.fade_out
                        };
                        (
                            start,
                            format!(
                                "{} fade {:.0} st",
                                if edge == FadeEdge::In { "In" } else { "Out" },
                                v
                            ),
                        )
                    })
                }
                ArrangerDrag::Loop { track, clip, .. } => label_at(track, clip)
                    .map(|(start, c)| (start, format!("Loops ×{}", c.loop_count()))),
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
        self.lane_headers(ui, header, grid);

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
                let selected_clips = self.selected_clip_ids().len();
                if selected_clips > 1 {
                    ui.label(RichText::new(format!("{selected_clips} clips selected")).small().color(pal.accent));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::hint(ui, pal, "Drag empty lanes to box-select clips · drag selected clips as a group · Alt-drag copies · corners fade · right edge loops");
                });
            });
        });
    }

    fn hit_test(&self, pos: Pos2, grid: Rect, tl: Timeline) -> Hit {
        if !grid.contains(pos) {
            return Hit::None;
        }
        let lane = ((pos.y - grid.top() + self.arranger.scroll_y) / LANE_H).floor();
        if lane < 0.0 {
            return Hit::None;
        }
        let lane = lane as usize;
        let Some(track) = self.project.tracks.get(lane) else {
            return Hit::None;
        };
        let step = tl.step(pos.x).max(0.0);
        let Some(c) = track
            .clips
            .iter()
            .rev()
            .find(|c| step >= c.start && step < c.start + ops::clip_len(track, c))
        else {
            return Hit::Lane {
                track: track.id,
                step,
            };
        };
        // Clip found: corner fade handles live in the 15 px top band, the loop
        // handle hugs the right edge below it. Handles win over the clip body.
        let x0 = tl.x(c.start);
        let x1 = tl.x(c.start + ops::clip_len(track, c));
        let lane_top = grid.top() + lane as f32 * LANE_H - self.arranger.scroll_y;
        let in_band = (lane_top + 4.0..=lane_top + 19.0).contains(&pos.y);
        if in_band {
            // On clips narrower than ~28 px the zones overlap; left wins.
            if pos.x - x0 <= 14.0 {
                return Hit::ClipFade {
                    track: track.id,
                    clip: c.id,
                    edge: FadeEdge::In,
                };
            }
            if x1 - pos.x <= 14.0 {
                return Hit::ClipFade {
                    track: track.id,
                    clip: c.id,
                    edge: FadeEdge::Out,
                };
            }
        } else if x1 - pos.x <= 7.0
            && x1 - x0 >= 48.0
            && pattern_steps(track, c) * f64::from(tl.pps) >= 14.0
        {
            return Hit::ClipLoop {
                track: track.id,
                clip: c.id,
            };
        }
        Hit::Clip {
            track: track.id,
            clip: c.id,
            pattern: c.pattern_id,
            start: c.start,
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
                        self.select_pattern(track, pattern);
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
                            .and_then(|t| t.clips.iter().find(|c| c.id == clip))
                            .map_or(1, |c| c.loop_count());
                        self.arranger.drag = Some(ArrangerDrag::Loop {
                            track,
                            clip,
                            grab_step,
                            origin_loops,
                        });
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
                } else if let Some(d) = self.arranger.drag.clone() {
                    match d {
                        ArrangerDrag::Clip(mut cd) => {
                            self.drag_clip_group(&mut cd, pos.x, tl);
                            self.arranger.drag = Some(ArrangerDrag::Clip(cd));
                        }
                        ArrangerDrag::Fade { track, clip, edge } => {
                            self.drag_clip_fade(track, clip, edge, pos.x, tl);
                        }
                        ArrangerDrag::Loop {
                            track,
                            clip,
                            grab_step,
                            origin_loops,
                        } => {
                            self.drag_clip_loop(track, clip, grab_step, origin_loops, pos.x, tl);
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
                    Hit::Clip { .. } | Hit::ClipFade { .. } | Hit::ClipLoop { .. } => {
                        self.tab = Tab::Editor;
                        self.focus = Focus::Editor;
                    }
                    Hit::Lane { track, step } => {
                        self.place_clip(track, None, snap_floor(step, self.arranger.snap.steps()))
                    }
                    Hit::None => {}
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                self.arranger.context = match self.hit_test(pos, grid, tl) {
                    Hit::Clip {
                        track,
                        clip,
                        pattern,
                        ..
                    } => {
                        self.select_track(track);
                        self.set_single_clip_selection(clip);
                        self.select_pattern(track, pattern);
                        Some(ContextTarget::Clip { track, clip })
                    }
                    Hit::ClipFade { track, clip, .. } | Hit::ClipLoop { track, clip } => {
                        self.select_clip(track, clip);
                        Some(ContextTarget::Clip { track, clip })
                    }
                    Hit::Lane { track, step } => {
                        self.select_track(track);
                        Some(ContextTarget::Lane {
                            track,
                            step: snap_floor(step, self.arranger.snap.steps()),
                        })
                    }
                    Hit::None => None,
                };
            }
        }
        resp.context_menu(|ui| self.arranger_context_menu(ui));
    }

    fn drag_clip_group(&mut self, cd: &mut ClipDrag, x: f32, tl: Timeline) {
        let g = self.arranger.snap.steps();
        let target = snap_round(tl.step(x) - cd.grab, g);
        let delta = ops::clip_group_delta(&self.project, &cd.originals, target - cd.origin);
        if (delta.abs() <= f64::EPSILON && !cd.moved) || cd.copy_failed {
            return;
        }
        let group = cd.group.as_str();
        if cd.copy && !cd.copied {
            let originals = cd.originals.clone();
            let primary_old = cd.clip;
            let mut copied = None;
            self.edit(
                if originals.len() == 1 {
                    "Copy clip"
                } else {
                    "Copy clips"
                },
                Some(group),
                |p| copied = ops::copy_clip_group(p, &originals, delta),
            );
            match copied {
                Some(ids) => {
                    let primary = originals
                        .iter()
                        .position(|(_, c)| c.id == primary_old)
                        .and_then(|i| ids.get(i).copied())
                        .unwrap_or_else(|| ids[0]);
                    let set: BTreeSet<u64> = ids.iter().copied().collect();
                    cd.clip = primary;
                    cd.track = self.clip_track_id(primary).unwrap_or(cd.track);
                    cd.origin += delta;
                    cd.copied = true;
                    cd.originals = ops::collect_clips(&self.project, &set);
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
            let originals = cd.originals.clone();
            self.edit(
                if originals.len() == 1 {
                    "Move clip"
                } else {
                    "Move clips"
                },
                Some(group),
                |p| {
                    ops::move_clip_group(p, &originals, delta);
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
        let first = (self.arranger.scroll_y / LANE_H).floor().max(0.0) as usize;
        for (i, track) in self.project.tracks.iter().enumerate().skip(first) {
            let y = grid.top() + i as f32 * LANE_H - self.arranger.scroll_y;
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
        let Some((start, span, fade_in, fade_out)) =
            ops::track(&self.project, track).and_then(|t| {
                t.clips
                    .iter()
                    .find(|c| c.id == clip)
                    .map(|c| (c.start, ops::clip_len(t, c), c.fade_in, c.fade_out))
            })
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
            ops::set_clip_fades(p, track, clip, new_in, new_out);
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
        let Some((pat_len, max_loops)) = ops::track(&self.project, track).and_then(|t| {
            t.clips
                .iter()
                .find(|c| c.id == clip)
                .map(|c| (pattern_steps(t, c), ops::max_clip_loops(t, c)))
        }) else {
            return;
        };
        if pat_len <= 0.0 {
            return;
        }
        let delta = ((tl.step(x) - grab_step) / pat_len).round() as i64;
        let loops = (i64::from(origin_loops) + delta).clamp(1, i64::from(max_loops)) as u32;
        self.edit("Clip loops", Some(&format!("clip-loops-{clip}")), |p| {
            ops::set_clip_loops(p, track, clip, loops);
        });
    }

    /// Sets a clip's loop count (context menu and tests); one undo step.
    pub(super) fn set_clip_loops(&mut self, track_id: u64, clip_id: u64, loops: u32) {
        self.edit("Clip loops", None, |p| {
            ops::set_clip_loops(p, track_id, clip_id, loops);
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
            ops::set_clip_fades(p, track_id, clip_id, fade_in, fade_out);
        });
    }

    fn arranger_context_menu(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        match self.arranger.context {
            Some(ContextTarget::Clip { track, clip }) => {
                if ui.button("Edit pattern").clicked() {
                    self.tab = Tab::Editor;
                    self.focus = Focus::Editor;
                }
                ui.separator();
                let repeat = |app: &mut Self, times: usize, label: &str| {
                    let mut last = None;
                    app.edit(label, None, |p| {
                        last = ops::repeat_clip(p, track, clip, times)
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
                    t.clips
                        .iter()
                        .find(|c| c.id == clip)
                        .map(|c| (c.start, ops::clip_len(t, c)))
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
                    t.clips.iter().find(|c| c.id == clip).map(|c| {
                        (
                            c.loops,
                            c.fade_in,
                            c.fade_out,
                            ops::clip_len(t, c),
                            ops::max_clip_loops(t, c),
                        )
                    })
                });
                if let Some((loops, fade_in, fade_out, span, max_loops)) = props {
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
                if ui.button("Make unique copy").on_hover_text("Give this clip its own copy of the pattern so edits don't change other clips").clicked() {
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
                        ops::delete_clip(p, track, clip);
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
                let patterns: Vec<(u64, String)> = ops::track(&self.project, track)
                    .map(|t| t.patterns.iter().map(|p| (p.id, p.name.clone())).collect())
                    .unwrap_or_default();
                for (pid, name) in patterns {
                    if ui.button(format!("Place \u{201c}{name}\u{201d}")).clicked() {
                        self.place_clip(track, Some(pid), step);
                    }
                }
                ui.separator();
                if ui.button("New empty pattern here").clicked() {
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
            }
            None => {
                ui.label(
                    RichText::new("Right-click a lane or clip")
                        .small()
                        .color(pal.text_faint),
                );
            }
        }
    }

    fn arranger_ruler_input(&mut self, ui: &mut Ui, resp: &egui::Response, tl: Timeline) {
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if resp.is_pointer_button_down_on() {
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
        let _ = resp.clone().on_hover_text(
            "Click or drag to move the playhead (snaps to beats; hold Shift for 1/16)",
        );
    }

    fn paint_lanes(&self, p: &Painter, grid: Rect, tl: Timeline, song_steps: f64) {
        let pal = self.palette();
        p.rect_filled(grid, 0.0, pal.canvas);
        let first = (self.arranger.scroll_y / LANE_H).floor().max(0.0) as usize;
        for (i, track) in self.project.tracks.iter().enumerate().skip(first) {
            let y = grid.top() + i as f32 * LANE_H - self.arranger.scroll_y;
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
            grid.top() + self.project.tracks.len() as f32 * LANE_H - self.arranger.scroll_y;
        if rows_end < grid.bottom() {
            p.rect_filled(
                Rect::from_min_max(pos2(grid.left(), rows_end), grid.max),
                0.0,
                pal.bg,
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
        let first = (self.arranger.scroll_y / LANE_H).floor().max(0.0) as usize;
        for (i, track) in self.project.tracks.iter().enumerate().skip(first) {
            let y = grid.top() + i as f32 * LANE_H - self.arranger.scroll_y;
            if y > grid.bottom() {
                break;
            }
            let audible = self.project.is_track_audible(i);
            let color = theme::rgb(track.color);
            for clip in &track.clips {
                let len = ops::clip_len(track, clip);
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
                    ops::pattern(track, clip.pattern_id),
                    color,
                    selected,
                    audible,
                    tl.pps,
                );
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

    fn paint_ruler(&self, ui: &Ui, ruler: Rect, tl: Timeline, song_steps: f64) {
        let pal = self.palette();
        let p = ui.painter_at(ruler);
        p.rect_filled(ruler, 0.0, pal.panel);
        let every = bar_label_every(tl.pps) as i64;
        let b0 = (tl.step(ruler.left()).max(0.0) / 16.0).floor() as i64;
        let b1 = (tl.step(ruler.right()) / 16.0).ceil() as i64;
        for b in b0..=b1 {
            let x = tl.x(b as f64 * 16.0);
            if b % every == 0 {
                p.vline(
                    x,
                    ruler.bottom() - 9.0..=ruler.bottom(),
                    Stroke::new(1.0_f32, pal.text_faint),
                );
                p.text(
                    pos2(x + 4.0, ruler.center().y - 2.0),
                    Align2::LEFT_CENTER,
                    format!("{}", b + 1),
                    FontId::proportional(11.0),
                    if (b as f64) * 16.0 < song_steps {
                        pal.text_dim
                    } else {
                        pal.text_faint
                    },
                );
            } else {
                p.vline(
                    x,
                    ruler.bottom() - 5.0..=ruler.bottom(),
                    Stroke::new(1.0_f32, pal.border),
                );
            }
        }
        let end = tl.x(song_steps);
        p.text(
            pos2(end + 4.0, ruler.bottom() - 3.0),
            Align2::LEFT_BOTTOM,
            "END",
            FontId::proportional(9.0),
            pal.danger,
        );
        let marker = tl.x(self.transport.start_marker);
        if (self.transport.start_marker - self.transport.position).abs() > 0.01 {
            p.add(Shape::convex_polygon(
                vec![
                    pos2(marker - 4.0, ruler.bottom() - 7.0),
                    pos2(marker + 4.0, ruler.bottom() - 7.0),
                    pos2(marker, ruler.bottom()),
                ],
                pal.link,
                Stroke::NONE,
            ));
        }
        let x = tl.x(self.transport.position);
        p.add(Shape::convex_polygon(
            vec![
                pos2(x - 6.0, ruler.top() + 3.0),
                pos2(x + 6.0, ruler.top() + 3.0),
                pos2(x + 6.0, ruler.bottom() - 9.0),
                pos2(x, ruler.bottom()),
                pos2(x - 6.0, ruler.bottom() - 9.0),
            ],
            pal.accent,
            Stroke::NONE,
        ));
        p.hline(
            ruler.x_range(),
            ruler.bottom() - 0.5,
            Stroke::new(1.0_f32, pal.border),
        );
    }

    fn lane_headers(&mut self, ui: &mut Ui, header: Rect, grid: Rect) {
        let pal = self.palette();
        let mut hui = ui.new_child(UiBuilder::new().max_rect(header));
        hui.set_clip_rect(header.intersect(ui.clip_rect()));
        hui.painter().rect_filled(header, 0.0, pal.panel);
        let first = (self.arranger.scroll_y / LANE_H).floor().max(0.0) as usize;
        let ids: Vec<u64> = self.project.tracks.iter().map(|t| t.id).collect();
        let mut action: Option<(u64, &str)> = None;
        for (i, id) in ids.iter().copied().enumerate().skip(first) {
            let y = grid.top() + i as f32 * LANE_H - self.arranger.scroll_y;
            if y > header.bottom() {
                break;
            }
            let Some(track) = ops::track(&self.project, id) else {
                continue;
            };
            let r = Rect::from_min_size(pos2(header.left(), y), vec2(HEADER_W, LANE_H));
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
            resp.context_menu(|ui| {
                if ui.button("Track settings…").clicked() {
                    action = Some((id, "inspect"));
                }
                if ui.button("Duplicate track").clicked() {
                    action = Some((id, "duplicate"));
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
        let add_y = grid.top() + ids.len() as f32 * LANE_H - self.arranger.scroll_y + 8.0;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
