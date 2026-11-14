//! Mixer: one channel strip per track plus the master strip.

use egui::{vec2, Align, Color32, Layout, Margin, Rect, RichText, Sense, Stroke, Ui};

use super::theme::{
    self, BORDER, BORDER_SOFT, CORAL, LIME, PERIWINKLE, RAISED, TEXT, TEXT_DIM, TEXT_FAINT,
};
use super::{ops, sidebar, CadenceApp};
use crate::model::{MAX_CUTOFF_HZ, MIN_CUTOFF_HZ};

const STRIP_W: f32 = 156.0;

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
        let Some(fx) = ops::track(&self.project, id).map(|t| t.effects.clone()) else {
            return;
        };
        let mut cut = cutoff_to_norm(fx.cutoff_hz);
        if theme::knob(
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
        for (label, value, default) in [
            ("Reverb", fx.reverb, 0.15),
            ("Delay", fx.delay, 0.0),
            ("Drive", fx.drive, 0.0),
        ] {
            let mut v = value;
            if theme::knob(
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

    pub(super) fn mixer_ui(&mut self, ui: &mut Ui) {
        let avail = ui.available_rect_before_wrap().shrink2(vec2(12.0, 10.0));
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(avail)
                .layout(Layout::left_to_right(Align::Min)),
        );
        let ui = &mut ui;
        let h = avail.height();
        let master_w = 150.0;
        let strips_rect = Rect::from_min_max(
            avail.min,
            egui::pos2(avail.right() - master_w - 10.0, avail.bottom()),
        );
        let mut strips = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(strips_rect)
                .layout(Layout::left_to_right(Align::Min)),
        );
        egui::ScrollArea::horizontal()
            .id_salt("mixer_scroll")
            .auto_shrink([false, false])
            .show(&mut strips, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let ids: Vec<u64> = self.project.tracks.iter().map(|t| t.id).collect();
                    for id in ids {
                        self.channel_strip(ui, id, h - 14.0);
                    }
                });
            });
        let master_rect =
            Rect::from_min_max(egui::pos2(avail.right() - master_w, avail.top()), avail.max);
        let mut m = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(master_rect)
                .layout(Layout::top_down(Align::Center)),
        );
        self.master_strip(&mut m, h - 14.0);
    }

    fn channel_strip(&mut self, ui: &mut Ui, id: u64, height: f32) {
        let Some(track) = ops::track(&self.project, id).cloned() else {
            return;
        };
        let idx = ops::track_index(&self.project, id).unwrap_or(0);
        let audible = self.project.is_track_audible(idx);
        let color = theme::rgb(track.color);
        let selected = self.sel.track == Some(id);
        let frame = egui::Frame::new()
            .fill(if selected {
                theme::mix(RAISED, color, 0.07)
            } else {
                RAISED
            })
            .stroke(Stroke::new(
                1.0_f32,
                if selected {
                    color.gamma_multiply(0.7)
                } else {
                    BORDER_SOFT
                },
            ))
            .corner_radius(8)
            .inner_margin(Margin::same(8));
        let resp = frame.show(ui, |ui| {
            ui.set_width(STRIP_W - 16.0);
            ui.set_height(height - 16.0);
            let top = ui.cursor().min;
            ui.painter().rect_filled(
                Rect::from_min_size(top - vec2(8.0, 8.0), vec2(STRIP_W, 4.0)),
                egui::CornerRadius {
                    nw: 8,
                    ne: 8,
                    sw: 0,
                    se: 0,
                },
                color,
            );
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let name = egui::Label::new(
                    RichText::new(&track.name)
                        .color(if audible { TEXT } else { TEXT_FAINT })
                        .strong(),
                )
                .truncate()
                .sense(Sense::click());
                ui.allocate_ui(vec2(STRIP_W - 76.0, 20.0), |ui| {
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
                    if theme::chip(ui, "S", track.solo, theme::AMBER, vec2(22.0, 20.0), "Solo")
                        .clicked()
                    {
                        self.toggle_solo(id);
                    }
                    if theme::chip(ui, "M", track.mute, CORAL, vec2(22.0, 20.0), "Mute").clicked() {
                        self.toggle_mute(id);
                    }
                });
            });
            ui.label(
                RichText::new(ops::instrument_label(&track))
                    .small()
                    .color(TEXT_FAINT),
            );
            ui.add_space(2.0);
            let fader_h = (ui.available_height() - 22.0).clamp(60.0, 260.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.vertical(|ui| {
                    let mut vol = track.volume;
                    if theme::fader(
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
                            .color(TEXT_DIM),
                    );
                });
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let ks = if fader_h > 190.0 { 34.0 } else { 28.0 };
                    let mut pan = (track.pan + 1.0) / 2.0;
                    if theme::knob(
                        ui,
                        &mut pan,
                        0.5,
                        true,
                        "Pan",
                        &sidebar::pan_text(track.pan),
                        PERIWINKLE,
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
                                .color(TEXT_FAINT),
                        );
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
            if theme::knob(
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
            if theme::knob(
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

    fn master_strip(&mut self, ui: &mut Ui, height: f32) {
        let frame = egui::Frame::new()
            .fill(RAISED)
            .stroke(Stroke::new(1.0_f32, BORDER))
            .corner_radius(8)
            .inner_margin(Margin::same(8));
        frame.show(ui, |ui| {
            ui.set_width(134.0);
            ui.set_height(height - 16.0);
            ui.label(RichText::new("Master").strong().color(TEXT));
            ui.label(
                RichText::new(format!("{} voices", self.transport.voices))
                    .small()
                    .color(TEXT_FAINT),
            );
            ui.add_space(4.0);
            let fader_h = (ui.available_height() - 24.0).clamp(60.0, 280.0);
            ui.horizontal_top(|ui| {
                let mut vol = self.project.master_volume;
                if theme::fader(ui, &mut vol, 0.8, fader_h, LIME, "Master volume").changed() {
                    self.edit("Master volume", Some("master"), |p| p.master_volume = vol);
                }
                let (rect, resp) = ui.allocate_exact_size(vec2(22.0, fader_h), Sense::hover());
                let r = rect.shrink2(vec2(2.0, 7.0));
                let w = (r.width() - 3.0) / 2.0;
                theme::meter(
                    ui.painter(),
                    Rect::from_min_size(r.min, vec2(w, r.height())),
                    self.transport.peak[0],
                    true,
                );
                theme::meter(
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
                    .color(TEXT_DIM),
            );
        });
    }
}
