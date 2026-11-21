//! Left sidebar: instrument library (built-ins + JSON packs) and track inspector.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{
    pos2, vec2, Align, Align2, Color32, Context, Frame, Id, Layout, Margin, Rect, RichText, Sense,
    Stroke, StrokeKind, Ui, UiBuilder, Vec2,
};

use super::theme::{
    self, Icon, Palette, Segment, CONTROL_M, CONTROL_S, GAP_L, GAP_M, GAP_S, GAP_XS, ICON_S,
    TEXT_BODY, TEXT_SMALL, TEXT_TITLE,
};
use super::{ops, CadenceApp, Focus, StatusKind, Tab};
use crate::font::{gm_program_catalog, FontInfo, GM_PROGRAM_NAMES};
use crate::model::{
    create_track, note_name, ArpDirection, ArpRate, ArpSettings, FontRef, Instrument, Track,
    MAX_ARP_OCTAVES, MAX_GLIDE_SECONDS, MIN_ARP_GATE,
};
use crate::packs::{InstrumentPreset, PackCatalog};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum SideTab {
    #[default]
    Library,
    Track,
}

/// egui temp-data keys for the sidebar disclosure rows (the collapsible
/// instrument list and the quick-add strip). Temp data — not a `CadenceApp`
/// field, which mod.rs owns — keeps the open/collapsed state for the session:
/// it survives tab switches and resets on restart, which is fine for view
/// chrome that is never part of the song.
const LIBRARY_OPEN: &str = "cadence.sidebar.library_open";
/// Temp-data key for the quick-add disclosure — `pub(super)` so headless
/// tests can open the strip without clicking blind.
pub(super) const QUICK_ADD_OPEN: &str = "cadence.sidebar.quick_add_open";
const SIDEBAR_COLLAPSED: &str = "cadence.sidebar.collapsed";
/// Font name the SoundFont bank currently lists programs for (session view
/// state, like the other sidebar chrome).
const LIBRARY_FONT: &str = "cadence.sidebar.library_font";
/// Last-frame rect of the first quick-add SoundFont row (for headless tests,
/// stored in temp data so mod.rs owns no extra fields).
pub(super) const QUICK_ADD_FONT_RECT: &str = "cadence.sidebar.quick_add_font_rect";
/// Last-frame rect of the quick-add "Add a SoundFont…" hint row.
pub(super) const QUICK_ADD_FONT_HINT_RECT: &str = "cadence.sidebar.quick_add_font_hint_rect";
/// Temp-data prefix of the per-section collapsed flags of the library list.
const BANK_CLOSED: &str = "cadence.sidebar.bank_closed";
/// Temp-data key of the cached [`LibraryView`].
const LIBRARY_CACHE: &str = "cadence.sidebar.library_cache";
/// Temp-data key of the cached quick-add picks.
const QUICK_PICKS_CACHE: &str = "cadence.sidebar.quick_picks_cache";
/// Widget id of the library search field.
const SEARCH_FIELD: &str = "cadence.sidebar.search";

/// Width of the collapsed sidebar rail.
const RAIL_W: f32 = 40.0;
/// Library card height (4 px grid).
const CARD_H: f32 = 44.0;
/// Quick-add row height.
const QUICK_H: f32 = CONTROL_M;
/// Bank section header height.
const SECTION_H: f32 = 24.0;
/// Width of the "Reset" filter button.
const RESET_W: f32 = 56.0;
/// Label column of the inspector rows.
const LABEL_W: f32 = 56.0;
/// Value readout column next to the inspector sliders.
const READOUT_W: f32 = 36.0;
/// Inspector knob size and the width a knob column occupies.
const KNOB: f32 = 40.0;
const KNOB_W: f32 = 44.0;

pub(super) fn instrument_color(inst: Instrument) -> [u8; 3] {
    inst.color()
}

/// "eight-bit.pulse-lead" → "Eight Bit": a readable pack heading from the id namespace.
pub(super) fn pack_title(preset_id: &str) -> String {
    let ns = preset_id.split('.').next().unwrap_or(preset_id);
    ns.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or(String::new(), |f| {
                f.to_uppercase().collect::<String>() + c.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn preset_summary(p: &InstrumentPreset) -> String {
    if let Some(kit) = &p.kit {
        let n = kit.pieces.len();
        return format!("kit · {n} piece{}", plural(n));
    }
    let waves: Vec<String> = p
        .oscillators
        .iter()
        .map(|o| format!("{:?}", o.waveform).to_lowercase())
        .collect();
    let mut parts = vec![if waves.is_empty() {
        "no oscillators".to_owned()
    } else {
        waves.join(" + ")
    }];
    if p.bit_depth < 24 {
        parts.push(format!("{}-bit", p.bit_depth));
    }
    if p.sample_hold > 1 {
        parts.push(format!("hold ×{}", p.sample_hold));
    }
    if p.monophonic {
        parts.push("mono".into());
    }
    parts.join(" · ")
}

/// Which slice of the instrument library the browser lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum LibraryBank {
    /// Built-ins plus every loaded pack preset.
    #[default]
    All,
    /// Only the built-in instruments.
    BuiltIn,
    /// One pack bank, keyed by preset-id namespace ("psy-bass").
    Pack(String),
    /// The SoundFont catalog: 128 GM programs grouped by family, played by
    /// an installed `.sf2` font.
    Font,
}

/// Last-frame library chrome geometry. Headless tests aim scroll and click
/// input with it (mirrors `editor::EditorRects`).
#[derive(Debug, Clone, Copy)]
pub(super) struct LibraryRects {
    pub search: Rect,
    pub bank: Rect,
    /// Viewport of the scrolling instrument list.
    pub list: Rect,
    /// Scroll offset of the instrument list.
    pub scroll: egui::Vec2,
    /// First GM program row of the SoundFont bank (`Rect::NOTHING` when the
    /// bank isn't showing rows).
    pub first_font_row: Rect,
    /// "Add a SoundFont…" button of the no-font hint card.
    pub font_hint: Rect,
}

impl Default for LibraryRects {
    fn default() -> Self {
        Self {
            search: Rect::NOTHING,
            bank: Rect::NOTHING,
            list: Rect::NOTHING,
            scroll: egui::Vec2::ZERO,
            first_font_row: Rect::NOTHING,
            font_hint: Rect::NOTHING,
        }
    }
}

/// Pack namespace of a preset id ("psy-bass.sub-sine" → "psy-bass").
pub(super) fn preset_namespace(preset_id: &str) -> &str {
    preset_id.split('.').next().unwrap_or(preset_id)
}

/// Display name of a bank filter.
fn bank_title(bank: &LibraryBank) -> String {
    match bank {
        LibraryBank::All => "All".to_owned(),
        LibraryBank::BuiltIn => "Built-in".to_owned(),
        LibraryBank::Pack(ns) => pack_title(ns),
        LibraryBank::Font => "SoundFont".to_owned(),
    }
}

/// Searchable bank label: the readable title plus the raw namespace, so both
/// "Psy Bass" and "psy-bass" match.
fn bank_search_text(namespace: &str) -> String {
    format!("{} {}", pack_title(namespace), namespace)
}

/// Bank choices for the chooser with their row counts, in display order:
/// All, Built-in, then each pack namespace in catalog order.
pub(super) fn bank_options(catalog: &PackCatalog) -> Vec<(LibraryBank, usize)> {
    let mut packs: Vec<(String, usize)> = Vec::new();
    for p in &catalog.instruments {
        let ns = preset_namespace(&p.id);
        match packs.iter_mut().find(|(n, _)| n == ns) {
            Some((_, count)) => *count += 1,
            None => packs.push((ns.to_owned(), 1)),
        }
    }
    let mut options = vec![
        (
            LibraryBank::All,
            Instrument::ALL.len() + catalog.instruments.len(),
        ),
        (LibraryBank::BuiltIn, Instrument::ALL.len()),
        // The GM catalog always offers 128 programs, whatever fonts are
        // installed — the bank body explains when none are.
        (LibraryBank::Font, GM_PROGRAM_NAMES.len()),
    ];
    options.extend(
        packs
            .into_iter()
            .map(|(ns, count)| (LibraryBank::Pack(ns), count)),
    );
    options
}

/// The library rows visible under a bank filter and search query, as the
/// headless tests see them (the list UI itself uses the same row indices).
#[cfg(test)]
pub(super) struct LibraryRows<'a> {
    pub builtins: Vec<Instrument>,
    pub presets: Vec<&'a InstrumentPreset>,
}

/// `query` is the trimmed, lowercased search text (empty matches everything).
/// Search hits the preset/built-in name, its description and its bank name.
#[cfg(test)]
pub(super) fn library_rows<'a>(
    bank: &LibraryBank,
    query: &str,
    catalog: &'a PackCatalog,
) -> LibraryRows<'a> {
    let (builtins, presets) = library_row_indices(bank, query, catalog);
    LibraryRows {
        builtins,
        presets: presets
            .into_iter()
            .map(|i| &catalog.instruments[i])
            .collect(),
    }
}

/// [`library_rows`] as indices into `catalog.instruments` (what the cached
/// library view stores).
fn library_row_indices(
    bank: &LibraryBank,
    query: &str,
    catalog: &PackCatalog,
) -> (Vec<Instrument>, Vec<usize>) {
    let text_hit = |fields: &[&str]| {
        query.is_empty() || fields.iter().any(|f| f.to_lowercase().contains(query))
    };
    let builtins = match bank {
        LibraryBank::All | LibraryBank::BuiltIn => Instrument::ALL
            .iter()
            .copied()
            .filter(|i| text_hit(&[i.name(), i.description(), "Built-in"]))
            .collect(),
        LibraryBank::Pack(_) | LibraryBank::Font => Vec::new(),
    };
    let presets = catalog
        .instruments
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            let bank_ok = match bank {
                LibraryBank::All => true,
                LibraryBank::BuiltIn | LibraryBank::Font => false,
                LibraryBank::Pack(b) => b == preset_namespace(&p.id),
            };
            bank_ok
                && text_hit(&[
                    p.name.as_str(),
                    p.description.as_str(),
                    bank_search_text(preset_namespace(&p.id)).as_str(),
                    p.id.as_str(),
                ])
        })
        .map(|(i, _)| i)
        .collect();
    (builtins, presets)
}

/// The GM programs the SoundFont bank lists, grouped by family in GM order
/// and filtered by `query` (trimmed, lowercased; empty matches everything).
/// Search hits the program name, the family name or the word "soundfont";
/// families left with no hits drop out entirely.
pub(super) fn font_rows(query: &str) -> Vec<(&'static str, Vec<(u8, &'static str)>)> {
    gm_program_catalog()
        .into_iter()
        .filter_map(|(family, programs)| {
            let hits: Vec<(u8, &'static str)> = programs
                .into_iter()
                .filter(|(_, name)| {
                    query.is_empty()
                        || name.to_lowercase().contains(query)
                        || family.to_lowercase().contains(query)
                        || "soundfont".contains(query)
                })
                .collect();
            (!hits.is_empty()).then_some((family, hits))
        })
        .collect()
}

/// MIDI pitch a kit preset's library audition should strike. An explicit
/// `audition_note` (anything but the serde default of 60) wins; otherwise the
/// first piece's pitch — a guaranteed real kit hit — then 60 as a last resort.
fn kit_audition_pitch(p: &InstrumentPreset) -> u8 {
    let Some(kit) = &p.kit else {
        return p.audition_note;
    };
    if p.audition_note != 60 {
        return p.audition_note;
    }
    kit.pieces.first().map_or(60, |piece| piece.pitch)
}

/// Pitch and duration for a track's library audition. Pack presets carry
/// `audition_note`/`audition_duration` metadata (MIDI 60 / 0.8 s when the
/// pack omits them); built-ins keep the historic fallback values. Kit presets
/// audition on a real piece pitch — see [`kit_audition_pitch`]. SoundFont
/// programs carry no audition metadata, so font tracks preview at the
/// caller's pitch for a fixed 0.8 s.
pub(super) fn audition_params(track: &Track, fallback_pitch: u8) -> (u8, f64) {
    if track.font.is_some() {
        return (fallback_pitch, 0.8);
    }
    match &track.preset {
        Some(p) => {
            let pitch = if p.kit.is_some() {
                kit_audition_pitch(p)
            } else {
                p.audition_note
            };
            (pitch, f64::from(p.audition_duration))
        }
        None => (fallback_pitch, 0.7),
    }
}

enum LibraryAction {
    Add(Instrument),
    AddPreset(InstrumentPreset),
    Audition(Instrument),
    AuditionPreset(InstrumentPreset),
    /// Add a track playing this GM program on its SoundFont.
    AddFont(FontRef),
    /// Audition this GM program on its SoundFont.
    AuditionFont(FontRef),
}

/// A SoundFont choice made inside the sound picker popup.
enum FontPick {
    /// Assign this font+program; the preset stays as the missing-font fallback.
    Assign(FontRef),
    /// Remove the font reference so the preset/built-in sound plays.
    Clear,
}

/// A sound the quick-add row can offer and a library row can drag onto the
/// arranger: a built-in instrument, a pack preset (owned — adding a
/// preset track embeds the whole preset anyway, so the clone is cheap and
/// keeps `&mut self` calls borrow-free) or a SoundFont GM program. The preset
/// is boxed to keep the enum small next to the 1-byte `Instrument`.
/// `QuickSound` is also the egui drag-and-drop payload between the library
/// and the arranger, so it must stay `Send + Sync + 'static`.
#[derive(Clone)]
pub(super) enum QuickSound {
    BuiltIn(Instrument),
    Preset(Box<InstrumentPreset>),
    /// A GM program played by the named installed SoundFont.
    Font(FontRef),
}

/// Accent color for SoundFont rows: a violet from the track palette, so font
/// sounds read differently from built-ins and packs at a glance.
pub(super) const FONT_COLOR: [u8; 3] = theme::TRACK_PALETTE[8];

impl QuickSound {
    pub(super) fn name(&self) -> &str {
        match self {
            Self::BuiltIn(i) => i.name(),
            Self::Preset(p) => p.name.as_str(),
            Self::Font(f) => f.program_name(),
        }
    }

    /// Hover text: the built-in description, or the preset's description plus
    /// its synthesis summary like the full library rows show; for fonts the
    /// GM program, its family and the font that plays it.
    fn tip(&self) -> String {
        match self {
            Self::BuiltIn(i) => i.description().to_owned(),
            Self::Preset(p) => format!("{}\n\n{}", p.description, preset_summary(p)),
            Self::Font(f) => format!(
                "{}\n\nGM program {} · {} · {}.sf2",
                f.program_name(),
                f.program,
                gm_family(f.program),
                f.name
            ),
        }
    }

    pub(super) fn color(&self) -> [u8; 3] {
        match self {
            Self::BuiltIn(i) => instrument_color(*i),
            Self::Preset(p) => ops::preset_color(&p.id),
            Self::Font(_) => FONT_COLOR,
        }
    }

    fn is_kit(&self) -> bool {
        matches!(self, Self::Preset(p) if p.kit.is_some())
    }
}

/// The quick-add picker's groups, best first: sounds already in the song,
/// then built-ins, then a few pack presets, then GM programs from an
/// installed SoundFont.
struct QuickPicks {
    /// Sounds already on tracks in this song, newest track first.
    recent: Vec<QuickSound>,
    /// Built-in instruments not already listed under `recent`.
    builtins: Vec<QuickSound>,
    /// The first preset of a few pack banks, for sounds beyond built-ins.
    presets: Vec<QuickSound>,
    /// A few iconic GM programs from an installed SoundFont.
    fonts: Vec<QuickSound>,
}

impl QuickPicks {
    #[cfg(test)]
    fn count(&self) -> usize {
        self.recent.len() + self.builtins.len() + self.presets.len() + self.fonts.len()
    }
}

/// The GM programs the quick-add SoundFont group spotlights — one iconic
/// real-instrument sound each, in catalog order.
const QUICK_FONT_PROGRAMS: [u8; 3] = [0, 24, 40];

/// Display name of the GM family containing `program` (0..=127); out-of-range
/// values clamp into the last family like [`crate::font::gm_program_name`].
pub(super) fn gm_family(program: u8) -> &'static str {
    crate::font::gm_family(program)
}

impl CadenceApp {
    pub(super) fn sidebar(&mut self, ctx: &Context) {
        let pal = self.palette();
        // Whole-panel collapse: a slim rail of icon buttons, so the
        // arranger/editor get the width back. Session state via egui temp
        // data, same as the library/quick-add disclosures.
        let collapsed = ctx
            .data(|d| d.get_temp::<bool>(Id::new(SIDEBAR_COLLAPSED)))
            .unwrap_or(false);
        let margin = if collapsed {
            Margin::symmetric(3, 8)
        } else {
            Margin {
                left: 12,
                right: 10,
                top: 10,
                bottom: 8,
            }
        };
        let frame = Frame::new()
            .fill(pal.panel)
            .inner_margin(margin)
            .stroke(Stroke::new(1.0_f32, pal.border_soft));
        let mut panel = egui::SidePanel::left("sidebar");
        if collapsed {
            panel = panel.exact_width(RAIL_W).resizable(false);
        } else {
            panel = panel
                .resizable(true)
                .default_width(272.0)
                .width_range(240.0..=380.0);
        }
        panel.frame(frame).show(ctx, |ui| {
            if collapsed {
                // No list chrome is visible — don't leave stale geometry for
                // tests/click hit-testing.
                self.library_rects = LibraryRects::default();
                self.sidebar_rail(ui, pal);
                return;
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP_XS;
                // The tab strip takes exactly what the collapse button leaves
                // over, so the row can never overflow the panel — overflow
                // would feed back into the panel's stored width and make it
                // ratchet wider every frame.
                let w = (ui.available_width() - CONTROL_M - GAP_XS).max(96.0);
                theme::segmented(
                    ui,
                    pal,
                    &mut self.side_tab,
                    &[
                        Segment::new(SideTab::Library, "Instruments")
                            .tip("Browse sounds and add them as tracks"),
                        Segment::new(SideTab::Track, "Track")
                            .tip("Settings for the selected track"),
                    ],
                    CONTROL_M,
                    Some(w),
                );
                if theme::ghost_icon_button(
                    ui,
                    pal,
                    Icon::SidebarCollapse,
                    CONTROL_M,
                    "Hide sidebar",
                )
                .clicked()
                {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(Id::new(SIDEBAR_COLLAPSED), true));
                }
            });
            ui.add_space(GAP_S);
            match self.side_tab {
                // The library manages its own scroll area so the search
                // and bank controls stay visible above the list.
                SideTab::Library => self.library_ui(ui),
                SideTab::Track => {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.inspector_ui(ui));
                }
            }
        });
    }

    /// The collapsed sidebar: an expand button plus one icon per tab that
    /// reopens the sidebar on that tab.
    fn sidebar_rail(&mut self, ui: &mut Ui, pal: Palette) {
        let mut open_tab = None;
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = GAP_XS;
            if theme::icon_button(
                ui,
                pal,
                Icon::SidebarExpand,
                CONTROL_M,
                false,
                pal.accent,
                "Show sidebar",
            )
            .clicked()
            {
                open_tab = Some(self.side_tab);
            }
            ui.add_space(GAP_XS);
            let (r, _) = ui.allocate_exact_size(vec2(CONTROL_M, 1.0), Sense::hover());
            ui.painter()
                .hline(r.x_range(), r.center().y, Stroke::new(1.0_f32, pal.border));
            ui.add_space(GAP_XS);
            for (tab, icon, tip) in [
                (
                    SideTab::Library,
                    Icon::Piano,
                    "Instruments: browse sounds and add tracks",
                ),
                (SideTab::Track, Icon::Settings, "Track settings"),
            ] {
                if theme::icon_button(
                    ui,
                    pal,
                    icon,
                    CONTROL_M,
                    self.side_tab == tab,
                    pal.accent,
                    tip,
                )
                .clicked()
                {
                    open_tab = Some(tab);
                }
            }
        });
        if let Some(tab) = open_tab {
            self.side_tab = tab;
            ui.ctx()
                .data_mut(|d| d.insert_temp(Id::new(SIDEBAR_COLLAPSED), false));
        }
    }

    fn library_ui(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        let used = self.project.tracks.len();
        let full = used >= ops::MAX_TRACKS;
        let total = Instrument::ALL.len() + self.catalog.instruments.len();
        // The header row doubles as the collapse toggle: the (possibly long)
        // list below folds away to a single hint line. Open state lives in
        // egui temp data so it survives switching to the Track tab.
        let open = library_header(ui, pal, used, full);
        if !open {
            // With the list hidden there is no search/bank/list chrome, so
            // the test-facing rects go back to nothing rather than stale.
            self.library_rects = LibraryRects::default();
            ui.add_space(2.0);
            theme::hint(
                ui,
                pal,
                &format!(
                    "{total} instrument{} — click the header to expand",
                    plural(total)
                ),
            );
            return;
        }
        ui.add_space(GAP_XS);
        self.library_rects.search =
            search_field(ui, pal, &mut self.library_filter, "Search sounds…");
        ui.add_space(GAP_XS);

        // Bank chooser + reset live above the scrolling list so they stay
        // reachable however far down the library the user has scrolled.
        let filtered =
            !self.library_filter.trim().is_empty() || self.library_bank != LibraryBank::All;
        let bank_row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_XS;
            let options = bank_options(&self.catalog);
            let count = options
                .iter()
                .find(|(b, _)| *b == self.library_bank)
                .map(|(_, n)| *n);
            let selected = match count {
                Some(n) => format!("{} · {n}", bank_title(&self.library_bank)),
                None => bank_title(&self.library_bank),
            };
            let reset_w = if filtered { RESET_W + GAP_XS } else { 0.0 };
            let mut pick = None;
            theme::combo(
                "library-bank",
                selected,
                (ui.available_width() - reset_w).max(90.0),
            )
            .show_ui(ui, |ui| {
                for (bank, n) in &options {
                    let on = *bank == self.library_bank;
                    if ui
                        .selectable_label(on, format!("{} · {n}", bank_title(bank)))
                        .clicked()
                    {
                        pick = Some(bank.clone());
                    }
                }
            });
            let mut reset = false;
            if filtered {
                reset = ui
                    .scope(|ui| {
                        ui.set_width(RESET_W);
                        theme::text_button(
                            ui,
                            pal,
                            "Reset",
                            CONTROL_M,
                            "Clear the search text and bank filter",
                        )
                    })
                    .inner
                    .clicked();
            }
            (pick, reset)
        });
        self.library_rects.bank = bank_row.response.rect;
        let (pick, reset) = bank_row.inner;
        if let Some(bank) = pick {
            self.library_bank = bank;
        }
        if reset {
            self.library_filter.clear();
            self.library_bank = LibraryBank::All;
        }
        ui.add_space(GAP_XS);
        if full {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP_XS;
                let (r, _) = ui.allocate_exact_size(vec2(ICON_S, ICON_S), Sense::hover());
                theme::paint_icon(ui.painter(), r, Icon::Warning, pal.danger);
                ui.label(
                    RichText::new("Track limit reached. Delete a track to add another.")
                        .small()
                        .color(pal.danger),
                );
            });
            ui.add_space(GAP_XS);
        }

        let query = self.library_filter.trim().to_lowercase();
        let bank = self.library_bank.clone();
        let view = library_view(ui.ctx(), &self.catalog, &bank, &query);
        let mut action = None;
        let list_rect = ui.available_rect_before_wrap();
        let list = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.library_list(ui, pal, &view, &bank, &query, full, &mut action)
            });
        self.library_rects.list = list_rect;
        self.library_rects.scroll = list.state.offset;
        if list.inner {
            self.library_filter.clear();
            self.library_bank = LibraryBank::All;
        }

        match action {
            Some(LibraryAction::Add(inst)) => self.add_track(inst),
            Some(LibraryAction::AddPreset(p)) => self.add_preset_track(&p),
            Some(LibraryAction::Audition(inst)) => {
                self.audition_instrument(&create_track(inst), inst.default_pitch())
            }
            Some(LibraryAction::AuditionPreset(p)) => {
                // Mirror add_preset_track: kits audition as Drums tracks.
                let inst = if p.kit.is_some() {
                    Instrument::Drums
                } else {
                    Instrument::Lead
                };
                let mut t = create_track(inst);
                t.preset = Some(p);
                self.audition_instrument(&t, 60);
            }
            Some(LibraryAction::AddFont(f)) => self.add_font_track(&f),
            Some(LibraryAction::AuditionFont(f)) => {
                self.audition_instrument(&Track::from_font(Instrument::Lead, f), 60);
            }
            None => {}
        }
    }

    /// The scrolling body of the library: collapsible bank sections with
    /// sticky headers, only the visible cards laid out. Returns `true` when
    /// the "clear search" action of the empty state was clicked.
    #[allow(clippy::too_many_arguments)]
    fn library_list(
        &mut self,
        ui: &mut Ui,
        pal: Palette,
        view: &LibraryView,
        bank: &LibraryBank,
        query: &str,
        full: bool,
        action: &mut Option<LibraryAction>,
    ) -> bool {
        // Stale geometry from a previous frame's bank must not leak into
        // hit-tests; the font bank re-fills its rects.
        self.library_rects.first_font_row = Rect::NOTHING;
        self.library_rects.font_hint = Rect::NOTHING;
        ui.spacing_mut().item_spacing.y = GAP_XS;
        let mut font_name = String::new();
        if *bank == LibraryBank::Font {
            let fonts: Vec<FontInfo> = self.fonts.fonts.clone();
            if fonts.is_empty() {
                // No .sf2 installed: a slim hint card pointing at the folder.
                self.fonts_hint_card(ui, pal);
                return false;
            }
            font_name = self.font_chooser(ui, pal, &fonts);
        }

        let w = ui.available_width();
        let clip = ui.clip_rect();
        let locked = !query.is_empty();
        let dragging = ui.ctx().dragged_id().is_some();
        let base = ui.id();
        let mut headers: Vec<HeaderPos> = Vec::with_capacity(view.sections.len());
        for (si, section) in view.sections.iter().enumerate() {
            if si > 0 {
                ui.add_space(GAP_XS);
            }
            let closed_id = Id::new((BANK_CLOSED, &section.key));
            let closed = !locked
                && ui
                    .ctx()
                    .data(|d| d.get_temp::<bool>(closed_id))
                    .unwrap_or(false);
            let (rect, clicked) = section_header(
                ui,
                pal,
                base.with(("section", &section.key)),
                &section.title,
                section.count,
                !closed,
                locked,
            );
            let closed = closed ^ clicked;
            if clicked {
                ui.ctx().data_mut(|d| d.insert_temp(closed_id, closed));
            }
            headers.push(HeaderPos {
                rect,
                open: !closed,
                id: closed_id,
            });
            if closed {
                continue;
            }
            for item in &section.items {
                let top = ui.cursor().min.y;
                let visible = top < clip.bottom() && top + CARD_H > clip.top();
                let row_id = library_item_id(base, item, &self.catalog);
                if !visible && !(dragging && ui.ctx().is_being_dragged(row_id)) {
                    ui.allocate_space(vec2(w, CARD_H));
                    continue;
                }
                let out = match *item {
                    LibItem::BuiltIn(inst) => {
                        let out = library_row(
                            ui,
                            pal,
                            &RowSpec {
                                name: inst.name(),
                                description: inst.description(),
                                color: theme::rgb(instrument_color(inst)),
                                badge: inst.is_drums().then_some("KIT"),
                                can_add: !full,
                                id: row_id,
                            },
                            || {
                                format!(
                                    "{}\n\nDouble-click to add, or drag onto the arrangement.",
                                    inst.description()
                                )
                            },
                            || QuickSound::BuiltIn(inst),
                        );
                        out.act.map(|add| {
                            if add {
                                LibraryAction::Add(inst)
                            } else {
                                LibraryAction::Audition(inst)
                            }
                        })
                    }
                    LibItem::Preset(i) => {
                        let preset = &self.catalog.instruments[i];
                        let out = library_row(
                            ui,
                            pal,
                            &RowSpec {
                                name: &preset.name,
                                description: &preset.description,
                                color: theme::rgb(ops::preset_color(&preset.id)),
                                badge: Some(if preset.kit.is_some() { "KIT" } else { "PACK" }),
                                can_add: !full,
                                id: row_id,
                            },
                            || {
                                format!(
                                    "{}\n\n{}\nPreview plays {} for {:.1} s.\n\nDouble-click to add, or drag onto the arrangement.",
                                    preset.description,
                                    preset_summary(preset),
                                    note_name(if preset.kit.is_some() {
                                        kit_audition_pitch(preset)
                                    } else {
                                        preset.audition_note
                                    }),
                                    preset.audition_duration,
                                )
                            },
                            || QuickSound::Preset(Box::new(preset.clone())),
                        );
                        out.act.map(|add| {
                            if add {
                                LibraryAction::AddPreset(preset.clone())
                            } else {
                                LibraryAction::AuditionPreset(preset.clone())
                            }
                        })
                    }
                    LibItem::Font(program, label) => {
                        let font = FontRef::new(font_name.clone(), program);
                        let out = library_row(
                            ui,
                            pal,
                            &RowSpec {
                                name: label,
                                description: &font_name,
                                color: theme::rgb(FONT_COLOR),
                                badge: Some("FONT"),
                                can_add: !full,
                                id: row_id,
                            },
                            || {
                                format!(
                                    "{label}\n\nSoundFont {font_name} · GM program {program} · {}\n\nDouble-click to add, or drag onto the arrangement.",
                                    gm_family(program)
                                )
                            },
                            || QuickSound::Font(font.clone()),
                        );
                        if self.library_rects.first_font_row == Rect::NOTHING {
                            self.library_rects.first_font_row = out.rect;
                        }
                        out.act.map(|add| {
                            if add {
                                LibraryAction::AddFont(font.clone())
                            } else {
                                LibraryAction::AuditionFont(font.clone())
                            }
                        })
                    }
                };
                if out.is_some() {
                    *action = out;
                }
            }
        }
        sticky_header(ui, pal, &view.sections, &headers, clip, locked);

        let mut clear = false;
        if view.sections.is_empty() {
            ui.add_space(GAP_L);
            let q = elide_query(query);
            let (title, hint, button) = if !query.is_empty() {
                (
                    format!("No sounds match “{q}”"),
                    "Try a different word, or clear the search.",
                    Some("Clear search"),
                )
            } else if *bank == LibraryBank::All {
                (
                    "No sounds yet".to_owned(),
                    "Import an instrument pack to add sounds.",
                    None,
                )
            } else {
                (
                    "Nothing in this bank".to_owned(),
                    "Pick another bank, or reset the filter.",
                    Some("Reset filter"),
                )
            };
            clear = theme::empty_state(ui, pal, Icon::Search, &title, hint, button);
        }

        if *bank == LibraryBank::Font {
            let n = self.fonts.errors.len();
            if n > 0 {
                ui.add_space(GAP_XS);
                ui.label(
                    RichText::new(format!("{n} font file{} failed to load", plural(n)))
                        .small()
                        .color(pal.warn),
                )
                .on_hover_text(self.fonts.errors.join("\n"));
            }
            ui.add_space(GAP_S);
            if theme::icon_label_button(
                ui,
                pal,
                Icon::Folder,
                "Open SoundFonts folder",
                CONTROL_M,
                "Drop .sf2 files into this folder; they appear in the Sound menu and here within a couple of seconds",
            )
            .clicked()
            {
                self.reveal_fonts_folder();
            }
            ui.add_space(GAP_M);
        } else {
            ui.add_space(GAP_L);
            ui.push_id("packs-footer", |ui| self.packs_footer(ui, pal));
            ui.add_space(GAP_S);
        }
        clear
    }

    /// The SoundFont chooser above the GM program list (only when several
    /// fonts are installed). Returns the font the programs belong to.
    fn font_chooser(&mut self, ui: &mut Ui, pal: Palette, fonts: &[FontInfo]) -> String {
        // Session font choice, kept in egui temp data like the disclosures.
        let stored = ui
            .ctx()
            .data_mut(|d| d.get_temp::<String>(Id::new(LIBRARY_FONT)));
        let selected = stored
            .filter(|name| fonts.iter().any(|i| &i.name == name))
            .unwrap_or_else(|| fonts[0].name.clone());
        if fonts.len() > 1 {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP_S;
                ui.label(theme::kicker_text(pal, "FONT"));
                let mut pick = None;
                theme::combo(
                    "library-font",
                    selected.clone(),
                    ui.available_width().max(90.0),
                )
                .show_ui(ui, |ui| {
                    for info in fonts {
                        if ui
                            .selectable_label(info.name == selected, &info.name)
                            .on_hover_text(format!("SoundFont · {} presets", info.presets))
                            .clicked()
                        {
                            pick = Some(info.name.clone());
                        }
                    }
                });
                if let Some(name) = pick {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(Id::new(LIBRARY_FONT), name));
                }
            });
        }
        selected
    }

    fn packs_footer(&mut self, ui: &mut Ui, pal: Palette) {
        theme::card(pal).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(theme::kicker_text(pal, "INSTRUMENT PACKS"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    theme::ghost_icon_button(
                        ui,
                        pal,
                        Icon::Info,
                        CONTROL_S,
                        "Packs are small .json files of synth presets. New packs load \
                         instantly: no restart needed. Plugin instruments (VST3 / AU / CLAP) \
                         are planned but not supported yet.",
                    );
                });
            });
            ui.label(
                RichText::new("Add more sounds with instrument packs.")
                    .small()
                    .color(pal.text_dim),
            );
            ui.add_space(GAP_XS);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP_XS;
                if theme::text_button(
                    ui,
                    pal,
                    "Import pack…",
                    CONTROL_M,
                    "Validate and install a pack .json file",
                )
                .clicked()
                {
                    self.import_pack_dialog();
                }
                if theme::text_button(
                    ui,
                    pal,
                    "Reload",
                    CONTROL_M,
                    "Re-read installed packs now (Cadence also checks every 2 seconds)",
                )
                .clicked()
                {
                    self.reload_packs(true);
                }
                if theme::icon_button(
                    ui,
                    pal,
                    Icon::Folder,
                    CONTROL_M,
                    false,
                    pal.accent,
                    "Open the packs folder",
                )
                .clicked()
                {
                    self.reveal_packs_folder();
                }
            });
            let n = self.catalog.errors.len();
            if n > 0 {
                ui.add_space(GAP_XS);
                if theme::icon_label_button(
                    ui,
                    pal,
                    Icon::Warning,
                    &format!("{n} pack problem{} — details", plural(n)),
                    CONTROL_S,
                    "Show which pack files could not be loaded and why",
                )
                .clicked()
                {
                    self.show_pack_errors();
                }
            }
        });
    }

    /// The no-fonts hint card: explains what SoundFonts are and offers one
    /// "Add a SoundFont…" button that opens (and creates) the user font
    /// folder; the path itself lives in the button's tooltip.
    fn fonts_hint_card(&mut self, ui: &mut Ui, pal: Palette) {
        theme::card(pal).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::kicker_text(pal, "SOUNDFONT"));
            ui.label(
                RichText::new(
                    "SoundFonts (.sf2) play real sampled General MIDI \
                     instruments — pianos, guitars, strings and more.",
                )
                .small()
                .color(pal.text_dim),
            );
            ui.add_space(GAP_XS);
            let dir = crate::font::user_fonts_dir();
            let resp = theme::icon_label_button(
                ui,
                pal,
                Icon::Folder,
                "Add a SoundFont…",
                CONTROL_M,
                &format!(
                    "Open the SoundFonts folder so you can drop .sf2 files into it:\n{}",
                    dir.display()
                ),
            );
            self.library_rects.font_hint = resp.rect;
            if resp.clicked() {
                self.reveal_fonts_folder();
            }
        });
    }

    /// Opens the user font directory in the platform file manager (creating
    /// it first). Headless sessions describe the folder instead of spawning.
    fn reveal_fonts_folder(&mut self) {
        let dir = crate::font::user_fonts_dir();
        if !self.io_enabled {
            self.set_status(
                StatusKind::Info,
                format!("SoundFonts folder: {}", dir.display()),
            );
            return;
        }
        match crate::font::reveal_user_fonts_dir() {
            Ok(dir) => self.set_status(
                StatusKind::Success,
                format!(
                    "Opened the SoundFonts folder ({}). Drop .sf2 files there; they appear within a couple of seconds.",
                    dir.display()
                ),
            ),
            Err(err) => self.set_status(StatusKind::Error, err),
        }
    }

    /// Opens the user pack directory in the platform file manager (creating
    /// it first). Headless sessions describe the folder instead of spawning.
    fn reveal_packs_folder(&mut self) {
        let dir = crate::packs::user_packs_dir();
        if !self.io_enabled {
            self.set_status(StatusKind::Info, format!("Packs folder: {}", dir.display()));
            return;
        }
        let opened = std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))
            .and_then(|()| {
                open_in_file_manager(&dir)
                    .map_err(|e| format!("Could not open {}: {e}", dir.display()))
            });
        match opened {
            Ok(()) => self.set_status(
                StatusKind::Success,
                format!("Opened the packs folder ({}).", dir.display()),
            ),
            Err(err) => self.set_status(StatusKind::Error, err),
        }
    }

    /// Adds a new track playing GM `program` on `font`. The track starts on
    /// the piano roll (fonts only play pitched programs) and is named after
    /// the font, matching [`Track::from_font`]; its sound label reads
    /// "Font · Program".
    pub(super) fn add_font_track(&mut self, font: &FontRef) {
        let program_name = font.program_name().to_owned();
        let mut id = None;
        self.edit(&format!("Add {program_name} track"), None, |p| {
            let unique = ops::unique_name(&font.name, p.tracks.iter().map(|t| t.name.clone()));
            id = ops::add_track(p, Instrument::Lead);
            if let Some(track) = id {
                ops::set_track_font(p, track, Some(font.clone()));
                if let Some(t) = ops::track_mut(p, track) {
                    t.name = unique;
                }
            }
        });
        self.finish_track_add(
            id,
            format!(
                "Added {program_name} ({}). Draw notes in the piano roll; its clip is already in the arrangement at bar 1.",
                font.name
            ),
        );
    }

    pub(super) fn add_preset_track(&mut self, preset: &InstrumentPreset) {
        let mut id = None;
        self.edit(&format!("Add {} track", preset.name), None, |p| {
            id = ops::add_preset_track(p, preset)
        });
        let editor = if preset.kit.is_some() {
            "the drum grid"
        } else {
            "the piano roll"
        };
        self.finish_track_add(
            id,
            format!("Added {}. Draw notes in {editor}; its clip is already in the arrangement at bar 1.", preset.name),
        );
    }

    /// Adds a track for a library `sound` dropped on the arranger, inserting
    /// it at `index` (clamped into `0..=len`, so one past the last lane is a
    /// plain append). The add and the reposition ride the same [`Self::edit`]
    /// as the library's add buttons use, so the whole drop is a single undo
    /// step.
    pub(super) fn add_sound_track_at(&mut self, sound: &QuickSound, index: usize) {
        let name = sound.name().to_owned();
        let mut id = None;
        self.edit(&format!("Add {name} track"), None, |p| {
            id = match sound {
                QuickSound::BuiltIn(inst) => ops::add_track(p, *inst),
                QuickSound::Preset(preset) => ops::add_preset_track(p, preset),
                QuickSound::Font(font) => {
                    let unique =
                        ops::unique_name(&font.name, p.tracks.iter().map(|t| t.name.clone()));
                    let added = ops::add_track(p, Instrument::Lead);
                    if let Some(track) = added {
                        ops::set_track_font(p, track, Some(font.clone()));
                        if let Some(t) = ops::track_mut(p, track) {
                            t.name = unique;
                        }
                    }
                    added
                }
            };
            if let Some(track) = id {
                let last = p.tracks.len().saturating_sub(1);
                if index < last {
                    ops::move_track(p, track, index as i32 - last as i32);
                }
            }
        });
        let success = match sound {
            QuickSound::BuiltIn(_) => format!("Added a {name} track"),
            QuickSound::Preset(preset) => {
                let editor = if preset.kit.is_some() {
                    "the drum grid"
                } else {
                    "the piano roll"
                };
                format!("Added {name}. Draw notes in {editor}; its clip is already in the arrangement at bar 1.")
            }
            QuickSound::Font(_) => {
                format!("Added {name}. Draw notes in the piano roll; its clip is already in the arrangement at bar 1.")
            }
        };
        self.finish_track_add(id, success);
    }

    /// Selects, reveals and announces a just-added track — the shared tail of
    /// every add path (library rows, quick add, menus, arranger drops).
    /// `None` means the track limit refused the add.
    pub(super) fn finish_track_add(&mut self, id: Option<u64>, success: String) {
        match id {
            Some(id) => {
                self.select_track(id);
                self.tab = Tab::Editor;
                self.focus = Focus::Editor;
                self.arranger.reveal_track = Some(id);
                self.set_status(StatusKind::Success, success);
            }
            None => self.set_status(
                StatusKind::Warning,
                format!("Track limit reached ({} tracks).", ops::MAX_TRACKS),
            ),
        }
    }

    fn audition_instrument(&mut self, track: &Track, pitch: u8) {
        match &self.engine {
            Some(engine) => {
                if track.instrument.is_drums() && track.preset.is_none() {
                    engine.preview(track, 36, 0.4, 0.9);
                    engine.preview(track, 42, 0.2, 0.6);
                } else {
                    let (pitch, seconds) = audition_params(track, pitch);
                    engine.preview(track, pitch, seconds, 0.8);
                }
            }
            None => self.set_status(
                StatusKind::Warning,
                "No audio output device, so instruments can't be auditioned.",
            ),
        }
    }

    /// The "Add track" fast path at the top of the Track tab: a button that
    /// opens a grouped chooser of common sounds — the song's own sounds
    /// first, then built-ins, a few pack presets and SoundFont GM programs —
    /// so a track can be added without opening the full instrument library.
    /// Open state lives in egui temp data.
    fn quick_add_ui(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        let full = self.project.tracks.len() >= ops::MAX_TRACKS;
        let open = add_track_button(ui, pal, full);
        // Keep stale rects out of hit-tests when the strip is folded.
        ui.ctx().data_mut(|d| {
            d.insert_temp(Id::new(QUICK_ADD_FONT_RECT), Rect::NOTHING);
            d.insert_temp(Id::new(QUICK_ADD_FONT_HINT_RECT), Rect::NOTHING);
        });
        if !open {
            return;
        }
        ui.add_space(GAP_XS);
        let cached = self.quick_picks_cached(ui.ctx());
        let picks = &cached.picks;
        let mut picked = None;
        let mut font_row = Rect::NOTHING;
        let mut font_hint_row = Rect::NOTHING;
        let mut reveal = false;
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = GAP_XS;
            quick_section(
                ui,
                pal,
                "IN THIS SONG",
                &picks.recent,
                full,
                &mut picked,
                None,
            );
            quick_section(
                ui,
                pal,
                "BUILT-IN",
                &picks.builtins,
                full,
                &mut picked,
                None,
            );
            quick_section(ui, pal, "PACKS", &picks.presets, full, &mut picked, None);
            if picked.is_none() {
                if self.fonts.fonts.is_empty() {
                    // No .sf2 installed: a slim hint row that opens the folder.
                    let next = ui.available_rect_before_wrap();
                    reveal = quick_add_font_hint_row(ui, pal);
                    font_hint_row = Rect::from_min_size(next.min, vec2(next.width(), QUICK_H));
                } else {
                    quick_section(
                        ui,
                        pal,
                        "SOUNDFONT",
                        &picks.fonts,
                        full,
                        &mut picked,
                        Some(&mut font_row),
                    );
                }
            }
        });
        if reveal {
            self.reveal_fonts_folder();
        }
        ui.ctx().data_mut(|d| {
            d.insert_temp(Id::new(QUICK_ADD_FONT_RECT), font_row);
            d.insert_temp(Id::new(QUICK_ADD_FONT_HINT_RECT), font_hint_row);
        });
        if let Some(sound) = picked {
            match sound {
                QuickSound::BuiltIn(inst) => self.add_track(inst),
                QuickSound::Preset(p) => self.add_preset_track(&p),
                QuickSound::Font(f) => self.add_font_track(&f),
            }
        }
        ui.add_space(GAP_XS);
        if theme::icon_label_button(
            ui,
            pal,
            Icon::ChevronRight,
            "Browse the full library",
            CONTROL_S,
            "Every built-in, pack preset and SoundFont program, searchable (Instruments tab)",
        )
        .clicked()
        {
            self.side_tab = SideTab::Library;
        }
    }

    /// [`Self::quick_picks`], cached in egui temp data so the chooser doesn't
    /// rebuild (and deep-clone presets for) its picks every frame. The key
    /// covers everything the picks read: the song state, the pack catalog
    /// and the installed fonts.
    fn quick_picks_cached(&self, ctx: &Context) -> Arc<CachedPicks> {
        let mut h = DefaultHasher::new();
        self.history.state().hash(&mut h);
        self.project.tracks.len().hash(&mut h);
        self.project.tracks.last().map(|t| t.id).hash(&mut h);
        catalog_stamp(&self.catalog).hash(&mut h);
        for f in &self.fonts.fonts {
            f.name.hash(&mut h);
        }
        let key = h.finish();
        let id = Id::new(QUICK_PICKS_CACHE);
        if let Some(c) = ctx.data(|d| d.get_temp::<Arc<CachedPicks>>(id)) {
            if c.key == key {
                return c;
            }
        }
        let cached = Arc::new(CachedPicks {
            key,
            picks: self.quick_picks(),
        });
        ctx.data_mut(|d| d.insert_temp(id, cached.clone()));
        cached
    }

    /// The sounds [`Self::quick_add_ui`] offers, best first. `recent` mirrors
    /// the sounds already on tracks (newest track first) so layering another
    /// part is one click. Font tracks appear under recents while their font
    /// is installed here (they couldn't be re-created before the quick add
    /// learned to offer fonts); a font that isn't installed can't be
    /// re-created, so those tracks are skipped.
    fn quick_picks(&self) -> QuickPicks {
        const MAX_RECENT: usize = 4;
        const MAX_PRESETS: usize = 3;
        let mut recent: Vec<QuickSound> = Vec::new();
        let mut used_inst: Vec<Instrument> = Vec::new();
        let mut used_preset: Vec<String> = Vec::new();
        let mut used_font: Vec<FontRef> = Vec::new();
        for track in self.project.tracks.iter().rev() {
            if recent.len() >= MAX_RECENT {
                break;
            }
            if let Some(font) = &track.font {
                let installed = self.fonts.fonts.iter().any(|i| i.name == font.name);
                if installed && !used_font.contains(font) {
                    used_font.push(font.clone());
                    recent.push(QuickSound::Font(font.clone()));
                }
                continue;
            }
            if let Some(p) = &track.preset {
                if !used_preset.contains(&p.id) {
                    used_preset.push(p.id.clone());
                    recent.push(QuickSound::Preset(Box::new(p.clone())));
                }
            } else if !used_inst.contains(&track.instrument) {
                used_inst.push(track.instrument);
                recent.push(QuickSound::BuiltIn(track.instrument));
            }
        }
        let builtins = Instrument::ALL
            .iter()
            .filter(|i| !used_inst.contains(*i))
            .map(|i| QuickSound::BuiltIn(*i))
            .collect();
        // One preset per pack bank keeps the picks varied instead of three
        // sounds from the same pack.
        let mut banks: Vec<&str> = Vec::new();
        let mut presets = Vec::new();
        for p in &self.catalog.instruments {
            if presets.len() >= MAX_PRESETS {
                break;
            }
            if used_preset.contains(&p.id) {
                continue;
            }
            let ns = preset_namespace(&p.id);
            if banks.contains(&ns) {
                continue;
            }
            banks.push(ns);
            used_preset.push(p.id.clone());
            presets.push(QuickSound::Preset(Box::new(p.clone())));
        }
        // Iconic GM programs from an installed SoundFont — the font the song
        // already uses when it has one, else the first installed font.
        let font_name = used_font
            .first()
            .map(|f| f.name.clone())
            .or_else(|| self.fonts.fonts.first().map(|i| i.name.clone()));
        let fonts = font_name
            .into_iter()
            .flat_map(|name| {
                QUICK_FONT_PROGRAMS
                    .iter()
                    .map(move |program| QuickSound::Font(FontRef::new(name.clone(), *program)))
            })
            .collect();
        QuickPicks {
            recent,
            builtins,
            presets,
            fonts,
        }
    }
}

/// What the inspector reads from the selected track each frame: a cheap copy
/// of the scalar settings that leaves the notes and clips behind.
struct Inspected {
    id: u64,
    name: String,
    color: [u8; 3],
    volume: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    glide: f32,
    arp: ArpSettings,
    patterns: usize,
    clips: usize,
    /// Pitched sound: the Play section (glide, arpeggiator) only exists for
    /// these; drum kits and audio lanes have no melodic notes to bend or split.
    melodic: bool,
    sound: SoundCtx,
}

impl Inspected {
    fn of(t: &Track) -> Self {
        Self {
            id: t.id,
            name: t.name.clone(),
            color: t.color,
            volume: t.volume,
            pan: t.pan,
            mute: t.mute,
            solo: t.solo,
            glide: t.glide,
            arp: t.arp,
            patterns: t.patterns.len(),
            clips: t.clips.len(),
            melodic: !t.audio && ops::arp_audible(t),
            sound: SoundCtx::of(t),
        }
    }
}

/// What the sound picker and a sound change need to know about a track.
struct SoundCtx {
    id: u64,
    name: String,
    instrument: Instrument,
    preset: Option<InstrumentPreset>,
    font: Option<FontRef>,
    /// Label of the active sound ("Studio drums", "GeneralUser · Violin").
    label: String,
    drum_grid: bool,
}

impl SoundCtx {
    fn of(t: &Track) -> Self {
        Self {
            id: t.id,
            name: t.name.clone(),
            instrument: t.instrument,
            preset: t.preset.clone(),
            font: t.font.clone(),
            label: ops::instrument_label(t).into_owned(),
            drum_grid: t.shows_drum_grid(),
        }
    }

    /// Whether the track still carries its auto-assigned sound name — the
    /// built-in/preset label or the font name — rather than a user-chosen
    /// one. Sound changes rename the track only while this holds.
    fn has_default_name(&self) -> bool {
        self.label == self.name || self.font.as_ref().is_some_and(|f| f.name == self.name)
    }
}

impl CadenceApp {
    fn inspector_ui(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        self.quick_add_ui(ui);
        ui.add_space(GAP_M);
        let Some(t) = self.selected_track().map(Inspected::of) else {
            theme::empty_state(
                ui,
                pal,
                Icon::Note,
                "No track selected",
                "Click a track in the arrangement, or add one with “Add track”.",
                None,
            );
            return;
        };
        let id = t.id;
        let color = theme::rgb(t.color);

        // ----- Track ---------------------------------------------------------
        inspector_heading(ui, pal, "TRACK");
        let mut name = t.name.clone();
        let renamed = field_row(ui, pal, "Name", |ui, w| {
            ui.add(
                egui::TextEdit::singleline(&mut name)
                    .desired_width(w)
                    .min_size(vec2(w, CONTROL_M))
                    .vertical_align(Align::Center)
                    .margin(Margin::symmetric(8, 4))
                    .char_limit(crate::model::MAX_NAME_CHARS),
            )
            .changed()
        });
        if renamed {
            self.edit("Rename track", Some(&format!("track-name-{id}")), |p| {
                if let Some(t) = ops::track_mut(p, id) {
                    t.name = name;
                }
            });
        }
        field_row(ui, pal, "Sound", |ui, w| self.sound_picker(ui, &t.sound, w));
        self.font_program_picker(ui, &t.sound);
        if let Some(preset) = &t.sound.preset {
            let summary = if t.sound.font.is_some() {
                // With a font assigned the preset only plays when the .sf2
                // is missing — say so instead of implying it is the sound.
                format!(
                    "Fallback while the SoundFont is missing: {}",
                    preset_summary(preset)
                )
            } else {
                preset_summary(preset)
            };
            indented(ui, |ui, w| {
                theme::truncated_label(ui, RichText::new(summary).small().color(pal.text_faint), w);
            });
            let installed = self.catalog.instruments.iter().find(|p| p.id == preset.id);
            match installed {
                Some(newer) if newer != preset => {
                    let p = newer.clone();
                    let update = indented(ui, |ui, _| {
                        theme::text_button(
                            ui,
                            pal,
                            "Update from installed pack",
                            CONTROL_S,
                            "The installed pack has a different version of this preset. The song keeps its embedded copy until you update.",
                        )
                        .clicked()
                    });
                    if update {
                        self.edit("Update preset", None, |pr| {
                            if let Some(t) = ops::track_mut(pr, id) {
                                t.preset = Some(p);
                            }
                        });
                    }
                }
                None => {
                    indented(ui, |ui, w| {
                        theme::truncated_label(
                            ui,
                            RichText::new("Pack not installed here; this song keeps its own copy.")
                                .small()
                                .color(pal.text_faint),
                            w,
                        );
                    });
                }
                _ => {}
            }
        }
        self.soundfonts_row(ui, pal);
        let picked = field_row(ui, pal, "Color", |ui, w| {
            color_swatches(ui, pal, t.color, w)
        });
        if let Some(c) = picked {
            self.edit("Track color", None, |p| {
                if let Some(t) = ops::track_mut(p, id) {
                    t.color = c;
                }
            });
        }

        // ----- Mix -----------------------------------------------------------
        ui.add_space(GAP_M);
        inspector_heading(ui, pal, "MIX");
        let mut vol = t.volume;
        let vol_changed = field_row(ui, pal, "Volume", |ui, w| {
            let changed = theme::slider_with(
                ui,
                pal,
                &mut vol,
                0.0..=1.0,
                (w - READOUT_W - GAP_S).max(60.0),
                color,
                None,
            )
            .changed();
            readout(ui, pal, &format!("{:.0}%", vol * 100.0));
            changed
        });
        if vol_changed {
            self.edit("Track volume", Some(&format!("vol-{id}")), |p| {
                if let Some(t) = ops::track_mut(p, id) {
                    t.volume = vol;
                }
            });
        }
        let mut pan = t.pan;
        let pan_changed = field_row(ui, pal, "Pan", |ui, w| {
            let changed = theme::slider_with(
                ui,
                pal,
                &mut pan,
                -1.0..=1.0,
                (w - READOUT_W - GAP_S).max(60.0),
                color,
                Some(0.0),
            )
            .changed();
            readout(ui, pal, &pan_text(pan));
            changed
        });
        if pan_changed {
            self.edit("Track pan", Some(&format!("pan-{id}")), |p| {
                if let Some(t) = ops::track_mut(p, id) {
                    t.pan = pan;
                }
            });
        }
        let (mute, solo) = indented(ui, |ui, _| {
            let size = vec2(CONTROL_S, CONTROL_S);
            let mute = theme::chip(
                ui,
                pal,
                "M",
                t.mute,
                pal.danger,
                size,
                "Mute: silence this track",
            )
            .clicked();
            let solo = theme::chip(
                ui,
                pal,
                "S",
                t.solo,
                pal.warn,
                size,
                "Solo: hear only soloed tracks",
            )
            .clicked();
            (mute, solo)
        });
        if mute {
            self.toggle_mute(id);
        }
        if solo {
            self.toggle_solo(id);
        }

        // ----- Effects -------------------------------------------------------
        ui.add_space(GAP_M);
        inspector_heading(ui, pal, "EFFECTS");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = knob_gap(ui.available_width());
            self.fx_knobs(ui, id, KNOB, color);
        });

        // ----- Play (pitched sounds only) ------------------------------------
        if t.melodic {
            ui.add_space(GAP_M);
            inspector_heading(ui, pal, "PLAY");
            self.play_section(ui, pal, &t, color);
        }

        // ----- Manage --------------------------------------------------------
        ui.add_space(GAP_M);
        inspector_heading(ui, pal, "MANAGE");
        self.manage_section(ui, pal, &t);
        ui.add_space(GAP_M);
    }

    /// The SoundFonts row: where to drop `.sf2` files, always one click away,
    /// plus a warning when a font file failed to load.
    fn soundfonts_row(&mut self, ui: &mut Ui, pal: Palette) {
        let dir = crate::font::user_fonts_dir();
        let tip = format!(
            "Drop .sf2 files into this folder; they appear in the Sound menu.\n{}",
            dir.display()
        );
        let open = ui
            .horizontal(|ui| {
                ui.set_height(CONTROL_M);
                ui.label(RichText::new("SoundFonts").color(pal.text_dim))
                    .on_hover_text(&tip);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    theme::icon_label_button(ui, pal, Icon::Folder, "Open folder", CONTROL_S, &tip)
                        .clicked()
                })
                .inner
            })
            .inner;
        if open {
            self.reveal_fonts_folder();
        }
        let n = self.fonts.errors.len();
        if n > 0 {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP_XS;
                let (r, _) = ui.allocate_exact_size(vec2(ICON_S, ICON_S), Sense::hover());
                theme::paint_icon(ui.painter(), r, Icon::Warning, pal.warn);
                ui.label(
                    RichText::new(format!("{n} font file{} failed to load", plural(n)))
                        .small()
                        .color(pal.warn),
                )
                .on_hover_text(self.fonts.errors.join("\n"));
            });
        }
    }

    /// Glide and the arpeggiator. Both are pitched-sound features, so the
    /// whole section is skipped for drum kits and audio lanes; the
    /// explanations live in tooltips.
    fn play_section(&mut self, ui: &mut Ui, pal: Palette, t: &Inspected, color: Color32) {
        let id = t.id;
        let arp = t.arp;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = knob_gap(ui.available_width());
            // Per-track portamento, seconds 0..=MAX_GLIDE_SECONDS (0 = off):
            // while on, melodic sounds retarget the held voice legato-style
            // instead of stacking — one note at a time (Track::glide).
            let mut norm = (t.glide / MAX_GLIDE_SECONDS).clamp(0.0, 1.0);
            let tip = if t.sound.font.is_some() {
                "Glide (portamento): how long a held note takes to slide to the next pitch. \
                 SoundFonts don't bend, so it only affects the fallback sound used while the font is missing. \
                 Drag · Shift for fine · Double-click resets to off."
            } else {
                "Glide (portamento): how long a held note takes to slide to the next pitch. \
                 One note at a time while above 0 s. Drag · Shift for fine · Double-click resets to off."
            };
            if theme::knob(
                ui,
                pal,
                &mut norm,
                0.0,
                false,
                "Glide",
                &glide_text(t.glide),
                color,
                KNOB,
            )
            .on_hover_text(tip)
            .changed()
            {
                let seconds = (norm * MAX_GLIDE_SECONDS).clamp(0.0, MAX_GLIDE_SECONDS);
                self.edit("Track glide", Some(&format!("glide-{id}")), |p| {
                    if let Some(t) = ops::track_mut(p, id) {
                        t.glide = seconds;
                    }
                });
                self.set_status(StatusKind::Info, format!("Glide: {}", glide_text(seconds)));
            }
            // While the arpeggiator runs, its gate knob sits next to Glide:
            // hit length as a fraction of the hit rate (90% default).
            if arp.enabled {
                let mut gate_norm =
                    ((arp.gate - MIN_ARP_GATE) / (1.0 - MIN_ARP_GATE)).clamp(0.0, 1.0);
                let gate_default = (0.9 - MIN_ARP_GATE) / (1.0 - MIN_ARP_GATE);
                if theme::knob(
                    ui,
                    pal,
                    &mut gate_norm,
                    gate_default,
                    false,
                    "Gate",
                    &format!("{:.0}%", arp.gate * 100.0),
                    color,
                    KNOB,
                )
                .on_hover_text(
                    "Arp gate: how long each arpeggio hit sounds, as a fraction of its rate. \
                     Drag · Shift for fine · Double-click resets to 90%.",
                )
                .changed()
                {
                    let gate = (MIN_ARP_GATE + gate_norm * (1.0 - MIN_ARP_GATE))
                        .clamp(MIN_ARP_GATE, 1.0);
                    self.edit("Track arpeggiator", Some(&format!("arp-gate-{id}")), |p| {
                        if let Some(t) = ops::track_mut(p, id) {
                            t.arp.gate = gate;
                        }
                    });
                    self.set_status(
                        StatusKind::Info,
                        format!("Arp gate: {:.0}%", gate * 100.0),
                    );
                }
            }
        });

        // Per-track live arpeggiator (Track::arp): while on, every chord on
        // the timeline (2+ notes starting together) subdivides into a
        // cycling run of hits; single notes play unchanged. Controls collect
        // a pick each so the undoable edit lands after the widgets.
        ui.add_space(GAP_XS);
        let mut toggle_arp = false;
        let mut rate_pick = None;
        let mut octave_pick = None;
        let mut direction_pick = None;
        ui.horizontal(|ui| {
            toggle_arp = theme::icon_toggle_button(
                ui,
                pal,
                Icon::Arp,
                "Arpeggiator",
                arp.enabled,
                CONTROL_M,
                "Arpeggiator: split held chords into a cycling note run. Single notes play unchanged.",
            )
            .clicked();
        });
        if arp.enabled {
            ui.add_space(GAP_XS);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP_XS;
                let w = ((ui.available_width() - 2.0 * GAP_XS) / 3.0).floor();
                theme::combo(("arp-rate", id), arp.rate.label(), w)
                    .truncate()
                    .show_ui(ui, |ui| {
                        for rate in ArpRate::ALL {
                            if ui
                                .selectable_label(rate == arp.rate, rate.label())
                                .on_hover_text(format!("{} notes", rate.note_name()))
                                .clicked()
                            {
                                rate_pick = Some(rate);
                            }
                        }
                    })
                    .response
                    .on_hover_text("Arpeggio rate");
                theme::combo(("arp-octaves", id), format!("{} oct", arp.octaves), w)
                    .truncate()
                    .show_ui(ui, |ui| {
                        for octaves in 1..=MAX_ARP_OCTAVES {
                            if ui
                                .selectable_label(octaves == arp.octaves, format!("{octaves} oct"))
                                .on_hover_text("Octaves the chord tones span upward")
                                .clicked()
                            {
                                octave_pick = Some(octaves);
                            }
                        }
                    })
                    .response
                    .on_hover_text("Octaves the chord tones span upward");
                theme::combo(("arp-direction", id), arp.direction.label(), w)
                    .truncate()
                    .show_ui(ui, |ui| {
                        for direction in ArpDirection::ALL {
                            if ui
                                .selectable_label(direction == arp.direction, direction.label())
                                .clicked()
                            {
                                direction_pick = Some(direction);
                            }
                        }
                    })
                    .response
                    .on_hover_text("Arpeggio direction");
            });
        }
        if toggle_arp {
            let on = !arp.enabled;
            self.edit("Track arpeggiator", None, |p| {
                if let Some(t) = ops::track_mut(p, id) {
                    t.arp.enabled = on;
                }
            });
            self.set_status(
                StatusKind::Info,
                format!("Arpeggiator: {}", if on { "on" } else { "off" }),
            );
        }
        if let Some(rate) = rate_pick {
            self.edit("Track arpeggiator", None, |p| {
                if let Some(t) = ops::track_mut(p, id) {
                    t.arp.rate = rate;
                }
            });
            self.set_status(
                StatusKind::Info,
                format!("Arp rate: {} notes.", rate.note_name()),
            );
        }
        if let Some(octaves) = octave_pick {
            self.edit("Track arpeggiator", None, |p| {
                if let Some(t) = ops::track_mut(p, id) {
                    t.arp.octaves = octaves;
                }
            });
            self.set_status(StatusKind::Info, format!("Arp octaves: {octaves}."));
        }
        if let Some(direction) = direction_pick {
            self.edit("Track arpeggiator", None, |p| {
                if let Some(t) = ops::track_mut(p, id) {
                    t.arp.direction = direction;
                }
            });
            self.set_status(
                StatusKind::Info,
                format!("Arp direction: {}.", direction.label()),
            );
        }
    }

    /// Duplicate, reorder and delete (danger, last).
    fn manage_section(&mut self, ui: &mut Ui, pal: Palette, t: &Inspected) {
        let id = t.id;
        ui.label(
            RichText::new(format!(
                "{} pattern{} · {} clip{}",
                t.patterns,
                plural(t.patterns),
                t.clips,
                plural(t.clips)
            ))
            .small()
            .color(pal.text_faint),
        );
        ui.add_space(GAP_XS);
        let can_add = self.project.tracks.len() < ops::MAX_TRACKS;
        let idx = ops::track_index(&self.project, id).unwrap_or(0);
        let can_down = idx + 1 < self.project.tracks.len();
        let (dup, up, down) = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP_XS;
                let dup = ui
                    .add_enabled_ui(can_add, |ui| {
                        theme::icon_label_button(
                            ui,
                            pal,
                            Icon::Duplicate,
                            "Duplicate",
                            CONTROL_M,
                            if can_add {
                                "Copy this track with all its patterns and clips"
                            } else {
                                "Track limit reached"
                            },
                        )
                        .clicked()
                    })
                    .inner;
                let up = ui
                    .add_enabled_ui(idx > 0, |ui| {
                        theme::icon_button(
                            ui,
                            pal,
                            Icon::ChevronUp,
                            CONTROL_M,
                            false,
                            pal.accent,
                            "Move track up",
                        )
                        .clicked()
                    })
                    .inner;
                let down = ui
                    .add_enabled_ui(can_down, |ui| {
                        theme::icon_button(
                            ui,
                            pal,
                            Icon::ChevronDown,
                            CONTROL_M,
                            false,
                            pal.accent,
                            "Move track down",
                        )
                        .clicked()
                    })
                    .inner;
                (dup, up, down)
            })
            .inner;
        ui.add_space(GAP_XS);
        let delete_tip = format!(
            "Delete this track (undo with {})",
            ui.ctx().format_shortcut(&super::SC_UNDO)
        );
        let delete = danger_button(ui, pal, Icon::Trash, "Delete track", &delete_tip);
        if dup {
            let mut new = None;
            self.edit("Duplicate track", None, |p| {
                new = ops::duplicate_track(p, id)
            });
            if let Some(new) = new {
                self.select_track(new);
            }
        }
        if up {
            self.edit("Move track", None, |p| {
                ops::move_track(p, id, -1);
            });
        }
        if down {
            self.edit("Move track", None, |p| {
                ops::move_track(p, id, 1);
            });
        }
        if delete {
            self.delete_track(id);
        }
    }

    /// The sound combo: built-ins, pack presets and the SoundFont slice, plus
    /// the GM program combo while a font is assigned.
    fn sound_picker(&mut self, ui: &mut Ui, sound: &SoundCtx, width: f32) {
        let pal = self.palette();
        let id = sound.id;
        let mut sound_pick: Option<(Instrument, Option<InstrumentPreset>)> = None;
        let mut font_pick: Option<FontPick> = None;
        theme::combo(("sound", id), sound.label.clone(), width)
            .truncate()
            .height(360.0)
            .show_ui(ui, |ui| {
                ui.label(theme::kicker_text(pal, "BUILT-IN"));
                for inst in Instrument::ALL {
                    // The check mark shows the *active* sound: while a font
                    // is assigned, built-ins are only the fallback.
                    let on =
                        sound.font.is_none() && sound.preset.is_none() && sound.instrument == inst;
                    if ui
                        .selectable_label(on, inst.name())
                        .on_hover_text(inst.description())
                        .clicked()
                    {
                        sound_pick = Some((inst, None));
                    }
                }
                let mut pack = String::new();
                for preset in &self.catalog.instruments {
                    let title = pack_title(&preset.id);
                    if title != pack {
                        ui.separator();
                        ui.label(theme::kicker_text(pal, &title.to_uppercase()));
                        pack = title;
                    }
                    let on = sound.font.is_none()
                        && sound.preset.as_ref().is_some_and(|p| p.id == preset.id);
                    if ui
                        .selectable_label(on, &preset.name)
                        .on_hover_text(&preset.description)
                        .clicked()
                    {
                        // Kits become Drums tracks (drum grid); melodic presets stay Lead.
                        let inst = if preset.kit.is_some() {
                            Instrument::Drums
                        } else {
                            Instrument::Lead
                        };
                        sound_pick = Some((inst, Some(preset.clone())));
                    }
                }
                self.font_section(ui, sound, &mut font_pick);
            });
        if let Some(pick) = font_pick {
            let font = match pick {
                FontPick::Assign(f) => Some(f),
                FontPick::Clear => None,
            };
            self.apply_font_pick_ctx(sound, font);
        }
        if let Some((inst, preset)) = sound_pick {
            self.apply_sound_pick_ctx(sound, inst, preset);
        }
    }

    /// Applies a built-in/preset pick from the sound combo. Choosing either
    /// clears the SoundFont reference: an explicit preset/built-in choice is
    /// the sound the user asked for, and a lingering font would keep
    /// overriding it (fonts win over presets at playback).
    #[cfg(test)]
    pub(super) fn apply_sound_pick(
        &mut self,
        track: &Track,
        inst: Instrument,
        preset: Option<InstrumentPreset>,
    ) {
        self.apply_sound_pick_ctx(&SoundCtx::of(track), inst, preset);
    }

    fn apply_sound_pick_ctx(
        &mut self,
        track: &SoundCtx,
        inst: Instrument,
        preset: Option<InstrumentPreset>,
    ) {
        let id = track.id;
        let was_drums = track.drum_grid;
        let label = preset
            .as_ref()
            .map_or(inst.name().to_owned(), |p| p.name.clone());
        let default_name = track.has_default_name();
        self.edit("Change sound", None, |p| {
            if let Some(t) = ops::track_mut(p, id) {
                t.instrument = inst;
                t.preset = preset;
                t.font = None;
                if default_name {
                    t.name = label.clone();
                }
            }
        });
        self.after_sound_change(was_drums);
    }

    /// Applies a SoundFont choice: `Some(font)` assigns the reference via
    /// [`ops::set_track_font`] (which keeps the preset as the missing-font
    /// fallback), `None` clears it so the preset/built-in sound plays again.
    #[cfg(test)]
    pub(super) fn apply_font_pick(&mut self, track: &Track, font: Option<FontRef>) {
        self.apply_font_pick_ctx(&SoundCtx::of(track), font);
    }

    fn apply_font_pick_ctx(&mut self, track: &SoundCtx, font: Option<FontRef>) {
        let id = track.id;
        let was_drums = track.drum_grid;
        let default_name = track.has_default_name();
        let new_name = match &font {
            // Track::from_font names font tracks after the font itself.
            Some(f) => f.name.clone(),
            // Clearing falls back to the preset/built-in sound's own name.
            None => track
                .preset
                .as_ref()
                .map_or_else(|| track.instrument.name().to_owned(), |p| p.name.clone()),
        };
        self.edit("Change sound", None, |p| {
            if ops::set_track_font(p, id, font) && default_name {
                if let Some(t) = ops::track_mut(p, id) {
                    t.name = new_name.clone();
                }
            }
        });
        self.after_sound_change(was_drums);
    }

    /// Post-edit bookkeeping shared by every sound change: reports when the
    /// editor flipped between drum grid and piano roll, then auditions the
    /// new sound.
    fn after_sound_change(&mut self, was_drums: bool) {
        let is_drums = self.selected_track().is_some_and(|t| t.shows_drum_grid());
        if was_drums != is_drums {
            self.set_status(
                StatusKind::Info,
                if is_drums {
                    "Now a drum track: the editor shows the step sequencer."
                } else {
                    "Now a melodic track: the editor shows the piano roll."
                },
            );
        }
        if let Some(t) = self.selected_track().cloned() {
            let pitch = if t.instrument.is_drums() && t.preset.is_none() && t.font.is_none() {
                36
            } else {
                60
            };
            self.audition_instrument(&t, pitch);
        }
    }

    /// The SOUNDFONT slice of the sound picker popup. "None" clears the font
    /// reference; each installed font assigns it — re-picking the active font
    /// keeps its program, a different font starts on program 0. A font the
    /// song references but this machine doesn't have is listed too, so the
    /// reference can be seen and cleared (playback already falls back
    /// automatically).
    fn font_section(&self, ui: &mut Ui, track: &SoundCtx, pick: &mut Option<FontPick>) {
        let pal = self.palette();
        ui.separator();
        ui.label(theme::kicker_text(pal, "SOUNDFONT"));
        let catalog_empty = self.fonts.fonts.is_empty();
        if catalog_empty {
            theme::hint(
                ui,
                pal,
                "No SoundFonts installed yet. Use “Open folder” under Sound to add some.",
            );
        }
        // The clear action doubles as the "no font" row at the top of a
        // populated list; with an empty catalog it still shows while a font
        // is assigned, so the reference can be removed.
        if !catalog_empty || track.font.is_some() {
            let on = track.font.is_none();
            if ui
                .selectable_label(on, "None — built-in / pack sound")
                .on_hover_text("Remove the SoundFont reference; the preset or built-in sound plays")
                .clicked()
            {
                *pick = Some(FontPick::Clear);
            }
        }
        for info in &self.fonts.fonts {
            let on = track.font.as_ref().is_some_and(|f| f.name == info.name);
            if ui
                .selectable_label(on, &info.name)
                .on_hover_text(format!("SoundFont · {} presets", info.presets))
                .clicked()
            {
                // Re-picking the active font keeps its program; a
                // different font starts on program 0.
                let program = track
                    .font
                    .as_ref()
                    .filter(|f| f.name == info.name)
                    .map_or(0, |f| f.program);
                *pick = Some(FontPick::Assign(FontRef::new(info.name.clone(), program)));
            }
        }
        // A font the song references but this machine doesn't have — shown
        // even with an empty catalog so the reference can be seen and
        // cleared (playback already falls back automatically).
        if let Some(font) = &track.font {
            if !self.fonts.fonts.iter().any(|i| i.name == font.name) {
                ui.label(
                    RichText::new(format!("{} — not installed here", font.name))
                        .small()
                        .color(pal.warn),
                );
                ui.label(
                    RichText::new("Its fallback sound plays until the .sf2 is added.")
                        .small()
                        .color(pal.text_faint),
                );
            }
        }
        if !self.fonts.errors.is_empty() {
            let n = self.fonts.errors.len();
            ui.label(
                RichText::new(format!("{n} font file{} failed to load", plural(n)))
                    .small()
                    .color(pal.warn),
            )
            .on_hover_text(self.fonts.errors.join("\n"));
        }
    }

    /// The GM program row shown under the sound picker while a SoundFont is
    /// assigned: all 128 programs grouped under their 16 family headers.
    fn font_program_picker(&mut self, ui: &mut Ui, track: &SoundCtx) {
        let Some(font) = track.font.clone() else {
            return;
        };
        let pal = self.palette();
        let id = track.id;
        let mut pick = None;
        field_row(ui, pal, "Program", |ui, w| {
            theme::combo(("font-program", id), font.program_name(), w)
                .truncate()
                .height(360.0)
                .show_ui(ui, |ui| {
                    for (family, programs) in gm_program_catalog() {
                        ui.separator();
                        ui.label(theme::kicker_text(pal, &family.to_uppercase()));
                        for (program, label) in programs {
                            if ui
                                .selectable_label(font.program == program, label)
                                .on_hover_text(format!("GM program {program} · {family}"))
                                .clicked()
                            {
                                pick = Some(program);
                            }
                        }
                    }
                });
        });
        if let Some(program) = pick {
            self.edit("Change font program", None, |p| {
                ops::set_track_font(p, id, Some(FontRef::new(font.name.clone(), program)));
            });
            if let Some(t) = self.selected_track().cloned() {
                self.audition_instrument(&t, 60);
            }
        }
    }

    pub(super) fn toggle_mute(&mut self, id: u64) {
        self.edit("Mute", None, |p| {
            if let Some(t) = ops::track_mut(p, id) {
                t.mute = !t.mute;
            }
        });
    }

    pub(super) fn toggle_solo(&mut self, id: u64) {
        self.edit("Solo", None, |p| {
            if let Some(t) = ops::track_mut(p, id) {
                t.solo = !t.solo;
            }
        });
    }

    pub(super) fn delete_track(&mut self, id: u64) {
        let name = ops::track(&self.project, id)
            .map(|t| t.name.clone())
            .unwrap_or_default();
        if self.edit("Delete track", None, |p| {
            ops::delete_track(p, id);
        }) {
            self.fix_selection();
            self.set_status(
                StatusKind::Info,
                format!(
                    "Deleted track \u{201c}{name}\u{201d}. Undo with {}.",
                    if cfg!(target_os = "macos") {
                        "Cmd+Z"
                    } else {
                        "Ctrl+Z"
                    }
                ),
            );
        }
    }
}

/// Layout facts the headless tests read back from the real frame code.
#[cfg(test)]
#[derive(Default)]
struct TestLog {
    /// Rect of every library card laid out since the last reset.
    rows: Vec<Rect>,
    /// Rect and title of every in-flow section header.
    headers: Vec<(Rect, String)>,
    /// Title of the section header pinned to the top of the list, if any.
    sticky: Option<String>,
}

#[cfg(test)]
thread_local! {
    static TEST_LOG: std::cell::RefCell<TestLog> = std::cell::RefCell::new(TestLog::default());
}

// ----- library view cache -----------------------------------------------------

/// One row of a library section.
#[derive(Clone, Copy, Debug, PartialEq)]
enum LibItem {
    BuiltIn(Instrument),
    /// Index into `PackCatalog::instruments`.
    Preset(usize),
    /// GM program number and name.
    Font(u8, &'static str),
}

/// A collapsible group of library rows: the built-ins, one pack bank, or one
/// GM instrument family.
#[derive(Debug)]
struct Section {
    /// Stable key for the collapsed-state memory ("builtin", "pack:psy-bass").
    key: String,
    title: String,
    count: usize,
    items: Vec<LibItem>,
}

/// The filtered, grouped library for one (catalog, bank, search) triple. Built
/// once and cached in egui temp data, so a frame only walks the sections.
#[derive(Debug)]
struct LibraryView {
    stamp: u64,
    bank: LibraryBank,
    query: String,
    sections: Vec<Section>,
}

/// Cheap fingerprint of the installed pack catalog: the preset ids in order.
/// Changes whenever a preset is added, removed or reordered, which is exactly
/// when cached row indices stop being valid.
fn catalog_stamp(catalog: &PackCatalog) -> u64 {
    let mut h = DefaultHasher::new();
    catalog.instruments.len().hash(&mut h);
    for p in &catalog.instruments {
        p.id.hash(&mut h);
    }
    h.finish()
}

fn build_view(catalog: &PackCatalog, bank: &LibraryBank, query: &str, stamp: u64) -> LibraryView {
    let mut sections: Vec<Section> = Vec::new();
    if *bank == LibraryBank::Font {
        for (family, programs) in font_rows(query) {
            sections.push(Section {
                key: format!("font:{family}"),
                title: family.to_uppercase(),
                count: programs.len(),
                items: programs
                    .into_iter()
                    .map(|(program, name)| LibItem::Font(program, name))
                    .collect(),
            });
        }
    } else {
        let (builtins, presets) = library_row_indices(bank, query, catalog);
        if !builtins.is_empty() {
            sections.push(Section {
                key: "builtin".to_owned(),
                title: "BUILT-IN".to_owned(),
                count: builtins.len(),
                items: builtins.into_iter().map(LibItem::BuiltIn).collect(),
            });
        }
        let mut last_ns: Option<&str> = None;
        for i in presets {
            let ns = preset_namespace(&catalog.instruments[i].id);
            if last_ns != Some(ns) {
                last_ns = Some(ns);
                sections.push(Section {
                    key: format!("pack:{ns}"),
                    title: format!("PACK · {}", pack_title(ns).to_uppercase()),
                    count: 0,
                    items: Vec::new(),
                });
            }
            if let Some(section) = sections.last_mut() {
                section.items.push(LibItem::Preset(i));
                section.count += 1;
            }
        }
    }
    LibraryView {
        stamp,
        bank: bank.clone(),
        query: query.to_owned(),
        sections,
    }
}

/// The cached [`LibraryView`] for the current filter; rebuilt only when the
/// catalog, the bank or the (trimmed, lowercased) search text changed.
fn library_view(
    ctx: &Context,
    catalog: &PackCatalog,
    bank: &LibraryBank,
    query: &str,
) -> Arc<LibraryView> {
    let stamp = catalog_stamp(catalog);
    let id = Id::new(LIBRARY_CACHE);
    if let Some(view) = ctx.data(|d| d.get_temp::<Arc<LibraryView>>(id)) {
        if view.stamp == stamp && view.bank == *bank && view.query == query {
            return view;
        }
    }
    let view = Arc::new(build_view(catalog, bank, query, stamp));
    ctx.data_mut(|d| d.insert_temp(id, view.clone()));
    view
}

/// [`QuickPicks`] plus the key it was built for (see
/// `CadenceApp::quick_picks_cached`).
struct CachedPicks {
    key: u64,
    picks: QuickPicks,
}

fn library_item_id(base: Id, item: &LibItem, catalog: &PackCatalog) -> Id {
    match item {
        LibItem::BuiltIn(inst) => base.with(("library-row", inst.name())),
        LibItem::Preset(i) => base.with(("library-row", &catalog.instruments[*i].id)),
        LibItem::Font(program, _) => base.with(("library-row", "font", program)),
    }
}

/// Shortens the search text for the "No sounds match" title.
fn elide_query(query: &str) -> String {
    if query.chars().count() > 24 {
        query.chars().take(23).collect::<String>() + "…"
    } else {
        query.to_owned()
    }
}

/// Opens `dir` in the platform file manager without waiting for it.
fn open_in_file_manager(dir: &std::path::Path) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = std::process::Command::new(opener).arg(dir).spawn()?;
    // Reap the opener so it never lingers as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

// ----- library chrome ---------------------------------------------------------

/// Library title row: a disclosure chevron, "Library", and the track usage
/// ("5 / 32 tracks"). The whole row toggles the list; `LIBRARY_OPEN` keeps
/// the state in egui temp data. Returns whether the list is open.
fn library_header(ui: &mut Ui, pal: Palette, used: usize, full: bool) -> bool {
    let id = Id::new(LIBRARY_OPEN);
    let mut open = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(true);
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), CONTROL_M), Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, open, "Library")
    });
    if resp.clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_temp(id, open));
    }
    let hot = resp.hovered();
    let p = ui.painter();
    theme::paint_icon(
        p,
        Rect::from_center_size(
            pos2(rect.left() + 8.0, rect.center().y),
            Vec2::splat(ICON_S),
        ),
        if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        if hot { pal.text } else { pal.text_dim },
    );
    let right = format!("{used} / {} tracks", ops::MAX_TRACKS);
    let right_color = if full { pal.danger } else { pal.text_faint };
    let galley = ui
        .painter()
        .layout_no_wrap(right, theme::body_font(TEXT_SMALL), right_color);
    let right_w = galley.size().x;
    ui.painter().galley(
        pos2(
            rect.right() - right_w,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        right_color,
    );
    ui.painter().text(
        pos2(rect.left() + 22.0, rect.center().y),
        Align2::LEFT_CENTER,
        "Library",
        theme::strong_font(ui, TEXT_TITLE),
        pal.text,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!(
            "{} the instrument list · {used} of {} tracks used in this song",
            if open { "Collapse" } else { "Expand" },
            ops::MAX_TRACKS
        ));
    open
}

/// Search box: magnifier, hint text, and a clear button while there is text.
/// Returns its rect (headless tests click it).
fn search_field(ui: &mut Ui, pal: Palette, text: &mut String, hint: &str) -> Rect {
    let id = Id::new(SEARCH_FIELD);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), CONTROL_M), Sense::hover());
    let focused = ui.memory(|m| m.has_focus(id));
    let bg = ui.painter().add(egui::Shape::Noop);
    let clear_w = if text.is_empty() {
        0.0
    } else {
        CONTROL_S + 2.0
    };
    let edit_rect = Rect::from_min_max(
        pos2(rect.left() + 28.0, rect.top()),
        pos2(rect.right() - clear_w - 4.0, rect.bottom()),
    );
    let mut edit_ui = ui.new_child(
        UiBuilder::new()
            .id_salt("search-edit")
            .max_rect(edit_rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    edit_ui.add(
        egui::TextEdit::singleline(text)
            .id(id)
            .frame(false)
            .hint_text(RichText::new(hint).color(pal.text_faint))
            .margin(Margin::ZERO)
            .vertical_align(Align::Center)
            .desired_width(edit_rect.width())
            .min_size(edit_rect.size()),
    );
    ui.painter().set(
        bg,
        egui::epaint::RectShape::new(
            rect,
            egui::CornerRadius::same(theme::RADIUS_M as u8),
            pal.inset,
            Stroke::new(
                1.0_f32,
                if focused {
                    pal.accent
                } else {
                    pal.control_border
                },
            ),
            StrokeKind::Inside,
        ),
    );
    theme::paint_icon(
        ui.painter(),
        Rect::from_center_size(
            pos2(rect.left() + 14.0, rect.center().y),
            Vec2::splat(ICON_S),
        ),
        Icon::Search,
        if focused {
            pal.text_dim
        } else {
            pal.text_faint
        },
    );
    if !text.is_empty() {
        let r = Rect::from_center_size(
            pos2(rect.right() - CONTROL_S * 0.5 - 2.0, rect.center().y),
            Vec2::splat(CONTROL_S),
        );
        let mut clear_ui = ui.new_child(UiBuilder::new().id_salt("search-clear").max_rect(r));
        if theme::ghost_icon_button(&mut clear_ui, pal, Icon::Close, CONTROL_S, "Clear search")
            .clicked()
        {
            text.clear();
            ui.memory_mut(|m| m.request_focus(id));
        }
    }
    rect
}

/// Where a section header landed this frame (for the sticky header).
struct HeaderPos {
    rect: Rect,
    open: bool,
    id: Id,
}

/// Paints a bank section header into `rect`: chevron, title, count. With
/// `pinned` it draws on an opaque strip so the rows scrolling beneath stay
/// hidden.
#[allow(clippy::too_many_arguments)]
fn paint_section_header(
    ui: &Ui,
    pal: Palette,
    rect: Rect,
    title: &str,
    count: usize,
    open: bool,
    locked: bool,
    hot: bool,
    pinned: bool,
) {
    let p = ui.painter();
    if pinned {
        p.rect_filled(rect, 0.0, pal.panel);
        p.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0_f32, pal.border_soft),
        );
    } else if hot && !locked {
        p.rect_filled(rect, theme::RADIUS_S, pal.raised);
    }
    let ink = if hot && !locked {
        pal.text
    } else {
        pal.text_dim
    };
    theme::paint_icon(
        p,
        Rect::from_center_size(pos2(rect.left() + 10.0, rect.center().y), Vec2::splat(12.0)),
        if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        if locked { pal.text_faint } else { ink },
    );
    let count_text = count.to_string();
    let count_galley = p.layout_no_wrap(count_text, theme::body_font(TEXT_SMALL), pal.text_faint);
    let count_w = count_galley.size().x;
    p.galley(
        pos2(
            rect.right() - 6.0 - count_w,
            rect.center().y - count_galley.size().y * 0.5,
        ),
        count_galley,
        pal.text_faint,
    );
    theme::paint_elided(
        ui,
        pos2(rect.left() + 24.0, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        theme::strong_font(ui, 10.5),
        ink,
        (rect.width() - 24.0 - count_w - 14.0).max(8.0),
    );
}

/// One in-flow section header. Returns its rect and whether it was clicked
/// (never while a search is active — every match must stay visible).
fn section_header(
    ui: &mut Ui,
    pal: Palette,
    id: Id,
    title: &str,
    count: usize,
    open: bool,
    locked: bool,
) -> (Rect, bool) {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), SECTION_H));
    #[cfg(test)]
    TEST_LOG.with(|l| l.borrow_mut().headers.push((rect, title.to_owned())));
    let resp = ui.interact(
        rect,
        id,
        if locked {
            Sense::hover()
        } else {
            Sense::click()
        },
    );
    if ui.is_rect_visible(rect) {
        paint_section_header(
            ui,
            pal,
            rect,
            title,
            count,
            open,
            locked,
            resp.hovered(),
            false,
        );
    }
    let clicked = resp.clicked();
    if !locked {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if open {
                "Collapse this group"
            } else {
                "Expand this group"
            });
    }
    (rect, clicked && !locked)
}

/// Pins the header of the section that has scrolled past the top of the
/// list, so the list always says which bank the visible rows belong to. The
/// next section's header pushes it up and out as it arrives.
fn sticky_header(
    ui: &mut Ui,
    pal: Palette,
    sections: &[Section],
    headers: &[HeaderPos],
    clip: Rect,
    locked: bool,
) {
    let Some(i) = headers
        .iter()
        .rposition(|h| h.rect.top() < clip.top() - 0.5)
    else {
        return;
    };
    let h = &headers[i];
    if !h.open {
        return;
    }
    let limit = headers.get(i + 1).map_or(f32::INFINITY, |n| n.rect.top());
    let y = clip.top().min(limit - SECTION_H);
    let rect = Rect::from_min_size(pos2(h.rect.left(), y), vec2(h.rect.width(), SECTION_H));
    let resp = ui.interact(
        rect,
        h.id.with("sticky"),
        if locked {
            Sense::hover()
        } else {
            Sense::click()
        },
    );
    let section = &sections[i];
    #[cfg(test)]
    TEST_LOG.with(|l| l.borrow_mut().sticky = Some(section.title.clone()));
    paint_section_header(
        ui,
        pal,
        rect,
        &section.title,
        section.count,
        true,
        locked,
        resp.hovered(),
        true,
    );
    if resp.clicked() && !locked {
        ui.ctx().data_mut(|d| d.insert_temp(h.id, true));
    }
}

// ----- library cards ----------------------------------------------------------

/// What a library card shows.
struct RowSpec<'a> {
    name: &'a str,
    description: &'a str,
    color: Color32,
    /// Small tag next to the name: KIT, PACK or FONT.
    badge: Option<&'static str>,
    can_add: bool,
    /// Drag-and-drop source id; unique per row.
    id: Id,
}

struct RowOut {
    rect: Rect,
    /// `Some(true)` = add, `Some(false)` = preview.
    act: Option<bool>,
}

/// [`Ui::dnd_drag_source`] with a lazily built payload: egui's version needs
/// the payload by value every frame, which for a library row means cloning a
/// whole preset per visible row per frame just in case it gets dragged. Here
/// the payload is only built once the row is actually being dragged.
fn dnd_source<R>(
    ui: &mut Ui,
    id: Id,
    payload: impl FnOnce() -> QuickSound,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    if ui.ctx().is_being_dragged(id) {
        egui::DragAndDrop::set_payload(ui.ctx(), payload());
        // Paint the body to a new layer, then move that layer under the
        // pointer (a dragged body can't be interacted with anyway).
        let layer_id = egui::LayerId::new(egui::Order::Tooltip, id);
        let egui::InnerResponse { inner, response } =
            ui.scope_builder(UiBuilder::new().layer_id(layer_id), add_contents);
        if let Some(pointer) = ui.ctx().pointer_interact_pos() {
            let delta = pointer - response.rect.center();
            ui.ctx().transform_layer_shapes(
                layer_id,
                egui::emath::TSTransform::from_translation(delta),
            );
        }
        egui::InnerResponse::new(inner, response)
    } else {
        let egui::InnerResponse { inner, response } = ui.scope(add_contents);
        let dnd = ui
            .interact(response.rect, id, Sense::drag())
            .on_hover_cursor(egui::CursorIcon::Grab);
        egui::InnerResponse::new(inner, dnd | response)
    }
}

/// Right-edge button boxes of a card: preview, then add.
fn card_buttons(rect: Rect) -> (Rect, Rect) {
    let add = Rect::from_center_size(
        pos2(rect.right() - GAP_S - CONTROL_S * 0.5, rect.center().y),
        Vec2::splat(CONTROL_S),
    );
    let preview = add.translate(vec2(-(CONTROL_S + GAP_XS), 0.0));
    (preview, add)
}

/// One library entry: color bar, name, one-line description, optional badge,
/// preview and add buttons. `tip` builds the hover text (only when hovered),
/// `payload` the drag-and-drop payload (only when dragged).
///
/// The click targets are registered *after* the drag source on purpose:
/// `dnd_drag_source` claims a drag interact on the row rect that would sit
/// on top of any interact inside the body, and egui drops click hits under
/// a drag-only top widget entirely — so interacts inside the body could
/// never fire. Registered here instead, they're the topmost click widgets
/// while drags still resolve to the row's drag source.
fn library_row(
    ui: &mut Ui,
    pal: Palette,
    spec: &RowSpec,
    tip: impl FnOnce() -> String,
    payload: impl FnOnce() -> QuickSound,
) -> RowOut {
    let dnd = dnd_source(ui, spec.id, payload, |ui| library_row_body(ui, pal, spec));
    let rect = dnd.response.rect;
    #[cfg(test)]
    TEST_LOG.with(|l| l.borrow_mut().rows.push(rect));
    if ui.ctx().is_being_dragged(spec.id) {
        return RowOut { rect, act: None };
    }
    let row = ui.interact(rect, spec.id.with("row"), Sense::click());
    row.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            spec.can_add,
            format!("Add {}", spec.name),
        )
    });
    row.clone().on_hover_ui(|ui| {
        ui.set_max_width(280.0);
        ui.label(tip());
    });
    let (preview_rect, add_rect) = card_buttons(rect);
    let preview = {
        let mut c = ui.new_child(
            UiBuilder::new()
                .id_salt(spec.id.with("preview"))
                .max_rect(preview_rect),
        );
        theme::ghost_icon_button(&mut c, pal, Icon::Play, CONTROL_S, "Preview")
    };
    let add = {
        let mut c = ui.new_child(
            UiBuilder::new()
                .id_salt(spec.id.with("add"))
                .max_rect(add_rect),
        );
        c.add_enabled_ui(spec.can_add, |ui| {
            theme::ghost_icon_button(
                ui,
                pal,
                Icon::Plus,
                CONTROL_S,
                if spec.can_add {
                    "Add as new track"
                } else {
                    "Track limit reached"
                },
            )
        })
        .inner
    };
    let act = if preview.clicked() {
        Some(false)
    } else if spec.can_add && (add.clicked() || row.double_clicked()) {
        Some(true)
    } else {
        None
    };
    RowOut { rect, act }
}

/// The painted body of [`library_row`] (split out so it can run inside
/// [`dnd_source`]): card, color bar, text and badge. Hover is read from the
/// pointer position, because the interacts live outside the body.
fn library_row_body(ui: &mut Ui, pal: Palette, spec: &RowSpec) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), CARD_H), Sense::hover());
    let hover = ui.rect_contains_pointer(rect);
    let p = ui.painter();
    p.rect(
        rect,
        theme::RADIUS_M,
        if hover { pal.control } else { pal.raised },
        Stroke::new(
            1.0_f32,
            if hover {
                pal.control_border
            } else {
                pal.border_soft
            },
        ),
        StrokeKind::Inside,
    );
    let bar = Rect::from_min_size(
        rect.min + vec2(GAP_S, 10.0),
        vec2(4.0, rect.height() - 20.0),
    );
    p.rect_filled(bar, 2.0, spec.color);
    let (preview_rect, _) = card_buttons(rect);
    let text_left = rect.left() + 20.0;
    let text_right = preview_rect.left() - GAP_XS;
    let mut name_w = text_right - text_left;
    if let Some(badge) = spec.badge {
        let galley = p.layout_no_wrap(
            badge.to_owned(),
            theme::strong_font(ui, 9.0),
            pal.text_faint,
        );
        let pill = Rect::from_min_size(
            pos2(text_right - galley.size().x - 10.0, rect.top() + 8.0),
            vec2(galley.size().x + 10.0, 14.0),
        );
        p.rect_filled(pill, theme::RADIUS_S, pal.inset);
        p.galley(
            pos2(pill.left() + 5.0, pill.center().y - galley.size().y * 0.5),
            galley,
            pal.text_faint,
        );
        name_w = pill.left() - GAP_XS - text_left;
    }
    theme::paint_elided(
        ui,
        pos2(text_left, rect.top() + 7.0),
        Align2::LEFT_TOP,
        spec.name,
        theme::strong_font(ui, TEXT_BODY),
        pal.text,
        name_w,
    );
    theme::paint_elided(
        ui,
        pos2(text_left, rect.top() + 25.0),
        Align2::LEFT_TOP,
        spec.description.lines().next().unwrap_or(""),
        theme::body_font(TEXT_SMALL),
        pal.text_dim,
        text_right - text_left,
    );
}

// ----- quick add ---------------------------------------------------------------

/// The "Add track" button that opens the quick-add chooser. Returns whether
/// the chooser is open (state in egui temp data under [`QUICK_ADD_OPEN`]).
fn add_track_button(ui: &mut Ui, pal: Palette, full: bool) -> bool {
    let id = Id::new(QUICK_ADD_OPEN);
    let mut open = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), CONTROL_M), Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, open, "Add track")
    });
    if resp.clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_temp(id, open));
    }
    let p = ui.painter();
    p.rect(
        rect,
        theme::RADIUS_M,
        if open {
            pal.control_active
        } else if resp.hovered() {
            pal.control_hover
        } else {
            pal.control
        },
        Stroke::new(1.0_f32, pal.control_border),
        StrokeKind::Inside,
    );
    theme::paint_icon(
        p,
        Rect::from_center_size(
            pos2(rect.left() + 16.0, rect.center().y),
            Vec2::splat(ICON_S),
        ),
        Icon::Plus,
        pal.text,
    );
    p.text(
        pos2(rect.left() + 30.0, rect.center().y),
        Align2::LEFT_CENTER,
        "Add track",
        theme::strong_font(ui, TEXT_BODY),
        pal.text,
    );
    theme::paint_icon(
        ui.painter(),
        Rect::from_center_size(
            pos2(rect.right() - 16.0, rect.center().y),
            Vec2::splat(ICON_S),
        ),
        if open {
            Icon::ChevronUp
        } else {
            Icon::ChevronDown
        },
        pal.text_dim,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if full {
            "Track limit reached — delete a track to add another."
        } else {
            "Add a track: pick a sound from this song, the built-ins, packs or SoundFonts"
        });
    open
}

/// One labelled group inside the quick-add list: a kicker plus slim rows.
/// Stops early once `picked` is set — only one add can fire per frame.
/// `first_row` (when given) receives the rect of the group's first row so
/// headless tests can aim clicks at it.
fn quick_section(
    ui: &mut Ui,
    pal: Palette,
    label: &str,
    sounds: &[QuickSound],
    full: bool,
    picked: &mut Option<QuickSound>,
    mut first_row: Option<&mut Rect>,
) {
    if sounds.is_empty() || picked.is_some() {
        return;
    }
    ui.add_space(2.0);
    ui.label(theme::kicker_text(pal, label));
    for (i, sound) in sounds.iter().enumerate() {
        if picked.is_some() {
            break;
        }
        let next = ui.available_rect_before_wrap();
        let dnd_id = ui.id().with(("quick-add", label, i));
        // Draggable like the library rows: the row paints the ghost while
        // dragged, and dropping it on the arranger adds the track there.
        let dnd = dnd_source(
            ui,
            dnd_id,
            || sound.clone(),
            |ui| quick_add_row(ui, pal, sound, !full),
        );
        if ui.ctx().is_being_dragged(dnd_id) {
            continue;
        }
        // The drag interact sits on top of the body, which would swallow any
        // click target inside it — register the click here, on top, so clicks
        // and drags resolve to different senses of the same row.
        let clicked = ui
            .interact(dnd.response.rect, dnd_id.with("click"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_ui(|ui| {
                ui.set_max_width(280.0);
                ui.label(if full {
                    "Track limit reached — delete a track to add another.".to_owned()
                } else {
                    sound.tip()
                });
            });
        if clicked.clicked() && !full {
            *picked = Some(sound.clone());
        }
        if let Some(out) = first_row.as_deref_mut() {
            if *out == Rect::NOTHING {
                *out = Rect::from_min_size(next.min, vec2(next.width(), QUICK_H));
            }
        }
    }
}

/// The slim "Add a SoundFont…" row the quick-add strip shows while no `.sf2`
/// is installed: same card look as the other quick rows, one click opens the
/// fonts folder. Returns true when the click should fire.
fn quick_add_font_hint_row(ui: &mut Ui, pal: Palette) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), QUICK_H), Sense::click());
    let hot = resp.hovered();
    paint_quick_card(ui, pal, rect, hot, theme::rgb(FONT_COLOR));
    theme::paint_elided(
        ui,
        pos2(rect.left() + 18.0, rect.center().y),
        Align2::LEFT_CENTER,
        "Add a SoundFont…",
        theme::body_font(TEXT_BODY),
        pal.text,
        rect.width() - 48.0,
    );
    theme::paint_icon(
        ui.painter(),
        Rect::from_center_size(
            pos2(rect.right() - 15.0, rect.center().y),
            Vec2::splat(ICON_S),
        ),
        Icon::Folder,
        pal.text_dim,
    );
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Add a SoundFont…")
    });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(
            "SoundFonts (.sf2) play real sampled instruments. Opens the SoundFonts folder.",
        )
        .clicked()
}

fn paint_quick_card(ui: &Ui, pal: Palette, rect: Rect, hot: bool, color: Color32) {
    let p = ui.painter();
    p.rect(
        rect,
        theme::RADIUS_M,
        if hot { pal.control } else { pal.raised },
        Stroke::new(
            1.0_f32,
            if hot {
                pal.control_border
            } else {
                pal.border_soft
            },
        ),
        StrokeKind::Inside,
    );
    p.rect_filled(
        Rect::from_min_size(rect.min + vec2(7.0, 6.0), vec2(3.0, rect.height() - 12.0)),
        1.5,
        color,
    );
}

/// The painted body of a quick-add row: rounded card, color bar, name. The
/// click target lives in [`quick_section`], which must register it *after*
/// the drag interact — a drag-only widget on top makes egui drop the click
/// hit entirely, so an interact inside the body could never fire.
fn quick_add_row(ui: &mut Ui, pal: Palette, sound: &QuickSound, can_add: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), QUICK_H), Sense::hover());
    let hot = can_add && ui.rect_contains_pointer(rect);
    paint_quick_card(ui, pal, rect, hot, theme::rgb(sound.color()));
    let ink = if can_add { pal.text } else { pal.text_disabled };
    let mut right = rect.right() - 30.0;
    if sound.is_kit() {
        let galley = ui.painter().layout_no_wrap(
            "KIT".to_owned(),
            theme::strong_font(ui, 9.0),
            pal.text_faint,
        );
        let pill = Rect::from_min_size(
            pos2(right - galley.size().x - 10.0, rect.center().y - 7.0),
            vec2(galley.size().x + 10.0, 14.0),
        );
        ui.painter().rect_filled(pill, theme::RADIUS_S, pal.inset);
        ui.painter().galley(
            pos2(pill.left() + 5.0, pill.center().y - galley.size().y * 0.5),
            galley,
            pal.text_faint,
        );
        right = pill.left() - GAP_XS;
    }
    theme::paint_elided(
        ui,
        pos2(rect.left() + 18.0, rect.center().y),
        Align2::LEFT_CENTER,
        sound.name(),
        theme::body_font(TEXT_BODY),
        ink,
        right - rect.left() - 18.0,
    );
    theme::paint_icon(
        ui.painter(),
        Rect::from_center_size(
            pos2(rect.right() - 15.0, rect.center().y),
            Vec2::splat(ICON_S),
        ),
        Icon::Plus,
        if can_add {
            if hot {
                pal.text
            } else {
                pal.text_dim
            }
        } else {
            pal.text_disabled
        },
    );
}

// ----- inspector widgets --------------------------------------------------------

/// Section heading of the inspector: a kicker with a hairline running to the
/// right edge.
fn inspector_heading(ui: &mut Ui, pal: Palette, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP_S;
        ui.label(theme::kicker_text(pal, text));
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().hline(
            r.x_range(),
            r.center().y,
            Stroke::new(1.0_f32, pal.border_soft),
        );
    });
    ui.add_space(GAP_XS);
}

/// A labelled inspector row: a fixed label column, then the control, which
/// gets the width that is left. Keeps every control of a section aligned.
fn field_row<R>(ui: &mut Ui, pal: Palette, label: &str, add: impl FnOnce(&mut Ui, f32) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP_S;
        let (r, _) = ui.allocate_exact_size(vec2(LABEL_W, CONTROL_M), Sense::hover());
        ui.painter().text(
            r.left_center(),
            Align2::LEFT_CENTER,
            label,
            theme::body_font(TEXT_BODY),
            pal.text_dim,
        );
        let w = ui.available_width();
        add(ui, w)
    })
    .inner
}

/// A row whose content lines up with the controls of [`field_row`].
fn indented<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui, f32) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP_S;
        ui.add_space(LABEL_W);
        let w = ui.available_width();
        add(ui, w)
    })
    .inner
}

/// Right-aligned value readout next to a slider (all readouts share one
/// column, so "80%" and "C" end on the same edge).
fn readout(ui: &mut Ui, pal: Palette, text: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(READOUT_W, CONTROL_M), Sense::hover());
    ui.painter().text(
        r.right_center(),
        Align2::RIGHT_CENTER,
        text,
        theme::body_font(TEXT_BODY),
        pal.text_dim,
    );
}

/// Track color swatches in a fixed 7 × 2 grid with the selected one ringed.
/// Returns the color clicked, if any.
fn color_swatches(ui: &mut Ui, pal: Palette, current: [u8; 3], width: f32) -> Option<[u8; 3]> {
    const COLS: usize = 7;
    let cell = (width / COLS as f32).floor().clamp(20.0, 28.0);
    let (rect, _) = ui.allocate_exact_size(vec2(cell * COLS as f32, 24.0 * 2.0), Sense::hover());
    let mut picked = None;
    for (i, c) in theme::TRACK_PALETTE
        .iter()
        .chain(crate::model::TRACK_COLORS.iter())
        .take(COLS * 2)
        .enumerate()
    {
        let (col, row) = (i % COLS, i / COLS);
        let r = Rect::from_min_size(
            rect.min + vec2(col as f32 * cell, row as f32 * 24.0),
            vec2(cell, 24.0),
        );
        let resp = ui.interact(r, ui.id().with(("swatch", i)), Sense::click());
        let radius = if resp.hovered() { 9.0 } else { 8.0 };
        ui.painter()
            .circle_filled(r.center(), radius, theme::rgb(*c));
        if *c == current {
            ui.painter()
                .circle_stroke(r.center(), 10.5, Stroke::new(2.0_f32, pal.text));
        }
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Use this track color")
            .clicked()
        {
            picked = Some(*c);
        }
    }
    picked
}

/// Full-width destructive button: danger ink and outline, so Delete never
/// reads like a routine action.
fn danger_button(ui: &mut Ui, pal: Palette, icon: Icon, label: &str, tooltip: &str) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), CONTROL_M), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let hot = resp.hovered() || resp.is_pointer_button_down_on();
    let p = ui.painter();
    p.rect(
        rect,
        theme::RADIUS_M,
        if hot {
            theme::mix(pal.control, pal.danger, 0.22)
        } else {
            pal.control
        },
        Stroke::new(
            1.0_f32,
            pal.danger.gamma_multiply(if hot { 0.9 } else { 0.55 }),
        ),
        StrokeKind::Inside,
    );
    let galley = p.layout_no_wrap(
        label.to_owned(),
        theme::strong_font(ui, TEXT_BODY),
        pal.danger,
    );
    let content_w = ICON_S + GAP_XS + galley.size().x;
    let x = rect.center().x - content_w * 0.5;
    theme::paint_icon(
        ui.painter(),
        Rect::from_center_size(pos2(x + ICON_S * 0.5, rect.center().y), Vec2::splat(ICON_S)),
        icon,
        pal.danger,
    );
    ui.painter().galley(
        pos2(x + ICON_S + GAP_XS, rect.center().y - galley.size().y * 0.5),
        galley,
        pal.danger,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
        .clicked()
}

/// Horizontal gap that spreads four knob columns edge to edge across
/// `width`, so Effects and Play share one grid.
fn knob_gap(width: f32) -> f32 {
    ((width - 4.0 * KNOB_W) / 3.0).max(GAP_XS)
}

/// Display text for the per-track glide knob: "Off" at zero, else seconds.
pub(super) fn glide_text(seconds: f32) -> String {
    if seconds <= 0.0 {
        "Off".to_owned()
    } else {
        format!("{seconds:.2} s")
    }
}

pub(super) fn pan_text(v: f32) -> String {
    if v.abs() < 0.02 {
        "C".to_owned()
    } else if v < 0.0 {
        format!("L{:.0}", -v * 100.0)
    } else {
        format!("R{:.0}", v * 100.0)
    }
}

pub(super) fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Project;

    /// Minimal valid preset for catalog fixtures (same shape tests.rs uses).
    fn preset(id: &str) -> InstrumentPreset {
        serde_json::from_str(&format!(
            r#"{{"id":"{id}","name":"{id}","oscillators":[{{}}]}}"#
        ))
        .expect("minimal preset parses")
    }

    fn catalog(ids: &[&str]) -> PackCatalog {
        PackCatalog {
            instruments: ids.iter().map(|id| preset(id)).collect(),
            errors: vec![],
        }
    }

    /// A headless app with a truly empty song (`Project::empty` seeds one
    /// drum track) and no installed packs unless the test adds them.
    fn app() -> CadenceApp {
        let mut app = CadenceApp::headless(Project::empty());
        app.project.tracks.clear();
        app
    }

    #[test]
    fn quick_picks_empty_song_is_builtins_plus_pack_heads() {
        let mut app = app();
        app.catalog = catalog(&[
            "test-a.kick",
            "test-a.lead",
            "test-b.pad",
            "test-b.choir",
            "test-c.wide",
        ]);
        let picks = app.quick_picks();
        assert!(picks.recent.is_empty());
        assert_eq!(picks.builtins.len(), Instrument::ALL.len());
        // One preset per bank, capped at three.
        let ids: Vec<&str> = picks
            .presets
            .iter()
            .map(|s| match s {
                QuickSound::Preset(p) => p.id.as_str(),
                QuickSound::BuiltIn(_) | QuickSound::Font(_) => "",
            })
            .collect();
        assert_eq!(ids, ["test-a.kick", "test-b.pad", "test-c.wide"]);
        assert_eq!(picks.count(), Instrument::ALL.len() + 3);
    }

    #[test]
    fn quick_picks_recents_dedupe_and_skip_fonts() {
        let mut app = app();
        app.catalog = catalog(&["test-a.kick", "test-a.lead", "test-b.pad"]);
        app.add_track(Instrument::Piano);
        app.add_preset_track(&preset("test-a.kick"));
        // No fonts are installed in headless, so this font track's sound
        // can't be re-created by the quick add and must stay out of recents
        // (its fallback preset would mislead).
        let font_track = app.project.tracks.last().unwrap().clone();
        app.project.tracks.push({
            let mut t = font_track;
            t.font = Some(crate::model::FontRef::new("GeneralUser", 0));
            t.id = crate::model::next_id();
            t
        });

        let picks = app.quick_picks();
        // Newest track first: the preset track was added after the piano.
        assert_eq!(picks.recent.len(), 2);
        assert!(matches!(
            picks.recent[0],
            QuickSound::Preset(ref p) if p.id == "test-a.kick"
        ));
        assert!(matches!(
            picks.recent[1],
            QuickSound::BuiltIn(Instrument::Piano)
        ));
        assert!(picks.fonts.is_empty(), "no installed font, no font picks");
        // Sounds already listed are not repeated in the other groups.
        assert!(!picks
            .builtins
            .iter()
            .any(|s| matches!(s, QuickSound::BuiltIn(Instrument::Piano))));
        assert!(!picks
            .presets
            .iter()
            .any(|s| matches!(s, QuickSound::Preset(ref p) if p.id == "test-a.kick")));
        // test-a.kick is recent, so test-a's bank rep becomes test-a.lead.
        assert!(matches!(
            picks.presets[0],
            QuickSound::Preset(ref p) if p.id == "test-a.lead"
        ));
    }

    /// Run with
    /// `cargo test --release --locked --lib perf_bench_sidebar -- --ignored --nocapture --test-threads=1`.
    /// Average headless frame cost with the sidebar on each tab (full pack
    /// catalog, demo song, nothing hovered).
    #[test]
    #[ignore = "release-mode performance measurement"]
    fn perf_bench_sidebar() {
        use std::time::Instant;
        let catalog =
            crate::packs::load_catalog_from(&std::env::temp_dir().join("cadence-bench-no-packs"));
        for (label, tab, quick_open, collapsed) in [
            (
                "rest-of-app (sidebar collapsed)",
                SideTab::Library,
                false,
                true,
            ),
            ("library", SideTab::Library, false, false),
            ("track", SideTab::Track, false, false),
            ("track+quick-add", SideTab::Track, true, false),
        ] {
            let ctx = egui::Context::default();
            let mut app = CadenceApp::headless(Project::demo());
            app.catalog = catalog.clone();
            app.side_tab = tab;
            ctx.data_mut(|d| {
                d.insert_temp(egui::Id::new(QUICK_ADD_OPEN), quick_open);
                d.insert_temp(egui::Id::new(SIDEBAR_COLLAPSED), collapsed);
            });
            let mut elapsed = std::time::Duration::ZERO;
            for n in 0..240 {
                let input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1360.0, 860.0))),
                    time: Some(n as f64 / 60.0),
                    ..Default::default()
                };
                let start = Instant::now();
                let _ = ctx.run(input, |ctx| app.frame(ctx));
                if n >= 40 {
                    elapsed += start.elapsed();
                }
            }
            println!(
                "sidebar,{label},{:.1} us/frame",
                elapsed.as_secs_f64() * 5000.0
            );
        }
    }

    #[test]
    fn quick_picks_recent_cap() {
        let mut app = app();
        for inst in Instrument::ALL {
            app.add_track(inst);
        }
        // Seven built-in tracks but the recent group caps at four.
        let picks = app.quick_picks();
        assert_eq!(picks.recent.len(), 4);
        assert_eq!(picks.builtins.len(), 3);
    }

    /// Installs a fabricated font catalog entry (no parsed SoundFont needed —
    /// the picker only reads `FontInfo`).
    fn install_font(app: &mut CadenceApp, name: &str) {
        app.fonts.fonts.push(FontInfo {
            name: name.to_owned(),
            key: crate::font::font_key(name),
            presets: 128,
        });
    }

    #[test]
    fn quick_picks_offer_iconic_soundfont_programs() {
        let mut app = app();
        install_font(&mut app, "GeneralUser");
        let picks = app.quick_picks();
        assert_eq!(picks.fonts.len(), 3);
        let programs: Vec<(String, u8)> = picks
            .fonts
            .iter()
            .map(|s| match s {
                QuickSound::Font(f) => (f.name.clone(), f.program),
                other => panic!("expected a font pick, got {}", other.name()),
            })
            .collect();
        assert_eq!(
            programs,
            vec![
                ("GeneralUser".to_owned(), 0),
                ("GeneralUser".to_owned(), 24),
                ("GeneralUser".to_owned(), 40),
            ]
        );
        assert_eq!(picks.fonts[0].name(), "Acoustic Grand Piano");
        assert_eq!(picks.fonts[1].name(), "Acoustic Guitar (nylon)");
        assert_eq!(picks.fonts[2].name(), "Violin");
        assert_eq!(picks.count(), Instrument::ALL.len() + 3);
    }

    #[test]
    fn quick_picks_recent_recreates_installed_font_tracks() {
        let mut app = app();
        install_font(&mut app, "GeneralUser");
        app.add_font_track(&FontRef::new("GeneralUser", 40));
        let picks = app.quick_picks();
        // The song's font track is re-creatable now, so it leads recents.
        assert_eq!(picks.recent.len(), 1);
        assert!(matches!(
            &picks.recent[0],
            QuickSound::Font(f) if f.name == "GeneralUser" && f.program == 40
        ));
        // The spotlight group prefers the font the song already uses.
        assert!(picks.fonts.iter().all(|s| matches!(
            s,
            QuickSound::Font(f) if f.name == "GeneralUser"
        )));
    }

    #[test]
    fn font_rows_group_all_programs_and_filter_by_search() {
        let all = font_rows("");
        assert_eq!(all.len(), 16, "16 family groups");
        assert_eq!(all.iter().map(|(_, p)| p.len()).sum::<usize>(), 128);
        assert_eq!(all[0].0, "Piano");
        assert_eq!(all[0].1[0], (0, "Acoustic Grand Piano"));
        assert_eq!(all[15].1[7], (127, "Gunshot"));
        // Search hits program names (both trumpets live in the Brass family)…
        let trumpet = font_rows("trumpet");
        assert_eq!(trumpet.len(), 1);
        assert_eq!(trumpet[0].0, "Brass");
        assert_eq!(trumpet[0].1, [(56, "Trumpet"), (59, "Muted Trumpet")]);
        // …and family names (the whole family stays together).
        let piano = font_rows("piano");
        assert_eq!(piano.len(), 1);
        assert_eq!(piano[0].1.len(), 8);
        // "soundfont" matches the whole catalog, for discoverability.
        assert_eq!(font_rows("soundfont").len(), 16);
        assert!(font_rows("definitely nothing").is_empty());
    }

    // ----- sidebar rework: layout, sections, cache, inspector ---------------------

    use egui::{Event, Modifiers, PointerButton, RawInput};

    /// A headless frame driver with a catalog big enough to scroll.
    struct Rig {
        ctx: egui::Context,
        app: CadenceApp,
        time: f64,
        size: egui::Vec2,
    }

    impl Rig {
        fn new(app: CadenceApp, w: f32, h: f32) -> Self {
            Self {
                ctx: egui::Context::default(),
                app,
                time: 0.0,
                size: vec2(w, h),
            }
        }

        fn demo(w: f32, h: f32) -> Self {
            let mut app = CadenceApp::headless(Project::demo());
            app.catalog = crate::packs::load_catalog_from(
                &std::env::temp_dir().join("cadence-sidebar-test-no-packs"),
            );
            Self::new(app, w, h)
        }

        fn step_with(&mut self, events: Vec<Event>) {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, self.size)),
                time: Some(self.time),
                focused: true,
                events,
                ..Default::default()
            };
            self.time += 1.0 / 60.0;
            let _ = self.ctx.run(input, |ctx| self.app.frame(ctx));
        }

        fn steps(&mut self, n: usize) {
            for _ in 0..n {
                self.step_with(vec![]);
            }
        }

        fn click(&mut self, pos: egui::Pos2) {
            self.step_with(vec![Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                self.step_with(vec![Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                }]);
            }
        }

        fn wheel(&mut self, at: egui::Pos2, dy: f32) {
            self.step_with(vec![Event::PointerMoved(at)]);
            self.step_with(vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, dy),
                modifiers: Modifiers::NONE,
            }]);
        }

        /// Width of the sidebar panel as egui stored it.
        fn panel_width(&self) -> f32 {
            egui::containers::panel::PanelState::load(&self.ctx, Id::new("sidebar"))
                .expect("sidebar panel state")
                .rect
                .width()
        }

        fn log(&self) -> (Vec<Rect>, Vec<(Rect, String)>, Option<String>) {
            TEST_LOG.with(|l| {
                let l = l.borrow();
                (l.rows.clone(), l.headers.clone(), l.sticky.clone())
            })
        }

        /// Frames with a cleared layout log, so only the last frame is seen.
        fn settle(&mut self) {
            self.steps(3);
            TEST_LOG.with(|l| *l.borrow_mut() = TestLog::default());
            self.step_with(vec![]);
        }
    }

    fn piano_track_selected(app: &mut CadenceApp) {
        app.add_track(Instrument::Piano);
    }

    #[test]
    fn sidebar_stays_within_its_cap_and_never_ratchets() {
        // 1100 x 760 is the minimum window; 1920 x 1080 the big one.
        for (w, h) in [(1100.0, 760.0), (1360.0, 860.0), (1920.0, 1080.0)] {
            for tab in [SideTab::Library, SideTab::Track] {
                let mut rig = Rig::demo(w, h);
                piano_track_selected(&mut rig.app);
                rig.app.side_tab = tab;
                // Open the chooser and the arpeggiator so the widest layouts render.
                rig.ctx
                    .data_mut(|d| d.insert_temp(Id::new(QUICK_ADD_OPEN), true));
                let id = rig.app.selected_track().expect("selected").id;
                rig.app.edit("Arp", None, |p| {
                    if let Some(t) = ops::track_mut(p, id) {
                        t.arp.enabled = true;
                    }
                });
                rig.steps(4);
                let first = rig.panel_width();
                assert!(
                    (240.0..=380.0).contains(&first),
                    "{w}x{h} {tab:?}: panel width {first}"
                );
                rig.steps(30);
                let later = rig.panel_width();
                assert!(
                    (later - first).abs() < 0.5,
                    "{w}x{h} {tab:?}: panel width drifted {first} -> {later}"
                );
            }
        }
    }

    #[test]
    fn collapsed_rail_is_slim_and_expands_back() {
        let mut rig = Rig::demo(1360.0, 860.0);
        rig.steps(3);
        rig.ctx
            .data_mut(|d| d.insert_temp(Id::new(SIDEBAR_COLLAPSED), true));
        rig.steps(4);
        let rail = rig.panel_width();
        assert!(
            (RAIL_W - 2.0..=RAIL_W + 2.0).contains(&rail),
            "rail width {rail}"
        );
        assert_eq!(rig.app.library_rects.search, Rect::NOTHING);
        rig.ctx
            .data_mut(|d| d.insert_temp(Id::new(SIDEBAR_COLLAPSED), false));
        rig.steps(6);
        assert!(rig.panel_width() > 200.0);
    }

    #[test]
    fn library_view_groups_sections_and_caches() {
        let cat = catalog(&["a.one", "a.two", "b.three", "a.four"]);
        // All: built-ins first, then one section per consecutive pack run.
        let v = build_view(&cat, &LibraryBank::All, "", catalog_stamp(&cat));
        let titles: Vec<&str> = v.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["BUILT-IN", "PACK · A", "PACK · B", "PACK · A"]);
        let counts: Vec<usize> = v.sections.iter().map(|s| s.count).collect();
        assert_eq!(counts, [Instrument::ALL.len(), 2, 1, 1]);
        assert!(v
            .sections
            .iter()
            .all(|s| s.count == s.items.len() && !s.items.is_empty()));
        // Bank filters keep their own single section.
        let v = build_view(&cat, &LibraryBank::Pack("b".into()), "", 0);
        assert_eq!(v.sections.len(), 1);
        assert_eq!(v.sections[0].items, [LibItem::Preset(2)]);
        // The SoundFont bank groups the 128 GM programs under 16 families.
        let v = build_view(&cat, &LibraryBank::Font, "", 0);
        assert_eq!(v.sections.len(), 16);
        assert_eq!(v.sections.iter().map(|s| s.count).sum::<usize>(), 128);
        // No hits: no sections (the empty state shows instead).
        assert!(build_view(&cat, &LibraryBank::All, "zzzz-nothing", 0)
            .sections
            .is_empty());

        // The cache hands out the same view until something it depends on
        // changes.
        let ctx = egui::Context::default();
        let a = library_view(&ctx, &cat, &LibraryBank::All, "");
        let b = library_view(&ctx, &cat, &LibraryBank::All, "");
        assert!(Arc::ptr_eq(&a, &b), "unchanged inputs reuse the view");
        let c = library_view(&ctx, &cat, &LibraryBank::All, "one");
        assert!(!Arc::ptr_eq(&b, &c), "a new query rebuilds");
        let d = library_view(&ctx, &cat, &LibraryBank::BuiltIn, "one");
        assert!(!Arc::ptr_eq(&c, &d), "a new bank rebuilds");
        let cat2 = catalog(&["a.one", "a.two"]);
        let e = library_view(&ctx, &cat2, &LibraryBank::BuiltIn, "one");
        assert!(!Arc::ptr_eq(&d, &e), "a changed catalog rebuilds");
    }

    #[test]
    fn library_lays_out_only_visible_cards_without_overlap() {
        let mut rig = Rig::demo(1360.0, 860.0);
        rig.settle();
        let (rows, headers, _) = rig.log();
        let list = rig.app.library_rects.list;
        assert!(list.height() > 200.0);
        let total = Instrument::ALL.len() + rig.app.catalog.instruments.len();
        assert!(total > 150, "the full catalog is loaded: {total}");
        assert!(
            rows.len() < 24,
            "only the visible cards are laid out, got {}",
            rows.len()
        );
        assert!(!rows.is_empty() && !headers.is_empty());
        for (i, r) in rows.iter().enumerate() {
            assert!((r.height() - CARD_H).abs() < 0.01, "uniform card height");
            assert!(r.right() <= list.right() + 0.5 && r.left() >= list.left() - 0.5);
            if let Some(next) = rows.get(i + 1) {
                assert!(r.bottom() <= next.top() + 0.01 || next.top() < r.top() - 1.0);
            }
        }
        // Headers sit above their rows and never overlap a card.
        for (h, _) in &headers {
            assert!((h.height() - SECTION_H).abs() < 0.01);
            assert!(rows.iter().all(|r| !r.intersects(h.shrink(1.0))));
        }
    }

    #[test]
    fn collapsing_a_section_hides_its_rows_and_scrolling_pins_the_header() {
        let mut rig = Rig::demo(1360.0, 860.0);
        rig.settle();
        let (open_rows, headers, sticky) = rig.log();
        assert!(sticky.is_none(), "nothing pinned at scroll 0");
        let builtin = headers
            .iter()
            .find(|(_, t)| t == "BUILT-IN")
            .expect("built-in header")
            .0;

        // Collapse BUILT-IN with a click on its header.
        rig.click(builtin.center());
        rig.settle();
        let (_, headers, _) = rig.log();
        let collapsed = ctx_closed(&rig.ctx, "builtin");
        assert!(collapsed, "the click stored the collapsed state");
        assert!(headers.len() >= 2, "other sections still show");
        assert!(!open_rows.is_empty());
        let first_pack = headers[1].0;
        assert!(
            first_pack.top() < builtin.bottom() + 2.0 * GAP_S,
            "the next section moved up under the collapsed header"
        );
        // Click again: expands, state remembered across frames.
        rig.click(builtin.center());
        rig.settle();
        assert!(!ctx_closed(&rig.ctx, "builtin"));

        // Scroll far enough that a header passes the top: it pins.
        let list = rig.app.library_rects.list;
        for _ in 0..6 {
            rig.wheel(list.center(), -240.0);
        }
        rig.settle();
        let (_, _, sticky) = rig.log();
        let pinned = sticky.expect("a header is pinned after scrolling");
        assert!(
            pinned.starts_with("PACK") || pinned == "BUILT-IN",
            "{pinned}"
        );
        assert!(rig.app.library_rects.scroll.y > 100.0);
    }

    fn ctx_closed(ctx: &egui::Context, key: &str) -> bool {
        ctx.data(|d| d.get_temp::<bool>(Id::new((BANK_CLOSED, key))))
            .unwrap_or(false)
    }

    #[test]
    fn search_with_no_matches_shows_the_empty_state_and_the_clear_button_works() {
        let mut rig = Rig::demo(1360.0, 860.0);
        rig.app.library_filter = "zzzz-no-such-sound".into();
        rig.settle();
        let (rows, headers, _) = rig.log();
        assert!(rows.is_empty() && headers.is_empty(), "no list rows");
        // The search field's ✕ is the last thing in the field; clicking it
        // clears the text and the full list comes back.
        let search = rig.app.library_rects.search;
        assert!(search.width() > 100.0 && (search.height() - CONTROL_M).abs() < 0.5);
        rig.click(pos2(search.right() - 14.0, search.center().y));
        assert_eq!(rig.app.library_filter, "");
        rig.settle();
        assert!(!rig.log().0.is_empty(), "rows are back");
    }

    #[test]
    fn long_filter_text_stays_inside_the_field() {
        let mut rig = Rig::demo(1100.0, 760.0);
        rig.app.library_filter = "x".repeat(200);
        rig.steps(4);
        let search = rig.app.library_rects.search;
        let panel = rig.panel_width();
        assert!(
            search.right() <= panel + 1.0,
            "search {search:?} vs {panel}"
        );
    }

    #[test]
    fn inspector_hides_play_for_drum_tracks_and_audio_lanes() {
        let drums = create_track(Instrument::Drums);
        assert!(!Inspected::of(&drums).melodic, "drum kit has no glide/arp");
        let piano = create_track(Instrument::Piano);
        assert!(Inspected::of(&piano).melodic);
        let mut kit = create_track(Instrument::Drums);
        let mut preset = preset("t.kit");
        preset.kit = Some(crate::packs::KitSpec {
            pieces: vec![crate::packs::KitPiece {
                pitch: 36,
                name: "Kick".into(),
                voice: crate::packs::KitVoice::default(),
            }],
        });
        kit.preset = Some(preset);
        assert!(!Inspected::of(&kit).melodic, "kit presets are drums too");
        let audio = crate::model::create_audio_track("Vocals");
        assert!(
            !Inspected::of(&audio).melodic,
            "audio lanes are not pitched"
        );
        let mut font = create_track(Instrument::Drums);
        font.font = Some(FontRef::new("GeneralUser", 0));
        assert!(
            Inspected::of(&font).melodic,
            "a font plays pitched programs"
        );
    }

    #[test]
    fn track_tab_renders_for_every_kind_of_track_in_every_theme() {
        for choice in theme::ThemeChoice::ALL {
            let mut rig = Rig::demo(1100.0, 760.0);
            rig.app.theme = choice;
            rig.app.side_tab = SideTab::Track;
            rig.ctx
                .data_mut(|d| d.insert_temp(Id::new(QUICK_ADD_OPEN), true));
            for inst in Instrument::ALL {
                rig.app.add_track(inst);
                rig.steps(2);
            }
            rig.app.add_font_track(&FontRef::new("GeneralUser", 24));
            rig.steps(2);
            rig.app.project.tracks.clear();
            rig.app.fix_selection();
            rig.steps(2);
            assert!(rig.panel_width() <= 380.0);
        }
    }

    #[test]
    fn dnd_payload_is_only_built_when_the_row_is_dragged() {
        let ctx = egui::Context::default();
        let built = std::cell::Cell::new(0);
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 300.0))),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                dnd_source(
                    ui,
                    Id::new("row"),
                    || {
                        built.set(built.get() + 1);
                        QuickSound::BuiltIn(Instrument::Piano)
                    },
                    |ui| ui.label("row"),
                );
            });
        });
        assert_eq!(built.get(), 0, "an idle row never clones its payload");
    }

    #[test]
    fn quick_picks_are_cached_until_the_song_changes() {
        let mut app = app();
        app.catalog = catalog(&["test-a.kick", "test-b.pad"]);
        let ctx = egui::Context::default();
        let a = app.quick_picks_cached(&ctx);
        let b = app.quick_picks_cached(&ctx);
        assert!(Arc::ptr_eq(&a, &b));
        app.add_track(Instrument::Piano);
        let c = app.quick_picks_cached(&ctx);
        assert!(!Arc::ptr_eq(&b, &c), "an edit rebuilds the picks");
        assert!(matches!(
            c.picks.recent[0],
            QuickSound::BuiltIn(Instrument::Piano)
        ));
    }

    #[test]
    fn readouts_and_pan_text_are_stable() {
        assert_eq!(pan_text(0.0), "C");
        assert_eq!(pan_text(-0.5), "L50");
        assert_eq!(pan_text(1.0), "R100");
        assert_eq!(glide_text(0.0), "Off");
        assert_eq!(glide_text(0.25), "0.25 s");
        assert_eq!(elide_query("short"), "short");
        assert_eq!(elide_query(&"y".repeat(40)).chars().count(), 24);
        assert!(knob_gap(250.0) > GAP_XS);
        assert_eq!(knob_gap(100.0), GAP_XS);
    }
}
