//! The app shell: menu bar, status bar and toasts, the arranger/editor split,
//! the bottom tab strip and the empty-song screen.
//!
//! The transport bar lives in `transport.rs`, dialogs in `dialogs.rs`. This
//! module owns everything that frames the screens: how tall each panel is
//! ([`Split`]), the menus (one consistent row style, see [`menu_item`]) and
//! how feedback reaches the user (status line for everything, a short-lived
//! [`Toast`] for outcomes worth noticing).

use std::time::{Duration, Instant};

use egui::containers::menu::SubMenu;
use egui::{
    pos2, vec2, Align, Color32, Context, CursorIcon, Frame, Id, KeyboardShortcut, LayerId, Layout,
    Margin, Order, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2,
    ViewportCommand, WidgetInfo, WidgetType,
};

use super::dialogs::Dialog;
use super::files::PendingAction;
use super::theme::{self, Icon, Palette, Segment, ThemeChoice};
use super::{
    arranger, editor, ops, sidebar, CadenceApp, Clipboard, StatusKind, Tab, SC_COPY, SC_CUT,
    SC_DUPLICATE, SC_EDITOR, SC_EXPORT_WAV, SC_HELP, SC_HOME, SC_KEYS, SC_LOOP, SC_MAX_ARRANGER,
    SC_MAX_EDITOR, SC_MIXER, SC_NEW, SC_OPEN, SC_PASTE, SC_PLAY, SC_PREFERENCES, SC_QUIT,
    SC_RECORD, SC_REDO, SC_SAVE, SC_SAVE_AS, SC_SELECT_ALL, SC_UNDO,
};
use crate::audio::AudioEngine;
use crate::model::Instrument;

/// Menu bar height.
pub(super) const MENU_H: f32 = 28.0;
/// Status bar height.
pub(super) const STATUS_H: f32 = 24.0;
/// Every menu is exactly this wide, so the bar never jumps between menus.
const MENU_W: f32 = 296.0;
/// Height of the draggable gap between the arranger and the editor.
const HANDLE_H: f32 = 12.0;
/// Height of the bottom tab strip card (tabs row + rule + card outline).
const TABS_H: f32 = 44.0;

// ----- panel split ------------------------------------------------------------

/// Which side of the split the user has asked to favor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PanelMode {
    /// Both panels visible: the automatic or user-dragged split.
    #[default]
    Balanced,
    /// The editor collapses to its tab strip.
    ArrangerMax,
    /// The arranger collapses to its toolbar and ruler.
    EditorMax,
}

/// Arranger/editor split. Heights are derived from the space available, so
/// the proportions survive window resizes; only a drag stores a share.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Split {
    pub(super) mode: PanelMode,
    /// The arranger's share of the area the user dragged it to. `None` keeps
    /// the automatic split.
    pub(super) user_share: Option<f32>,
}

/// The arranger never gets less than this unless the user collapses it:
/// toolbar + ruler + room for about a lane and a half.
pub(super) const MIN_ARRANGER_H: f32 = 168.0;
/// The arranger when collapsed to its toolbar and ruler.
const ARRANGER_COLLAPSED_H: f32 = 104.0;
/// The editor never gets less than this from a drag.
const MIN_EDITOR_H: f32 = 200.0;
/// Piano-roll rows the automatic split tries to keep visible, in px.
pub(super) const EDITOR_GRID_TARGET: f32 = 260.0;
/// The tab strip on top of the editor card (6 + 28 + 6).
const TAB_STRIP_H: f32 = 40.0;
/// Editor panel height around its grid with the chord strip closed: card
/// outline, tab strip and the piano roll's own chrome (pattern toolbar,
/// chord row, roll toolbar, ruler, velocity lane). A layout test checks it
/// against the real frame.
pub(super) const EDITOR_CHROME: f32 = 2.0 + TAB_STRIP_H + editor::ROLL_CHROME;
/// What the open chord strip adds to [`EDITOR_CHROME`].
pub(super) const CHORD_STRIP_EXTRA: f32 = editor::CHORD_STRIP_EXTRA;
/// The automatic split never gives the arranger more than this share.
const MAX_AUTO_SHARE: f32 = 0.55;

impl Split {
    /// Arranger height for an area `total` px tall (handle included).
    pub(super) fn arranger_height(&self, total: f32, chord_open: bool) -> f32 {
        let avail = (total - HANDLE_H).max(0.0);
        let lo = MIN_ARRANGER_H.min(avail * 0.5);
        match self.mode {
            PanelMode::ArrangerMax => (avail - TABS_H).max(lo),
            PanelMode::EditorMax => ARRANGER_COLLAPSED_H.min(avail * 0.5),
            PanelMode::Balanced => {
                let hi = (avail - MIN_EDITOR_H).max(lo);
                let want = match self.user_share {
                    Some(share) => share * avail,
                    None => Self::automatic(avail, chord_open),
                };
                want.clamp(lo, hi)
            }
        }
    }

    /// The default split: as much arranger as fits while the piano roll
    /// keeps [`EDITOR_GRID_TARGET`] px, capped at [`MAX_AUTO_SHARE`].
    fn automatic(avail: f32, chord_open: bool) -> f32 {
        let editor =
            EDITOR_CHROME + if chord_open { CHORD_STRIP_EXTRA } else { 0.0 } + EDITOR_GRID_TARGET;
        (avail - editor).clamp(MIN_ARRANGER_H, (avail * MAX_AUTO_SHARE).max(MIN_ARRANGER_H))
    }

    /// A drag put the arranger's bottom edge `arranger_h` px below the top.
    fn drag_to(&mut self, arranger_h: f32, total: f32) {
        let avail = (total - HANDLE_H).max(1.0);
        self.mode = PanelMode::Balanced;
        self.user_share = Some((arranger_h / avail).clamp(0.0, 1.0));
    }

    /// Switches to `target`, or back to balanced when it is already active.
    pub(super) fn toggle(&mut self, target: PanelMode) {
        self.mode = if self.mode == target {
            PanelMode::Balanced
        } else {
            target
        };
    }

    /// Forgets the dragged split and any maximized panel.
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
}

// ----- shell state ------------------------------------------------------------

/// View state owned by the shell (never saved).
#[derive(Debug, Default)]
pub(super) struct ShellState {
    pub(super) split: Split,
    toast: Option<Toast>,
    /// Tab shown last frame; a change restores a collapsed editor.
    last_tab: Option<Tab>,
    window_fitted: bool,
    #[cfg(test)]
    pub(super) probe: LayoutProbe,
}

/// Panel rectangles of the last frame, for headless layout tests.
#[cfg(test)]
#[derive(Debug)]
pub(super) struct LayoutProbe {
    pub(super) arranger: Rect,
    pub(super) handle: Rect,
    pub(super) editor: Rect,
    pub(super) menu: Rect,
    pub(super) status: Rect,
    pub(super) toast: Rect,
    pub(super) status_right: Rect,
    pub(super) status_fit: Option<StatusFit>,
    /// Stands in for a running engine's device name in the status bar.
    pub(super) fake_device: Option<String>,
}

#[cfg(test)]
impl Default for LayoutProbe {
    fn default() -> Self {
        Self {
            arranger: Rect::NOTHING,
            handle: Rect::NOTHING,
            editor: Rect::NOTHING,
            menu: Rect::NOTHING,
            status: Rect::NOTHING,
            toast: Rect::NOTHING,
            status_right: Rect::NOTHING,
            status_fit: None,
            fake_device: None,
        }
    }
}

impl ShellState {
    /// Queues a toast for outcomes worth noticing. Info messages stay in the
    /// status bar only; an identical toast that just appeared is not
    /// restarted.
    pub(super) fn note(&mut self, kind: StatusKind, text: &str) {
        if kind == StatusKind::Info {
            return;
        }
        if self.toast.as_ref().is_some_and(|t| {
            t.kind == kind && t.text == text && t.since.elapsed() < Duration::from_secs(1)
        }) {
            return;
        }
        self.toast = Some(Toast {
            kind,
            text: text.to_owned(),
            since: Instant::now(),
        });
    }
}

// ----- toast ---------------------------------------------------------------------

/// A short-lived notice above the status bar.
#[derive(Debug, Clone)]
pub(super) struct Toast {
    kind: StatusKind,
    text: String,
    since: Instant,
}

/// Time to fade out at the end of a toast's life.
const TOAST_FADE: Duration = Duration::from_millis(400);

impl Toast {
    /// Full-opacity time: successes pass quickly, problems linger.
    fn hold(kind: StatusKind) -> Duration {
        match kind {
            StatusKind::Info | StatusKind::Success => Duration::from_millis(3000),
            StatusKind::Warning => Duration::from_millis(4500),
            StatusKind::Error => Duration::from_millis(6000),
        }
    }

    /// Opacity `age` after the toast appeared; `None` once it is gone.
    fn opacity(kind: StatusKind, age: Duration) -> Option<f32> {
        let hold = Self::hold(kind);
        if age <= hold {
            Some(1.0)
        } else if age < hold + TOAST_FADE {
            Some(1.0 - (age - hold).as_secs_f32() / TOAST_FADE.as_secs_f32())
        } else {
            None
        }
    }
}

fn kind_color(pal: Palette, kind: StatusKind) -> Color32 {
    match kind {
        StatusKind::Info => pal.text_faint,
        StatusKind::Success => pal.accent,
        StatusKind::Warning => pal.warn,
        StatusKind::Error => pal.danger,
    }
}

// ----- menu rows ---------------------------------------------------------------

/// A painter that ignores the blanket fade of a disabled `Ui` (rows pick their
/// own legible disabled colors).
fn unfaded(ui: &Ui) -> egui::Painter {
    let mut painter = ui.painter().clone();
    if !ui.is_enabled() {
        let alpha = ui.visuals().disabled_alpha();
        if alpha > 0.0 {
            painter.set_opacity((painter.opacity() / alpha).min(1.0));
        }
    }
    painter
}

/// Left edge of a menu label: room for the check column.
const MENU_LABEL_X: f32 = 32.0;

/// One menu row: check column, label, right-aligned shortcut hint. Disabled
/// rows stay legible. Closes the menu when clicked.
fn menu_item(
    ui: &mut Ui,
    pal: Palette,
    label: &str,
    hint: &str,
    enabled: bool,
    checked: Option<bool>,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), theme::CONTROL_S), sense);
    resp.widget_info(|| {
        WidgetInfo::selected(WidgetType::Button, enabled, checked == Some(true), label)
    });
    if ui.is_rect_visible(rect) {
        paint_menu_row(ui, pal, rect, &resp, label, hint, enabled, checked, None);
    }
    if resp.clicked() {
        ui.close();
    }
    resp
}

#[allow(clippy::too_many_arguments)]
fn paint_menu_row(
    ui: &Ui,
    pal: Palette,
    rect: Rect,
    resp: &Response,
    label: &str,
    hint: &str,
    enabled: bool,
    checked: Option<bool>,
    submenu_open: Option<bool>,
) {
    let p = unfaded(ui);
    let hot = enabled && (resp.hovered() || resp.has_focus() || submenu_open == Some(true));
    if hot {
        p.rect_filled(
            rect.shrink2(vec2(4.0, 0.0)),
            theme::RADIUS_S,
            pal.control_hover,
        );
    }
    let ink = if enabled { pal.text } else { pal.text_disabled };
    if checked == Some(true) {
        theme::paint_icon(
            &p,
            Rect::from_center_size(
                pos2(rect.left() + 18.0, rect.center().y),
                Vec2::splat(theme::ICON_S),
            ),
            Icon::Check,
            if enabled { pal.accent } else { ink },
        );
    }
    let right_pad = 14.0;
    let mut label_right = rect.right() - right_pad;
    if submenu_open.is_some() {
        theme::paint_icon(
            &p,
            Rect::from_center_size(
                pos2(
                    rect.right() - right_pad - theme::ICON_S * 0.5 + 2.0,
                    rect.center().y,
                ),
                Vec2::splat(theme::ICON_S),
            ),
            Icon::ChevronRight,
            if enabled { pal.text_dim } else { ink },
        );
        label_right -= theme::ICON_S + theme::GAP_S;
    } else if !hint.is_empty() {
        let hint_ink = if enabled {
            pal.text_faint
        } else {
            pal.text_disabled
        };
        let galley = p.layout_no_wrap(hint.to_owned(), theme::body_font(12.0), hint_ink);
        let pos = pos2(
            rect.right() - right_pad - galley.size().x,
            rect.center().y - galley.size().y * 0.5,
        );
        label_right = pos.x - theme::GAP_M;
        p.galley(pos, galley, hint_ink);
    }
    let galley = theme::elide(
        ui,
        label,
        theme::body_font(theme::TEXT_BODY),
        ink,
        (label_right - rect.left() - MENU_LABEL_X).max(8.0),
    );
    p.galley(
        pos2(
            rect.left() + MENU_LABEL_X,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        ink,
    );
}

/// A row that opens a submenu on hover or click.
fn submenu(ui: &mut Ui, pal: Palette, label: &str, enabled: bool, content: impl FnOnce(&mut Ui)) {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), theme::CONTROL_S), sense);
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    let open = egui::Popup::is_id_open(ui.ctx(), SubMenu::id_from_widget_id(resp.id));
    if ui.is_rect_visible(rect) {
        paint_menu_row(ui, pal, rect, &resp, label, "", enabled, None, Some(open));
    }
    if enabled {
        SubMenu::new().show(ui, &resp, |ui| {
            menu_scope(ui);
            content(ui);
        });
    }
}

/// Fixed width, tight rows: call first in every menu.
fn menu_scope(ui: &mut Ui) {
    ui.set_width(MENU_W);
    ui.spacing_mut().item_spacing.y = 0.0;
}

fn menu_sep(ui: &mut Ui, pal: Palette) {
    ui.add_space(theme::GAP_XS);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(
        Rect::from_min_max(
            pos2(rect.left() + theme::GAP_S, rect.top()),
            pos2(rect.right() - theme::GAP_S, rect.bottom()),
        ),
        0.0,
        pal.border_soft,
    );
    ui.add_space(theme::GAP_XS);
}

/// Shortcut text in the platform's notation ("Ctrl+S" / "⌘S").
fn keys(ui: &Ui, sc: &KeyboardShortcut) -> String {
    ui.ctx().format_shortcut(sc)
}

impl CadenceApp {
    // ----- menu bar ------------------------------------------------------------

    pub(super) fn menu_bar(&mut self, ctx: &Context) {
        let pal = self.palette();
        let frame = Frame::new()
            .fill(pal.bg)
            .inner_margin(Margin::symmetric(8, 0));
        let panel = egui::TopBottomPanel::top("menu_bar")
            .exact_height(MENU_H)
            .frame(frame)
            .show_separator_line(false)
            .show(ctx, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("File", |ui| self.file_menu(ui));
                    ui.menu_button("Edit", |ui| self.edit_menu(ui));
                    ui.menu_button("Track", |ui| self.track_menu(ui));
                    ui.menu_button("Transport", |ui| self.transport_menu(ui));
                    ui.menu_button("View", |ui| self.view_menu(ui));
                    ui.menu_button("Help", |ui| self.help_menu(ui));
                });
            });
        #[cfg(test)]
        {
            self.shell.probe.menu = panel.response.rect;
        }
        #[cfg(not(test))]
        let _ = panel;
    }

    fn file_menu(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        menu_scope(ui);
        if menu_item(ui, pal, "New Song…", &keys(ui, &SC_NEW), true, None).clicked() {
            self.request(PendingAction::New);
        }
        if menu_item(ui, pal, "New from Demo", "", true, None).clicked() {
            self.request(PendingAction::Demo);
        }
        if menu_item(ui, pal, "Open…", &keys(ui, &SC_OPEN), true, None).clicked() {
            self.request(PendingAction::Open);
        }
        let recent = self.existing_recent_files();
        let mut pick = None;
        let mut clear = false;
        submenu(ui, pal, "Open Recent", !recent.is_empty(), |ui| {
            for path in &recent {
                let name = super::files::file_label(path);
                let folder = path
                    .parent()
                    .and_then(|d| d.file_name())
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let folder = if folder.chars().count() > 18 {
                    format!("{}…", folder.chars().take(17).collect::<String>())
                } else {
                    folder
                };
                if menu_item(ui, pal, &name, &folder, true, None)
                    .on_hover_text(path.display().to_string())
                    .clicked()
                {
                    pick = Some(path.clone());
                }
            }
            menu_sep(ui, pal);
            if menu_item(ui, pal, "Clear Recent", "", true, None).clicked() {
                clear = true;
            }
        });
        if let Some(path) = pick {
            self.request(PendingAction::OpenPath(path));
        }
        if clear {
            self.clear_recent_files();
        }
        menu_sep(ui, pal);
        if menu_item(ui, pal, "Save", &keys(ui, &SC_SAVE), true, None).clicked() {
            self.save();
        }
        if menu_item(ui, pal, "Save As…", &keys(ui, &SC_SAVE_AS), true, None).clicked() {
            self.save_as();
        }
        if menu_item(ui, pal, "Save Copy with Audio…", "", true, None)
            .on_hover_text(
                "Save a copy of the song plus a \"<name> Audio\" folder with every WAV it \
                 uses, so it plays on another computer",
            )
            .clicked()
        {
            self.save_copy_with_audio_dialog();
        }
        menu_sep(ui, pal);
        if menu_item(ui, pal, "Import MIDI…", "", true, None).clicked() {
            self.request(PendingAction::ImportMidi);
        }
        if menu_item(ui, pal, "Import Audio File…", "", true, None).clicked() {
            self.import_audio_clip_dialog();
        }
        if menu_item(ui, pal, "Import Sample…", "", true, None).clicked() {
            self.import_sample_dialog();
        }
        if menu_item(ui, pal, "Import Instrument Pack…", "", true, None).clicked() {
            self.import_pack_dialog();
        }
        if menu_item(ui, pal, "Clean Up Unused Audio…", "", true, None)
            .on_hover_text(
                "Delete imported WAVs that no open, autosaved or recent song uses (asks first)",
            )
            .clicked()
        {
            self.open_audio_cleanup();
        }
        menu_sep(ui, pal);
        let idle = self.export.is_none();
        let busy = "Exporting…";
        let wav = if idle { "Export WAV…" } else { busy };
        if menu_item(ui, pal, wav, &keys(ui, &SC_EXPORT_WAV), idle, None).clicked() {
            self.export_wav_dialog();
        }
        let normalize = format!(
            "Normalize WAV Export ({:.0} LUFS)",
            crate::audio::DEFAULT_TARGET_LUFS
        );
        if menu_item(ui, pal, &normalize, "", true, Some(self.export_normalize))
            .on_hover_text(
                "Bring exported WAVs to a streaming-ready loudness (ITU-R BS.1770). \
                 Peaks stay under -1 dBFS; takes a few extra render passes.",
            )
            .clicked()
        {
            self.export_normalize = !self.export_normalize;
        }
        let stems = if idle { "Export Stems…" } else { busy };
        if menu_item(ui, pal, stems, "", idle, None).clicked() {
            self.export_stems_dialog();
        }
        if menu_item(ui, pal, "Export MIDI…", "", true, None).clicked() {
            self.export_midi_dialog();
        }
        menu_sep(ui, pal);
        if menu_item(
            ui,
            pal,
            "Preferences…",
            &keys(ui, &SC_PREFERENCES),
            true,
            None,
        )
        .clicked()
        {
            self.dialog = Dialog::Preferences;
        }
        menu_sep(ui, pal);
        let quit = if cfg!(target_os = "macos") {
            "Quit Cadence"
        } else {
            "Exit"
        };
        if menu_item(ui, pal, quit, &keys(ui, &SC_QUIT), true, None).clicked() {
            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn edit_menu(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        menu_scope(ui);
        let undo = self
            .history
            .undo_label()
            .map_or("Undo".to_owned(), |l| format!("Undo {l}"));
        if menu_item(
            ui,
            pal,
            &undo,
            &keys(ui, &SC_UNDO),
            self.history.can_undo(),
            None,
        )
        .clicked()
        {
            self.undo();
        }
        let redo = self
            .history
            .redo_label()
            .map_or("Redo".to_owned(), |l| format!("Redo {l}"));
        if menu_item(
            ui,
            pal,
            &redo,
            &keys(ui, &SC_REDO),
            self.history.can_redo(),
            None,
        )
        .on_hover_text("Also Ctrl+Y")
        .clicked()
        {
            self.redo();
        }
        menu_sep(ui, pal);
        let can_copy = self.can_copy();
        if menu_item(ui, pal, "Cut", &keys(ui, &SC_CUT), can_copy, None).clicked() {
            self.cut_selection();
        }
        if menu_item(ui, pal, "Copy", &keys(ui, &SC_COPY), can_copy, None).clicked() {
            self.copy_selection();
        }
        let can_paste = !matches!(self.clipboard, Clipboard::None);
        if menu_item(ui, pal, "Paste", &keys(ui, &SC_PASTE), can_paste, None)
            .on_hover_text(if can_paste {
                "Paste at the playhead (notes paste at the pointer over the grid)"
            } else {
                "Copy notes, clips or a track first"
            })
            .clicked()
        {
            let ctx = ui.ctx().clone();
            self.paste_clipboard(&ctx);
        }
        let has_sel = !self.sel.notes.is_empty() || !self.selected_clip_ids().is_empty();
        if menu_item(
            ui,
            pal,
            "Duplicate",
            &keys(ui, &SC_DUPLICATE),
            has_sel,
            None,
        )
        .clicked()
        {
            self.duplicate_selection();
        }
        if menu_item(ui, pal, "Delete", "Del", has_sel, None).clicked() {
            self.delete_selection();
        }
        menu_sep(ui, pal);
        if menu_item(ui, pal, "Select All", &keys(ui, &SC_SELECT_ALL), true, None)
            .on_hover_text("Clips when the arranger has focus, notes when the editor has")
            .clicked()
        {
            self.select_all();
        }
        if menu_item(ui, pal, "Deselect", "Esc", true, None).clicked() {
            self.clear_selection();
        }
        menu_sep(ui, pal);
        // While a menu is open the pointer is on the menu, not the grid, so
        // this reflects the playhead/selection fallback.
        let ctx = ui.ctx().clone();
        let can_split = self.split_clip_target(&ctx).is_some();
        if menu_item(ui, pal, "Split Clip at Playhead", "B", can_split, None)
            .on_hover_text("Hover a clip, or select one the playhead crosses")
            .clicked()
        {
            self.split_clip_at_playhead(&ctx);
        }
        if menu_item(ui, pal, "Add Marker at Playhead", "", true, None).clicked() {
            self.add_marker_at_playhead();
        }
        menu_sep(ui, pal);
        let notes = self.has_note_targets();
        let target = self.editor_target();
        if menu_item(ui, pal, "Quantize Notes", "", notes, None)
            .on_hover_text("Snap note starts to the grid (selected notes, or the whole pattern)")
            .clicked()
        {
            if let Some((track, pattern)) = target {
                self.quantize_notes(track, pattern);
            }
        }
        if menu_item(ui, pal, "Humanize Notes", "", notes, None)
            .on_hover_text("Select notes or open a pattern that has notes")
            .clicked()
        {
            if let Some((track, pattern)) = target {
                self.humanize_notes(track, pattern);
            }
        }
        if menu_item(ui, pal, "Arpeggiator…", "", self.can_arpeggiate(), None)
            .on_hover_text("Select a melodic track with a pattern to arpeggiate")
            .clicked()
        {
            self.open_arpeggiator();
        }
    }

    fn track_menu(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        menu_scope(ui);
        let room = self.project.tracks.len() < ops::MAX_TRACKS;
        submenu(ui, pal, "Add Track", true, |ui| {
            for inst in Instrument::ALL {
                if menu_item(ui, pal, inst.name(), "", room, None)
                    .on_hover_text(inst.description())
                    .clicked()
                {
                    self.add_track(inst);
                }
            }
            menu_sep(ui, pal);
            if menu_item(ui, pal, "Audio Track", "", room, None)
                .on_hover_text("A lane for imported WAV clips")
                .clicked()
            {
                self.add_audio_track();
            }
        });
        menu_sep(ui, pal);
        let selected = self.sel.track;
        let index = selected.and_then(|id| ops::track_index(&self.project, id));
        let count = self.project.tracks.len();
        if menu_item(
            ui,
            pal,
            "Duplicate Track",
            "",
            selected.is_some() && room,
            None,
        )
        .clicked()
        {
            if let Some(id) = selected {
                self.duplicate_track(id);
            }
        }
        if menu_item(ui, pal, "Delete Track", "", selected.is_some(), None)
            .on_hover_text("Undo brings it back")
            .clicked()
        {
            if let Some(id) = selected {
                self.delete_track(id);
            }
        }
        menu_sep(ui, pal);
        let up = index.is_some_and(|i| i > 0);
        if menu_item(ui, pal, "Move Track Up", "", up, None).clicked() {
            if let Some(id) = selected {
                self.edit("Move track", None, |p| {
                    ops::move_track(p, id, -1);
                });
            }
        }
        let down = index.is_some_and(|i| i + 1 < count);
        if menu_item(ui, pal, "Move Track Down", "", down, None).clicked() {
            if let Some(id) = selected {
                self.edit("Move track", None, |p| {
                    ops::move_track(p, id, 1);
                });
            }
        }
        menu_sep(ui, pal);
        if menu_item(ui, pal, "Track Settings", "", selected.is_some(), None)
            .on_hover_text("Name, color, sound, volume and pan of the selected track")
            .clicked()
        {
            self.side_tab = sidebar::SideTab::Track;
        }
    }

    /// Duplicates a track after itself and selects the copy.
    fn duplicate_track(&mut self, id: u64) {
        let mut new = None;
        self.edit("Duplicate track", None, |p| {
            new = ops::duplicate_track(p, id)
        });
        match new {
            Some(n) => {
                self.select_track(n);
                self.arranger.reveal_track = Some(n);
            }
            None => self.track_limit_status(),
        }
    }

    /// Edit ▸ Add Marker at Playhead: a section marker at the playhead,
    /// snapped to the arranger grid.
    fn add_marker_at_playhead(&mut self) {
        let step =
            arranger::snap_round(self.transport.position, self.arranger.snap.steps()).max(0.0);
        let mut id = None;
        self.edit("Add marker", None, |p| id = ops::add_marker(p, step, None));
        if id.is_none() {
            self.set_status(
                StatusKind::Warning,
                format!("Songs are limited to {} section markers.", ops::MAX_MARKERS),
            );
        }
    }

    fn transport_menu(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        menu_scope(ui);
        let play = if self.transport.playing {
            "Pause"
        } else {
            "Play"
        };
        if menu_item(ui, pal, play, &keys(ui, &SC_PLAY), true, None).clicked() {
            self.toggle_play();
        }
        if menu_item(ui, pal, "Stop", "", true, None)
            .on_hover_text("Press again to return to the start")
            .clicked()
        {
            self.stop();
        }
        if menu_item(ui, pal, "Return to Start", &keys(ui, &SC_HOME), true, None).clicked() {
            self.seek(0.0);
        }
        menu_sep(ui, pal);
        let looping = self.transport.looping;
        if menu_item(ui, pal, "Loop", &keys(ui, &SC_LOOP), true, Some(looping)).clicked() {
            self.toggle_loop();
        }
        let recording = self.transport.recording;
        if menu_item(
            ui,
            pal,
            "Record",
            &keys(ui, &SC_RECORD),
            true,
            Some(recording),
        )
        .clicked()
        {
            self.toggle_record();
        }
        let metronome = self.transport.metronome;
        if menu_item(ui, pal, "Metronome", "", true, Some(metronome)).clicked() {
            self.toggle_metronome();
        }
        let count_in = self.transport.count_in;
        if menu_item(
            ui,
            pal,
            "Count-in Before Recording",
            "",
            true,
            Some(count_in),
        )
        .on_hover_text("With Record on, Play clicks one bar before the song starts")
        .clicked()
        {
            self.toggle_count_in();
        }
    }

    fn view_menu(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        menu_scope(ui);
        for (tab, label, sc) in [
            (Tab::Editor, "Pattern Editor", &SC_EDITOR),
            (Tab::Mixer, "Mixer", &SC_MIXER),
            (Tab::Keys, "Keyboard", &SC_KEYS),
        ] {
            if menu_item(ui, pal, label, &keys(ui, sc), true, Some(self.tab == tab)).clicked() {
                self.tab = tab;
            }
        }
        menu_sep(ui, pal);
        if menu_item(ui, pal, "Zoom In", "=", true, None).clicked() {
            self.zoom(1.25);
        }
        if menu_item(ui, pal, "Zoom Out", "-", true, None).clicked() {
            self.zoom(0.8);
        }
        if menu_item(ui, pal, "Fit Song in Arranger", "", true, None).clicked() {
            self.arranger.fit_request = true;
        }
        let follow = self.arranger.follow;
        if menu_item(ui, pal, "Follow Playhead", "", true, Some(follow)).clicked() {
            self.arranger.follow = !follow;
        }
        menu_sep(ui, pal);
        let mode = self.shell.split.mode;
        if menu_item(
            ui,
            pal,
            "Maximize Editor",
            &keys(ui, &SC_MAX_EDITOR),
            true,
            Some(mode == PanelMode::EditorMax),
        )
        .on_hover_text("Collapse the arranger to its toolbar and ruler")
        .clicked()
        {
            self.shell.split.toggle(PanelMode::EditorMax);
        }
        if menu_item(
            ui,
            pal,
            "Maximize Arranger",
            &keys(ui, &SC_MAX_ARRANGER),
            true,
            Some(mode == PanelMode::ArrangerMax),
        )
        .on_hover_text("Collapse the editor to its tab strip (double-click the divider)")
        .clicked()
        {
            self.shell.split.toggle(PanelMode::ArrangerMax);
        }
        if menu_item(ui, pal, "Reset Panel Sizes", "", true, None).clicked() {
            self.shell.split.reset();
        }
        menu_sep(ui, pal);
        let current = self.theme;
        submenu(ui, pal, "Theme", true, |ui| {
            for choice in ThemeChoice::ALL {
                if menu_item(ui, pal, choice.label(), "", true, Some(current == choice))
                    .on_hover_text(choice.description())
                    .clicked()
                {
                    let ctx = ui.ctx().clone();
                    self.set_theme(&ctx, choice);
                }
            }
        });
    }

    fn help_menu(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        menu_scope(ui);
        if menu_item(
            ui,
            pal,
            "Shortcuts & Tips…",
            &keys(ui, &SC_HELP),
            true,
            None,
        )
        .clicked()
        {
            self.dialog = Dialog::Help;
        }
        if menu_item(ui, pal, "Check for Updates…", "", true, None).clicked() {
            let ctx = ui.ctx().clone();
            self.check_updates_now(&ctx);
        }
        if menu_item(ui, pal, "About Cadence…", "", true, None).clicked() {
            self.dialog = Dialog::About;
        }
    }

    // ----- shortcuts for the layout -----------------------------------------------

    /// Ctrl+Shift+Up / Down: favor the editor / the arranger (again: restore).
    pub(super) fn handle_layout_shortcuts(&mut self, ctx: &Context) {
        ctx.input_mut(|i| {
            if i.consume_shortcut(&SC_MAX_EDITOR) {
                self.shell.split.toggle(PanelMode::EditorMax);
            } else if i.consume_shortcut(&SC_MAX_ARRANGER) {
                self.shell.split.toggle(PanelMode::ArrangerMax);
            }
        });
    }

    // ----- window ----------------------------------------------------------------

    /// Once the monitor size is known, shrinks a window that opened larger
    /// than the screen can show (a 1440×900 request on a 1366×768 laptop).
    pub(super) fn fit_window(&mut self, ctx: &Context) {
        if self.shell.window_fitted {
            return;
        }
        let (monitor, inner) = ctx.input(|i| (i.viewport().monitor_size, i.viewport().inner_rect));
        let (Some(monitor), Some(inner)) = (monitor, inner) else {
            return;
        };
        self.shell.window_fitted = true;
        // Room for the title bar, window borders and the taskbar.
        let limit = vec2(
            (monitor.x - 24.0).max(MIN_WINDOW.x),
            (monitor.y - 96.0).max(MIN_WINDOW.y),
        );
        let size = inner.size();
        let target = vec2(size.x.min(limit.x), size.y.min(limit.y));
        if target != size {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(target));
        }
    }

    // ----- status bar and toast ---------------------------------------------------

    pub(super) fn status_bar(&mut self, ctx: &Context) {
        let pal = self.palette();
        let frame = Frame::new()
            .fill(pal.bg)
            .inner_margin(Margin::symmetric(12, 0))
            .stroke(Stroke::new(1.0_f32, pal.border_soft));
        let panel = egui::TopBottomPanel::bottom("status_bar")
            .exact_height(STATUS_H)
            .frame(frame)
            .show(ctx, |ui| {
                let full = ui.available_rect_before_wrap();
                let budget = (full.width() - STATUS_LEFT_MIN).max(0.0);
                let right = ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(full)
                        .layout(Layout::right_to_left(Align::Center)),
                    |ui| self.status_right(ui, pal, budget),
                );
                #[cfg(test)]
                {
                    self.shell.probe.status_right = right.response.rect;
                }
                let left = Rect::from_min_max(
                    full.min,
                    pos2(
                        (right.response.rect.left() - theme::GAP_L).max(full.left()),
                        full.max.y,
                    ),
                );
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(left)
                        .layout(Layout::left_to_right(Align::Center)),
                    |ui| self.status_left(ui, pal),
                );
            });
        #[cfg(test)]
        {
            self.shell.probe.status = panel.response.rect;
        }
        #[cfg(not(test))]
        let _ = panel;
    }

    /// The last message with its severity dot.
    fn status_left(&self, ui: &mut Ui, pal: Palette) {
        ui.spacing_mut().item_spacing.x = theme::GAP_S;
        let (kind, text) = match &self.status {
            Some(s) => (s.kind, s.text.as_str()),
            None => (StatusKind::Info, "Ready · Press F1 for shortcuts"),
        };
        theme::color_dot(ui, kind_color(pal, kind), 8.0);
        let ink = match (&self.status, kind) {
            (None, _) => pal.text_faint,
            (_, StatusKind::Info) => pal.text_dim,
            _ => pal.text,
        };
        ui.add(
            egui::Label::new(RichText::new(text).size(theme::TEXT_SMALL + 1.0).color(ink))
                .truncate(),
        );
    }

    /// Song summary, audio state and export progress, right to left, fitted
    /// into `budget` px (see [`fit_status`] for what gives way first).
    fn status_right(&mut self, ui: &mut Ui, pal: Palette, budget: f32) {
        let sp = theme::GAP_S;
        ui.spacing_mut().item_spacing.x = sp;
        let font = egui::FontId::proportional(theme::TEXT_SMALL + 1.0);
        let small =
            |text: String, color: Color32| RichText::new(text).font(font.clone()).color(color);
        let text_w = |ui: &Ui, text: &str| {
            ui.painter()
                .layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER)
                .size()
                .x
        };
        let dot = 8.0;
        let pad = ui.spacing().button_padding.x;

        // What each item needs, measured before anything is drawn.
        let audio = self.status_audio();
        let (audio_w, device_w) = match &audio {
            StatusAudio::Ready { text, device, .. } => (
                dot + 2.0 * sp + 2.0 * pad + text_w(ui, text),
                if device.is_empty() {
                    0.0
                } else {
                    text_w(ui, &format!(" · {device}"))
                },
            ),
            StatusAudio::Off { .. } => (
                theme::text_button_width(ui, "Retry audio", theme::CONTROL_S)
                    + text_w(ui, "No audio output")
                    + dot
                    + 3.0 * sp,
                0.0,
            ),
        };
        let autosave_w = if self.autosave_failures > 0 {
            theme::GAP_M + text_w(ui, "Autosave paused") + dot + 2.0 * sp
        } else {
            0.0
        };
        let t = self.project.tracks.len();
        let secs = ops::steps_to_seconds(self.project.total_steps(), self.project.bpm);
        let summary = format!(
            "{t} track{} · {} bars · {}",
            if t == 1 { "" } else { "s" },
            self.project.bars,
            ops::format_time(secs)
        );
        let export = self.export.as_ref().map(|job| {
            let p = job.progress();
            (format!("{} {:.0}%", job.verb(), p * 100.0), p)
        });
        let widths = StatusWidths {
            fixed: audio_w
                + autosave_w
                + export.as_ref().map_or(0.0, |(label, _)| {
                    theme::GAP_M + text_w(ui, label) + 2.0 * sp
                }),
            device: device_w,
            update: self.update_status_width(ui, false),
            update_short: self.update_status_width(ui, true),
            summary: theme::GAP_M + text_w(ui, &summary) + sp,
            export: export.is_some(),
        };
        let fit = fit_status(budget, &widths);
        #[cfg(test)]
        {
            self.shell.probe.status_fit = Some(fit);
        }

        match audio {
            StatusAudio::Ready {
                color,
                text,
                tip,
                device,
            } => {
                let full = if device.is_empty() {
                    text.clone()
                } else {
                    format!("{text} · {device}")
                };
                let room = text_w(ui, &text) + fit.device;
                let label = theme::elide(ui, &full, font.clone(), Color32::PLACEHOLDER, room);
                let tip = if !device.is_empty() && (label.elided || fit.device <= 0.0) {
                    format!("Output: {device}\n{tip}")
                } else {
                    tip
                };
                // The audio state label doubles as the output picker —
                // the same status footprint, one click to switch device.
                ui.menu_button(egui::WidgetText::from(label), |ui| {
                    ui.set_min_width(200.0);
                    ui.set_max_width(DEVICE_MENU_W);
                    ui.label(RichText::new("Audio output").small().color(pal.text_faint));
                    if ui
                        .selectable_label(self.audio_device_pref.is_none(), "System default")
                        .clicked()
                    {
                        self.set_audio_device(None);
                        ui.close();
                    }
                    for name in AudioEngine::output_devices() {
                        let picked = self.audio_device_pref.as_deref() == Some(name.as_str());
                        let mut resp =
                            ui.add(egui::Button::selectable(picked, name.as_str()).truncate());
                        if text_w(ui, &name) > DEVICE_MENU_W - 2.0 * pad {
                            resp = resp.on_hover_text(name.as_str());
                        }
                        if resp.clicked() {
                            self.set_audio_device(Some(name));
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text(tip);
                theme::color_dot(ui, color, dot);
            }
            StatusAudio::Off { tip } => {
                if theme::text_button(ui, pal, "Retry audio", theme::CONTROL_S, &tip).clicked() {
                    self.retry_audio(true);
                }
                ui.label(small("No audio output".to_owned(), pal.text_dim))
                    .on_hover_text("Editing, saving and WAV export still work");
                theme::color_dot(ui, pal.danger, dot);
            }
        }
        if self.autosave_failures > 0 {
            ui.add_space(theme::GAP_M);
            let tip = format!(
                "Crash-recovery autosave could not be written; Cadence retries automatically. Save the song to keep your work safe meanwhile.\n\n{}",
                self.autosave_error.as_deref().unwrap_or_default()
            );
            ui.label(small("Autosave paused".to_owned(), pal.text))
                .on_hover_text(tip);
            theme::color_dot(ui, pal.warn, dot);
        }
        self.update_status_item(ui, pal, fit.short_update);
        if fit.summary {
            ui.add_space(theme::GAP_M);
            ui.label(small(summary, pal.text_faint));
        }
        if let Some((label, p)) = export {
            ui.add_space(theme::GAP_M);
            ui.label(small(label, pal.accent));
            ui.add(
                egui::ProgressBar::new(p)
                    .desired_width(fit.export_bar)
                    .desired_height(8.0)
                    .fill(pal.accent),
            );
        }
    }

    /// The audio item of the status bar: the engine's state and device.
    fn status_audio(&self) -> StatusAudio {
        let device = match &self.engine {
            Some(engine) => Some(engine.device_name()),
            #[cfg(test)]
            None => self.shell.probe.fake_device.clone(),
            #[cfg(not(test))]
            None => None,
        };
        let Some(device) = device else {
            return StatusAudio::Off {
                tip: self.audio_error.clone().unwrap_or_default(),
            };
        };
        let pal = self.palette();
        let (color, text, tip) = match &self.transport.engine_error {
            Some(e) => (pal.danger, "Audio error", e.clone()),
            None => (
                pal.accent,
                "Audio ready",
                format!("{} voice(s) sounding", self.transport.voices),
            ),
        };
        StatusAudio::Ready {
            color,
            text: text.to_owned(),
            tip,
            device,
        }
    }

    /// Draws the toast for Success/Warning/Error messages above the status
    /// bar, bottom-centered. It floats, so nothing shifts when it appears.
    pub(super) fn toast_ui(&mut self, ctx: &Context) {
        let pal = self.palette();
        let Some(toast) = &mut self.shell.toast else {
            return;
        };
        let age = toast.since.elapsed();
        let Some(opacity) = Toast::opacity(toast.kind, age) else {
            self.shell.toast = None;
            return;
        };
        let (kind, text) = (toast.kind, toast.text.clone());
        let color = kind_color(pal, kind);
        let icon = if kind == StatusKind::Success {
            Icon::Check
        } else {
            Icon::Warning
        };
        // Painted on its own layer rather than shown in an `Area`: it floats
        // over the work surface and must never take the pointer. A new Area
        // is hit-tested as interactable, at its full sizing-pass rect, on the
        // frame it appears — that swallowed clicks and hover over half the
        // editor for one frame whenever a toast popped up.
        let mut painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("cadence-toast")));
        painter.set_opacity(opacity);
        let screen = ctx.content_rect();
        let pad = vec2(14.0, 10.0);
        let max_text =
            (screen.width() - 2.0 * pad.x - theme::ICON_L - theme::GAP_S - 48.0).clamp(80.0, 520.0);
        let galley = painter.layout(text, theme::body_font(theme::TEXT_BODY), pal.text, max_text);
        let size = vec2(
            pad.x * 2.0 + theme::ICON_L + theme::GAP_S + galley.size().x,
            pad.y * 2.0 + galley.size().y.max(theme::ICON_L),
        );
        let bottom = screen.bottom() - STATUS_H - theme::GAP_L;
        let rect = Rect::from_min_size(
            pos2(
                (screen.center().x - size.x * 0.5).round(),
                (bottom - size.y).round(),
            ),
            size,
        );
        painter.add(
            ctx.style()
                .visuals
                .popup_shadow
                .as_shape(rect, theme::RADIUS_L),
        );
        painter.rect(
            rect,
            theme::RADIUS_L,
            pal.raised,
            Stroke::new(1.0_f32, theme::mix(pal.border, color, 0.6)),
            StrokeKind::Inside,
        );
        let inner = rect.shrink2(pad);
        theme::paint_icon(
            &painter,
            Rect::from_min_size(
                pos2(inner.left(), inner.center().y - theme::ICON_L * 0.5),
                Vec2::splat(theme::ICON_L),
            ),
            icon,
            color,
        );
        painter.galley(
            pos2(
                inner.left() + theme::ICON_L + theme::GAP_S,
                inner.center().y - galley.size().y * 0.5,
            ),
            galley,
            pal.text,
        );
        #[cfg(test)]
        {
            self.shell.probe.toast = rect;
        }
        let hold = Toast::hold(kind);
        ctx.request_repaint_after(if age < hold {
            hold - age
        } else {
            Duration::from_millis(33)
        });
    }

    // ----- central area -----------------------------------------------------------

    pub(super) fn central(&mut self, ctx: &Context) {
        let pal = self.palette();
        let frame = Frame::new().fill(pal.bg).inner_margin(Margin {
            left: 8,
            right: 8,
            top: 6,
            bottom: 6,
        });
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            if self.project.tracks.is_empty() {
                self.empty_state(ui);
                return;
            }
            // Any way of switching tabs (shortcut, context menu) reveals a
            // collapsed editor.
            if self.shell.last_tab != Some(self.tab) {
                if self.shell.last_tab.is_some() && self.shell.split.mode == PanelMode::ArrangerMax
                {
                    self.shell.split.mode = PanelMode::Balanced;
                }
                self.shell.last_tab = Some(self.tab);
            }
            let area = ui.available_rect_before_wrap();
            let total = area.height();
            let chord_open = self.editor.chord_open;
            let mut arranger_h = self.shell.split.arranger_height(total, chord_open);
            let handle_at = |h: f32| {
                Rect::from_min_size(
                    pos2(area.left(), area.top() + h),
                    vec2(area.width(), HANDLE_H),
                )
            };
            let handle_id = ui.id().with("cadence-split-handle");
            let resp = ui.interact(handle_at(arranger_h), handle_id, Sense::click_and_drag());
            resp.widget_info(|| {
                WidgetInfo::labeled(WidgetType::Other, true, "Resize arranger and editor")
            });
            if resp.dragged() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let wanted = pos.y - area.top() - HANDLE_H * 0.5;
                    self.shell.split.drag_to(wanted, total);
                    arranger_h = self.shell.split.arranger_height(total, chord_open);
                }
            }
            if resp.double_clicked() {
                self.shell.split.toggle(PanelMode::ArrangerMax);
                arranger_h = self.shell.split.arranger_height(total, chord_open);
            }
            let hot = resp.hovered() || resp.dragged();
            if hot {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
            }
            let resp = resp.on_hover_text(
                "Drag to resize · double-click to maximize the arranger, again to restore",
            );
            let handle = handle_at(arranger_h);
            let arranger = Rect::from_min_size(area.min, vec2(area.width(), arranger_h));
            let editor = Rect::from_min_max(handle.left_bottom(), area.max);
            #[cfg(test)]
            {
                self.shell.probe.arranger = arranger;
                self.shell.probe.handle = handle;
                self.shell.probe.editor = editor;
            }

            ui.scope_builder(UiBuilder::new().max_rect(arranger), |ui| {
                ui.set_clip_rect(arranger.intersect(ui.clip_rect()));
                theme::card(pal)
                    .inner_margin(Margin::same(0))
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        self.arranger_ui(ui);
                    });
            });
            paint_split_handle(ui, pal, handle, hot, resp.dragged());
            ui.scope_builder(UiBuilder::new().max_rect(editor), |ui| {
                ui.set_clip_rect(editor.intersect(ui.clip_rect()));
                theme::card(pal)
                    .inner_margin(Margin::same(0))
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        // The pattern editor's toolbars start right under the
                        // tab strip; the other tabs keep the usual gap.
                        let gap = ui.spacing().item_spacing.y;
                        ui.spacing_mut().item_spacing.y = 0.0;
                        self.bottom_tabs(ui);
                        if ui.available_height() < 48.0 {
                            return;
                        }
                        if self.tab != Tab::Editor {
                            ui.add_space(gap);
                            ui.spacing_mut().item_spacing.y = gap;
                        }
                        match self.tab {
                            Tab::Editor => self.editor_ui(ui),
                            Tab::Mixer => self.mixer_ui(ui),
                            Tab::Keys => self.keys_ui(ui),
                        }
                    });
            });
        });
    }

    /// The tab strip on top of the editor card: Pattern Editor | Mixer |
    /// Keyboard, and the buttons that favor one side of the split.
    fn bottom_tabs(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        let ctx = ui.ctx().clone();
        let tips = [
            theme::shortcut_tip(&ctx, "Pattern Editor", &SC_EDITOR),
            theme::shortcut_tip(&ctx, "Mixer", &SC_MIXER),
            theme::shortcut_tip(&ctx, "Keyboard", &SC_KEYS),
        ];
        let max_editor_tip = theme::shortcut_tip(&ctx, "Maximize editor", &SC_MAX_EDITOR);
        let max_arranger_tip = theme::shortcut_tip(&ctx, "Maximize arranger", &SC_MAX_ARRANGER);
        let mode = self.shell.split.mode;
        let mut tab = self.tab;
        let mut toggle = None;
        let mut tabs_clicked = false;
        Frame::new()
            .inner_margin(Margin {
                left: 8,
                right: 8,
                top: 6,
                bottom: 6,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::GAP_XS;
                    let resp = theme::segmented(
                        ui,
                        pal,
                        &mut tab,
                        &[
                            Segment::new(Tab::Editor, "Pattern Editor")
                                .icon(Icon::Note)
                                .tip(&tips[0]),
                            Segment::new(Tab::Mixer, "Mixer")
                                .icon(Icon::Mixer)
                                .tip(&tips[1]),
                            Segment::new(Tab::Keys, "Keyboard")
                                .icon(Icon::Keyboard)
                                .tip(&tips[2]),
                        ],
                        theme::CONTROL_M,
                        None,
                    );
                    tabs_clicked = resp.clicked();
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::icon_button(
                            ui,
                            pal,
                            Icon::ChevronDown,
                            theme::CONTROL_S,
                            mode == PanelMode::ArrangerMax,
                            pal.accent,
                            &max_arranger_tip,
                        )
                        .clicked()
                        {
                            toggle = Some(PanelMode::ArrangerMax);
                        }
                        if theme::icon_button(
                            ui,
                            pal,
                            Icon::ChevronUp,
                            theme::CONTROL_S,
                            mode == PanelMode::EditorMax,
                            pal.accent,
                            &max_editor_tip,
                        )
                        .clicked()
                        {
                            toggle = Some(PanelMode::EditorMax);
                        }
                    });
                });
            });
        let r = ui.cursor();
        ui.painter()
            .hline(r.x_range(), r.top(), Stroke::new(1.0_f32, pal.border_soft));
        if let Some(target) = toggle {
            self.shell.split.toggle(target);
        } else if tabs_clicked && mode == PanelMode::ArrangerMax {
            self.shell.split.mode = PanelMode::Balanced;
        }
        self.tab = tab;
    }

    // ----- empty song -------------------------------------------------------------

    pub(super) fn empty_state(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        let card = theme::card(pal).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.22).max(16.0));
                theme::empty_state(
                    ui,
                    pal,
                    Icon::Note,
                    "Start with an instrument",
                    "Each track gets its own patterns, clips and mixer channel — up to 32 tracks. \
                     You can also drag a sound from the library.",
                    None,
                );
                ui.add_space(theme::GAP_L);
                let (w, h, gap) = (128.0, theme::CONTROL_L, theme::GAP_S);
                let columns = (((ui.available_width() + gap) / (w + gap)).floor() as usize).max(1);
                for row in Instrument::ALL.chunks(columns) {
                    let used = row.len() as f32 * w + (row.len() as f32 - 1.0) * gap;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = vec2(gap, gap);
                        ui.add_space(((ui.available_width() - used) * 0.5).max(0.0));
                        for &inst in row {
                            let color = theme::rgb(sidebar::instrument_color(inst));
                            let label = format!("+  {}", inst.name());
                            let size = vec2(w, h);
                            let tip = inst.description();
                            if theme::solid_button(ui, pal, &label, color, size, tip).clicked() {
                                self.add_track(inst);
                            }
                        }
                    });
                }
            });
        });
        // A dragged library sound can start the song too: the arranger isn't
        // shown while there are no tracks, so the empty canvas takes the
        // drop and adds the first track.
        if card
            .response
            .dnd_hover_payload::<sidebar::QuickSound>()
            .is_some()
        {
            ui.painter().rect_stroke(
                card.response.rect.shrink(1.0),
                theme::RADIUS_L,
                Stroke::new(1.5_f32, pal.accent),
                StrokeKind::Inside,
            );
            ui.ctx().set_cursor_icon(CursorIcon::Copy);
        }
        if let Some(sound) = card.response.dnd_release_payload::<sidebar::QuickSound>() {
            self.add_sound_track_at(&sound, 0);
        }
    }
}

/// Room the status message keeps before the right side gives way.
const STATUS_LEFT_MIN: f32 = 220.0;
/// The device name never takes more than this in the status bar…
const STATUS_DEVICE_MAX: f32 = 220.0;
/// …and is hidden rather than cut below this.
const STATUS_DEVICE_MIN: f32 = 64.0;
const STATUS_EXPORT_BAR: f32 = 120.0;
const STATUS_EXPORT_BAR_MIN: f32 = 48.0;
/// Widest entry in the audio-output menu; longer names are cut.
const DEVICE_MENU_W: f32 = 320.0;

/// The audio item of the status bar.
enum StatusAudio {
    /// An engine is running on `device`.
    Ready {
        color: Color32,
        text: String,
        tip: String,
        device: String,
    },
    /// No audio output; `tip` says why.
    Off { tip: String },
}

/// Widths the right side of the status bar wants, gaps included.
#[derive(Clone, Copy, Debug, Default)]
struct StatusWidths {
    /// Items that always show: audio state, autosave warning, export label.
    fixed: f32,
    /// " · device name" after the audio state (0: none).
    device: f32,
    /// Update pill with its full / compact label (0: none).
    update: f32,
    update_short: f32,
    /// Song summary ("4 tracks · 16 bars · 0:32").
    summary: f32,
    /// An export is running (its progress bar).
    export: bool,
}

/// What the right side of the status bar shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct StatusFit {
    /// Room for the device name (0: hidden).
    pub(super) device: f32,
    pub(super) summary: bool,
    pub(super) short_update: bool,
    pub(super) export_bar: f32,
}

/// Fits the status items into `budget` px. When space runs out they give way
/// in a fixed order: the device name is capped, then the song summary hides,
/// the export bar shrinks, the update pill takes its compact label, and the
/// device name is cut further and finally hidden. Audio state, autosave
/// warning and export progress always stay.
fn fit_status(budget: f32, w: &StatusWidths) -> StatusFit {
    let mut fit = StatusFit {
        device: w.device.min(STATUS_DEVICE_MAX),
        summary: true,
        short_update: false,
        export_bar: if w.export { STATUS_EXPORT_BAR } else { 0.0 },
    };
    let total = |f: &StatusFit| {
        w.fixed
            + f.device
            + if f.short_update {
                w.update_short
            } else {
                w.update
            }
            + if f.summary { w.summary } else { 0.0 }
            + f.export_bar
    };
    if total(&fit) <= budget {
        return fit;
    }
    fit.summary = false;
    if total(&fit) <= budget {
        return fit;
    }
    if w.export {
        fit.export_bar = STATUS_EXPORT_BAR_MIN;
        if total(&fit) <= budget {
            return fit;
        }
    }
    fit.short_update = true;
    let over = total(&fit) - budget;
    if over > 0.0 {
        let cut = fit.device - over;
        fit.device = if cut >= STATUS_DEVICE_MIN { cut } else { 0.0 };
    }
    fit
}

/// Smallest window the layout is designed for (see `main.rs`).
pub(super) const MIN_WINDOW: Vec2 = vec2(1100.0, 760.0);

/// The divider between arranger and editor: a grip pill that lights up when
/// the pointer is near, with a faint rule across the gap while it is.
fn paint_split_handle(ui: &Ui, pal: Palette, rect: Rect, hot: bool, dragging: bool) {
    let painter = ui.painter();
    let center = rect.center();
    if hot {
        painter.rect_filled(
            Rect::from_center_size(center, vec2(rect.width(), 2.0)),
            1.0,
            theme::mix(pal.bg, pal.accent, if dragging { 0.7 } else { 0.4 }),
        );
    }
    let pill = Rect::from_center_size(center, vec2(if hot { 48.0 } else { 36.0 }, 4.0));
    painter.rect_filled(pill, 2.0, if hot { pal.accent } else { pal.control_border });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_stays_then_fades_then_goes() {
        let kind = StatusKind::Success;
        assert_eq!(Toast::opacity(kind, Duration::ZERO), Some(1.0));
        assert_eq!(Toast::opacity(kind, Duration::from_millis(2900)), Some(1.0));
        let mid = Toast::opacity(kind, Duration::from_millis(3200)).unwrap();
        assert!((0.0..1.0).contains(&mid), "fading: {mid}");
        assert_eq!(Toast::opacity(kind, Duration::from_millis(3400)), None);
        assert!(Toast::hold(StatusKind::Error) > Toast::hold(StatusKind::Success));
    }

    #[test]
    fn info_messages_never_become_toasts() {
        let mut shell = ShellState::default();
        shell.note(StatusKind::Info, "Copied 3 notes.");
        assert!(shell.toast.is_none());
        shell.note(StatusKind::Success, "Saved.");
        assert_eq!(
            shell.toast.as_ref().map(|t| t.text.as_str()),
            Some("Saved.")
        );
        // An identical message that just landed does not restart the clock.
        let first = shell.toast.as_ref().unwrap().since;
        shell.note(StatusKind::Success, "Saved.");
        assert_eq!(shell.toast.as_ref().unwrap().since, first);
        shell.note(StatusKind::Error, "Could not save.");
        assert_eq!(
            shell.toast.as_ref().map(|t| t.kind),
            Some(StatusKind::Error)
        );
    }

    #[test]
    fn split_heights_respect_every_bound() {
        for total in [300.0_f32, 400.0, 620.0, 740.0, 1000.0, 2000.0] {
            for chord in [false, true] {
                for mode in [
                    PanelMode::Balanced,
                    PanelMode::ArrangerMax,
                    PanelMode::EditorMax,
                ] {
                    for share in [None, Some(0.0), Some(0.3), Some(1.0)] {
                        let split = Split {
                            mode,
                            user_share: share,
                        };
                        let h = split.arranger_height(total, chord);
                        assert!(h.is_finite() && h >= 0.0, "{total} {mode:?}: {h}");
                        assert!(
                            h + HANDLE_H <= total + 0.01,
                            "arranger overflows: {total} {mode:?} {share:?} -> {h}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn automatic_split_favors_the_piano_roll_but_keeps_lanes() {
        let split = Split::default();
        // 1360×860 window: 860 - menu - transport - status - margins.
        let total = 740.0;
        let closed = split.arranger_height(total, false);
        let open = split.arranger_height(total, true);
        assert!(open < closed, "an open chord strip gives the editor more");
        assert!(open >= MIN_ARRANGER_H);
        assert!(
            closed <= total * MAX_AUTO_SHARE + 0.01,
            "never more than {MAX_AUTO_SHARE} of the area"
        );
        // Plenty of room: the arranger takes at most 55%.
        let tall = split.arranger_height(1500.0, false);
        assert!(tall <= 1500.0 * MAX_AUTO_SHARE + 0.01);
    }

    #[test]
    fn dragging_stores_a_share_that_survives_resizes() {
        let mut split = Split::default();
        split.drag_to(300.0, 740.0);
        assert_eq!(split.mode, PanelMode::Balanced);
        let a = split.arranger_height(740.0, false);
        let b = split.arranger_height(1000.0, false);
        assert!((a - 300.0).abs() < 1.0);
        assert!(b > a, "bigger window, bigger arranger: the share is kept");
        let share_a = a / (740.0 - HANDLE_H);
        let share_b = b / (1000.0 - HANDLE_H);
        assert!((share_a - share_b).abs() < 0.01);
    }

    #[test]
    fn maximize_toggles_and_reset_forgets_the_drag() {
        let mut split = Split::default();
        split.drag_to(250.0, 740.0);
        split.toggle(PanelMode::ArrangerMax);
        assert_eq!(split.mode, PanelMode::ArrangerMax);
        split.toggle(PanelMode::ArrangerMax);
        assert_eq!(split.mode, PanelMode::Balanced);
        assert!(
            split.user_share.is_some(),
            "restore keeps the dragged split"
        );
        split.toggle(PanelMode::EditorMax);
        split.toggle(PanelMode::ArrangerMax);
        assert_eq!(
            split.mode,
            PanelMode::ArrangerMax,
            "switching sides directly"
        );
        split.reset();
        assert_eq!(split.mode, PanelMode::Balanced);
        assert!(split.user_share.is_none());
    }

    // ----- layout tests through the real frame ------------------------------------

    use crate::model::Project;
    use egui::{Pos2, RawInput, Vec2};

    const CMD_SHIFT: egui::Modifiers = egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT);

    /// A bare context plus an app, driven one full frame at a time.
    struct Harness {
        ctx: Context,
        app: CadenceApp,
        size: Vec2,
        time: f64,
    }

    impl Harness {
        fn new(size: Vec2) -> Self {
            let mut h = Self {
                ctx: Context::default(),
                app: CadenceApp::headless(Project::demo()),
                size,
                time: 0.0,
            };
            h.frames(4);
            h
        }

        fn input(&self, events: Vec<egui::Event>, modifiers: egui::Modifiers) -> RawInput {
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                focused: true,
                time: Some(self.time),
                modifiers,
                events,
                ..Default::default()
            }
        }

        fn frame_with(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
            let input = self.input(events, modifiers);
            let _ = self.ctx.run(input, |ctx| self.app.frame(ctx));
            self.time += 1.0 / 60.0;
        }

        fn frames(&mut self, n: usize) {
            for _ in 0..n {
                self.frame_with(Vec::new(), egui::Modifiers::NONE);
            }
        }

        fn resize(&mut self, size: Vec2) {
            self.size = size;
            self.frames(4);
        }

        fn grid_height(&self) -> f32 {
            self.app.editor_test_rects().0.height()
        }

        fn pointer(&mut self, pos: Pos2, pressed: Option<bool>) {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if let Some(pressed) = pressed {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            self.frame_with(events, egui::Modifiers::NONE);
        }

        fn click(&mut self, pos: Pos2) {
            self.pointer(pos, Some(true));
            self.pointer(pos, Some(false));
        }

        fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
            let event = egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers,
            };
            self.frame_with(vec![event], modifiers);
            self.frames(2);
        }

        fn select(&mut self, track: usize, chord_open: bool) {
            let id = self.app.project.tracks[track].id;
            self.app.select_track(id);
            self.app.editor.chord_open = chord_open;
            self.frames(4);
        }
    }

    #[test]
    fn transport_menu_toggles_the_count_in() {
        let mut app = CadenceApp::headless(crate::model::Project::demo());
        let ctx = Context::default();
        let label = "Count-in Before Recording";
        let run = |app: &mut CadenceApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 600.0))),
                focused: true,
                events,
                ..Default::default()
            };
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_width(260.0);
                    app.transport_menu(ui);
                });
            })
        };
        run(&mut app, Vec::new());
        let output = run(&mut app, Vec::new());
        let row = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(t) if t.galley.text() == label => Some(t.visual_bounding_rect()),
                _ => None,
            })
            .expect("the Transport menu lists the count-in");
        let before = app.transport.count_in;
        for expected in [!before, before] {
            let at = row.center();
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            run(&mut app, vec![egui::Event::PointerMoved(at), button(true)]);
            run(&mut app, vec![button(false)]);
            assert_eq!(app.transport.count_in, expected);
        }
    }

    #[test]
    fn status_items_give_way_in_order() {
        let w = StatusWidths {
            fixed: 120.0,
            device: 400.0,
            update: 160.0,
            update_short: 90.0,
            summary: 150.0,
            export: true,
        };
        let roomy = fit_status(2000.0, &w);
        assert_eq!(roomy.device, STATUS_DEVICE_MAX, "the device name is capped");
        assert!(roomy.summary && !roomy.short_update);
        assert_eq!(roomy.export_bar, STATUS_EXPORT_BAR);
        // fixed 120 + device 220 + update 160 + export 120 = 620 without the summary.
        let fit = fit_status(620.0, &w);
        assert!(!fit.summary && !fit.short_update);
        assert_eq!(fit.export_bar, STATUS_EXPORT_BAR);
        let fit = fit_status(560.0, &w);
        assert_eq!(fit.export_bar, STATUS_EXPORT_BAR_MIN);
        assert!(!fit.short_update);
        let fit = fit_status(480.0, &w);
        assert!(fit.short_update);
        assert_eq!(fit.device, STATUS_DEVICE_MAX);
        let fit = fit_status(400.0, &w);
        assert!(fit.device < STATUS_DEVICE_MAX && fit.device >= STATUS_DEVICE_MIN);
        let fit = fit_status(250.0, &w);
        assert_eq!(fit.device, 0.0, "hidden rather than cut to nothing");
        assert_eq!(fit.export_bar, STATUS_EXPORT_BAR_MIN, "export stays");
    }

    /// At the 1100 px minimum a very long device name, the update pill and
    /// the autosave warning still leave the status message its room; in
    /// narrower windows the lower-priority items give way instead of
    /// overflowing.
    #[test]
    fn status_bar_fits_a_long_device_name_at_minimum_width() {
        let long = "Extremely Long Professional USB Audio Interface With Many Outputs \
                    (Channels 1-2) — Built-in Speakers Fallback";
        let mut shrunk = Vec::new();
        for width in [MIN_WINDOW.x, 900.0, 700.0] {
            let mut ui = Harness::new(vec2(width, MIN_WINDOW.y));
            ui.app.shell.probe.fake_device = Some(long.to_owned());
            ui.app.update.phase = super::super::updates::Phase::Ready(crate::update::Release {
                version: "9.9.10".into(),
                ..Default::default()
            });
            ui.app.update.install_on_quit = Some(false);
            ui.app.autosave_failures = 1;
            ui.frames(3);
            let p = &ui.app.shell.probe;
            let fit = p.status_fit.expect("status bar drawn");
            assert!(fit.device <= STATUS_DEVICE_MAX, "{width}: {fit:?}");
            assert!(
                p.status.contains_rect(p.status_right),
                "{width}: right side overflows: {:?} in {:?}",
                p.status_right,
                p.status
            );
            assert!(
                p.status_right.left() >= p.status.left() + STATUS_LEFT_MIN,
                "{width}: status message squeezed: {:?} in {:?} ({fit:?})",
                p.status_right,
                p.status
            );
            if width == MIN_WINDOW.x {
                assert!(fit.device > 0.0, "the device still shows at 1100 px");
            }
            shrunk.push(fit);
        }
        assert!(!shrunk[2].summary && shrunk[2].short_update, "{shrunk:?}");
        // Without the long name everything shows in full.
        let mut ui = Harness::new(MIN_WINDOW);
        ui.app.shell.probe.fake_device = Some("Speakers".into());
        ui.frames(2);
        let fit = ui.app.shell.probe.status_fit.unwrap();
        assert!(fit.summary && !fit.short_update, "{fit:?}");
    }

    /// The piano roll (melodic track) keeps room to work in at the sizes
    /// the app is designed for. At 1360×860 — the default 1376×899 outer
    /// window — it gets ≥ 260 px with the chord strip closed or open; at
    /// the 1100×760 minimum the arranger keeps its floor, so the roll gets
    /// what is left (and the drum grid, which has less chrome, stays big).
    #[test]
    fn editor_grid_keeps_room_at_common_sizes() {
        for chord in [false, true] {
            for (w, h) in [(1360.0, 860.0), (1440.0, 900.0), (1920.0, 1080.0)] {
                let mut ui = Harness::new(vec2(w, h));
                ui.select(2, chord);
                assert!(
                    ui.grid_height() >= EDITOR_GRID_TARGET - 0.5,
                    "{w}x{h} chord={chord}: piano roll grid is {:.0} px",
                    ui.grid_height()
                );
                // Every chord palette fits one row of chips there.
                for tab in editor::ChordStripTab::ALL {
                    ui.app.editor.chord_tab = tab;
                    ui.frames(2);
                    assert!(
                        ui.grid_height() >= EDITOR_GRID_TARGET - 0.5,
                        "{w}x{h} chord={chord} {tab:?}: grid is {:.0} px",
                        ui.grid_height()
                    );
                }
                ui.app.editor.chord_tab = editor::ChordStripTab::Diatonic;
            }
            let mut ui = Harness::new(vec2(1100.0, 760.0));
            ui.select(2, chord);
            assert!(
                ui.grid_height() >= 130.0,
                "1100x760 chord={chord}: piano roll grid is {:.0} px",
                ui.grid_height()
            );
            ui.select(0, chord);
            assert!(
                ui.grid_height() >= 220.0,
                "1100x760 chord={chord}: drum grid is {:.0} px",
                ui.grid_height()
            );
        }
    }

    /// The split's model of the editor chrome matches the real frame, so
    /// the default split gives the piano roll exactly what it promises.
    #[test]
    fn editor_chrome_constants_match_the_frame() {
        for (w, h) in [(1100.0, 760.0), (1360.0, 860.0), (1920.0, 1080.0)] {
            for chord in [false, true] {
                let mut ui = Harness::new(vec2(w, h));
                ui.select(2, chord);
                let chrome = ui.app.shell.probe.editor.height() - ui.grid_height();
                let want = EDITOR_CHROME + if chord { CHORD_STRIP_EXTRA } else { 0.0 };
                assert!(
                    (chrome - want).abs() < 0.5,
                    "{w}x{h} chord={chord}: chrome {chrome} != {want}"
                );
            }
        }
    }

    /// Opening the chord strip in the default split grows the editor
    /// upwards: the piano roll itself stays exactly where it was.
    #[test]
    fn opening_the_chord_strip_keeps_the_piano_roll_in_place() {
        let mut ui = Harness::new(vec2(1440.0, 900.0));
        ui.select(2, false);
        let closed = ui.app.editor_test_rects().0;
        ui.select(2, true);
        let open = ui.app.editor_test_rects().0;
        assert!(
            (closed.top() - open.top()).abs() < 0.5,
            "{closed:?} -> {open:?}"
        );
        assert!((closed.height() - open.height()).abs() < 0.5);
    }

    /// At 1080p the arranger shows at least six lanes by default.
    #[test]
    fn default_split_shows_six_lanes_at_1080p() {
        let ui = Harness::new(vec2(1920.0, 1080.0));
        let arranger = ui.app.shell.probe.arranger.height();
        let needed = 40.0 + arranger::RULER_H + 16.0 + 6.0 * arranger::LANE_H;
        assert!(
            arranger >= needed,
            "{arranger:.0} px arranger, need {needed:.0}"
        );
    }

    /// Panels tile the central area without overlap or overflow in every
    /// mode, tab and size, and never panic on tiny areas.
    #[test]
    fn panels_never_overlap_or_overflow() {
        for (w, h) in [
            (1100.0, 760.0),
            (1360.0, 860.0),
            (1920.0, 1080.0),
            (900.0, 500.0),
        ] {
            for mode in [
                PanelMode::Balanced,
                PanelMode::ArrangerMax,
                PanelMode::EditorMax,
            ] {
                for tab in [Tab::Editor, Tab::Mixer, Tab::Keys] {
                    let mut ui = Harness::new(vec2(w, h));
                    ui.app.tab = tab;
                    ui.app.shell.split.mode = mode;
                    ui.frames(3);
                    let p = &ui.app.shell.probe;
                    let key = format!("{w}x{h} {mode:?} {tab:?}");
                    assert!(p.arranger.bottom() <= p.handle.top() + 0.01, "{key}");
                    assert!(p.handle.bottom() <= p.editor.top() + 0.01, "{key}");
                    assert!(
                        p.editor.bottom() <= p.status.top() + 0.5,
                        "{key}: editor {:?} status {:?}",
                        p.editor,
                        p.status
                    );
                    assert!(p.menu.height() > 0.0 && p.menu.bottom() <= p.arranger.top());
                }
            }
        }
    }

    #[test]
    fn maximize_modes_collapse_the_other_panel() {
        let mut ui = Harness::new(vec2(1360.0, 860.0));
        let normal = ui.app.shell.probe.editor.height();
        ui.key(egui::Key::ArrowDown, CMD_SHIFT);
        assert_eq!(ui.app.shell.split.mode, PanelMode::ArrangerMax);
        let collapsed = ui.app.shell.probe.editor.height();
        assert!(
            collapsed <= TABS_H + 1.0,
            "editor is just its tab strip: {collapsed}"
        );
        // The same shortcut again restores.
        ui.key(egui::Key::ArrowDown, CMD_SHIFT);
        assert_eq!(ui.app.shell.split.mode, PanelMode::Balanced);
        assert!((ui.app.shell.probe.editor.height() - normal).abs() < 1.0);
        ui.key(egui::Key::ArrowUp, CMD_SHIFT);
        assert_eq!(ui.app.shell.split.mode, PanelMode::EditorMax);
        assert!(ui.app.shell.probe.arranger.height() <= ARRANGER_COLLAPSED_H + 1.0);
        assert!(ui.grid_height() > 300.0, "the editor takes the room");
    }

    #[test]
    fn dragging_the_divider_resizes_and_survives_window_resizes() {
        let mut ui = Harness::new(vec2(1360.0, 860.0));
        let start = ui.app.shell.probe.arranger.height();
        let handle = ui.app.shell.probe.handle.center();
        ui.pointer(handle, Some(true));
        ui.pointer(handle + vec2(0.0, 40.0), None);
        ui.pointer(handle + vec2(0.0, 40.0), Some(false));
        ui.frames(2);
        let dragged = ui.app.shell.probe.arranger.height();
        assert!(dragged > start + 20.0, "dragged {start:.0} -> {dragged:.0}");
        assert!(ui.app.shell.split.user_share.is_some());
        let total = |ui: &Harness| {
            ui.app.shell.probe.arranger.height() + HANDLE_H + ui.app.shell.probe.editor.height()
        };
        let share = dragged / total(&ui);
        ui.resize(vec2(1920.0, 1080.0));
        let big = ui.app.shell.probe.arranger.height();
        let big_share = big / total(&ui);
        assert!(big > dragged, "a taller window gives the arranger more");
        assert!(
            (big_share - share).abs() < 0.05,
            "{share:.2} vs {big_share:.2}"
        );
    }

    #[test]
    fn double_clicking_the_divider_maximizes_then_restores() {
        let mut ui = Harness::new(vec2(1360.0, 860.0));
        let handle = ui.app.shell.probe.handle.center();
        ui.click(handle);
        ui.click(handle);
        ui.frames(2);
        assert_eq!(ui.app.shell.split.mode, PanelMode::ArrangerMax);
        // Let the multi-click window lapse, or egui counts a triple click.
        ui.time += 2.0;
        ui.frames(2);
        let handle = ui.app.shell.probe.handle.center();
        ui.click(handle);
        ui.click(handle);
        ui.frames(2);
        assert_eq!(ui.app.shell.split.mode, PanelMode::Balanced);
    }

    #[test]
    fn switching_tabs_reveals_a_collapsed_editor() {
        let mut ui = Harness::new(vec2(1360.0, 860.0));
        ui.app.shell.split.mode = PanelMode::ArrangerMax;
        ui.frames(2);
        ui.app.tab = Tab::Mixer;
        ui.frames(2);
        assert_eq!(ui.app.shell.split.mode, PanelMode::Balanced);
    }

    #[test]
    fn toasts_follow_status_severity_only() {
        let mut ui = Harness::new(vec2(1360.0, 860.0));
        ui.app.set_status(StatusKind::Info, "Copied 2 notes.");
        assert!(ui.app.shell.toast.is_none());
        ui.app.set_status(StatusKind::Success, "Saved.");
        ui.frames(2);
        assert!(ui.app.shell.toast.is_some());
        assert_eq!(ui.app.status.as_ref().unwrap().text, "Saved.");
        // Expired toasts are dropped without a frame ever needing them.
        ui.app.shell.toast.as_mut().unwrap().since = Instant::now() - Duration::from_secs(30);
        ui.frames(2);
        assert!(ui.app.shell.toast.is_none());
    }

    /// Release behavior sweep through the real shortcuts: tab switching,
    /// track add / duplicate / delete with undo and both redo shortcuts,
    /// loop, seek and record toggles.
    #[test]
    fn shortcuts_drive_tabs_tracks_undo_redo_and_transport() {
        let mut ui = Harness::new(vec2(1360.0, 860.0));
        let cmd = egui::Modifiers::COMMAND;
        for (key, tab) in [
            (egui::Key::Num2, Tab::Mixer),
            (egui::Key::Num3, Tab::Keys),
            (egui::Key::Num1, Tab::Editor),
        ] {
            ui.key(key, cmd);
            assert_eq!(ui.app.tab, tab, "{key:?}");
        }
        let tracks = ui.app.project.tracks.len();
        ui.app.add_track(Instrument::Pluck);
        ui.frames(2);
        assert_eq!(ui.app.project.tracks.len(), tracks + 1);
        let added = ui.app.project.tracks.last().unwrap().id;
        ui.app.duplicate_track(added);
        ui.frames(2);
        assert_eq!(ui.app.project.tracks.len(), tracks + 2);
        ui.app.delete_track(added);
        ui.frames(2);
        assert_eq!(ui.app.project.tracks.len(), tracks + 1);
        // Undo walks back delete, duplicate, add; both redo shortcuts work.
        for want in [tracks + 2, tracks + 1, tracks] {
            ui.key(egui::Key::Z, cmd);
            assert_eq!(ui.app.project.tracks.len(), want);
        }
        ui.key(egui::Key::Z, CMD_SHIFT);
        assert_eq!(ui.app.project.tracks.len(), tracks + 1);
        ui.key(egui::Key::Y, cmd);
        assert_eq!(ui.app.project.tracks.len(), tracks + 2);
        // Transport keys.
        let looping = ui.app.transport.looping;
        ui.key(egui::Key::L, egui::Modifiers::NONE);
        assert_ne!(ui.app.transport.looping, looping, "L toggles the loop");
        ui.app.transport.position = 40.0;
        ui.key(egui::Key::Home, egui::Modifiers::NONE);
        assert_eq!(ui.app.transport.position, 0.0, "Home returns to the start");
        let recording = ui.app.transport.recording;
        ui.key(egui::Key::R, egui::Modifiers::NONE);
        assert_ne!(ui.app.transport.recording, recording, "R arms recording");
        // F1 opens the shortcuts, Esc closes them.
        ui.key(egui::Key::F1, egui::Modifiers::NONE);
        assert!(matches!(ui.app.dialog, Dialog::Help));
        ui.key(egui::Key::Escape, egui::Modifiers::NONE);
        assert!(matches!(ui.app.dialog, Dialog::None));
    }

    /// A toast floats over the work surface but never takes the pointer —
    /// not even on the frame it appears (an `Area` used to swallow clicks
    /// over half the editor for that one frame).
    #[test]
    fn toasts_never_block_the_work_surface() {
        let mut ui = Harness::new(vec2(1360.0, 860.0));
        ui.select(0, false);
        let notes = |ui: &Harness| {
            let (tid, pid) = ui.app.editor_target().unwrap();
            ops::pattern(ops::track(&ui.app.project, tid).unwrap(), pid)
                .unwrap()
                .notes
                .len()
        };
        let before = notes(&ui);
        ui.app.set_status(
            StatusKind::Success,
            "Saved “Afterglow” to the songs folder.",
        );
        ui.frames(1);
        let toast = ui.app.shell.probe.toast;
        assert!(toast.is_positive(), "the toast is painted");
        assert!(toast.bottom() <= ui.app.shell.probe.status.top());
        let grid = ui.app.editor_test_rects().0;
        assert!(grid.contains(toast.center()), "{toast:?} over {grid:?}");
        ui.click(toast.center());
        assert_ne!(
            notes(&ui),
            before,
            "the drum grid under the toast got the click"
        );
    }
}
