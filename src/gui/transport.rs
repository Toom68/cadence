//! Transport state, engine control and the top transport bar.

use std::time::{Duration, Instant};

use egui::{
    vec2, Align, Align2, Context, FontId, Frame, Layout, Margin, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind,
};

use super::theme::{self, Icon};
use super::{ops, CadenceApp, StatusKind};

/// Grace period after `play` during which a not-yet-playing engine status
/// is not treated as "song ended".
const START_GRACE: Duration = Duration::from_millis(400);

/// Taps kept for tap tempo (newest last).
const TAP_HISTORY: usize = 8;
/// A pause longer than this between taps restarts the count.
const TAP_GAP: Duration = Duration::from_millis(2_000);
/// Intervals outside this window are hesitations or double-clicks, not tempo.
const TAP_MIN_INTERVAL_MS: f64 = 50.0;
const TAP_MAX_INTERVAL_MS: f64 = 2_000.0;
/// The tapped BPM stays visible next to the TAP button this long.
const TAP_FEEDBACK: Duration = Duration::from_secs(2);

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
    /// Recent tap-tempo timestamps, oldest first.
    taps: Vec<Instant>,
    /// Last BPM produced by tap tempo and when, for the brief readout.
    tap_bpm: Option<(f64, Instant)>,
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

    /// Records a tap for tap tempo. Once three taps land inside the window the
    /// averaged interval becomes the project tempo — the same undoable edit as
    /// dragging the TEMPO value.
    pub(super) fn tap_tempo(&mut self) {
        let now = Instant::now();
        {
            let t = &mut self.transport;
            if t.taps
                .last()
                .is_some_and(|last| now.duration_since(*last) > TAP_GAP)
            {
                t.taps.clear();
            }
            t.taps.push(now);
            if t.taps.len() > TAP_HISTORY {
                let excess = t.taps.len() - TAP_HISTORY;
                t.taps.drain(..excess);
            }
        }
        let Some(bpm) = bpm_from_taps(&self.transport.taps) else {
            return;
        };
        let bpm = bpm.clamp(ops::MIN_BPM, ops::MAX_BPM);
        self.transport.tap_bpm = Some((bpm, now));
        self.edit("Tap tempo", Some("bpm"), |p| p.bpm = bpm);
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
        let pal = self.palette();
        let frame = Frame::new()
            .fill(pal.panel)
            .inner_margin(Margin::symmetric(14, 8))
            .stroke(Stroke::new(1.0_f32, pal.border_soft));
        egui::TopBottomPanel::top("transport")
            .exact_height(66.0)
            .frame(frame)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    self.brand(ui);
                    ui.add_space(6.0);
                    divider(ui, pal);

                    if theme::icon_button(
                        ui,
                        pal,
                        Icon::ToStart,
                        32.0,
                        false,
                        pal.accent,
                        "Return to start (Home)",
                    )
                    .clicked()
                    {
                        self.seek(0.0);
                    }
                    if theme::play_button(ui, pal, self.transport.playing, 42.0).clicked() {
                        self.toggle_play();
                    }
                    if theme::icon_button(
                        ui,
                        pal,
                        Icon::Stop,
                        32.0,
                        false,
                        pal.accent,
                        "Stop (press again to return to the start)",
                    )
                    .clicked()
                    {
                        self.stop();
                    }
                    ui.add_space(2.0);
                    if theme::icon_button(
                        ui,
                        pal,
                        Icon::Loop,
                        32.0,
                        self.transport.looping,
                        pal.accent,
                        "Loop the whole song (L)",
                    )
                    .clicked()
                    {
                        self.toggle_loop();
                    }
                    if theme::icon_button(
                        ui,
                        pal,
                        Icon::Metronome,
                        32.0,
                        self.transport.metronome,
                        pal.link,
                        "Metronome click",
                    )
                    .clicked()
                    {
                        self.toggle_metronome();
                    }
                    self.tap_button(ui);
                    ui.add_space(4.0);
                    self.position_display(ui);
                    divider(ui, pal);
                    self.tempo_controls(ui);

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        self.master_section(ui);
                    });
                });
            });
    }

    fn brand(&mut self, ui: &mut egui::Ui) {
        let pal = self.palette();
        let (rect, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 8.0, pal.accent);
        let bar_ink = theme::ink_on(pal.accent);
        for (i, h) in [0.45f32, 0.8, 0.6].iter().enumerate() {
            let x = rect.left() + 8.0 + i as f32 * 6.0;
            let bar = Rect::from_min_max(
                Pos2::new(x, rect.bottom() - 7.0 - h * 14.0),
                Pos2::new(x + 3.5, rect.bottom() - 7.0),
            );
            p.rect_filled(bar, 1.5, bar_ink);
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(theme::kicker_text(pal, "CADENCE  ·  SONG"));
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
        let pal = self.palette();
        let (rect, resp) = ui.allocate_exact_size(vec2(148.0, 46.0), Sense::hover());
        let p = ui.painter();
        p.rect(
            rect,
            8.0,
            pal.inset,
            Stroke::new(1.0_f32, pal.border_soft),
            StrokeKind::Inside,
        );
        let pos = self.transport.position;
        p.text(
            Pos2::new(rect.left() + 12.0, rect.top() + 5.0),
            Align2::LEFT_TOP,
            ops::format_position(pos).trim_start(),
            FontId::monospace(20.0),
            if self.transport.playing {
                pal.accent
            } else {
                pal.text
            },
        );
        let now = ops::steps_to_seconds(pos, self.project.bpm);
        let total = ops::steps_to_seconds(self.project.total_steps(), self.project.bpm);
        p.text(
            Pos2::new(rect.left() + 12.0, rect.bottom() - 5.0),
            Align2::LEFT_BOTTOM,
            format!("{} / {}", ops::format_time(now), ops::format_time(total)),
            FontId::monospace(10.5),
            pal.text_faint,
        );
        p.text(
            Pos2::new(rect.right() - 10.0, rect.top() + 7.0),
            Align2::RIGHT_TOP,
            "BAR",
            FontId::proportional(9.0),
            pal.text_faint,
        );
        resp.on_hover_text("Playhead position: bar.beat.sixteenth · elapsed / song length");
    }

    /// TAP chip for tap tempo plus a brief readout: the BPM the taps produced,
    /// or the tap count while the first taps are still being collected.
    fn tap_button(&mut self, ui: &mut egui::Ui) {
        let pal = self.palette();
        if theme::chip(
            ui,
            pal,
            "TAP",
            false,
            pal.link,
            vec2(46.0, 32.0),
            "Tap tempo: click along with the music; 3 taps set the tempo",
        )
        .clicked()
        {
            self.tap_tempo();
        }
        let now = Instant::now();
        let fresh_bpm = self
            .transport
            .tap_bpm
            .filter(|(_, at)| now.duration_since(*at) < TAP_FEEDBACK);
        let collecting = fresh_bpm.is_none()
            && self.transport.taps.len() < 3
            && self
                .transport
                .taps
                .last()
                .is_some_and(|t| now.duration_since(*t) < TAP_GAP);
        let readout = if let Some((bpm, _)) = fresh_bpm {
            Some((format!("{bpm:.1} bpm"), pal.accent))
        } else if collecting {
            Some((
                format!("tap {}/3", self.transport.taps.len()),
                pal.text_faint,
            ))
        } else {
            None
        };
        if let Some((text, color)) = readout {
            ui.label(RichText::new(text).small().color(color));
        }
    }

    fn tempo_controls(&mut self, ui: &mut egui::Ui) {
        let pal = self.palette();
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(theme::kicker_text(pal, "TEMPO"));
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
            ui.label(theme::kicker_text(pal, "SWING"));
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
            .color(pal.text_dim),
        );
    }

    fn master_section(&mut self, ui: &mut egui::Ui) {
        let pal = self.palette();
        let (rect, resp) = ui.allocate_exact_size(vec2(12.0, 40.0), Sense::hover());
        let half = (rect.width() - 2.0) / 2.0;
        theme::meter(
            ui.painter(),
            pal,
            Rect::from_min_size(rect.min, vec2(half, rect.height())),
            self.transport.peak[0],
            true,
        );
        theme::meter(
            ui.painter(),
            pal,
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
                    .color(pal.text_dim),
            ),
        );
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(theme::kicker_text(pal, "MASTER"));
            let mut vol = self.project.master_volume;
            ui.spacing_mut().slider_width = 110.0;
            let resp = ui
                .add(egui::Slider::new(&mut vol, 0.0..=1.0).show_value(false))
                .on_hover_text(format!("Master volume {:.0}%", vol * 100.0));
            if resp.changed() {
                self.edit("Master volume", Some("master"), |p| p.master_volume = vol);
            }
        });
        divider(ui, pal);
    }
}

fn divider(ui: &mut egui::Ui, pal: theme::Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 38.0), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        Stroke::new(1.0_f32, pal.border_soft),
    );
}

/// Beats per minute from inter-tap intervals in milliseconds: the mean
/// interval inverted to BPM. Intervals must be finite and inside
/// [`TAP_MIN_INTERVAL_MS`]..=[`TAP_MAX_INTERVAL_MS`]; stragglers are skipped
/// rather than failing the gesture. Fewer than two usable intervals — three
/// taps — yields `None`.
fn bpm_from_intervals_ms(intervals: &[f64]) -> Option<f64> {
    let mut sum = 0.0;
    let mut n = 0usize;
    for &ms in intervals {
        if ms.is_finite() && (TAP_MIN_INTERVAL_MS..=TAP_MAX_INTERVAL_MS).contains(&ms) {
            sum += ms;
            n += 1;
        }
    }
    if n < 2 {
        return None;
    }
    let bpm = 60_000.0 / (sum / n as f64);
    bpm.is_finite().then_some(bpm)
}

/// Beats per minute from tap timestamps: consecutive deltas in milliseconds
/// feed [`bpm_from_intervals_ms`]. Only the newest [`TAP_HISTORY`] taps count.
fn bpm_from_taps(taps: &[Instant]) -> Option<f64> {
    let start = taps.len().saturating_sub(TAP_HISTORY);
    let mut intervals = [0.0f64; TAP_HISTORY];
    let mut n = 0;
    for pair in taps[start..].windows(2) {
        intervals[n] = pair[1].saturating_duration_since(pair[0]).as_secs_f64() * 1_000.0;
        n += 1;
    }
    bpm_from_intervals_ms(&intervals[..n])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_bpm(actual: Option<f64>, expected: f64) {
        let bpm = actual.expect("expected a tempo");
        assert!((bpm - expected).abs() < 1e-9, "{bpm} != {expected}");
    }

    #[test]
    fn bpm_from_intervals_averages_and_filters() {
        // Even 500 ms taps → 120 BPM.
        assert_bpm(bpm_from_intervals_ms(&[500.0, 500.0]), 120.0);
        // Uneven taps average out: (400 + 600) / 2 = 500 ms.
        assert_bpm(bpm_from_intervals_ms(&[400.0, 600.0]), 120.0);
        // Fewer than two usable intervals is not a tempo yet.
        assert_eq!(bpm_from_intervals_ms(&[]), None);
        assert_eq!(bpm_from_intervals_ms(&[500.0]), None);
        // Stale, over-fast and non-finite intervals are skipped, not fatal.
        assert_bpm(
            bpm_from_intervals_ms(&[2_500.0, 500.0, 500.0, f64::NAN, 20.0]),
            120.0,
        );
        assert_eq!(
            bpm_from_intervals_ms(&[f64::INFINITY, f64::NEG_INFINITY]),
            None
        );
    }

    #[test]
    fn bpm_from_taps_needs_three_taps_and_ignores_old_ones() {
        let base = Instant::now();
        // Two taps → one interval → no tempo.
        let two = [base, base + Duration::from_millis(500)];
        assert_eq!(bpm_from_taps(&two), None);
        // Three taps 500 ms apart → 120 BPM.
        let three: Vec<Instant> = (0..3)
            .map(|i| base + Duration::from_millis(500 * i as u64))
            .collect();
        assert_bpm(bpm_from_taps(&three), 120.0);
        // A >2 s hole inside the window is filtered by the interval check.
        let hole = [
            base,
            base + Duration::from_millis(500),
            base + Duration::from_millis(3_000),
            base + Duration::from_millis(3_500),
        ];
        assert_bpm(bpm_from_taps(&hole), 120.0);
        // Only the newest eight taps count: two early 1 s taps are dropped.
        let mut taps = vec![
            base - Duration::from_millis(2_000),
            base - Duration::from_millis(1_000),
        ];
        taps.extend((0..8).map(|i| base + Duration::from_millis(500 * i as u64)));
        assert_eq!(taps.len(), 10);
        assert_bpm(bpm_from_taps(&taps), 120.0);
    }
}
