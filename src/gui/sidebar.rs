//! Left sidebar: instrument library (built-ins + JSON packs) and track inspector.

use egui::{
    vec2, Align, Align2, Color32, Context, FontId, Frame, Layout, Margin, Pos2, Rect, RichText,
    Sense, Stroke, StrokeKind, Ui, UiBuilder,
};

use super::theme::{self, Icon, Palette};
use super::{ops, CadenceApp, Focus, StatusKind, Tab};
use crate::font::{gm_program_catalog, FontInfo, GM_PROGRAM_NAMES};
use crate::model::{
    create_track, note_name, ArpDirection, ArpRate, FontRef, Instrument, Track, MAX_ARP_OCTAVES,
    MAX_GLIDE_SECONDS, MIN_ARP_GATE,
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

/// The library rows visible under a bank filter and search query. Shared by
/// the list UI and headless tests so the counts cannot drift apart.
pub(super) struct LibraryRows<'a> {
    pub builtins: Vec<Instrument>,
    pub presets: Vec<&'a InstrumentPreset>,
}

/// `query` is the trimmed, lowercased search text (empty matches everything).
/// Search hits the preset/built-in name, its description and its bank name.
pub(super) fn library_rows<'a>(
    bank: &LibraryBank,
    query: &str,
    catalog: &'a PackCatalog,
) -> LibraryRows<'a> {
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
        .filter(|p| {
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
        .collect();
    LibraryRows { builtins, presets }
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
    fn count(&self) -> usize {
        self.recent.len() + self.builtins.len() + self.presets.len() + self.fonts.len()
    }
}

/// The GM programs the quick-add SoundFont group spotlights — one iconic
/// real-instrument sound each, in catalog order.
const QUICK_FONT_PROGRAMS: [u8; 3] = [0, 24, 40];

/// Translucent lift painted over a clickable card while hovered. The row
/// bodies can't show hover themselves: their interacts sit *under* the
/// drag source's, so hover is painted by the caller after the real click
/// interact is registered (see [`quick_section`] and [`library_row`]).
const HOVER_LIFT: Color32 = Color32::from_rgba_premultiplied(10, 10, 10, 10);
/// Slightly stronger lift for the small buttons inside a library row.
const BUTTON_LIFT: Color32 = Color32::from_rgba_premultiplied(22, 22, 22, 22);

/// Display name of the GM family containing `program` (0..=127); out-of-range
/// values clamp into the last family like [`crate::font::gm_program_name`].
pub(super) fn gm_family(program: u8) -> &'static str {
    crate::font::gm_family(program)
}

/// Whether the track still carries its auto-assigned sound name — the
/// built-in/preset label or the font name — rather than a user-chosen one.
/// Sound changes rename the track only while this holds.
fn has_default_sound_name(track: &Track) -> bool {
    ops::instrument_label(track) == track.name
        || track.font.as_ref().is_some_and(|f| f.name == track.name)
}

impl CadenceApp {
    pub(super) fn sidebar(&mut self, ctx: &Context) {
        let pal = self.palette();
        // Whole-panel collapse: a slim rail with just an expand handle, so
        // the arranger/editor get the width back. Session state via egui
        // temp data, same as the library/quick-add disclosures.
        let collapsed = ctx
            .data(|d| d.get_temp::<bool>(egui::Id::new(SIDEBAR_COLLAPSED)))
            .unwrap_or(false);
        let frame = Frame::new()
            .fill(pal.panel)
            .inner_margin(Margin {
                left: 12,
                right: 10,
                top: 10,
                bottom: 8,
            })
            .stroke(Stroke::new(1.0_f32, pal.border_soft));
        let mut panel = egui::SidePanel::left("sidebar");
        if collapsed {
            panel = panel.exact_width(30.0).resizable(false);
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
                ui.vertical_centered(|ui| {
                    ui.add_space(6.0);
                    if ui
                        .add(
                            egui::Button::new("»")
                                .min_size(vec2(22.0, 22.0))
                                .fill(pal.control),
                        )
                        .on_hover_text("Show sidebar")
                        .clicked()
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new(SIDEBAR_COLLAPSED), false);
                        });
                    }
                });
                return;
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                // Reserve room for the « collapse button (24px) + all three
                // inter-item gaps (4px each) so the row can never overflow
                // the panel — overflow would feed back into the panel's
                // stored width and make it ratchet wider every frame.
                let w = (ui.available_width() - 36.0) / 2.0;
                for (tab, label) in [(SideTab::Library, "Instruments"), (SideTab::Track, "Track")] {
                    let on = self.side_tab == tab;
                    let b = egui::Button::new(RichText::new(label).color(if on {
                        theme::ink_on(pal.accent)
                    } else {
                        pal.text_dim
                    }))
                    .fill(if on { pal.accent } else { pal.control })
                    .min_size(vec2(w, 28.0));
                    if ui.add(b).clicked() {
                        self.side_tab = tab;
                    }
                }
                if ui
                    .add(
                        egui::Button::new("«")
                            .min_size(vec2(24.0, 28.0))
                            .fill(pal.control),
                    )
                    .on_hover_text("Hide sidebar")
                    .clicked()
                {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(egui::Id::new(SIDEBAR_COLLAPSED), true));
                }
            });
            ui.add_space(8.0);
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

    fn library_ui(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        let full = self.project.tracks.len() >= ops::MAX_TRACKS;
        let total = Instrument::ALL.len() + self.catalog.instruments.len();
        // The header row doubles as the collapse toggle: the (possibly long)
        // list below folds away to a single hint line. Open state lives in
        // egui temp data so it survives switching to the Track tab.
        let open = disclosure_header(
            ui,
            pal,
            LIBRARY_OPEN,
            "LIBRARY",
            "Add an instrument",
            Some((
                RichText::new(format!("{}/{}", self.project.tracks.len(), ops::MAX_TRACKS))
                    .small()
                    .color(if full { pal.danger } else { pal.text_faint }),
                "Tracks in this song / maximum",
            )),
            true,
        );
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
        ui.add_space(4.0);
        self.library_rects.search = ui
            .add(
                egui::TextEdit::singleline(&mut self.library_filter)
                    .hint_text("Search instruments…")
                    .desired_width(f32::INFINITY),
            )
            .rect;
        ui.add_space(4.0);

        // Bank chooser + reset live above the scrolling list so they stay
        // reachable however far down the library the user has scrolled.
        let bank_row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let options = bank_options(&self.catalog);
            let count = options
                .iter()
                .find(|(b, _)| *b == self.library_bank)
                .map(|(_, n)| *n);
            let selected = match count {
                Some(n) => format!("{} · {n}", bank_title(&self.library_bank)),
                None => bank_title(&self.library_bank),
            };
            let mut pick = None;
            egui::ComboBox::from_id_salt("library-bank")
                .width((ui.available_width() - 62.0).max(90.0))
                .selected_text(selected)
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
            let filtered =
                !self.library_filter.trim().is_empty() || self.library_bank != LibraryBank::All;
            if ui
                .add_enabled(
                    filtered,
                    egui::Button::new("Reset").min_size(vec2(54.0, 0.0)),
                )
                .on_hover_text("Clear the search text and bank filter")
                .clicked()
            {
                self.library_filter.clear();
                self.library_bank = LibraryBank::All;
            }
            pick
        });
        self.library_rects.bank = bank_row.response.rect;
        if let Some(bank) = bank_row.inner {
            self.library_bank = bank;
        }
        ui.add_space(4.0);
        if full {
            ui.label(
                RichText::new("Track limit reached. Delete a track to add another.")
                    .small()
                    .color(pal.danger),
            );
        }

        let query = self.library_filter.trim().to_lowercase();
        let bank = self.library_bank.clone();
        let mut action = None;
        let show_pack_headers = matches!(bank, LibraryBank::All);
        let list_rect = ui.available_rect_before_wrap();
        let list = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Stale geometry from a previous frame's bank must not leak
                // into hit-tests; `font_bank_ui` re-fills its rects.
                self.library_rects.first_font_row = Rect::NOTHING;
                self.library_rects.font_hint = Rect::NOTHING;
                if bank == LibraryBank::Font {
                    self.font_bank_ui(ui, pal, &query, full, &mut action);
                    return;
                }
                let rows = library_rows(&bank, &query, &self.catalog);
                if !rows.builtins.is_empty() {
                    ui.add_space(2.0);
                    ui.label(theme::kicker_text(pal, "BUILT-IN"));
                    for inst in &rows.builtins {
                        if let Some(a) = library_row(
                            ui,
                            pal,
                            inst.name(),
                            inst.description(),
                            theme::rgb(instrument_color(*inst)),
                            inst.is_drums(),
                            !full,
                            ui.id().with(("library-row", inst.name())),
                            QuickSound::BuiltIn(*inst),
                        ) {
                            action = Some(if a {
                                LibraryAction::Add(*inst)
                            } else {
                                LibraryAction::Audition(*inst)
                            });
                        }
                    }
                }

                let mut current_pack = String::new();
                for preset in rows.presets.iter().copied() {
                    let title = pack_title(&preset.id);
                    if show_pack_headers && title != current_pack {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(theme::kicker_text(
                                pal,
                                &format!("PACK · {}", title.to_uppercase()),
                            ));
                        });
                        current_pack = title;
                    }
                    let tip = format!(
                        "{}\n\n{}\npreview {} · {:.1} s\nid: {}",
                        preset.description,
                        preset_summary(preset),
                        note_name(if preset.kit.is_some() {
                            kit_audition_pitch(preset)
                        } else {
                            preset.audition_note
                        }),
                        preset.audition_duration,
                        preset.id
                    );
                    if let Some(a) = library_row(
                        ui,
                        pal,
                        &preset.name,
                        &tip,
                        theme::rgb(ops::preset_color(&preset.id)),
                        preset.kit.is_some(),
                        !full,
                        ui.id().with(("library-row", &preset.id)),
                        QuickSound::Preset(Box::new(preset.clone())),
                    ) {
                        action = Some(if a {
                            LibraryAction::AddPreset(preset.clone())
                        } else {
                            LibraryAction::AuditionPreset(preset.clone())
                        });
                    }
                }
                if rows.builtins.is_empty()
                    && rows.presets.is_empty()
                    && (!query.is_empty() || bank != LibraryBank::All)
                {
                    theme::hint(ui, pal, "No instruments match.");
                }

                ui.add_space(10.0);
                self.packs_footer(ui);
            });
        self.library_rects.list = list_rect;
        self.library_rects.scroll = list.state.offset;

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

    fn packs_footer(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        Frame::new().fill(pal.raised).corner_radius(8).inner_margin(Margin::same(10)).stroke(Stroke::new(1.0_f32, pal.border_soft)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::kicker_text(pal, "INSTRUMENT PACKS"));
            ui.label(RichText::new("Packs are small JSON files of synth presets. No rebuild or restart needed.").small().color(pal.text_dim));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new("Import Pack…").min_size(vec2(0.0, 26.0))).on_hover_text("Validate and install a pack .json file").clicked() {
                    self.import_pack_dialog();
                }
                if ui.add(egui::Button::new("Reload").min_size(vec2(0.0, 26.0))).on_hover_text("Re-read installed packs now (Cadence also checks every 2 seconds)").clicked() {
                    self.reload_packs(true);
                }
            });
            let n = self.catalog.errors.len();
            if n > 0 {
                let text = RichText::new(format!("{n} pack problem{} — details", if n == 1 { "" } else { "s" })).small().color(pal.warn);
                if ui.add(egui::Label::new(text).sense(Sense::click())).on_hover_text("Show which pack files could not be loaded and why").clicked() {
                    self.show_pack_errors();
                }
            }
            let dir = crate::packs::user_packs_dir();
            let resp = ui.add(egui::Label::new(RichText::new(format!("Folder: {}", dir.display())).small().color(pal.text_faint)).truncate().sense(Sense::click()));
            if resp.on_hover_text(format!("{}\nClick to copy the path", dir.display())).clicked() {
                ui.ctx().copy_text(dir.display().to_string());
                self.set_status(StatusKind::Info, "Copied the packs folder path to the clipboard.");
            }
            ui.add_space(2.0);
            ui.label(RichText::new("Plugin instruments (VST3 / AU / CLAP) are planned but not supported yet.").small().color(pal.text_faint));
        });
    }

    /// The SoundFont bank body: a font chooser (when several are installed),
    /// then the 128 GM programs grouped under collapsible family headers.
    /// Rows add a track playing that program on the chosen font; ▶ auditions
    /// it through the engine's font bank.
    fn font_bank_ui(
        &mut self,
        ui: &mut Ui,
        pal: Palette,
        query: &str,
        full: bool,
        action: &mut Option<LibraryAction>,
    ) {
        let fonts: Vec<FontInfo> = self.fonts.fonts.clone();
        if fonts.is_empty() {
            // No .sf2 installed: a slim hint card pointing at the folder.
            self.fonts_hint_card(ui, pal);
            return;
        }
        // Session font choice, kept in egui temp data like the disclosures.
        let stored = ui
            .ctx()
            .data_mut(|d| d.get_temp::<String>(egui::Id::new(LIBRARY_FONT)));
        let selected = stored
            .filter(|name| fonts.iter().any(|i| &i.name == name))
            .unwrap_or_else(|| fonts[0].name.clone());
        if fonts.len() > 1 {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(theme::kicker_text(pal, "FONT"));
                let mut pick = None;
                egui::ComboBox::from_id_salt("library-font")
                    .width((ui.available_width() - 30.0).max(90.0))
                    .selected_text(selected.clone())
                    .show_ui(ui, |ui| {
                        for info in &fonts {
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
                        .data_mut(|d| d.insert_temp(egui::Id::new(LIBRARY_FONT), name));
                }
            });
            ui.add_space(4.0);
        }
        let rows = font_rows(query);
        let mut first_row = Rect::NOTHING;
        for (family, programs) in &rows {
            egui::CollapsingHeader::new(
                RichText::new(family.to_uppercase())
                    .text_style(theme::kicker())
                    .color(pal.text_faint),
            )
            .default_open(true)
            .show(ui, |ui| {
                for (program, label) in programs {
                    let next = ui.available_rect_before_wrap();
                    let font = FontRef::new(selected.clone(), *program);
                    if let Some(a) = library_row(
                        ui,
                        pal,
                        label,
                        &format!(
                            "SoundFont · GM program {program} · {family} · {selected}\n\n\
                             Click + to add a track, ▶ to hear it."
                        ),
                        theme::rgb(FONT_COLOR),
                        false,
                        !full,
                        ui.id().with(("library-row", "font", program)),
                        QuickSound::Font(font.clone()),
                    ) {
                        *action = Some(if a {
                            LibraryAction::AddFont(font)
                        } else {
                            LibraryAction::AuditionFont(font)
                        });
                    }
                    if first_row == Rect::NOTHING {
                        first_row = Rect::from_min_size(next.min, vec2(next.width(), 42.0));
                    }
                }
            });
        }
        self.library_rects.first_font_row = first_row;
        if rows.is_empty() {
            theme::hint(ui, pal, "No SoundFont programs match.");
        }
        let n = self.fonts.errors.len();
        if n > 0 {
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("{n} font file{} failed to load", plural(n)))
                    .small()
                    .color(pal.warn),
            )
            .on_hover_text(self.fonts.errors.join("\n"));
        }
        ui.add_space(10.0);
    }

    /// The no-fonts hint card: explains what SoundFonts are and offers one
    /// "Add a SoundFont…" button that opens (and creates) the user font
    /// folder, plus a click-to-copy path line like the packs footer.
    fn fonts_hint_card(&mut self, ui: &mut Ui, pal: Palette) {
        Frame::new()
            .fill(pal.raised)
            .corner_radius(8)
            .inner_margin(Margin::same(10))
            .stroke(Stroke::new(1.0_f32, pal.border_soft))
            .show(ui, |ui| {
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
                ui.add_space(4.0);
                let resp = ui
                    .add(egui::Button::new("Add a SoundFont…").min_size(vec2(0.0, 26.0)))
                    .on_hover_text("Open the SoundFonts folder so you can drop .sf2 files into it");
                self.library_rects.font_hint = resp.rect;
                if resp.clicked() {
                    self.reveal_fonts_folder();
                }
                let dir = crate::font::user_fonts_dir();
                let path = ui.add(
                    egui::Label::new(
                        RichText::new(format!("Folder: {}", dir.display()))
                            .small()
                            .color(pal.text_faint),
                    )
                    .truncate()
                    .sense(Sense::click()),
                );
                if path
                    .on_hover_text(format!("{}\nClick to copy the path", dir.display()))
                    .clicked()
                {
                    ui.ctx().copy_text(dir.display().to_string());
                    self.set_status(
                        StatusKind::Info,
                        "Copied the SoundFonts folder path to the clipboard.",
                    );
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

    /// The "+ Track" fast path at the top of the Track tab: a collapsible
    /// strip of common sounds — the song's own sounds first, then built-ins,
    /// a few pack presets and SoundFont GM programs — that adds a track
    /// without opening the full instrument library. Open state lives in egui
    /// temp data.
    fn quick_add_ui(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        let picks = self.quick_picks();
        let n = picks.count();
        let open = disclosure_header(
            ui,
            pal,
            QUICK_ADD_OPEN,
            "QUICK ADD",
            "+ Track",
            Some((
                RichText::new(format!("{n} sound{}", plural(n)))
                    .small()
                    .color(pal.text_faint),
                "Sounds the quick add offers",
            )),
            false,
        );
        // Keep stale rects out of hit-tests when the strip is folded.
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new(QUICK_ADD_FONT_RECT), Rect::NOTHING);
            d.insert_temp(egui::Id::new(QUICK_ADD_FONT_HINT_RECT), Rect::NOTHING);
        });
        if !open {
            return;
        }
        ui.add_space(2.0);
        let full = self.project.tracks.len() >= ops::MAX_TRACKS;
        let mut picked = None;
        let mut font_row = Rect::NOTHING;
        let mut font_hint_row = Rect::NOTHING;
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
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
                    if quick_add_font_hint_row(ui, pal) {
                        self.reveal_fonts_folder();
                    }
                    font_hint_row = Rect::from_min_size(next.min, vec2(next.width(), 30.0));
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
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new(QUICK_ADD_FONT_RECT), font_row);
            d.insert_temp(egui::Id::new(QUICK_ADD_FONT_HINT_RECT), font_hint_row);
        });
        if let Some(sound) = picked {
            match sound {
                QuickSound::BuiltIn(inst) => self.add_track(inst),
                QuickSound::Preset(p) => self.add_preset_track(&p),
                QuickSound::Font(f) => self.add_font_track(&f),
            }
        }
        ui.add_space(2.0);
        let resp = ui.add(
            egui::Label::new(
                RichText::new("Open the Instruments tab for the full library")
                    .small()
                    .color(pal.link),
            )
            .sense(Sense::click()),
        );
        if resp
            .on_hover_text("Every built-in, pack preset and SoundFont program, searchable")
            .clicked()
        {
            self.side_tab = SideTab::Library;
        }
        ui.add_space(8.0);
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

    fn inspector_ui(&mut self, ui: &mut Ui) {
        let pal = self.palette();
        self.quick_add_ui(ui);
        let Some(track) = self.selected_track().cloned() else {
            theme::section_header(ui, pal, "TRACK", "No track selected");
            theme::hint(
                ui,
                pal,
                "Click a track in the arrangement, or add one from Instruments.",
            );
            return;
        };
        let id = track.id;
        let color = theme::rgb(track.color);
        theme::section_header(ui, pal, "SELECTED TRACK", &ops::instrument_label(&track));
        ui.add_space(6.0);

        ui.label(RichText::new("Name").small().color(pal.text_dim));
        let mut name = track.name.clone();
        if ui
            .add(
                egui::TextEdit::singleline(&mut name)
                    .desired_width(f32::INFINITY)
                    .char_limit(crate::model::MAX_NAME_CHARS),
            )
            .changed()
        {
            self.edit("Rename track", Some(&format!("track-name-{id}")), |p| {
                if let Some(t) = ops::track_mut(p, id) {
                    t.name = name;
                }
            });
        }
        ui.add_space(4.0);
        ui.label(RichText::new("Sound").small().color(pal.text_dim));
        self.sound_picker(ui, &track);
        if let Some(preset) = &track.preset {
            let summary = if track.font.is_some() {
                // With a font assigned the preset only plays when the .sf2
                // is missing — say so instead of implying it is the sound.
                format!(
                    "Fallback while the SoundFont is missing: {}",
                    preset_summary(preset)
                )
            } else {
                preset_summary(preset)
            };
            ui.label(RichText::new(summary).small().color(pal.text_faint));
            let installed = self.catalog.instruments.iter().find(|p| p.id == preset.id);
            match installed {
                Some(newer) if newer != preset => {
                    let p = newer.clone();
                    if ui.button("Update sound from installed pack").on_hover_text("The installed pack has a different version of this preset. The song keeps its embedded copy until you update.").clicked() {
                        self.edit("Update preset", None, |pr| {
                            if let Some(t) = ops::track_mut(pr, id) {
                                t.preset = Some(p);
                            }
                        });
                    }
                }
                None => {
                    ui.label(
                        RichText::new(
                            "Pack not installed here; the song carries its own copy of this sound.",
                        )
                        .small()
                        .color(pal.text_faint),
                    );
                }
                _ => {}
            }
        }

        ui.add_space(6.0);
        ui.label(RichText::new("Color").small().color(pal.text_dim));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
            for c in theme::TRACK_PALETTE
                .iter()
                .chain(crate::model::TRACK_COLORS.iter())
                .take(14)
            {
                let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                let on = *c == track.color;
                ui.painter().circle_filled(
                    rect.center(),
                    if resp.hovered() { 9.5 } else { 8.5 },
                    theme::rgb(*c),
                );
                if on {
                    ui.painter()
                        .circle_stroke(rect.center(), 10.0, Stroke::new(2.0_f32, pal.text));
                }
                if resp.on_hover_text("Track color").clicked() {
                    let c = *c;
                    self.edit("Track color", None, |p| {
                        if let Some(t) = ops::track_mut(p, id) {
                            t.color = c;
                        }
                    });
                }
            }
        });

        ui.add_space(10.0);
        ui.label(theme::kicker_text(pal, "MIX"));
        let mut vol = track.volume;
        let mut pan = track.pan;
        ui.horizontal(|ui| {
            ui.add_sized(
                vec2(48.0, 20.0),
                egui::Label::new(RichText::new("Volume").small().color(pal.text_dim)),
            );
            ui.spacing_mut().slider_width = (ui.available_width() - 60.0).max(80.0);
            if ui
                .add(
                    egui::Slider::new(&mut vol, 0.0..=1.0)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                )
                .changed()
            {
                self.edit("Track volume", Some(&format!("vol-{id}")), |p| {
                    if let Some(t) = ops::track_mut(p, id) {
                        t.volume = vol;
                    }
                });
            }
        });
        ui.horizontal(|ui| {
            ui.add_sized(
                vec2(48.0, 20.0),
                egui::Label::new(RichText::new("Pan").small().color(pal.text_dim)),
            );
            ui.spacing_mut().slider_width = (ui.available_width() - 60.0).max(80.0);
            let resp = ui.add(
                egui::Slider::new(&mut pan, -1.0..=1.0).custom_formatter(|v, _| pan_text(v as f32)),
            );
            if resp.changed() {
                self.edit("Track pan", Some(&format!("pan-{id}")), |p| {
                    if let Some(t) = ops::track_mut(p, id) {
                        t.pan = pan;
                    }
                });
            }
            if resp.double_clicked() {
                self.edit("Center pan", None, |p| {
                    if let Some(t) = ops::track_mut(p, id) {
                        t.pan = 0.0;
                    }
                });
            }
        });
        ui.horizontal(|ui| {
            if theme::chip(
                ui,
                pal,
                "Mute",
                track.mute,
                pal.danger,
                vec2(64.0, 26.0),
                "Silence this track",
            )
            .clicked()
            {
                self.toggle_mute(id);
            }
            if theme::chip(
                ui,
                pal,
                "Solo",
                track.solo,
                pal.warn,
                vec2(64.0, 26.0),
                "Hear only soloed tracks",
            )
            .clicked()
            {
                self.toggle_solo(id);
            }
        });

        ui.add_space(10.0);
        ui.label(theme::kicker_text(pal, "EFFECTS"));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            self.fx_knobs(ui, id, 40.0, color);
        });

        ui.add_space(10.0);
        ui.label(theme::kicker_text(pal, "PLAY"));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // Per-track portamento, seconds 0..=MAX_GLIDE_SECONDS (0 = off):
            // while on, melodic sounds retarget the held voice legato-style
            // instead of stacking — one note at a time (Track::glide).
            let mut norm = (track.glide / MAX_GLIDE_SECONDS).clamp(0.0, 1.0);
            if theme::knob(
                ui,
                pal,
                &mut norm,
                0.0,
                false,
                "Glide",
                &glide_text(track.glide),
                color,
                40.0,
            )
            .on_hover_text(
                "Glide (portamento): how long a held note takes to slide to the next pitch. \
                 One note at a time while above 0 s. Drag · Shift for fine · Double-click resets to off.",
            )
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
            if track.arp.enabled {
                let mut gate_norm =
                    ((track.arp.gate - MIN_ARP_GATE) / (1.0 - MIN_ARP_GATE)).clamp(0.0, 1.0);
                let gate_default = (0.9 - MIN_ARP_GATE) / (1.0 - MIN_ARP_GATE);
                if theme::knob(
                    ui,
                    pal,
                    &mut gate_norm,
                    gate_default,
                    false,
                    "Gate",
                    &format!("{:.0}%", track.arp.gate * 100.0),
                    color,
                    40.0,
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
        if track.font.is_some() {
            theme::hint(
                ui,
                pal,
                "Glide doesn't bend SoundFonts — it applies to the missing-font fallback sound.",
            );
        } else {
            theme::hint(
                ui,
                pal,
                "One note at a time while above 0 s — held notes slide to the next pitch.",
            );
        }

        ui.add_space(6.0);
        // Per-track live arpeggiator (Track::arp): while on, every chord on
        // the timeline (2+ notes starting together) subdivides into a
        // cycling run of hits; single notes play unchanged. Controls collect
        // a pick each so the undoable edit lands after the widgets.
        let arp = track.arp;
        let mut toggle_arp = false;
        let mut rate_pick = None;
        let mut octave_pick = None;
        let mut direction_pick = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if theme::chip(
                ui,
                pal,
                "Arp",
                arp.enabled,
                color,
                vec2(44.0, 26.0),
                "Arpeggiator: split held chords into a cycling note run",
            )
            .clicked()
            {
                toggle_arp = true;
            }
            if arp.enabled {
                egui::ComboBox::from_id_salt(("arp-rate", id))
                    .width(56.0)
                    .selected_text(arp.rate.label())
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
                    });
                egui::ComboBox::from_id_salt(("arp-octaves", id))
                    .width(64.0)
                    .selected_text(format!("{} oct", arp.octaves))
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
                    });
                egui::ComboBox::from_id_salt(("arp-direction", id))
                    .width(90.0)
                    .selected_text(arp.direction.label())
                    .show_ui(ui, |ui| {
                        for direction in ArpDirection::ALL {
                            if ui
                                .selectable_label(direction == arp.direction, direction.label())
                                .clicked()
                            {
                                direction_pick = Some(direction);
                            }
                        }
                    });
            }
        });
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
        if arp.enabled {
            theme::hint(
                ui,
                pal,
                if ops::arp_audible(&track) {
                    "Held chords split into a cycling run; single notes play unchanged."
                } else {
                    "Arp is idle here — drum and kit sounds have no chords to split."
                },
            );
        }

        ui.add_space(10.0);
        ui.label(theme::kicker_text(pal, "TRACK"));
        let clips = track.clips.len();
        ui.label(
            RichText::new(format!(
                "{} pattern{} · {} clip{}",
                track.patterns.len(),
                plural(track.patterns.len()),
                clips,
                plural(clips)
            ))
            .small()
            .color(pal.text_faint),
        );
        ui.horizontal_wrapped(|ui| {
            let can_add = self.project.tracks.len() < ops::MAX_TRACKS;
            if ui
                .add_enabled(can_add, egui::Button::new("Duplicate"))
                .on_hover_text("Copy this track with all its patterns and clips")
                .clicked()
            {
                let mut new = None;
                self.edit("Duplicate track", None, |p| {
                    new = ops::duplicate_track(p, id)
                });
                if let Some(new) = new {
                    self.select_track(new);
                }
            }
            let idx = ops::track_index(&self.project, id).unwrap_or(0);
            if ui
                .add_enabled(idx > 0, egui::Button::new("Move up"))
                .clicked()
            {
                self.edit("Move track", None, |p| {
                    ops::move_track(p, id, -1);
                });
            }
            if ui
                .add_enabled(
                    idx + 1 < self.project.tracks.len(),
                    egui::Button::new("Move down"),
                )
                .clicked()
            {
                self.edit("Move track", None, |p| {
                    ops::move_track(p, id, 1);
                });
            }
            let del = egui::Button::new(RichText::new("Delete track").color(pal.danger))
                .stroke(Stroke::new(1.0_f32, pal.danger.gamma_multiply(0.5)));
            if ui
                .add(del)
                .on_hover_text("Remove this track (undo with Cmd/Ctrl+Z)")
                .clicked()
            {
                self.delete_track(id);
            }
        });
    }

    fn sound_picker(&mut self, ui: &mut Ui, track: &crate::model::Track) {
        let pal = self.palette();
        let id = track.id;
        // Font tracks show the full "Font · Program" label so the font name
        // stays visible in the picker.
        let current = ops::instrument_label(track).into_owned();
        let mut sound_pick: Option<(Instrument, Option<InstrumentPreset>)> = None;
        let mut font_pick: Option<FontPick> = None;
        egui::ComboBox::from_id_salt(("sound", id))
            .width(ui.available_width() - 8.0)
            .selected_text(current)
            .height(360.0)
            .show_ui(ui, |ui| {
                ui.label(theme::kicker_text(pal, "BUILT-IN"));
                for inst in Instrument::ALL {
                    // The check mark shows the *active* sound: while a font
                    // is assigned, built-ins are only the fallback.
                    let on =
                        track.font.is_none() && track.preset.is_none() && track.instrument == inst;
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
                    let on = track.font.is_none()
                        && track.preset.as_ref().is_some_and(|p| p.id == preset.id);
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
                self.font_section(ui, track, &mut font_pick);
            });
        if let Some(pick) = font_pick {
            let font = match pick {
                FontPick::Assign(f) => Some(f),
                FontPick::Clear => None,
            };
            self.apply_font_pick(track, font);
        }
        if let Some((inst, preset)) = sound_pick {
            self.apply_sound_pick(track, inst, preset);
        }
        self.font_program_picker(ui, track);
        // Discoverability: with no fonts installed, point at the folder once,
        // quietly, under the picker.
        if self.fonts.fonts.is_empty() {
            ui.add_space(2.0);
            theme::hint(
                ui,
                pal,
                &format!(
                    "SoundFonts: drop .sf2 files into {}",
                    crate::font::user_fonts_dir().display()
                ),
            );
        }
    }

    /// Applies a built-in/preset pick from the sound combo. Choosing either
    /// clears the SoundFont reference: an explicit preset/built-in choice is
    /// the sound the user asked for, and a lingering font would keep
    /// overriding it (fonts win over presets at playback).
    pub(super) fn apply_sound_pick(
        &mut self,
        track: &crate::model::Track,
        inst: Instrument,
        preset: Option<InstrumentPreset>,
    ) {
        let id = track.id;
        let was_drums = track.shows_drum_grid();
        let label = preset
            .as_ref()
            .map_or(inst.name().to_owned(), |p| p.name.clone());
        let default_name = has_default_sound_name(track);
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
    pub(super) fn apply_font_pick(&mut self, track: &crate::model::Track, font: Option<FontRef>) {
        let id = track.id;
        let was_drums = track.shows_drum_grid();
        let default_name = has_default_sound_name(track);
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
    fn font_section(&self, ui: &mut Ui, track: &crate::model::Track, pick: &mut Option<FontPick>) {
        let pal = self.palette();
        ui.separator();
        ui.label(theme::kicker_text(pal, "SOUNDFONT"));
        let catalog_empty = self.fonts.fonts.is_empty();
        if catalog_empty {
            theme::hint(
                ui,
                pal,
                &format!(
                    "No .sf2 fonts installed — drop files into {}",
                    crate::font::user_fonts_dir().display()
                ),
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

    /// The GM program combo shown under the sound picker while a SoundFont is
    /// assigned: all 128 programs grouped under their 16 family headers.
    fn font_program_picker(&mut self, ui: &mut Ui, track: &crate::model::Track) {
        let Some(font) = track.font.clone() else {
            return;
        };
        let pal = self.palette();
        let id = track.id;
        ui.add_space(4.0);
        ui.label(RichText::new("Program").small().color(pal.text_dim));
        let mut pick = None;
        egui::ComboBox::from_id_salt(("font-program", id))
            .width(ui.available_width() - 8.0)
            .selected_text(font.program_name())
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

/// One library entry. `drag_id` names the row's drag-and-drop source and
/// must be unique — the caller salts it with the preset id or the built-in
/// name. `payload` is what an arranger drop turns into a track.
///
/// The click targets are registered *after* the drag source on purpose:
/// `dnd_drag_source` claims a drag interact on the row rect that would sit
/// on top of any interact inside the body, and egui drops click hits under
/// a drag-only top widget entirely — so interacts inside the body could
/// never fire. Registered here instead, they're the topmost click widgets
/// while drags still resolve to the row's drag source. Hover feedback is
/// painted afterwards for the same reason. Returns `Some(true)` for add,
/// `Some(false)` for audition.
#[allow(clippy::too_many_arguments)]
fn library_row(
    ui: &mut Ui,
    pal: Palette,
    name: &str,
    description: &str,
    color: Color32,
    kit: bool,
    can_add: bool,
    drag_id: egui::Id,
    payload: QuickSound,
) -> Option<bool> {
    let dnd = ui.dnd_drag_source(drag_id, payload, |ui| {
        library_row_body(ui, pal, name, description, color, kit, can_add)
    });
    let rect = dnd.response.rect;
    let audition_rect = Rect::from_center_size(
        Pos2::new(rect.right() - 50.0, rect.center().y),
        vec2(26.0, 26.0),
    );
    let add_rect = Rect::from_center_size(
        Pos2::new(rect.right() - 18.0, rect.center().y),
        vec2(26.0, 26.0),
    );
    let row = ui
        .interact(rect, drag_id.with("row"), Sense::click())
        .on_hover_text(format!(
            "{description}\n\nClick + to add a track, ▶ to hear it. Double-click the row to add, or drag it onto the arrangement."
        ));
    row.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, can_add, format!("Add {name}"))
    });
    let audition = ui
        .interact(audition_rect, drag_id.with("audition"), Sense::click())
        .on_hover_text(format!("Audition {name}"));
    let add = ui
        .interact(add_rect, drag_id.with("add"), Sense::click())
        .on_hover_text(if can_add {
            format!("Add a {name} track")
        } else {
            "Track limit reached".to_owned()
        });
    let p = ui.painter();
    if row.hovered() || audition.hovered() || add.hovered() {
        p.rect_filled(rect, 8.0, HOVER_LIFT);
    }
    for (r, hot) in [
        (audition_rect, audition.hovered()),
        (add_rect, add.hovered() && can_add),
    ] {
        if hot {
            p.rect_filled(r, 6.0, BUTTON_LIFT);
        }
    }
    if audition.clicked() {
        Some(false)
    } else if can_add && (add.clicked() || row.double_clicked()) {
        Some(true)
    } else {
        None
    }
}

/// The painted body of [`library_row`] (split out so the row can run inside
/// [`Ui::dnd_drag_source`]). Paints the base, non-hover state only: hover and
/// clicks live in [`library_row`], which registers its interacts *after* the
/// drag source — interacts here would sit under the drag interact and could
/// never fire.
#[allow(clippy::too_many_arguments)]
fn library_row_body(
    ui: &mut Ui,
    pal: Palette,
    name: &str,
    description: &str,
    color: Color32,
    kit: bool,
    can_add: bool,
) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 42.0), Sense::hover());
    let audition_rect = Rect::from_center_size(
        Pos2::new(rect.right() - 50.0, rect.center().y),
        vec2(26.0, 26.0),
    );
    let add_rect = Rect::from_center_size(
        Pos2::new(rect.right() - 18.0, rect.center().y),
        vec2(26.0, 26.0),
    );
    let p = ui.painter();
    p.rect(
        rect,
        8.0,
        pal.control,
        Stroke::new(1.0_f32, pal.border_soft),
        StrokeKind::Inside,
    );
    let bar = Rect::from_min_size(rect.min + vec2(8.0, 9.0), vec2(4.0, rect.height() - 18.0));
    p.rect_filled(bar, 2.0, color);
    let text_w = rect.width() - 96.0;
    let galley =
        ui.fonts_mut(|f| f.layout_no_wrap(name.to_owned(), FontId::proportional(13.5), pal.text));
    p.with_clip_rect(Rect::from_min_size(
        rect.min,
        vec2(text_w + 20.0, rect.height()),
    ))
    .galley(
        Pos2::new(rect.left() + 20.0, rect.top() + 6.0),
        galley,
        pal.text,
    );
    let first_line = description.lines().next().unwrap_or("");
    let desc = ui.fonts_mut(|f| {
        f.layout_no_wrap(
            first_line.to_owned(),
            FontId::proportional(10.5),
            pal.text_faint,
        )
    });
    p.with_clip_rect(Rect::from_min_size(
        rect.min,
        vec2(text_w + 20.0, rect.height()),
    ))
    .galley(
        Pos2::new(rect.left() + 20.0, rect.top() + 24.0),
        desc,
        pal.text_faint,
    );
    if kit {
        p.text(
            Pos2::new(rect.right() - 68.0, rect.top() + 6.0),
            Align2::RIGHT_TOP,
            "KIT",
            FontId::proportional(9.0),
            pal.text_faint,
        );
    }
    for r in [audition_rect, add_rect] {
        p.rect(
            r,
            6.0,
            pal.raised,
            Stroke::new(1.0_f32, pal.border),
            StrokeKind::Inside,
        );
    }
    theme::paint_icon(
        p,
        Rect::from_center_size(audition_rect.center() + vec2(1.0, 0.0), vec2(9.0, 10.0)),
        Icon::Play,
        pal.text_dim,
    );
    theme::paint_icon(
        p,
        Rect::from_center_size(add_rect.center(), vec2(10.0, 10.0)),
        Icon::Plus,
        if can_add { pal.text } else { pal.text_faint },
    );
}

/// Slim clickable section header with a disclosure chevron (▸ closed, ▾
/// open). The whole row toggles; `open` persists in egui temp data under
/// `key` so it survives tab switches for the session. `right` is an optional
/// right-aligned label with hover text. Returns the open state to use for
/// this frame's body — already flipped when the header was clicked.
///
/// The row is a [`UiBuilder::sense`] scope, so its click sense registers
/// *below* the labels inside it: clicks anywhere on the row toggle, while a
/// `right` label keeps its own hover tooltip.
fn disclosure_header(
    ui: &mut Ui,
    pal: Palette,
    key: &str,
    kicker: &str,
    title: &str,
    right: Option<(RichText, &'static str)>,
    default_open: bool,
) -> bool {
    let id = egui::Id::new(key);
    let mut open = ui
        .ctx()
        .data_mut(|d| d.get_temp::<bool>(id))
        .unwrap_or(default_open);
    // Bound the row's max_rect the way `ui.horizontal` does (one interact
    // height, content may still grow it): an unbounded max_rect would let the
    // Center cross-align stretch every child frame to the panel's full
    // remaining height and swallow the whole sidebar.
    let avail = ui.available_rect_before_wrap();
    let row_rect =
        Rect::from_min_size(avail.min, vec2(avail.width(), ui.spacing().interact_size.y));
    let mut chev = Rect::NOTHING;
    let resp = ui
        .scope_builder(
            UiBuilder::new()
                .max_rect(row_rect)
                .layout(Layout::left_to_right(Align::Center))
                .sense(Sense::click()),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (r, _) = ui.allocate_exact_size(vec2(12.0, 16.0), Sense::hover());
                chev = r;
                theme::section_header(ui, pal, kicker, title);
                if let Some((text, tip)) = right {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(text).on_hover_text(tip);
                    });
                }
            },
        )
        .response;
    paint_disclosure(
        ui.painter(),
        chev,
        open,
        if resp.hovered() {
            pal.text_dim
        } else {
            pal.text_faint
        },
    );
    if resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_temp(id, open));
    }
    open
}

/// The little disclosure chevron: ▸ when closed, ▾ when open.
fn paint_disclosure(p: &egui::Painter, r: Rect, open: bool, color: Color32) {
    let c = r.center();
    let stroke = Stroke::new(1.6_f32, color);
    let (hx, hy) = (r.width() * 0.30, r.height() * 0.22);
    let [a, b, d] = if open {
        // ▾ — vertex at bottom center.
        [c + vec2(-hx, -hy), c + vec2(0.0, hy), c + vec2(hx, -hy)]
    } else {
        // ▸ — vertex at right center.
        [c + vec2(-hy, -hx), c + vec2(hy, 0.0), c + vec2(-hy, hx)]
    };
    p.line_segment([a, b], stroke);
    p.line_segment([b, d], stroke);
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
        let tip = if full {
            "Track limit reached — delete a track to add another.".to_owned()
        } else {
            sound.tip()
        };
        let next = ui.available_rect_before_wrap();
        let dnd_id = ui.id().with(("quick-add", label, i));
        // Draggable like the library rows: the row paints the ghost while
        // dragged, and dropping it on the arranger adds the track there.
        let dnd = ui.dnd_drag_source(dnd_id, sound.clone(), |ui| {
            quick_add_row(ui, pal, sound, !full)
        });
        // The drag interact sits on top of the body, which would swallow any
        // click target inside it — register the click here, on top, so clicks
        // and drags resolve to different senses of the same row.
        let clicked = ui
            .interact(dnd.response.rect, dnd_id.with("click"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(&tip);
        if clicked.hovered() && !full {
            ui.painter().rect(
                dnd.response.rect,
                7.0,
                HOVER_LIFT,
                Stroke::new(1.0_f32, pal.border),
                StrokeKind::Inside,
            );
        }
        if clicked.clicked() && !full {
            *picked = Some(sound.clone());
        }
        if let Some(out) = first_row.as_deref_mut() {
            if *out == Rect::NOTHING {
                *out = Rect::from_min_size(next.min, vec2(next.width(), 30.0));
            }
        }
    }
}

/// The slim "Add a SoundFont…" row the quick-add strip shows while no `.sf2`
/// is installed: same card look as the other quick rows, one click opens the
/// fonts folder. Returns true when the click should fire.
fn quick_add_font_hint_row(ui: &mut Ui, pal: Palette) -> bool {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
    let p = ui.painter();
    p.rect(
        rect,
        7.0,
        if resp.hovered() {
            pal.control_hover
        } else {
            pal.control
        },
        Stroke::new(
            1.0_f32,
            if resp.hovered() {
                pal.border
            } else {
                pal.border_soft
            },
        ),
        StrokeKind::Inside,
    );
    let bar = Rect::from_min_size(rect.min + vec2(7.0, 6.0), vec2(3.0, rect.height() - 12.0));
    p.rect_filled(bar, 1.5, theme::rgb(FONT_COLOR));
    let galley = ui.fonts_mut(|f| {
        f.layout_no_wrap(
            "Add a SoundFont…".to_owned(),
            FontId::proportional(12.5),
            pal.text,
        )
    });
    p.with_clip_rect(Rect::from_min_size(
        rect.min,
        vec2(rect.width() - 48.0, rect.height()),
    ))
    .galley(
        Pos2::new(rect.left() + 18.0, rect.center().y - galley.size().y * 0.5),
        galley,
        pal.text,
    );
    theme::paint_icon(
        p,
        Rect::from_center_size(
            Pos2::new(rect.right() - 15.0, rect.center().y),
            vec2(9.0, 9.0),
        ),
        Icon::Plus,
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

/// The painted body of a quick-add row: rounded card, color bar, name. The
/// click target lives in [`quick_section`], which must register it *after*
/// [`Ui::dnd_drag_source`]'s drag interact — a drag-only widget on top makes
/// egui drop the click hit entirely, so an interact inside the body could
/// never fire. Hover feedback is painted by the caller for the same reason.
fn quick_add_row(ui: &mut Ui, pal: Palette, sound: &QuickSound, can_add: bool) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
    let p = ui.painter();
    p.rect(
        rect,
        7.0,
        if can_add { pal.control } else { pal.panel },
        Stroke::new(1.0_f32, pal.border_soft),
        StrokeKind::Inside,
    );
    let bar = Rect::from_min_size(rect.min + vec2(7.0, 6.0), vec2(3.0, rect.height() - 12.0));
    p.rect_filled(bar, 1.5, theme::rgb(sound.color()));
    let ink = if can_add { pal.text } else { pal.text_faint };
    let galley = ui
        .fonts_mut(|f| f.layout_no_wrap(sound.name().to_owned(), FontId::proportional(12.5), ink));
    p.with_clip_rect(Rect::from_min_size(
        rect.min,
        vec2(rect.width() - 48.0, rect.height()),
    ))
    .galley(
        Pos2::new(rect.left() + 18.0, rect.center().y - galley.size().y * 0.5),
        galley,
        ink,
    );
    if sound.is_kit() {
        p.text(
            Pos2::new(rect.right() - 34.0, rect.center().y),
            Align2::RIGHT_CENTER,
            "KIT",
            FontId::proportional(8.5),
            pal.text_faint,
        );
    }
    theme::paint_icon(
        p,
        Rect::from_center_size(
            Pos2::new(rect.right() - 15.0, rect.center().y),
            vec2(9.0, 9.0),
        ),
        Icon::Plus,
        if can_add {
            pal.text_dim
        } else {
            pal.text_faint
        },
    );
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
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1360.0, 860.0))),
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
}
