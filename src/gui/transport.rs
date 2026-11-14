//! Transport state, engine control and the top transport bar.

use std::time::{Duration, Instant};

use egui::{
    vec2, Align, Align2, Context, FontId, Frame, Layout, Margin, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind,
};

use super::theme::{self, Icon, BORDER_SOFT, INK, LIME, PERIWINKLE, TEXT, TEXT_DIM, TEXT_FAINT};
use super::{ops, CadenceApp, StatusKind};

/// Grace period after `play` during which a not-yet-playing engine status
/// is not treated as "song ended".
const START_GRACE: Duration = Duration::from_millis(400);

#[derive(Debug, Default)]
pub(super) struct TransportState {
    pub playing: bool,
    /// Current playhead position in steps.
    pub position: f64,
    /// Where Stop returns to (last seek position).
    pub start_marker: f64,
    pub looping: bool,
    pub metronome: bool,
    pub peak: [f32; 2],
    pub voices: usize,
    pub engine_error: Option<String>,
    started_at: Option<Instant>,
}

impl CadenceApp {
    pub(super) fn toggle_play(&mut self) {
        if self.transport.playing {
            self.pause();
        } else {
            self.play();
        }
    }

    pub(super) fn play(&mut self) {
        let Some(engine) = &self.engine else {
            self.set_status(StatusKind::Warning, "No audio output device, so playback is unavailable. You can keep editing and export to WAV.");
            return;
        };
        let total = self.project.total_steps();
        let mut from = self.transport.position;
        if from >= total - 0.01 {
            from = 0.0;
        }
        match engine.play(&self.project, from, self.transport.looping) {
            Ok(()) => {
                self.engine_dirty = false;
                self.transport.playing = true;
                self.transport.position = from;
                self.transport.started_at = Some(Instant::now());
            }
            Err(err) => self.set_status(
                StatusKind::Error,
                format!("Could not start playback: {err}"),
            ),
        }
    }

    pub(super) fn pause(&mut self) {
        if let Some(engine) = &self.engine {
            engine.pause();
        }
        self.transport.playing = false;
    }

    /// Stops playback. Pressing Stop while stopped returns to the song start.
    pub(super) fn stop(&mut self) {
        if let Some(engine) = &self.engine {
            engine.stop();
        }
        let t = &mut self.transport;
        let was_playing = t.playing;
        t.playing = false;
        t.position = if was_playing || t.position != t.start_marker {
            t.start_marker
        } else {
            0.0
        };
        if !was_playing {
            t.start_marker = t.position;
        }
    }

    pub(super) fn seek(&mut self, step: f64) {
        let step = step.clamp(0.0, self.project.total_steps().max(0.0));
        self.transport.position = step;
        self.transport.start_marker = step;
        if self.transport.playing {
            if let Some(engine) = &self.engine {
                if let Err(err) = engine.play(&self.project, step, self.transport.looping) {
                    self.set_status(StatusKind::Error, format!("Could not seek: {err}"));
                }
                self.engine_dirty = false;
                self.transport.started_at = Some(Instant::now());
            }
        }
    }

    pub(super) fn toggle_loop(&mut self) {
        self.transport.looping = !self.transport.looping;
        if self.transport.playing {
            let pos = self.transport.position;
            if let Some(engine) = &self.engine {
                let _ = engine.play(&self.project, pos, self.transport.looping);
                self.engine_dirty = false;
                self.transport.started_at = Some(Instant::now());
            }
        }
    }

    pub(super) fn toggle_metronome(&mut self) {
        self.transport.metronome = !self.transport.metronome;
        if let Some(engine) = &self.engine {
            engine.set_metronome(self.transport.metronome);
        }
    }

    /// Reads the engine's real transport state once per frame.
    pub(super) fn poll_engine(&mut self) {
        let Some(engine) = &self.engine else {
            self.transport.playing = false;
            return;
        };
        let st = engine.status();
        let t = &mut self.transport;
        if t.playing {
            if st.playing {
                t.position = st.step;
            } else if t.started_at.is_none_or(|s| s.elapsed() > START_GRACE) {
                t.playing = false;
                t.position = t.start_marker;
            }
        }
        for (peak, new) in t.peak.iter_mut().zip([st.peak_l, st.peak_r]) {
            let v = if new.is_finite() { new.max(0.0) } else { 0.0 };
            *peak = if v >= *peak { v } else { (*peak * 0.86).max(v) };
            if *peak < 0.001 {
                *peak = 0.0;
            }
        }
        t.voices = st.voices;
        if st.error != t.engine_error {
            if let Some(err) = &st.error {
                let msg = format!("Audio engine: {err}");
                t.engine_error = st.error.clone();
                self.set_status(StatusKind::Error, msg);
            } else {
                t.engine_error = None;
            }
        }
    }

    pub(super) fn transport_bar(&mut self, ctx: &Context) {
        let frame = Frame::new()
            .fill(theme::PANEL)
            .inner_margin(Margin::symmetric(14, 8))
            .stroke(Stroke::new(1.0_f32, BORDER_SOFT));
        egui::TopBottomPanel::top("transport")
            .exact_height(66.0)
            .frame(frame)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    self.brand(ui);
                    ui.add_space(6.0);
                    divider(ui);

                    if theme::icon_button(
                        ui,
                        Icon::ToStart,
                        32.0,
                        false,
                        LIME,
                        "Return to start (Home)",
                    )
                    .clicked()
                    {
                        self.seek(0.0);
                    }
                    if theme::play_button(ui, self.transport.playing, 42.0).clicked() {
                        self.toggle_play();
                    }
                    if theme::icon_button(
                        ui,
                        Icon::Stop,
                        32.0,
                        false,
                        LIME,
                        "Stop (press again to return to the start)",
                    )
                    .clicked()
                    {
                        self.stop();
                    }
                    ui.add_space(2.0);
                    if theme::icon_button(
                        ui,
                        Icon::Loop,
                        32.0,
                        self.transport.looping,
                        LIME,
                        "Loop the whole song (L)",
                    )
                    .clicked()
                    {
                        self.toggle_loop();
                    }
                    if theme::icon_button(
                        ui,
                        Icon::Metronome,
                        32.0,
                        self.transport.metronome,
                        PERIWINKLE,
                        "Metronome click",
                    )
                    .clicked()
                    {
                        self.toggle_metronome();
                    }
                    ui.add_space(4.0);
                    self.position_display(ui);
                    divider(ui);
                    self.tempo_controls(ui);

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        self.master_section(ui);
                    });
                });
            });
    }

    fn brand(&mut self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 8.0, LIME);
        for (i, h) in [0.45f32, 0.8, 0.6].iter().enumerate() {
            let x = rect.left() + 8.0 + i as f32 * 6.0;
            let bar = Rect::from_min_max(
                Pos2::new(x, rect.bottom() - 7.0 - h * 14.0),
                Pos2::new(x + 3.5, rect.bottom() - 7.0),
            );
            p.rect_filled(bar, 1.5, INK);
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(theme::kicker_text("CADENCE  ·  SONG"));
            let mut name = self.project.name.clone();
            let resp = ui.add(
                egui::TextEdit::singleline(&mut name)
                    .desired_width(168.0)
                    .font(FontId::proportional(15.0))
                    .hint_text("Song name")
                    .char_limit(80)
                    .margin(Margin::symmetric(4, 1)),
            );
            if resp.changed() {
                let name = name.clone();
                self.edit("Rename song", Some("song-name"), |p| p.name = name);
            }
            resp.on_hover_text("Song name (used for file names on export)");
        });
    }

    fn position_display(&self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(vec2(148.0, 46.0), Sense::hover());
        let p = ui.painter();
        p.rect(
            rect,
            8.0,
            INK,
            Stroke::new(1.0_f32, BORDER_SOFT),
            StrokeKind::Inside,
        );
        let pos = self.transport.position;
        p.text(
            Pos2::new(rect.left() + 12.0, rect.top() + 5.0),
            Align2::LEFT_TOP,
            ops::format_position(pos).trim_start(),
            FontId::monospace(20.0),
            if self.transport.playing { LIME } else { TEXT },
        );
        let now = ops::steps_to_seconds(pos, self.project.bpm);
        let total = ops::steps_to_seconds(self.project.total_steps(), self.project.bpm);
        p.text(
            Pos2::new(rect.left() + 12.0, rect.bottom() - 5.0),
            Align2::LEFT_BOTTOM,
            format!("{} / {}", ops::format_time(now), ops::format_time(total)),
            FontId::monospace(10.5),
            TEXT_FAINT,
        );
        p.text(
            Pos2::new(rect.right() - 10.0, rect.top() + 7.0),
            Align2::RIGHT_TOP,
            "BAR",
            FontId::proportional(9.0),
            TEXT_FAINT,
        );
        resp.on_hover_text("Playhead position: bar.beat.sixteenth · elapsed / song length");
    }

    fn tempo_controls(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(theme::kicker_text("TEMPO"));
            let mut bpm = self.project.bpm;
            let resp = ui.add_sized(
                vec2(84.0, 24.0),
                egui::DragValue::new(&mut bpm)
                    .range(ops::MIN_BPM..=ops::MAX_BPM)
                    .speed(0.25)
                    .fixed_decimals(1)
                    .suffix(" bpm"),
            );
            if resp.changed() {
                self.edit("Change tempo", Some("bpm"), |p| {
                    p.bpm = bpm.clamp(ops::MIN_BPM, ops::MAX_BPM)
                });
            }
            resp.on_hover_text("Tempo in beats per minute. Drag or double-click to type (30–300).");
        });
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(theme::kicker_text("SWING"));
            let mut swing = self.project.swing;
            ui.spacing_mut().slider_width = 92.0;
            let resp =
                ui.add(egui::Slider::new(&mut swing, 0.0..=ops::MAX_SWING).show_value(false));
            let resp = resp.on_hover_text(format!(
                "Swing delays every other 16th note: {:.0}%",
                swing / ops::MAX_SWING * 100.0
            ));
            if resp.changed() {
                self.edit("Change swing", Some("swing"), |p| {
                    p.swing = swing.clamp(0.0, ops::MAX_SWING)
                });
            }
            if resp.double_clicked() {
                self.edit("Reset swing", None, |p| p.swing = 0.0);
            }
        });
        ui.label(
            RichText::new(format!(
                "{:.0}%",
                self.project.swing / ops::MAX_SWING * 100.0
            ))
            .small()
            .color(TEXT_DIM),
        );
    }

    fn master_section(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(vec2(12.0, 40.0), Sense::hover());
        let half = (rect.width() - 2.0) / 2.0;
        theme::meter(
            ui.painter(),
            Rect::from_min_size(rect.min, vec2(half, rect.height())),
            self.transport.peak[0],
            true,
        );
        theme::meter(
            ui.painter(),
            Rect::from_min_size(rect.min + vec2(half + 2.0, 0.0), vec2(half, rect.height())),
            self.transport.peak[1],
            true,
        );
        resp.on_hover_text(format!(
            "Master output peak  L {:.0}%  R {:.0}%",
            self.transport.peak[0] * 100.0,
            self.transport.peak[1] * 100.0
        ));
        ui.add_sized(
            vec2(34.0, 20.0),
            egui::Label::new(
                RichText::new(format!("{:.0}%", self.project.master_volume * 100.0))
                    .small()
                    .color(TEXT_DIM),
            ),
        );
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(theme::kicker_text("MASTER"));
            let mut vol = self.project.master_volume;
            ui.spacing_mut().slider_width = 110.0;
            let resp = ui
                .add(egui::Slider::new(&mut vol, 0.0..=1.0).show_value(false))
                .on_hover_text(format!("Master volume {:.0}%", vol * 100.0));
            if resp.changed() {
                self.edit("Master volume", Some("master"), |p| p.master_volume = vol);
            }
        });
        divider(ui);
    }
}

fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 38.0), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        Stroke::new(1.0_f32, BORDER_SOFT),
    );
}
