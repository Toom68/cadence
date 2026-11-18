//! Mixer: one channel strip per track, the four fixed subgroup bus strips,
//! plus the master strip.

use egui::{
    pos2, vec2, Align, Color32, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
};

use super::theme;
use super::{ops, sidebar, CadenceApp};
use crate::model::{Track, MAX_CUTOFF_HZ, MIN_CUTOFF_HZ};

const STRIP_W: f32 = 156.0;
/// Narrower channel strip used by the compact mixer density.
const STRIP_W_COMPACT: f32 = 110.0;
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
        let pal = self.palette();
        let avail = ui.available_rect_before_wrap().shrink2(vec2(12.0, 10.0));
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(avail)
                .layout(Layout::left_to_right(Align::Min)),
        );
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
                    .color(pal.text_faint),
            );
            tb.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::chip(
                    ui,
                    pal,
                    "Compact",
                    compact,
                    pal.link,
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
        // Compact strips keep a fixed small height instead of stretching to
        // the panel — dense rectangles, vertically centred in the leftover
        // space. Expanded strips still fill the height.
        let strip_h = if compact { 168.0 } else { (h - 14.0).max(48.0) };
        let master_w = if compact { 120.0 } else { 150.0 };
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
        // The master strip centres vertically too when compact strips are
        // shorter than the panel.
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
        let pal = self.palette();
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
        // The pedalboard grows the strip to fit fader + pan knob + four pedal
        // cells + three arrows + the row gaps, plus frame margins. Compact
        // strips drop the inline FX knob grid (the pedalboard covers effects).
        let strip_w = if fx_open {
            34.0 + 44.0 + 4.0 * PEDAL_CELL_W + 3.0 * ARROW_W + 8.0 * gap + 14.0 + 2.0 * margin
        } else if compact {
            STRIP_W_COMPACT
        } else {
            STRIP_W
        };
        let content_w = strip_w - 2.0 * margin;
        let frame = egui::Frame::new()
            .fill(if selected {
                theme::mix(pal.raised, color, 0.07)
            } else {
                pal.raised
            })
            .stroke(Stroke::new(
                1.0_f32,
                if selected {
                    color.gamma_multiply(0.7)
                } else {
                    pal.border_soft
                },
            ))
            .corner_radius(if compact { 5 } else { 8 })
            .inner_margin(Margin::same(margin as i8));
        let resp = frame.show(ui, |ui| {
            ui.set_width(content_w);
            ui.set_height(height - 2.0 * margin);
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
                        .color(if audible { pal.text } else { pal.text_faint })
                        .strong(),
                )
                .truncate()
                .sense(Sense::click());
                let name_w = (content_w - if compact { 48.0 } else { 60.0 }).max(24.0);
                let name_h = if compact { 14.0 } else { 20.0 };
                ui.allocate_ui(vec2(name_w, name_h), |ui| {
                    if ui
                        .add(name)
                        .on_hover_text(format!("{}\n{}", track.name, ops::instrument_label(&track)))
                        .clicked()
                    {
                        self.select_track(id);
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    let (cw, ch) = if compact { (18.0, 15.0) } else { (22.0, 20.0) };
                    if theme::chip(ui, pal, "S", track.solo, pal.warn, vec2(cw, ch), "Solo")
                        .clicked()
                    {
                        self.toggle_solo(id);
                    }
                    if theme::chip(ui, pal, "M", track.mute, pal.danger, vec2(cw, ch), "Mute")
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
                let bw = if compact { 30.0 } else { 40.0 };
                let inst_w = (ui.available_width() - fw - bw - 2.0 * gap).max(24.0);
                let inst_h = if compact { 12.0 } else { 16.0 };
                ui.allocate_ui(vec2(inst_w, inst_h), |ui| {
                    let full = ops::instrument_label(&track).into_owned();
                    ui.add(
                        egui::Label::new(
                            RichText::new(full.as_str()).small().color(pal.text_faint),
                        )
                        .truncate(),
                    )
                    .on_hover_text(full);
                });
                if theme::chip(
                    ui,
                    pal,
                    "FX",
                    fx_open,
                    pal.link,
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
            let fader_h = (ui.available_height() - 20.0).clamp(
                if compact { 40.0 } else { 60.0 },
                if compact { 120.0 } else { 260.0 },
            );
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.vertical(|ui| {
                    let mut vol = track.volume;
                    if theme::fader(
                        ui,
                        pal,
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
                            .color(pal.text_dim),
                    );
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
                    if theme::knob(
                        ui,
                        pal,
                        &mut pan,
                        0.5,
                        true,
                        "Pan",
                        &sidebar::pan_text(track.pan),
                        pal.link,
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
                                .spacing(vec2(2.0, 2.0))
                                .show(ui, |ui| {
                                    ui.vertical(|ui| self.fx_knob_column(ui, id, ks, color, 0));
                                    ui.vertical(|ui| self.fx_knob_column(ui, id, ks, color, 1));
                                });
                        } else {
                            ui.label(
                                RichText::new("Enlarge the panel for effects")
                                    .small()
                                    .color(pal.text_faint),
                            );
                        }
                    }
                });
                if fx_open {
                    self.fx_pedal_chain(ui, &track, color, fader_h);
                }
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
        let pal = self.palette();
        let Some(fx) = ops::track(&self.project, id).map(|t| t.effects.clone()) else {
            return;
        };
        if col == 0 {
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
        }
        let items: &[(&str, f32, f32)] = if col == 0 {
            &[("Reverb", fx.reverb, 0.15)]
        } else {
            &[("Delay", fx.delay, 0.0), ("Drive", fx.drive, 0.0)]
        };
        for &(label, value, default) in items {
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

    /// The strip's FX pedalboard: the four effects as a horizontal chain of
    /// pedal-like blocks in engine order, chevrons between them. `h` is the
    /// fader height so pedals center against the channel controls.
    fn fx_pedal_chain(&mut self, ui: &mut Ui, track: &Track, color: Color32, h: f32) {
        let pal = self.palette();
        for (i, pedal) in Pedal::ALL.into_iter().enumerate() {
            if i > 0 {
                let (rect, _) = ui.allocate_exact_size(vec2(ARROW_W, h), Sense::hover());
                let c = rect.center();
                let tip = pos2(c.x + 2.0, c.y);
                let stroke = Stroke::new(1.5_f32, pal.text_faint);
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
        let pal = self.palette();
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
            (pal.control, pal.border)
        } else {
            (pal.control.gamma_multiply(0.7), pal.border_soft)
        };
        ui.painter().rect(
            prect,
            8.0,
            fill,
            Stroke::new(1.0_f32, edge),
            StrokeKind::Inside,
        );
        // theme::knob is at least 44 px wide and `size + 30` tall; pick a size
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
        let changed = theme::knob(
            &mut child,
            pal,
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
        let pal = self.palette();
        let compact = compact_mode(ui);
        let num = index as u8 + 1;
        let bus = self.project.buses[index].clone();
        let audible = self.project.is_bus_audible(num);
        let members = ops::bus_members(&self.project, num);
        let margin = if compact { 4.0 } else { 6.0 };
        let gap = if compact { 2.0 } else { 3.0 };
        let strip_w = if compact { 64.0 } else { 88.0 };
        let content_w = strip_w - 2.0 * margin;
        let color = pal.link;
        let frame = egui::Frame::new()
            .fill(pal.raised)
            .stroke(Stroke::new(1.0_f32, pal.border_soft))
            .corner_radius(if compact { 5 } else { 8 })
            .inner_margin(Margin::same(margin as i8));
        let resp = frame.show(ui, |ui| {
            ui.set_width(content_w);
            ui.set_height(height - 2.0 * margin);
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
                        .desired_width(content_w)
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
                if theme::chip(ui, pal, "S", bus.solo, pal.warn, vec2(cw, ch), "Solo bus").clicked()
                {
                    self.edit("Bus solo", Some(&format!("bsolo-{index}")), |p| {
                        p.buses[index].solo = !p.buses[index].solo;
                    });
                }
                if theme::chip(ui, pal, "M", bus.mute, pal.danger, vec2(cw, ch), "Mute bus")
                    .clicked()
                {
                    self.edit("Bus mute", Some(&format!("bmute-{index}")), |p| {
                        p.buses[index].mute = !p.buses[index].mute;
                    });
                }
            });
            ui.add_space(1.0);
            let fader_h = (ui.available_height() - 18.0).clamp(
                if compact { 36.0 } else { 56.0 },
                if compact { 110.0 } else { 240.0 },
            );
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.vertical(|ui| {
                    let mut vol = bus.volume;
                    if theme::fader(
                        ui,
                        pal,
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
                            .color(pal.text_dim),
                    );
                });
                ui.vertical(|ui| {
                    let mut pan = (bus.pan + 1.0) / 2.0;
                    let ks = if compact { 18.0 } else { 24.0 };
                    if theme::knob(
                        ui,
                        pal,
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
        if !audible {
            ui.painter()
                .rect_filled(resp.response.rect, 8.0, Color32::from_black_alpha(70));
        }
    }

    fn master_strip(&mut self, ui: &mut Ui, height: f32) {
        let pal = self.palette();
        let frame = egui::Frame::new()
            .fill(pal.raised)
            .stroke(Stroke::new(1.0_f32, pal.border))
            .corner_radius(8)
            .inner_margin(Margin::same(8));
        frame.show(ui, |ui| {
            ui.set_width(134.0);
            ui.set_height(height - 16.0);
            ui.label(RichText::new("Master").strong().color(pal.text));
            ui.label(
                RichText::new(format!("{} voices", self.transport.voices))
                    .small()
                    .color(pal.text_faint),
            );
            ui.add_space(4.0);
            let fader_h = (ui.available_height() - 24.0).clamp(60.0, 280.0);
            ui.horizontal_top(|ui| {
                let mut vol = self.project.master_volume;
                if theme::fader(ui, pal, &mut vol, 0.8, fader_h, pal.accent, "Master volume")
                    .changed()
                {
                    self.edit("Master volume", Some("master"), |p| p.master_volume = vol);
                }
                let (rect, resp) = ui.allocate_exact_size(vec2(22.0, fader_h), Sense::hover());
                let r = rect.shrink2(vec2(2.0, 7.0));
                let w = (r.width() - 3.0) / 2.0;
                theme::meter(
                    ui.painter(),
                    pal,
                    Rect::from_min_size(r.min, vec2(w, r.height())),
                    self.transport.peak[0],
                    true,
                );
                theme::meter(
                    ui.painter(),
                    pal,
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
                    .color(pal.text_dim),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{compact_mode, fx_key, pedals_open, Pedal, COMPACT_KEY};
    use crate::gui::{CadenceApp, Tab};
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
}
