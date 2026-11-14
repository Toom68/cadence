//! Left sidebar: instrument library (built-ins + JSON packs) and track inspector.

use egui::{
    vec2, Align, Align2, Color32, Context, FontId, Frame, Layout, Margin, Pos2, Rect, RichText,
    Sense, Stroke, StrokeKind, Ui,
};

use super::theme::{
    self, Icon, BORDER, BORDER_SOFT, CONTROL, CONTROL_HOVER, CORAL, LIME, PERIWINKLE, RAISED, TEXT,
    TEXT_DIM, TEXT_FAINT,
};
use super::{ops, CadenceApp, Focus, StatusKind, Tab};
use crate::model::{create_track, Instrument};
use crate::packs::InstrumentPreset;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum SideTab {
    #[default]
    Library,
    Track,
}

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

enum LibraryAction {
    Add(Instrument),
    AddPreset(InstrumentPreset),
    Audition(Instrument),
    AuditionPreset(InstrumentPreset),
}

impl CadenceApp {
    pub(super) fn sidebar(&mut self, ctx: &Context) {
        let frame = Frame::new()
            .fill(theme::PANEL)
            .inner_margin(Margin {
                left: 12,
                right: 10,
                top: 10,
                bottom: 8,
            })
            .stroke(Stroke::new(1.0_f32, BORDER_SOFT));
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(272.0)
            .width_range(240.0..=380.0)
            .frame(frame)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let w = (ui.available_width() - 4.0) / 2.0;
                    for (tab, label) in
                        [(SideTab::Library, "Instruments"), (SideTab::Track, "Track")]
                    {
                        let on = self.side_tab == tab;
                        let b = egui::Button::new(RichText::new(label).color(if on {
                            theme::INK
                        } else {
                            TEXT_DIM
                        }))
                        .fill(if on { LIME } else { CONTROL })
                        .min_size(vec2(w, 28.0));
                        if ui.add(b).clicked() {
                            self.side_tab = tab;
                        }
                    }
                });
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.side_tab {
                        SideTab::Library => self.library_ui(ui),
                        SideTab::Track => self.inspector_ui(ui),
                    });
            });
    }

    fn library_ui(&mut self, ui: &mut Ui) {
        let full = self.project.tracks.len() >= ops::MAX_TRACKS;
        ui.horizontal(|ui| {
            theme::section_header(ui, "LIBRARY", "Add an instrument");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{}/{}", self.project.tracks.len(), ops::MAX_TRACKS))
                        .small()
                        .color(if full { CORAL } else { TEXT_FAINT }),
                )
                .on_hover_text("Tracks in this song / maximum");
            });
        });
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.library_filter)
                .hint_text("Search instruments…")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(6.0);
        if full {
            ui.label(
                RichText::new("Track limit reached. Delete a track to add another.")
                    .small()
                    .color(CORAL),
            );
        }
        let filter = self.library_filter.trim().to_lowercase();
        let matches = |name: &str, desc: &str| {
            filter.is_empty()
                || name.to_lowercase().contains(&filter)
                || desc.to_lowercase().contains(&filter)
        };
        let mut action = None;

        ui.add_space(2.0);
        ui.label(theme::kicker_text("BUILT-IN"));
        for inst in Instrument::ALL {
            if !matches(inst.name(), inst.description()) {
                continue;
            }
            if let Some(a) = library_row(
                ui,
                inst.name(),
                inst.description(),
                theme::rgb(instrument_color(inst)),
                inst.is_drums(),
                !full,
            ) {
                action = Some(if a {
                    LibraryAction::Add(inst)
                } else {
                    LibraryAction::Audition(inst)
                });
            }
        }

        let mut current_pack = String::new();
        let presets = self.catalog.instruments.clone();
        for preset in &presets {
            if !matches(&preset.name, &preset.description) {
                continue;
            }
            let title = pack_title(&preset.id);
            if title != current_pack {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(theme::kicker_text(&format!(
                        "PACK · {}",
                        title.to_uppercase()
                    )));
                });
                current_pack = title;
            }
            let tip = format!(
                "{}\n\n{}\nid: {}",
                preset.description,
                preset_summary(preset),
                preset.id
            );
            if let Some(a) = library_row(
                ui,
                &preset.name,
                &tip,
                theme::rgb(ops::preset_color(&preset.id)),
                false,
                !full,
            ) {
                action = Some(if a {
                    LibraryAction::AddPreset(preset.clone())
                } else {
                    LibraryAction::AuditionPreset(preset.clone())
                });
            }
        }
        if !filter.is_empty()
            && current_pack.is_empty()
            && !Instrument::ALL
                .iter()
                .any(|i| matches(i.name(), i.description()))
        {
            theme::hint(ui, "No instruments match your search.");
        }

        ui.add_space(10.0);
        self.packs_footer(ui);

        match action {
            Some(LibraryAction::Add(inst)) => self.add_track(inst),
            Some(LibraryAction::AddPreset(p)) => self.add_preset_track(&p),
            Some(LibraryAction::Audition(inst)) => {
                self.audition_instrument(&create_track(inst), inst.default_pitch())
            }
            Some(LibraryAction::AuditionPreset(p)) => {
                let mut t = create_track(Instrument::Lead);
                t.preset = Some(p);
                self.audition_instrument(&t, 60);
            }
            None => {}
        }
    }

    fn packs_footer(&mut self, ui: &mut Ui) {
        Frame::new().fill(RAISED).corner_radius(8).inner_margin(Margin::same(10)).stroke(Stroke::new(1.0_f32, BORDER_SOFT)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::kicker_text("INSTRUMENT PACKS"));
            ui.label(RichText::new("Packs are small JSON files of synth presets. No rebuild or restart needed.").small().color(TEXT_DIM));
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
                let text = RichText::new(format!("{n} pack problem{} — details", if n == 1 { "" } else { "s" })).small().color(theme::AMBER);
                if ui.add(egui::Label::new(text).sense(Sense::click())).on_hover_text("Show which pack files could not be loaded and why").clicked() {
                    self.show_pack_errors();
                }
            }
            let dir = crate::packs::user_packs_dir();
            let resp = ui.add(egui::Label::new(RichText::new(format!("Folder: {}", dir.display())).small().color(TEXT_FAINT)).truncate().sense(Sense::click()));
            if resp.on_hover_text(format!("{}\nClick to copy the path", dir.display())).clicked() {
                ui.ctx().copy_text(dir.display().to_string());
                self.set_status(StatusKind::Info, "Copied the packs folder path to the clipboard.");
            }
            ui.add_space(2.0);
            ui.label(RichText::new("Plugin instruments (VST3 / AU / CLAP) are planned but not supported yet.").small().color(TEXT_FAINT));
        });
    }

    pub(super) fn add_preset_track(&mut self, preset: &InstrumentPreset) {
        let mut id = None;
        self.edit(&format!("Add {} track", preset.name), None, |p| {
            id = ops::add_preset_track(p, preset)
        });
        match id {
            Some(id) => {
                self.select_track(id);
                self.tab = Tab::Editor;
                self.focus = Focus::Editor;
                self.arranger.reveal_track = Some(id);
                self.set_status(StatusKind::Success, format!("Added {}. Draw notes in the piano roll; its clip is already in the arrangement at bar 1.", preset.name));
            }
            None => self.set_status(
                StatusKind::Warning,
                format!("Track limit reached ({} tracks).", ops::MAX_TRACKS),
            ),
        }
    }

    fn audition_instrument(&mut self, track: &crate::model::Track, pitch: u8) {
        match &self.engine {
            Some(engine) => {
                if track.instrument.is_drums() && track.preset.is_none() {
                    engine.preview(track, 36, 0.4, 0.9);
                    engine.preview(track, 42, 0.2, 0.6);
                } else {
                    engine.preview(track, pitch, 0.7, 0.8);
                }
            }
            None => self.set_status(
                StatusKind::Warning,
                "No audio output device, so instruments can't be auditioned.",
            ),
        }
    }

    fn inspector_ui(&mut self, ui: &mut Ui) {
        let Some(track) = self.selected_track().cloned() else {
            theme::section_header(ui, "TRACK", "No track selected");
            theme::hint(
                ui,
                "Click a track in the arrangement, or add one from Instruments.",
            );
            return;
        };
        let id = track.id;
        let color = theme::rgb(track.color);
        theme::section_header(ui, "SELECTED TRACK", ops::instrument_label(&track));
        ui.add_space(6.0);

        ui.label(RichText::new("Name").small().color(TEXT_DIM));
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
        ui.label(RichText::new("Sound").small().color(TEXT_DIM));
        self.sound_picker(ui, &track);
        if let Some(preset) = &track.preset {
            ui.label(
                RichText::new(preset_summary(preset))
                    .small()
                    .color(TEXT_FAINT),
            );
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
                        .color(TEXT_FAINT),
                    );
                }
                _ => {}
            }
        }

        ui.add_space(6.0);
        ui.label(RichText::new("Color").small().color(TEXT_DIM));
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
                        .circle_stroke(rect.center(), 10.0, Stroke::new(2.0_f32, TEXT));
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
        ui.label(theme::kicker_text("MIX"));
        let mut vol = track.volume;
        let mut pan = track.pan;
        ui.horizontal(|ui| {
            ui.add_sized(
                vec2(48.0, 20.0),
                egui::Label::new(RichText::new("Volume").small().color(TEXT_DIM)),
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
                egui::Label::new(RichText::new("Pan").small().color(TEXT_DIM)),
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
                "Mute",
                track.mute,
                CORAL,
                vec2(64.0, 26.0),
                "Silence this track",
            )
            .clicked()
            {
                self.toggle_mute(id);
            }
            if theme::chip(
                ui,
                "Solo",
                track.solo,
                theme::AMBER,
                vec2(64.0, 26.0),
                "Hear only soloed tracks",
            )
            .clicked()
            {
                self.toggle_solo(id);
            }
        });

        ui.add_space(10.0);
        ui.label(theme::kicker_text("EFFECTS"));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            self.fx_knobs(ui, id, 40.0, color);
        });

        ui.add_space(10.0);
        ui.label(theme::kicker_text("TRACK"));
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
            .color(TEXT_FAINT),
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
            let del = egui::Button::new(RichText::new("Delete track").color(CORAL))
                .stroke(Stroke::new(1.0_f32, CORAL.gamma_multiply(0.5)));
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
        let id = track.id;
        let current = ops::instrument_label(track).to_owned();
        let mut choice: Option<(Instrument, Option<InstrumentPreset>)> = None;
        egui::ComboBox::from_id_salt(("sound", id))
            .width(ui.available_width() - 8.0)
            .selected_text(current)
            .height(360.0)
            .show_ui(ui, |ui| {
                ui.label(theme::kicker_text("BUILT-IN"));
                for inst in Instrument::ALL {
                    let on = track.preset.is_none() && track.instrument == inst;
                    if ui
                        .selectable_label(on, inst.name())
                        .on_hover_text(inst.description())
                        .clicked()
                    {
                        choice = Some((inst, None));
                    }
                }
                let mut pack = String::new();
                for preset in &self.catalog.instruments {
                    let title = pack_title(&preset.id);
                    if title != pack {
                        ui.separator();
                        ui.label(theme::kicker_text(&title.to_uppercase()));
                        pack = title;
                    }
                    let on = track.preset.as_ref().is_some_and(|p| p.id == preset.id);
                    if ui
                        .selectable_label(on, &preset.name)
                        .on_hover_text(&preset.description)
                        .clicked()
                    {
                        choice = Some((Instrument::Lead, Some(preset.clone())));
                    }
                }
            });
        if let Some((inst, preset)) = choice {
            let was_drums = track.instrument.is_drums() && track.preset.is_none();
            let label = preset
                .as_ref()
                .map_or(inst.name().to_owned(), |p| p.name.clone());
            let default_name = ops::instrument_label(track) == track.name;
            self.edit("Change sound", None, |p| {
                if let Some(t) = ops::track_mut(p, id) {
                    t.instrument = inst;
                    t.preset = preset;
                    if default_name {
                        t.name = label.clone();
                    }
                }
            });
            let is_drums =
                inst.is_drums() && self.selected_track().is_some_and(|t| t.preset.is_none());
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
                let pitch = if t.instrument.is_drums() && t.preset.is_none() {
                    36
                } else {
                    60
                };
                self.audition_instrument(&t, pitch);
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

/// One library entry. Returns `Some(true)` for add, `Some(false)` for audition.
fn library_row(
    ui: &mut Ui,
    name: &str,
    description: &str,
    color: Color32,
    drums: bool,
    can_add: bool,
) -> Option<bool> {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 42.0), Sense::click());
    let audition_rect = Rect::from_center_size(
        Pos2::new(rect.right() - 50.0, rect.center().y),
        vec2(26.0, 26.0),
    );
    let add_rect = Rect::from_center_size(
        Pos2::new(rect.right() - 18.0, rect.center().y),
        vec2(26.0, 26.0),
    );
    let audition = ui.interact(audition_rect, resp.id.with("audition"), Sense::click());
    let add = ui.interact(add_rect, resp.id.with("add"), Sense::click());
    let hovered = resp.hovered() || audition.hovered() || add.hovered();
    let p = ui.painter();
    p.rect(
        rect,
        8.0,
        if hovered { CONTROL_HOVER } else { CONTROL },
        Stroke::new(1.0_f32, if hovered { BORDER } else { BORDER_SOFT }),
        StrokeKind::Inside,
    );
    let bar = Rect::from_min_size(rect.min + vec2(8.0, 9.0), vec2(4.0, rect.height() - 18.0));
    p.rect_filled(bar, 2.0, color);
    let text_w = rect.width() - 96.0;
    let galley =
        ui.fonts_mut(|f| f.layout_no_wrap(name.to_owned(), FontId::proportional(13.5), TEXT));
    p.with_clip_rect(Rect::from_min_size(
        rect.min,
        vec2(text_w + 20.0, rect.height()),
    ))
    .galley(
        Pos2::new(rect.left() + 20.0, rect.top() + 6.0),
        galley,
        TEXT,
    );
    let first_line = description.lines().next().unwrap_or("");
    let desc = ui.fonts_mut(|f| {
        f.layout_no_wrap(
            first_line.to_owned(),
            FontId::proportional(10.5),
            TEXT_FAINT,
        )
    });
    p.with_clip_rect(Rect::from_min_size(
        rect.min,
        vec2(text_w + 20.0, rect.height()),
    ))
    .galley(
        Pos2::new(rect.left() + 20.0, rect.top() + 24.0),
        desc,
        TEXT_FAINT,
    );
    if drums {
        p.text(
            Pos2::new(rect.right() - 68.0, rect.top() + 6.0),
            Align2::RIGHT_TOP,
            "KIT",
            FontId::proportional(9.0),
            TEXT_FAINT,
        );
    }
    for (r, hot) in [
        (audition_rect, audition.hovered()),
        (add_rect, add.hovered()),
    ] {
        p.rect(
            r,
            6.0,
            if hot { theme::CONTROL_ACTIVE } else { RAISED },
            Stroke::new(1.0_f32, BORDER),
            StrokeKind::Inside,
        );
    }
    theme::paint_icon(
        p,
        Rect::from_center_size(audition_rect.center() + vec2(1.0, 0.0), vec2(9.0, 10.0)),
        Icon::Play,
        if audition.hovered() {
            PERIWINKLE
        } else {
            TEXT_DIM
        },
    );
    theme::paint_icon(
        p,
        Rect::from_center_size(add_rect.center(), vec2(10.0, 10.0)),
        Icon::Plus,
        if !can_add {
            TEXT_FAINT
        } else if add.hovered() {
            LIME
        } else {
            TEXT
        },
    );
    let audition = audition.on_hover_text(format!("Audition {name}"));
    let add = add.on_hover_text(if can_add {
        format!("Add a {name} track")
    } else {
        "Track limit reached".to_owned()
    });
    let resp = resp.on_hover_text(format!(
        "{description}\n\nClick + to add a track, ▶ to hear it. Double-click the row to add."
    ));
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, can_add, format!("Add {name}"))
    });
    if audition.clicked() {
        Some(false)
    } else if can_add && (add.clicked() || resp.double_clicked()) {
        Some(true)
    } else {
        None
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
