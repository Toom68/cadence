//! Pattern editor: drum step sequencer and melodic piano roll.
//! Both are custom-painted canvases with their own scroll/zoom, note
//! draw/drag/resize/box-select, a velocity lane and drum accent painting.

use std::collections::BTreeSet;

use egui::{
    pos2, vec2, Align, Align2, Color32, CursorIcon, FontId, Layout, Margin, PointerButton, Pos2,
    Rect, RichText, Sense, Shape, Stroke, StrokeKind, Ui,
};

use super::arranger::snap_floor;
use super::theme::{
    self, BORDER, BORDER_SOFT, CANVAS, CANVAS_ALT, CORAL, GRID_BAR, GRID_BEAT, GRID_FAINT, INK,
    LIME, PANEL, PERIWINKLE, TEXT, TEXT_DIM, TEXT_FAINT,
};
use super::{ops, sidebar, CadenceApp, StatusKind};
use crate::model::{note_name, Note, Pattern, Track, DRUMS};

const KEYS_W: f32 = 66.0;
const DRUM_LABEL_W: f32 = 132.0;
const RULER_H: f32 = 24.0;
const VEL_H: f32 = 42.0;
const ROW_H: f32 = 16.0;
const RESIZE_ZONE: f32 = 7.0;
const MIN_PPS: f32 = 8.0;
const MAX_PPS: f32 = 64.0;
const FIRST_PITCH: u8 = 0;
const LAST_PITCH: u8 = 127;
const PITCHES: usize = (LAST_PITCH - FIRST_PITCH + 1) as usize;

#[derive(Debug, Clone)]
struct NoteDrag {
    /// Snapshots of the dragged notes when the gesture began.
    originals: Vec<Note>,
    /// Steps the pointer has moved so far, snapped.
    dstep: f64,
    dpitch: i32,
    mode: DragMode,
    anchor: Note,
    grab_step_offset: f64,
    grab_pitch: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragMode {
    Move,
    Resize,
}

#[derive(Debug, Clone)]
struct NoteMarquee {
    start: Pos2,
    current: Pos2,
    base: BTreeSet<u64>,
    additive: bool,
}

#[derive(Debug, Clone, Copy)]
struct PendingNote {
    pos: Pos2,
    pitch: u8,
    start: f64,
    draw_on_click: bool,
}

/// Last-frame editor geometry. `grid` feeds scrollbar logic; the rest is
/// written each frame so `#[cfg(test)]` helpers can aim clicks at real layout.
#[derive(Debug)]
#[allow(dead_code)]
struct EditorRects {
    grid: Rect,
    vel: Rect,
    keys: Rect,
    ruler: Rect,
    /// Drum label column (drum editor only).
    labels: Rect,
}

impl Default for EditorRects {
    fn default() -> Self {
        Self {
            grid: Rect::NOTHING,
            vel: Rect::NOTHING,
            keys: Rect::NOTHING,
            ruler: Rect::NOTHING,
            labels: Rect::NOTHING,
        }
    }
}

/// What a click under the pointer hit.
#[derive(Clone, Copy, Debug)]
struct NoteHit {
    index: usize,
    zone: HitZone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HitZone {
    Body,
    Resize,
}

#[derive(Debug)]
pub(super) struct EditorView {
    /// Default length for new notes, in steps.
    pub note_len: f64,
    /// Velocity applied to newly drawn notes and new drum hits.
    pub velocity: f32,
    /// Move/resize snap in steps.
    pub snap: f64,
    pub roll_pps: f32,
    roll_scroll_x: f32,
    roll_scroll_y: f32,
    pub drum_pps: f32,
    drum_scroll_x: f32,
    last_track: Option<u64>,
    drag: Option<NoteDrag>,
    draw: Option<u64>,
    pending_note: Option<PendingNote>,
    marquee: Option<NoteMarquee>,
    right_erase: bool,
    drum_paint: Option<bool>,
    vel_drag: Option<Vec<u64>>,
    /// Last-frame geometry, also used by headless tests to compute click points.
    rects: EditorRects,
    grid_id: egui::Id,
}

impl Default for EditorView {
    fn default() -> Self {
        Self {
            note_len: 2.0,
            velocity: 0.8,
            snap: 1.0,
            roll_pps: 22.0,
            roll_scroll_x: 0.0,
            roll_scroll_y: 0.0,
            drum_pps: 30.0,
            drum_scroll_x: 0.0,
            last_track: None,
            drag: None,
            draw: None,
            pending_note: None,
            marquee: None,
            right_erase: false,
            drum_paint: None,
            vel_drag: None,
            rects: EditorRects::default(),
            grid_id: egui::Id::NULL,
        }
    }
}

impl EditorView {
    pub fn reset_view(&mut self) {
        self.last_track = None;
        self.drag = None;
        self.draw = None;
        self.pending_note = None;
        self.marquee = None;
        self.drum_paint = None;
        self.vel_drag = None;
        self.roll_scroll_x = 0.0;
        self.drum_scroll_x = 0.0;
    }

    pub fn end_gesture(&mut self) {
        self.drag = None;
        self.draw = None;
        self.pending_note = None;
        self.marquee = None;
        self.right_erase = false;
        self.drum_paint = None;
        self.vel_drag = None;
    }

    /// Zooms the piano roll horizontally about the visible centre.
    pub fn zoom(&mut self, factor: f32) {
        self.roll_pps = (self.roll_pps * factor).clamp(MIN_PPS, MAX_PPS);
        self.drum_pps = (self.drum_pps * factor).clamp(MIN_PPS, MAX_PPS);
    }
}

#[derive(Clone, Copy)]
struct Roll {
    grid: Rect,
    keys: Rect,
    ruler: Rect,
    vel: Rect,
    pps: f32,
    scroll_x: f32,
    scroll_y: f32,
}

impl Roll {
    fn x(&self, step: f64) -> f32 {
        self.grid.left() + step as f32 * self.pps - self.scroll_x
    }
    fn step(&self, x: f32) -> f64 {
        ((x - self.grid.left() + self.scroll_x) / self.pps) as f64
    }
    fn y(&self, pitch: u8) -> f32 {
        self.grid.top() + (LAST_PITCH - pitch) as f32 * ROW_H - self.scroll_y
    }
    fn pitch_at(&self, y: f32) -> Option<u8> {
        let row = ((y - self.grid.top() + self.scroll_y) / ROW_H).floor() as i64;
        let pitch = LAST_PITCH as i64 - row;
        (FIRST_PITCH as i64..=LAST_PITCH as i64)
            .contains(&pitch)
            .then_some(pitch as u8)
    }
    fn note_rect(&self, n: &Note) -> Rect {
        let x = self.x(n.start);
        let y = self.y(n.pitch);
        Rect::from_min_size(
            pos2(x + 1.0, y + 1.0),
            vec2((n.duration as f32 * self.pps - 1.5).max(3.0), ROW_H - 2.0),
        )
    }
}

#[derive(Clone, Copy)]
struct DrumGrid {
    grid: Rect,
    labels: Rect,
    ruler: Rect,
    pps: f32,
    scroll_x: f32,
    row_h: f32,
    len: f64,
}

impl DrumGrid {
    fn x(&self, step: f64) -> f32 {
        self.grid.left() + step as f32 * self.pps - self.scroll_x
    }
    fn step(&self, x: f32) -> f64 {
        ((x - self.grid.left() + self.scroll_x) / self.pps) as f64
    }
    fn row_y(&self, row: usize) -> f32 {
        self.grid.top() + row as f32 * self.row_h
    }
    fn cell(&self, pos: Pos2) -> Option<(usize, u32)> {
        if !self.grid.contains(pos) {
            return None;
        }
        let row = ((pos.y - self.grid.top()) / self.row_h).floor() as usize;
        let step = self.step(pos.x).floor() as i64;
        (0..DRUMS.len())
            .contains(&row)
            .then_some(row)
            .filter(|_| step >= 0 && (step as f64) < self.len)
            .map(|r| (r, step as u32))
    }
}

impl CadenceApp {
    /// Center (grid-local) point of a drum cell for tests; None if not showing drums.
    #[cfg(test)]
    pub(crate) fn drum_cell_center(&self, row: usize, step: u32) -> Option<Pos2> {
        let r = &self.editor.rects;
        if r.grid == Rect::NOTHING || row >= DRUMS.len() {
            return None;
        }
        let len = self
            .selected_track()
            .and_then(|t| self.current_pattern_id(t))
            .and_then(|pid| self.selected_track().and_then(|t| ops::pattern(t, pid)))
            .map_or(16.0, |p| p.length as f64);
        if (step as f64) >= len {
            return None;
        }
        let g = drum_geometry(r, self.editor.drum_pps, self.editor.drum_scroll_x);
        let x = g.x(step as f64) + g.pps * 0.5;
        let y = g.row_y(row) + g.row_h * 0.5;
        (x >= g.grid.left() && x <= g.grid.right()).then(|| pos2(x, y))
    }

    #[cfg(test)]
    pub(crate) fn roll_cell_center(&self, pitch: u8, step: f64) -> Option<Pos2> {
        let r = &self.editor.rects;
        if r.grid == Rect::NOTHING {
            return None;
        }
        let roll = self.roll_for_test();
        let p = pos2(roll.x(step) + roll.pps * 0.25, roll.y(pitch) + ROW_H * 0.5);
        roll.grid.contains(p).then_some(p)
    }

    /// Screen x of a pattern step on the piano roll (tests).
    #[cfg(test)]
    pub(crate) fn roll_x(&self, step: f64) -> Option<f32> {
        let r = &self.editor.rects;
        if r.grid == Rect::NOTHING {
            return None;
        }
        let roll = self.roll_for_test();
        Some(roll.x(step))
    }

    /// Current editor canvas rects: (grid, velocity lane, ruler).
    #[cfg(test)]
    pub(crate) fn editor_test_rects(&self) -> (Rect, Rect, Rect) {
        let r = &self.editor.rects;
        (r.grid, r.vel, r.ruler)
    }

    #[cfg(test)]
    fn roll_for_test(&self) -> Roll {
        let r = &self.editor.rects;
        Roll {
            grid: r.grid,
            keys: r.keys,
            ruler: r.ruler,
            vel: r.vel,
            pps: self.editor.roll_pps,
            scroll_x: self.editor.roll_scroll_x,
            scroll_y: self.editor.roll_scroll_y,
        }
    }

    /// Sets the piano-roll scroll for tests.
    #[cfg(test)]
    pub(crate) fn set_roll_scroll(&mut self, x: f32, y: f32) {
        self.editor.roll_scroll_x = x.max(0.0);
        self.editor.roll_scroll_y = y.max(0.0);
    }
}

#[cfg(test)]
fn drum_geometry(rects: &EditorRects, pps: f32, scroll_x: f32) -> DrumGrid {
    DrumGrid {
        grid: rects.grid,
        labels: rects.labels,
        ruler: rects.ruler,
        pps,
        scroll_x,
        row_h: rects.grid.height() / DRUMS.len() as f32,
        len: 1.0,
    }
}

impl CadenceApp {
    pub(super) fn editor_ui(&mut self, ui: &mut Ui) {
        let Some(track) = self.selected_track().cloned() else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(
                        "No track selected — pick one in the arrangement or add an instrument.",
                    )
                    .color(TEXT_DIM),
                );
            });
            return;
        };
        let Some(pid) = self.current_pattern_id(&track) else {
            ui.centered_and_justified(|ui| {
                if ui.button("Create a pattern").clicked() {
                    let mut new = None;
                    let tid = track.id;
                    self.edit("New pattern", None, |p| {
                        if let Some(t) = ops::track_mut(p, tid) {
                            new = ops::new_pattern(t, 16);
                        }
                    });
                    if let Some(new) = new {
                        self.select_pattern(tid, new);
                    }
                }
            });
            return;
        };
        if self.editor.last_track != Some(track.id) {
            self.editor.last_track = Some(track.id);
            self.editor_roll_home(&track, pid);
        }
        self.editor_header(ui, &track, pid);
        if track.instrument.is_drums() && track.preset.is_none() {
            self.drum_editor(ui, &track, pid);
        } else {
            self.piano_roll(ui, &track, pid);
        }
    }

    /// Scrolls the piano roll so existing notes (or the instrument's home octave) are visible.
    fn editor_roll_home(&mut self, track: &Track, pid: u64) {
        let pat = ops::pattern(track, pid);
        let pitch_center = pat
            .filter(|p| !p.notes.is_empty())
            .map(|p| p.notes.iter().map(|n| n.pitch as u32).sum::<u32>() / p.notes.len() as u32)
            .unwrap_or_else(|| {
                // An empty pack-preset pattern homes on its audition note;
                // built-ins keep their instrument default pitch.
                track.preset.as_ref().map_or_else(
                    || track.instrument.default_pitch() as u32,
                    |p| u32::from(p.audition_note),
                )
            });
        let row = (LAST_PITCH as u32 - pitch_center) as f32;
        // Before the first paint the grid rect is still empty; use a typical
        // editor height so the first frame already shows the right octave.
        let grid_h = self.editor.rects.grid.height();
        let h = if grid_h > 40.0 { grid_h } else { 280.0 };
        let content = PITCHES as f32 * ROW_H;
        self.editor.roll_scroll_y = (row * ROW_H - h * 0.5).clamp(0.0, (content - h).max(0.0));
        self.editor.roll_scroll_x = 0.0;
    }

    // ----- header ----------------------------------------------------------

    fn editor_header(&mut self, ui: &mut Ui, track: &Track, pid: u64) {
        egui::Frame::new()
            .inner_margin(Margin {
                left: 12,
                right: 12,
                top: 4,
                bottom: 4,
            })
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    theme::color_dot(ui, theme::rgb(track.color), 10.0);
                    ui.label(RichText::new(&track.name).size(15.5).color(TEXT));
                    ui.label(
                        RichText::new(ops::instrument_label(track))
                            .small()
                            .color(TEXT_DIM),
                    );
                    ui.label(RichText::new("/").color(TEXT_FAINT));

                    // Pattern selector
                    let cur = ops::pattern(track, pid)
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                    let mut pick = None;
                    egui::ComboBox::from_id_salt(("pattern", track.id))
                        .width(150.0)
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            for p in &track.patterns {
                                if ui
                                    .selectable_label(
                                        p.id == pid,
                                        format!("{} · {}", p.name, ops::length_label(p.length)),
                                    )
                                    .clicked()
                                {
                                    pick = Some(p.id);
                                }
                            }
                        });
                    if let Some(new) = pick {
                        self.select_pattern(track.id, new);
                    }
                    // Rename
                    let mut name = ops::pattern(track, pid)
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                    let w = (name.len() as f32 * 7.5 + 24.0).clamp(70.0, 160.0);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut name)
                            .desired_width(w)
                            .char_limit(crate::model::MAX_NAME_CHARS),
                    );
                    if resp.changed() {
                        self.edit("Rename pattern", Some(&format!("patname-{pid}")), |p| {
                            if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                                pat.name = name.clone();
                            }
                        });
                    }
                    resp.on_hover_text("Pattern name");

                    // Length
                    let plen = ops::pattern(track, pid).map(|p| p.length).unwrap_or(16);
                    let mut new_len = None;
                    egui::ComboBox::from_id_salt(("patlen", track.id))
                        .width(74.0)
                        .selected_text(ops::length_label(plen))
                        .show_ui(ui, |ui| {
                            let mut lens = ops::PATTERN_LENGTHS.to_vec();
                            if !lens.contains(&plen) {
                                lens.push(plen);
                                lens.sort_unstable();
                            }
                            for l in lens {
                                ui.selectable_value(&mut new_len, Some(l), ops::length_label(l));
                            }
                        });
                    if let Some(l) = new_len {
                        if l != plen {
                            self.edit("Pattern length", None, |p| {
                                ops::set_track_pattern_length(p, track.id, pid, l);
                            });
                            self.set_status(
                                StatusKind::Info,
                                format!("Pattern length set to {}.", ops::length_label(l)),
                            );
                        }
                    }

                    if theme::icon_button(
                        ui,
                        theme::Icon::Plus,
                        26.0,
                        false,
                        LIME,
                        "New empty pattern",
                    )
                    .clicked()
                    {
                        let len = ops::pattern(track, pid).map(|p| p.length).unwrap_or(16);
                        let mut new = None;
                        let tid = track.id;
                        self.edit("New pattern", None, |p| {
                            if let Some(t) = ops::track_mut(p, tid) {
                                new = ops::new_pattern(t, len);
                            }
                        });
                        match new {
                            Some(n) => self.select_pattern(tid, n),
                            None => self.set_status(
                                StatusKind::Warning,
                                format!(
                                    "Tracks hold up to {} patterns.",
                                    ops::MAX_PATTERNS_PER_TRACK
                                ),
                            ),
                        }
                    }
                    if theme::icon_button(
                        ui,
                        theme::Icon::Duplicate,
                        26.0,
                        false,
                        LIME,
                        "Duplicate pattern",
                    )
                    .clicked()
                    {
                        let mut new = None;
                        let tid = track.id;
                        self.edit("Duplicate pattern", None, |p| {
                            if let Some(t) = ops::track_mut(p, tid) {
                                new = ops::duplicate_pattern_in(t, pid);
                            }
                        });
                        match new {
                            Some(n) => self.select_pattern(tid, n),
                            None => self.set_status(
                                StatusKind::Warning,
                                format!(
                                    "Tracks hold up to {} patterns.",
                                    ops::MAX_PATTERNS_PER_TRACK
                                ),
                            ),
                        }
                    }
                    let note_count = ops::pattern(track, pid).map_or(0, |p| p.notes.len());
                    if theme::icon_button(
                        ui,
                        theme::Icon::Trash,
                        26.0,
                        false,
                        CORAL,
                        "Clear all notes in this pattern",
                    )
                    .clicked()
                        && note_count > 0
                    {
                        self.edit("Clear pattern", None, |p| {
                            if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                                pat.notes.clear();
                            }
                        });
                        self.sel.notes.clear();
                    }
                    let many = track.patterns.len() > 1;
                    let del = egui::Button::new(RichText::new("Delete").small())
                        .min_size(vec2(0.0, 26.0));
                    if ui
                        .add_enabled(many, del)
                        .on_hover_text(if many {
                            "Delete this pattern (its clips are removed too)"
                        } else {
                            "A track keeps at least one pattern"
                        })
                        .clicked()
                    {
                        self.edit("Delete pattern", None, |p| {
                            if let Some(t) = ops::track_mut(p, track.id) {
                                ops::delete_pattern(t, pid);
                            }
                        });
                        self.fix_selection();
                    }

                    ui.separator();
                    if ui
                        .add(
                            egui::Button::new("Place clip")
                                .small()
                                .min_size(vec2(0.0, 26.0)),
                        )
                        .on_hover_text("Place this pattern at the playhead (snapped to the bar)")
                        .clicked()
                    {
                        let start = snap_floor(self.transport.position, 16.0);
                        self.place_clip(track.id, Some(pid), start);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::chip(
                            ui,
                            "Track",
                            self.side_tab == sidebar::SideTab::Track,
                            PERIWINKLE,
                            vec2(52.0, 24.0),
                            "Open track settings",
                        )
                        .clicked()
                        {
                            self.side_tab = sidebar::SideTab::Track;
                        }
                    });
                });
            });
    }

    // ----- piano roll ------------------------------------------------------

    fn piano_roll(&mut self, ui: &mut Ui, track: &Track, pid: u64) {
        self.roll_toolbar(ui, track, pid);
        let Some(pat) = ops::pattern(track, pid).cloned() else {
            return;
        };
        let area = ui.available_rect_before_wrap();
        if area.width() < 200.0 || area.height() < 80.0 {
            return;
        }
        let grid = Rect::from_min_max(
            pos2(area.left() + KEYS_W, area.top() + RULER_H),
            pos2(area.right(), area.bottom() - VEL_H),
        );
        let keys = Rect::from_min_max(
            pos2(area.left(), grid.top()),
            pos2(grid.left(), grid.bottom()),
        );
        let ruler = Rect::from_min_max(
            pos2(grid.left(), area.top()),
            pos2(grid.right(), grid.top()),
        );
        let vel = Rect::from_min_max(
            pos2(grid.left(), grid.bottom()),
            pos2(grid.right(), area.bottom()),
        );
        let vel_labels = Rect::from_min_max(
            pos2(area.left(), vel.top()),
            pos2(grid.left(), vel.bottom()),
        );
        self.editor.rects = EditorRects {
            grid,
            vel,
            keys,
            ruler,
            labels: Rect::NOTHING,
        };
        self.editor.grid_id = ui.id().with("piano_grid");

        let content_w = pat.length as f32 * self.editor.roll_pps;
        let content_h = PITCHES as f32 * ROW_H;
        // scroll/zoom input
        let grid_resp = ui.interact(grid, self.editor.grid_id, Sense::click_and_drag());
        grid_resp
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Piano roll"));
        if ui.rect_contains_pointer(area) {
            let (scroll, zoom, hover) =
                ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
            if zoom != 1.0 {
                let anchor = hover.map_or(grid.width() * 0.5, |h| (h.x - grid.left()).max(0.0));
                let step = (anchor + self.editor.roll_scroll_x) / self.editor.roll_pps;
                self.editor.roll_pps = (self.editor.roll_pps * zoom).clamp(MIN_PPS, MAX_PPS);
                self.editor.roll_scroll_x = (step * self.editor.roll_pps - anchor).max(0.0);
            }
            if scroll != egui::Vec2::ZERO {
                self.editor.roll_scroll_x -= scroll.x;
                self.editor.roll_scroll_y -= scroll.y;
                ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
            }
        }
        self.editor.roll_scroll_x = self
            .editor
            .roll_scroll_x
            .clamp(0.0, (content_w - grid.width()).max(0.0));
        self.editor.roll_scroll_y = self
            .editor
            .roll_scroll_y
            .clamp(0.0, (content_h - grid.height()).max(0.0));
        let roll = Roll {
            grid,
            keys,
            ruler,
            vel,
            pps: self.editor.roll_pps,
            scroll_x: self.editor.roll_scroll_x,
            scroll_y: self.editor.roll_scroll_y,
        };

        self.roll_input(ui, track, pid, &pat, &roll, &grid_resp);
        self.paint_roll(ui, track, &pat, &roll, grid_resp.interact_pointer_pos());
        self.paint_keys(ui, track, &roll);
        self.paint_roll_ruler(ui, track, &pat, &roll);
        self.paint_vel_lane(ui, track.id, &pat, &roll, vel_labels);
        let mut sx = self.editor.roll_scroll_x;
        let hbar = Rect::from_min_max(pos2(grid.left(), vel.bottom() - 8.0), grid.right_bottom());
        theme::scrollbar(
            ui,
            hbar.shrink2(vec2(0.0, 1.0)),
            ui.id().with("roll_hbar"),
            false,
            content_w,
            grid.width(),
            &mut sx,
        );
        self.editor.roll_scroll_x = sx;
        let mut sy = self.editor.roll_scroll_y;
        let vbar = Rect::from_min_max(
            pos2(area.left() - 0.0, grid.top()),
            pos2(area.left() + 8.0, grid.bottom()),
        );
        theme::scrollbar(
            ui,
            vbar.shrink2(vec2(1.0, 0.0)),
            ui.id().with("roll_vbar"),
            true,
            content_h,
            grid.height(),
            &mut sy,
        );
        self.editor.roll_scroll_y = sy;
    }

    fn roll_toolbar(&mut self, ui: &mut Ui, track: &Track, pid: u64) {
        egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 0, bottom: 4 }).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 5.0);
                theme::color_dot(ui, LIME, 6.0);
                ui.label(RichText::new("Piano roll").small().color(TEXT_DIM));
                ui.label(RichText::new("Note").small().color(TEXT_FAINT));
                egui::ComboBox::from_id_salt(("notelen", track.id)).width(62.0).selected_text(note_len_label(self.editor.note_len)).show_ui(ui, |ui| {
                    for (v, l) in [(1.0, "1/16"), (2.0, "1/8"), (4.0, "1/4"), (8.0, "1/2"), (16.0, "1 bar")] {
                        ui.selectable_value(&mut self.editor.note_len, v, l);
                    }
                });
                ui.label(RichText::new("Snap").small().color(TEXT_FAINT));
                egui::ComboBox::from_id_salt(("editsnap", track.id)).width(62.0).selected_text(note_len_label(self.editor.snap)).show_ui(ui, |ui| {
                    for (v, l) in [(0.5, "1/32"), (1.0, "1/16"), (2.0, "1/8"), (4.0, "1/4")] {
                        ui.selectable_value(&mut self.editor.snap, v, l);
                    }
                });
                ui.label(RichText::new("Velocity").small().color(TEXT_FAINT));
                let mut v = self.editor.velocity;
                ui.spacing_mut().slider_width = 110.0;
                if ui.add(egui::Slider::new(&mut v, 0.05..=1.0).custom_formatter(|x, _| format!("{:.0}%", x * 100.0))).changed() {
                    self.editor.velocity = v;
                }
                let n = self.sel.notes.len();
                if n > 0 {
                    ui.label(RichText::new(format!("{n} selected")).small().color(LIME));
                    if ui.add(egui::Button::new("Delete").small()).on_hover_text("Delete selected notes (Del)").clicked() {
                        self.delete_selection();
                    }
                }
                let outside = ops::pattern(track, pid).map_or(0, |p| {
                    p.notes.iter().filter(|n| n.start.fract() != 0.0).count()
                });
                if outside > 0 {
                    ui.label(RichText::new(format!("{outside} off-grid")).small().color(TEXT_FAINT)).on_hover_text("These notes sit between grid steps (e.g. imported from MIDI). Moving them keeps their offset.");
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    theme::hint(ui, "Click to draw · drag empty grid to box-select · drag selected notes as a group · right edge resizes · right-click erases");
                });
            });
        });
    }

    fn roll_note_at(&self, pat: &Pattern, roll: &Roll, pos: Pos2) -> Option<NoteHit> {
        // Topmost (last drawn) first.
        for (i, n) in pat.notes.iter().enumerate().rev() {
            let r = roll.note_rect(n);
            if r.contains(pos) {
                let zone = if pos.x >= r.right() - RESIZE_ZONE {
                    HitZone::Resize
                } else {
                    HitZone::Body
                };
                return Some(NoteHit { index: i, zone });
            }
        }
        None
    }

    fn update_note_marquee_selection(&mut self, pat: &Pattern, roll: &Roll) {
        let Some(m) = self.editor.marquee.clone() else {
            return;
        };
        let rect = Rect::from_two_pos(m.start, m.current);
        let mut ids = if m.additive { m.base } else { BTreeSet::new() };
        ids.extend(
            pat.notes
                .iter()
                .filter(|n| roll.note_rect(n).intersects(rect))
                .map(|n| n.id),
        );
        self.sel.notes = ids;
    }

    fn finish_pending_note(
        &mut self,
        track: &Track,
        pid: u64,
        pat: &Pattern,
        pending: PendingNote,
    ) {
        if !pending.draw_on_click {
            return;
        }
        let existing = pat.notes.iter().find(|n| {
            n.pitch == pending.pitch && pending.start >= n.start && pending.start < n.end()
        });
        match existing {
            Some(n) => {
                self.sel.notes.clear();
                self.sel.notes.insert(n.id);
            }
            None => {
                let vel = self.editor.velocity;
                let len = self.editor.note_len;
                let mut new = None;
                self.edit("Draw note", Some("draw-note"), |p| {
                    if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                        new = ops::add_note(pat, pending.pitch, pending.start, len, vel);
                    }
                });
                if let Some(new) = new {
                    self.sel.notes.clear();
                    self.sel.notes.insert(new);
                    self.preview(track.id, pending.pitch, vel);
                }
            }
        }
    }

    fn roll_input(
        &mut self,
        ui: &mut Ui,
        track: &Track,
        pid: u64,
        pat: &Pattern,
        roll: &Roll,
        resp: &egui::Response,
    ) {
        let (pressed, released, shift, alt, secondary_pressed, secondary_down) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.modifiers.shift,
                i.modifiers.alt,
                i.pointer.secondary_pressed(),
                i.pointer.secondary_down(),
            )
        });
        let hover = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
        if let Some(h) = hover.filter(|_| resp.hovered() && self.editor.drag.is_none()) {
            match self.roll_note_at(pat, roll, h) {
                Some(hit) if hit.zone == HitZone::Resize => {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal)
                }
                Some(_) => ui.ctx().set_cursor_icon(CursorIcon::Move),
                None => {
                    let _ = roll.pitch_at(h.y);
                }
            }
        }
        if pressed && resp.is_pointer_button_down_on() {
            self.focus = super::Focus::Editor;
            if let Some(pos) = ui.input(|i| i.pointer.press_origin()) {
                match self.roll_note_at(pat, roll, pos) {
                    Some(hit) => {
                        let note = pat.notes[hit.index].clone();
                        if shift {
                            if !self.sel.notes.remove(&note.id) {
                                self.sel.notes.insert(note.id);
                            }
                        } else {
                            if !self.sel.notes.contains(&note.id) {
                                self.sel.notes.clear();
                                self.sel.notes.insert(note.id);
                            }
                            let originals: Vec<Note> = pat
                                .notes
                                .iter()
                                .filter(|n| self.sel.notes.contains(&n.id))
                                .cloned()
                                .collect();
                            let mode = match hit.zone {
                                HitZone::Resize => DragMode::Resize,
                                HitZone::Body => DragMode::Move,
                            };
                            self.editor.drag = Some(NoteDrag {
                                originals,
                                dstep: 0.0,
                                dpitch: 0,
                                mode,
                                anchor: note.clone(),
                                grab_step_offset: roll.step(pos.x) - note.start,
                                grab_pitch: roll.pitch_at(pos.y).unwrap_or(note.pitch),
                            });
                            self.preview(track.id, note.pitch, note.velocity);
                        }
                    }
                    None => {
                        if shift || alt {
                            self.editor.marquee = Some(NoteMarquee {
                                start: pos,
                                current: pos,
                                base: self.sel.notes.clone(),
                                additive: shift,
                            });
                            if !shift {
                                self.sel.notes.clear();
                            }
                        } else if let Some(pitch) = roll.pitch_at(pos.y) {
                            let snap = self.editor.snap;
                            let start = snap_floor(roll.step(pos.x).max(0.0), snap)
                                .min(pat.length as f64 - 0.5)
                                .max(0.0);
                            self.editor.pending_note = Some(PendingNote {
                                pos,
                                pitch,
                                start,
                                draw_on_click: true,
                            });
                        }
                    }
                }
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            let pos = resp
                .interact_pointer_pos()
                .or_else(|| ui.input(|i| i.pointer.interact_pos()));
            if let Some(pos) = pos {
                if let Some(pending) = self.editor.pending_note.take() {
                    self.editor.marquee = Some(NoteMarquee {
                        start: pending.pos,
                        current: pos,
                        base: BTreeSet::new(),
                        additive: false,
                    });
                    self.sel.notes.clear();
                    self.update_note_marquee_selection(pat, roll);
                }
                if let Some(d) = self.editor.drag.clone() {
                    let snap = self.editor.snap;
                    let (ds, dp) = match d.mode {
                        DragMode::Move => (
                            snap_floor(roll.step(pos.x) - d.grab_step_offset, snap)
                                - snap_floor(d.anchor.start, snap),
                            (roll.pitch_at(pos.y).unwrap_or(d.grab_pitch) as i32
                                - d.grab_pitch as i32)
                                .clamp(-120, 120),
                        ),
                        DragMode::Resize => (
                            (((roll.step(pos.x) - d.anchor.start) / snap).round() * snap)
                                - d.anchor.duration,
                            0,
                        ),
                    };
                    if ds != d.dstep || dp != d.dpitch {
                        let dp_changed = dp != d.dpitch;
                        let mut drag = d;
                        drag.dstep = ds;
                        drag.dpitch = dp;
                        let label = match drag.mode {
                            DragMode::Move => "Move notes",
                            DragMode::Resize => "Resize notes",
                        };
                        let originals = drag.originals.clone();
                        if drag.mode == DragMode::Move && dp_changed {
                            self.preview(
                                track.id,
                                (drag.anchor.pitch as i32 + dp).clamp(0, ops::MAX_PITCH as i32)
                                    as u8,
                                0.7,
                            );
                        }
                        self.edit(label, Some("note-drag"), |p| {
                            if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                                match drag.mode {
                                    DragMode::Move => ops::move_notes(pat, &originals, ds, dp),
                                    DragMode::Resize => ops::resize_notes(
                                        pat,
                                        &originals,
                                        ds,
                                        snap.max(ops::MIN_NOTE_DURATION),
                                    ),
                                }
                            }
                        });
                        self.editor.drag = Some(drag);
                    }
                }
                if let Some(id) = self.editor.draw {
                    let snap = self.editor.snap.max(0.5);
                    let start = pat
                        .notes
                        .iter()
                        .find(|n| n.id == id)
                        .map(|n| n.start)
                        .unwrap_or(0.0);
                    let dur = ((roll.step(pos.x) - start) / snap).ceil() * snap;
                    self.edit("Draw note", Some("draw-note"), |p| {
                        if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                            if let Some(n) = pat.notes.iter_mut().find(|n| n.id == id) {
                                n.duration = dur.clamp(
                                    ops::MIN_NOTE_DURATION,
                                    (pat.length as f64 - n.start).max(ops::MIN_NOTE_DURATION),
                                );
                            }
                        }
                    });
                }
                if let Some(m) = self.editor.marquee.as_mut() {
                    m.current = pos;
                    self.update_note_marquee_selection(pat, roll);
                }
            }
        }
        if released {
            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                if let Some(m) = self.editor.marquee.as_mut() {
                    m.current = pos;
                    self.update_note_marquee_selection(pat, roll);
                }
            }
            if self.editor.marquee.take().is_none() {
                if let Some(pending) = self.editor.pending_note.take() {
                    self.finish_pending_note(track, pid, pat, pending);
                }
            }
            self.editor.draw = None;
        }
        // Right button erases notes under the cursor as it moves.
        if secondary_pressed && resp.is_pointer_button_down_on() {
            self.editor.right_erase = true;
        }
        if !secondary_down {
            self.editor.right_erase = false;
        }
        if self.editor.right_erase {
            if let Some(pos) = hover {
                if let Some(hit) = self.roll_note_at(pat, roll, pos) {
                    let id = pat.notes[hit.index].id;
                    self.edit("Erase note", Some("erase-notes"), |p| {
                        if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                            ops::delete_notes(pat, &[id]);
                        }
                    });
                    self.sel.notes.remove(&id);
                }
            }
        }
        // Velocity lane input
        let vel_resp = ui.interact(roll.vel, ui.id().with("vel_lane"), Sense::click_and_drag());
        if vel_resp.is_pointer_button_down_on() || vel_resp.dragged_by(PointerButton::Primary) {
            if let Some(pos) = vel_resp.interact_pointer_pos() {
                let v =
                    (1.0 - (pos.y - roll.vel.top()) / roll.vel.height().max(1.0)).clamp(0.05, 1.0);
                let step = roll.step(pos.x);
                // Which notes are under the x position (any note starting nearest)?
                let hit_note = pat
                    .notes
                    .iter()
                    .filter(|n| step >= n.start)
                    .min_by(|a, b| {
                        (step - a.start)
                            .partial_cmp(&(step - b.start))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .cloned();
                let targets: Vec<u64> = if !self.sel.notes.is_empty() {
                    self.sel.notes.iter().copied().collect()
                } else {
                    hit_note.map(|n| vec![n.id]).unwrap_or_default()
                };
                if !targets.is_empty() {
                    let drag_ids = self
                        .editor
                        .vel_drag
                        .clone()
                        .unwrap_or_else(|| targets.clone());
                    self.edit("Note velocity", Some("vel-drag"), |p| {
                        if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                            for n in pat.notes.iter_mut() {
                                if drag_ids.contains(&n.id) {
                                    n.velocity = v;
                                }
                            }
                        }
                    });
                    self.editor.vel_drag = Some(drag_ids);
                }
            }
        }
    }

    fn paint_roll(&self, ui: &Ui, track: &Track, pat: &Pattern, roll: &Roll, hover: Option<Pos2>) {
        let p = ui.painter_at(roll.grid.union(roll.vel).union(roll.ruler));
        // Row backgrounds
        let first_row = (roll.scroll_y / ROW_H).floor().max(0.0) as usize;
        let last_row = ((roll.scroll_y + roll.grid.height()) / ROW_H)
            .ceil()
            .min(PITCHES as f32) as usize;
        for row in first_row..last_row {
            let pitch = LAST_PITCH - row as u8;
            let y = roll.grid.top() + row as f32 * ROW_H - roll.scroll_y;
            let black = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
            let r = Rect::from_min_size(pos2(roll.grid.left(), y), vec2(roll.grid.width(), ROW_H));
            p.rect_filled(
                r,
                0.0,
                if black {
                    Color32::from_black_alpha(38)
                } else {
                    CANVAS
                },
            );
            if pitch.is_multiple_of(12) {
                p.hline(roll.grid.x_range(), y, Stroke::new(1.0_f32, GRID_BAR));
            } else {
                p.hline(roll.grid.x_range(), y, Stroke::new(0.5_f32, GRID_FAINT));
            }
        }
        // Step/beat/bar lines
        let s0 = roll.step(roll.grid.left()).max(0.0).floor() as i64;
        let s1 = roll.step(roll.grid.right()).ceil() as i64;
        let step_lines = roll.pps >= 14.0;
        for s in s0.max(0)..=s1.min(pat.length as i64 * 2) {
            if s < 0 {
                break;
            }
            let x = roll.x(s as f64);
            if s % 16 == 0 {
                p.vline(x, roll.grid.y_range(), Stroke::new(1.0_f32, GRID_BAR));
            } else if s % 4 == 0 {
                p.vline(x, roll.grid.y_range(), Stroke::new(1.0_f32, GRID_BEAT));
            } else if step_lines {
                p.vline(x, roll.grid.y_range(), Stroke::new(0.5_f32, GRID_FAINT));
            }
        }
        // Beyond pattern end
        let end_x = roll.x(pat.length as f64);
        if end_x < roll.grid.right() {
            p.rect_filled(
                Rect::from_min_max(
                    pos2(end_x, roll.grid.top()),
                    pos2(roll.grid.right(), roll.vel.bottom()),
                ),
                0.0,
                Color32::from_black_alpha(80),
            );
        }
        // Notes
        let color = theme::rgb(track.color);
        let visible_lo = roll
            .pitch_at(roll.grid.top() + roll.grid.height())
            .unwrap_or(0);
        let visible_hi = roll.pitch_at(roll.grid.top()).unwrap_or(127);
        for n in &pat.notes {
            if n.pitch < visible_lo || n.pitch > visible_hi {
                continue;
            }
            let r = roll.note_rect(n);
            if r.right() < roll.grid.left() || r.left() > roll.grid.right() {
                continue;
            }
            let selected = self.sel.notes.contains(&n.id);
            let body = theme::mix(BORDER_SOFT, color, 0.35 + n.velocity * 0.65);
            p.rect_filled(r, 3.0, body);
            if selected {
                p.rect_stroke(
                    r.expand(1.0),
                    3.0,
                    Stroke::new(1.5_f32, TEXT),
                    StrokeKind::Inside,
                );
            }
            // Resize affordance on the right edge
            if r.width() > 8.0 {
                p.rect_filled(
                    Rect::from_min_max(
                        pos2(r.right() - 3.5, r.top() + 3.0),
                        pos2(r.right() - 1.5, r.bottom() - 3.0),
                    ),
                    1.0,
                    Color32::from_black_alpha(90),
                );
            }
            if r.width() > 26.0 {
                p.text(
                    pos2(r.left() + 5.0, r.center().y),
                    Align2::LEFT_CENTER,
                    note_name(n.pitch),
                    FontId::proportional(9.5),
                    INK,
                );
            }
        }
        // Ghost of dragged position? (current drag delta shown by note movement already)
        // Marquee
        if let Some(m) = &self.editor.marquee {
            let r = Rect::from_two_pos(m.start, m.current).intersect(roll.grid);
            if r.is_positive() {
                p.rect_filled(r, 0.0, PERIWINKLE.gamma_multiply(0.12));
                p.rect_stroke(r, 0.0, Stroke::new(1.0_f32, PERIWINKLE), StrokeKind::Inside);
            }
        }
        // Playhead inside a clip of this pattern
        if self.transport.playing {
            if let Some(local) = self.pattern_playhead(track, pat.id) {
                let x = roll.x(local);
                if x >= roll.grid.left() && x <= roll.grid.right() {
                    p.vline(
                        x,
                        roll.grid.top()..=roll.vel.bottom(),
                        Stroke::new(1.5_f32, LIME),
                    );
                }
            }
        }
        // Hover ghost cell
        if self.editor.drag.is_none() && self.editor.draw.is_none() {
            if let Some(pos) = hover
                .filter(|h| roll.grid.contains(*h) && self.roll_note_at(pat, roll, *h).is_none())
            {
                if let Some(pitch) = roll.pitch_at(pos.y) {
                    let step = snap_floor(roll.step(pos.x), self.editor.snap);
                    let gh = Rect::from_min_size(
                        pos2(roll.x(step), roll.y(pitch) + 1.0),
                        vec2(self.editor.note_len as f32 * roll.pps, ROW_H - 2.0),
                    );
                    p.rect_filled(gh, 3.0, color.gamma_multiply(0.18));
                    p.rect_stroke(
                        gh,
                        3.0,
                        Stroke::new(1.0_f32, color.gamma_multiply(0.6)),
                        StrokeKind::Inside,
                    );
                }
            }
        }
        // Empty pattern hint
        if pat.notes.is_empty() {
            p.text(
                roll.grid.center(),
                Align2::CENTER_CENTER,
                "Empty pattern — click the grid to draw notes",
                FontId::proportional(13.0),
                TEXT_FAINT,
            );
        }
        // Borders
        p.hline(
            roll.grid.x_range(),
            roll.ruler.bottom(),
            Stroke::new(1.0_f32, BORDER),
        );
        p.vline(
            roll.grid.left(),
            roll.grid.top()..=roll.vel.bottom(),
            Stroke::new(1.0_f32, BORDER),
        );
        p.hline(
            roll.grid.x_range(),
            roll.vel.top(),
            Stroke::new(1.0_f32, BORDER),
        );
    }

    fn pattern_playhead(&self, track: &Track, pid: u64) -> Option<f64> {
        let pos = self.transport.position;
        for clip in &track.clips {
            if clip.pattern_id == pid {
                let end = clip.start + ops::pattern(track, pid).map_or(0.0, |p| p.length as f64);
                if pos >= clip.start && pos < end {
                    return Some(pos - clip.start);
                }
            }
        }
        None
    }

    fn paint_keys(&self, ui: &mut Ui, track: &Track, roll: &Roll) {
        let p = ui.painter_at(roll.keys);
        p.rect_filled(roll.keys, 0.0, PANEL);
        let first_row = (roll.scroll_y / ROW_H).floor().max(0.0) as usize;
        let last_row = ((roll.scroll_y + roll.grid.height()) / ROW_H)
            .ceil()
            .min(PITCHES as f32) as usize;
        let mut preview_pitch = None;
        for row in first_row..last_row {
            let pitch = LAST_PITCH - row as u8;
            let black = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
            let y = roll.grid.top() + row as f32 * ROW_H - roll.scroll_y;
            let r = Rect::from_min_max(
                pos2(roll.keys.left(), y),
                pos2(
                    if black {
                        roll.keys.left() + 40.0
                    } else {
                        roll.keys.right()
                    },
                    y + ROW_H,
                ),
            );
            let resp = ui.interact(r, ui.id().with(("key", pitch)), Sense::click());
            if resp.hovered() || resp.is_pointer_button_down_on() {
                p.rect_filled(r, 0.0, theme::CONTROL_HOVER);
            } else {
                p.rect_filled(
                    r,
                    0.0,
                    if black {
                        Color32::from_rgb(15, 14, 13)
                    } else {
                        theme::mix(PANEL, TEXT, 0.22)
                    },
                );
            }
            if pitch.is_multiple_of(12) {
                p.text(
                    pos2(roll.keys.right() - 6.0, y + ROW_H * 0.5),
                    Align2::RIGHT_CENTER,
                    note_name(pitch),
                    FontId::proportional(9.5),
                    TEXT_FAINT,
                );
            }
            if resp.clicked() || resp.dragged_by(PointerButton::Primary) {
                preview_pitch = Some(pitch);
            }
        }
        p.vline(
            roll.keys.right(),
            roll.keys.y_range(),
            Stroke::new(1.0_f32, BORDER),
        );
        if let Some(pitch) = preview_pitch {
            let vel = self.editor.velocity;
            self.preview(track.id, pitch, vel);
        }
    }

    fn paint_roll_ruler(&self, ui: &Ui, track: &Track, pat: &Pattern, roll: &Roll) {
        let p = ui.painter_at(roll.ruler);
        p.rect_filled(roll.ruler, 0.0, PANEL);
        let pps = roll.pps;
        let every = [1i64, 2, 4, 8, 16]
            .into_iter()
            .find(|k| *k as f32 * 16.0 * pps >= 44.0)
            .unwrap_or(16);
        let b0 = (roll.step(roll.ruler.left()).max(0.0) / 16.0).floor() as i64;
        let b1 = (roll.step(roll.ruler.right()) / 16.0).ceil() as i64;
        for b in b0..=b1.min((pat.length as i64 + 15) / 16) {
            let x = roll.x(b as f64 * 16.0);
            if b % every == 0 && b * 16 < pat.length as i64 {
                p.text(
                    pos2(x + 3.0, roll.ruler.center().y),
                    Align2::LEFT_CENTER,
                    format!("{}", b + 1),
                    FontId::proportional(10.5),
                    TEXT_DIM,
                );
            }
            p.vline(
                x,
                roll.ruler.bottom() - 6.0..=roll.ruler.bottom(),
                Stroke::new(1.0_f32, BORDER),
            );
        }
        if self.transport.playing {
            if let Some(local) = self.pattern_playhead(track, pat.id) {
                let x = roll.x(local);
                p.add(Shape::convex_polygon(
                    vec![
                        pos2(x - 5.0, roll.ruler.bottom() - 8.0),
                        pos2(x + 5.0, roll.ruler.bottom() - 8.0),
                        pos2(x, roll.ruler.bottom() - 1.0),
                    ],
                    LIME,
                    Stroke::NONE,
                ));
            }
        }
    }

    fn paint_vel_lane(&self, ui: &Ui, track_id: u64, pat: &Pattern, roll: &Roll, labels: Rect) {
        let p = ui.painter_at(roll.vel.union(labels));
        p.rect_filled(labels, 0.0, PANEL);
        p.text(
            labels.center(),
            Align2::CENTER_CENTER,
            "VEL",
            FontId::proportional(9.0),
            TEXT_FAINT,
        );
        p.rect_filled(roll.vel, 0.0, CANVAS_ALT);
        let color = theme::rgb(self.selected_track().map_or([140, 140, 140], |t| t.color));
        let s0 = roll.step(roll.vel.left()).max(0.0);
        let s1 = roll.step(roll.vel.right());
        for n in &pat.notes {
            if n.start < s0 - n.duration || n.start > s1 {
                continue;
            }
            let x = roll.x(n.start) + 1.0;
            let w = (n.duration as f32 * roll.pps - 1.5).max(2.0);
            let h = roll.vel.height() * n.velocity;
            let r = Rect::from_min_max(
                pos2(x, roll.vel.bottom() - h),
                pos2((x + w).min(roll.vel.right()), roll.vel.bottom()),
            );
            if r.right() < roll.vel.left() {
                continue;
            }
            let selected = self.sel.notes.contains(&n.id);
            p.rect_filled(
                r,
                1.0,
                if selected {
                    theme::mix(color, TEXT, 0.4)
                } else {
                    color.gamma_multiply(0.85)
                },
            );
        }
        let _ = track_id;
    }

    // ----- drum grid --------------------------------------------------------

    fn drum_toolbar(&mut self, ui: &mut Ui, track: &Track, pid: u64) {
        egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 0, bottom: 4 }).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 5.0);
                theme::color_dot(ui, CORAL, 6.0);
                ui.label(RichText::new("Step sequencer").small().color(TEXT_DIM));
                ui.label(RichText::new("Velocity").small().color(TEXT_FAINT));
                let mut v = self.editor.velocity;
                ui.spacing_mut().slider_width = 120.0;
                if ui.add(egui::Slider::new(&mut v, 0.05..=1.0).custom_formatter(|x, _| format!("{:.0}%", x * 100.0))).changed() {
                    self.editor.velocity = v;
                }
                if ui.add(egui::Button::new("Fill").small()).on_hover_text("Four-on-the-floor kick every beat").clicked() {
                    let vel = self.editor.velocity;
                    self.edit("Fill pattern", None, |p| {
                        if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                            for s in (0..pat.length).step_by(4) {
                                ops::set_drum(pat, 36, s, true, vel);
                            }
                        }
                    });
                }
                let stray = ops::pattern(track, pid).map_or(0, |p| p.notes.iter().filter(|n| !DRUMS.iter().any(|(d, _)| *d == n.pitch)).count());
                if stray > 0 {
                    ui.label(RichText::new(format!("{stray} off-kit note{}", sidebar::plural(stray))).small().color(theme::AMBER))
                        .on_hover_text("Notes on pitches outside the 8 kit rows (e.g. imported). They still play; drag a row to move them is not possible here — open the mixer or re-map via MIDI.");
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    theme::hint(ui, "Click or drag to paint steps · Shift+click cycles accents · right-click erases");
                });
            });
        });
    }

    fn drum_editor(&mut self, ui: &mut Ui, track: &Track, pid: u64) {
        self.drum_toolbar(ui, track, pid);
        let Some(pat) = ops::pattern(track, pid).cloned() else {
            return;
        };
        let area = ui.available_rect_before_wrap();
        if area.width() < 200.0 || area.height() < 80.0 {
            return;
        }
        let grid = Rect::from_min_max(
            pos2(area.left() + DRUM_LABEL_W, area.top() + RULER_H),
            area.max,
        );
        let labels = Rect::from_min_max(
            pos2(area.left(), grid.top()),
            pos2(grid.left(), grid.bottom()),
        );
        let ruler = Rect::from_min_max(
            pos2(grid.left(), area.top()),
            pos2(grid.right(), grid.top()),
        );
        let row_h = grid.height() / DRUMS.len() as f32;
        // Zoom clamps: never smaller than "whole pattern fits", never below MIN_PPS.
        let fit_pps = grid.width() / pat.length as f32;
        let pps = self
            .editor
            .drum_pps
            .clamp(fit_pps.clamp(MIN_PPS, 64.0), 64.0);
        let content_w = pat.length as f32 * pps;
        self.editor.drum_pps = pps;
        self.editor.rects = EditorRects {
            grid,
            vel: Rect::NOTHING,
            keys: labels,
            ruler,
            labels,
        };
        let g = DrumGrid {
            grid,
            labels,
            ruler,
            pps,
            scroll_x: self.editor.drum_scroll_x,
            row_h,
            len: pat.length as f64,
        };

        if ui.rect_contains_pointer(grid) {
            let (scroll, zoom, hover) =
                ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
            if zoom != 1.0 && content_w > grid.width() {
                let anchor = hover.map_or(grid.width() * 0.5, |h| (h.x - grid.left()).max(0.0));
                let step = (anchor + self.editor.drum_scroll_x) / pps;
                self.editor.drum_pps = (pps * zoom).clamp(fit_pps.min(MIN_PPS), 64.0);
                let new = self.editor.drum_pps;
                self.editor.drum_scroll_x = (step * new - anchor).max(0.0);
            }
            if scroll.x != 0.0 {
                self.editor.drum_scroll_x -= scroll.x;
                ui.input_mut(|i| i.smooth_scroll_delta.x = 0.0);
            }
        }
        self.editor.drum_scroll_x = self
            .editor
            .drum_scroll_x
            .clamp(0.0, (content_w - grid.width()).max(0.0));
        let g = DrumGrid {
            scroll_x: self.editor.drum_scroll_x,
            pps: self.editor.drum_pps,
            ..g
        };

        let resp = ui.interact(grid, ui.id().with("drum_grid"), Sense::click_and_drag());
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Drum step sequencer")
        });
        self.drum_input(ui, track, pid, &pat, &g, &resp);
        self.paint_drum_grid(ui, track, &pat, &g);
        self.paint_drum_labels(ui, track, &g);
        self.paint_drum_ruler(ui, track, &pat, &g);
    }

    fn drum_input(
        &mut self,
        ui: &mut Ui,
        track: &Track,
        pid: u64,
        pat: &Pattern,
        g: &DrumGrid,
        resp: &egui::Response,
    ) {
        let (pressed, shift, secondary_pressed, secondary_down) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.modifiers.shift,
                i.pointer.secondary_pressed(),
                i.pointer.secondary_down(),
            )
        });
        if pressed && resp.is_pointer_button_down_on() {
            self.focus = super::Focus::Editor;
            if let Some(pos) = ui.input(|i| i.pointer.press_origin()) {
                if let Some((row, step)) = g.cell(pos) {
                    let pitch = DRUMS[row].0;
                    let hit = ops::drum_hit(pat, pitch, step).is_some();
                    if shift && hit {
                        self.edit("Drum accent", None, |p| {
                            if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                                ops::cycle_drum_velocity(pat, pitch, step);
                            }
                        });
                        let v = self
                            .selected_track()
                            .and_then(|t| ops::pattern(t, pid))
                            .and_then(|p| ops::drum_hit(p, pitch, step))
                            .map(|n| n.velocity);
                        self.preview(track.id, pitch, v.unwrap_or(0.8));
                        self.editor.drum_paint = None;
                    } else {
                        let on = !hit;
                        self.editor.drum_paint = Some(on);
                        let vel = self.editor.velocity;
                        self.edit(
                            if on {
                                "Paint drum step"
                            } else {
                                "Erase drum step"
                            },
                            Some("drum-paint"),
                            |p| {
                                if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                                    ops::set_drum(pat, pitch, step, on, vel);
                                }
                            },
                        );
                        if on {
                            self.preview(track.id, pitch, vel);
                        }
                    }
                }
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let (Some(on), Some(pos)) = (self.editor.drum_paint, resp.interact_pointer_pos()) {
                if let Some((row, step)) = g.cell(pos) {
                    let pitch = DRUMS[row].0;
                    let cur = ops::drum_hit(pat, pitch, step).is_some();
                    if cur != on {
                        let vel = self.editor.velocity;
                        self.edit(
                            if on {
                                "Paint drum step"
                            } else {
                                "Erase drum step"
                            },
                            Some("drum-paint"),
                            |p| {
                                if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                                    ops::set_drum(pat, pitch, step, on, vel);
                                }
                            },
                        );
                    }
                }
            }
        }
        if secondary_pressed && resp.is_pointer_button_down_on() {
            self.editor.right_erase = true;
        }
        if !secondary_down {
            self.editor.right_erase = false;
        }
        if self.editor.right_erase {
            if let Some(pos) = resp.interact_pointer_pos() {
                if let Some((row, step)) = g.cell(pos) {
                    let pitch = DRUMS[row].0;
                    if ops::drum_hit(pat, pitch, step).is_some() {
                        self.edit("Erase drum step", Some("drum-erase"), |p| {
                            if let Some(pat) = ops::pattern_mut(p, track.id, pid) {
                                ops::set_drum(pat, pitch, step, false, 0.0);
                            }
                        });
                    }
                }
            }
        }
    }

    fn paint_drum_grid(&self, ui: &Ui, track: &Track, pat: &Pattern, g: &DrumGrid) {
        let p = ui.painter_at(g.grid);
        let color = theme::rgb(track.color);
        for row in 0..DRUMS.len() {
            let y = g.row_y(row);
            let r = Rect::from_min_size(pos2(g.grid.left(), y), vec2(g.grid.width(), g.row_h));
            p.rect_filled(r, 0.0, if row % 2 == 1 { CANVAS_ALT } else { CANVAS });
        }
        let s0 = g.step(g.grid.left()).max(0.0).floor() as i64;
        let s1 = g.step(g.grid.right()).ceil() as i64;
        for s in s0.max(0)..=(s1.min(pat.length as i64 + 16)) {
            let x = g.x(s as f64);
            if s % 16 == 0 {
                p.vline(x, g.grid.y_range(), Stroke::new(1.0_f32, GRID_BAR));
            } else if s % 4 == 0 {
                p.vline(x, g.grid.y_range(), Stroke::new(1.0_f32, GRID_BEAT));
            } else if g.pps > 18.0 {
                p.vline(x, g.grid.y_range(), Stroke::new(0.5_f32, GRID_FAINT));
            }
        }
        for s in 4..pat.length as i64 {
            if s % 8 == 4 {
                let x = g.x(s as f64);
                p.rect_filled(
                    Rect::from_min_max(
                        pos2(x, g.grid.top()),
                        pos2(
                            (x + 4.0 * g.pps)
                                .min(g.x(pat.length as f64))
                                .min(g.grid.right()),
                            g.grid.bottom(),
                        ),
                    ),
                    0.0,
                    Color32::from_white_alpha(4),
                );
            }
        }
        let end_x = g.x(pat.length as f64);
        if end_x < g.grid.right() {
            p.rect_filled(
                Rect::from_min_max(pos2(end_x, g.grid.top()), g.grid.max),
                0.0,
                Color32::from_black_alpha(80),
            );
        }
        // Notes
        for n in &pat.notes {
            let Some(row) = DRUMS.iter().position(|(d, _)| *d == n.pitch) else {
                continue;
            };
            let step = n.start.floor();
            let x = g.x(step);
            let y = g.row_y(row);
            if x + g.pps < g.grid.left() || x > g.grid.right() {
                continue;
            }
            let cell =
                Rect::from_min_size(pos2(x + 2.0, y + 3.0), vec2(g.pps - 4.0, g.row_h - 6.0));
            let selected = self.sel.notes.contains(&n.id);
            p.rect_filled(
                cell,
                4.0,
                theme::mix(BORDER_SOFT, color, 0.35 + n.velocity * 0.65),
            );
            if selected {
                p.rect_stroke(
                    cell.expand(1.0),
                    4.0,
                    Stroke::new(1.5_f32, TEXT),
                    StrokeKind::Inside,
                );
            }
            if n.velocity < 0.95 {
                let tick = Rect::from_min_max(
                    pos2(cell.left() + 3.0, cell.bottom() - 4.0),
                    pos2(
                        cell.left() + 3.0 + (cell.width() - 6.0) * n.velocity,
                        cell.bottom() - 2.0,
                    ),
                );
                p.rect_filled(tick, 1.0, Color32::from_black_alpha(110));
            }
        }
        // Playhead
        if self.transport.playing {
            if let Some(local) = self.pattern_playhead(track, pat.id) {
                let x = g.x(local);
                if x >= g.grid.left() && x <= g.grid.right() {
                    p.vline(x, g.grid.y_range(), Stroke::new(1.5_f32, LIME));
                }
            }
        }
        if pat.notes.is_empty() {
            p.text(
                g.grid.center(),
                Align2::CENTER_CENTER,
                "Empty beat — click steps to add hits",
                FontId::proportional(13.0),
                TEXT_FAINT,
            );
        }
        p.rect_stroke(
            g.grid,
            0.0,
            Stroke::new(1.0_f32, BORDER),
            StrokeKind::Inside,
        );
    }

    fn paint_drum_labels(&mut self, ui: &mut Ui, track: &Track, g: &DrumGrid) {
        let p = ui.painter_at(g.labels);
        p.rect_filled(g.labels, 0.0, PANEL);
        let mut preview = None;
        for (row, (pitch, name)) in DRUMS.iter().enumerate() {
            let y = g.row_y(row);
            let r = Rect::from_min_max(
                pos2(g.labels.left(), y),
                pos2(g.labels.right(), y + g.row_h),
            );
            let resp = ui.interact(r, ui.id().with(("drum-label", row)), Sense::click());
            p.rect_filled(
                r,
                0.0,
                if resp.hovered() {
                    theme::mix(PANEL, theme::CONTROL_HOVER, 0.5)
                } else {
                    PANEL
                },
            );
            p.text(
                pos2(r.left() + 12.0, r.center().y),
                Align2::LEFT_CENTER,
                format!("{:02}", row + 1),
                FontId::monospace(10.0),
                TEXT_FAINT,
            );
            p.text(
                pos2(r.left() + 34.0, r.center().y),
                Align2::LEFT_CENTER,
                *name,
                FontId::proportional(12.0),
                if resp.hovered() { TEXT } else { TEXT_DIM },
            );
            if resp.hovered() {
                p.text(
                    pos2(r.right() - 10.0, r.center().y),
                    Align2::RIGHT_CENTER,
                    "▶",
                    FontId::proportional(10.0),
                    PERIWINKLE,
                );
            }
            if resp.on_hover_text(format!("Preview {name}")).clicked() {
                preview = Some(*pitch);
            }
            p.hline(
                g.labels.x_range(),
                r.bottom(),
                Stroke::new(0.5_f32, BORDER_SOFT),
            );
        }
        p.vline(
            g.labels.right(),
            g.labels.y_range(),
            Stroke::new(1.0_f32, BORDER),
        );
        if let Some(pitch) = preview {
            self.preview(track.id, pitch, 0.9);
        }
    }

    fn paint_drum_ruler(&self, ui: &Ui, track: &Track, pat: &Pattern, g: &DrumGrid) {
        let p = ui.painter_at(g.ruler);
        p.rect_filled(g.ruler, 0.0, PANEL);
        for s in 0..pat.length {
            if s.is_multiple_of(4) {
                let x = g.x(s as f64);
                let label = if s.is_multiple_of(16) {
                    format!("{}", s / 16 + 1)
                } else {
                    format!("{}", s / 4 % 4 + 1)
                };
                p.text(
                    pos2(x + 3.0, g.ruler.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    FontId::proportional(10.0),
                    TEXT_DIM,
                );
            }
        }
        if self.transport.playing {
            if let Some(local) = self.pattern_playhead(track, pat.id) {
                let x = g.x(local);
                p.add(Shape::convex_polygon(
                    vec![
                        pos2(x - 5.0, g.ruler.bottom() - 8.0),
                        pos2(x + 5.0, g.ruler.bottom() - 8.0),
                        pos2(x, g.ruler.bottom() - 1.0),
                    ],
                    LIME,
                    Stroke::NONE,
                ));
            }
        }
    }
}

fn note_len_label(steps: f64) -> String {
    match steps {
        0.5 => "1/32".into(),
        1.0 => "1/16".into(),
        2.0 => "1/8".into(),
        4.0 => "1/4".into(),
        8.0 => "1/2".into(),
        16.0 => "1 bar".into(),
        other => format!("{other}"),
    }
}
