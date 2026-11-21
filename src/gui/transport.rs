//! Transport state, engine control and the top transport bar.
//!
//! The bar is laid out from fixed widths per size tier ([`Plan`]): the widest
//! tier that fits the window is used, so nothing can overlap or be clipped at
//! any window width the app allows.

use std::time::{Duration, Instant};

use egui::{
    pos2, vec2, Align, Align2, Context, FontId, Frame, Id, Layout, Margin, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use super::theme::{self, Icon, Palette};
use super::{arranger, ops, CadenceApp, StatusKind, SC_HOME, SC_LOOP, SC_RECORD, SC_REDO, SC_UNDO};

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
    /// Repeat bounds `(start_step, end_step)` for `looping`, drawn on the
    /// arranger ruler. Session-only — never saved into the `.cadence` file.
    /// `None` (or `looping` off) plays to the song end as before.
    pub loop_region: Option<(f64, f64)>,
    pub metronome: bool,
    /// MIDI recording armed: performances from the Keyboard tab and the
    /// computer-keyboard row land in the selected track's pattern. Session
    /// state only — never saved into the `.cadence` file.
    pub recording: bool,
    /// Notes recorded since the last time record was armed, for the status
    /// line ("Recorded N notes").
    pub recorded_notes: usize,
    pub peak: [f32; 2],
    pub voices: usize,
    pub engine_error: Option<String>,
    started_at: Option<Instant>,
    /// Recent tap-tempo timestamps, oldest first.
    taps: Vec<Instant>,
    /// Last BPM produced by tap tempo and when, for the brief readout.
    tap_bpm: Option<(f64, Instant)>,
    /// The position display shows elapsed time first instead of bar.beat.
    pub time_first: bool,
    /// The master meter shows a clip warning until then.
    clip_until: Option<Instant>,
    /// Text of the bar's displays, rebuilt only when it would change.
    cache: BarCache,
    /// Edit buffer behind the song-name field.
    name_buf: String,
}

/// How long the master meter keeps its clip warning lit.
const CLIP_HOLD: Duration = Duration::from_millis(1_500);

/// Strings of the transport bar, keyed by what they show so that a playing
/// song does not rebuild them every frame.
#[derive(Debug, Default)]
struct BarCache {
    /// Playhead step the position text was built for.
    step: Option<u64>,
    position: String,
    /// "bar 12.3.4": the position as the small line of the time-first display.
    bar_line: String,
    /// Tenths of a second (now, total) the time text was built for.
    time_key: Option<(u64, u64)>,
    time_now: String,
    /// "0:03.2 / 0:18.5"
    time_pair: String,
    loop_key: Option<(bool, Option<(u64, u64)>)>,
    loop_tip: String,
    /// History state the undo/redo tooltips were built for.
    history_key: Option<u64>,
    undo_tip: String,
    redo_tip: String,
    /// Tooltips that never change; built on the first frame.
    tips: Option<Tips>,
}

impl CadenceApp {
    pub(super) fn toggle_play(&mut self) {
        if self.transport.playing {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Toggles MIDI recording: notes played from the Keyboard tab or the
    /// computer-keyboard row are quantized to the Snap grid and inserted
    /// into the selected track's pattern. Armed while playing = live
    /// recording; armed while stopped = step recording (the playhead
    /// advances one Snap step per note).
    pub(super) fn toggle_record(&mut self) {
        self.transport.recording = !self.transport.recording;
        if self.transport.recording {
            self.transport.recorded_notes = 0;
            self.editor.record_pending.clear();
            self.editor.kb_record_pitch = None;
            self.set_status(
                StatusKind::Info,
                "Recording… play and perform, or step-record at the playhead while stopped.",
            );
        } else {
            self.set_status(StatusKind::Info, "Recording stopped.");
        }
    }

    /// Where playback actually begins: rewinds past the song end, and jumps
    /// back to the loop region's start when the position is beyond its end —
    /// otherwise `play` would chase (and briefly sound) notes that can never
    /// be reached before the first wrap.
    fn play_start(&self, from: f64) -> f64 {
        let total = self.project.total_steps();
        let mut from = if from.is_finite() { from.max(0.0) } else { 0.0 };
        if from >= total - 0.01 {
            from = 0.0;
        }
        if self.transport.looping {
            if let Some((a, b)) = self.transport.loop_region {
                let (a, b) = (a.max(0.0), b.min(total));
                if b - a > 0.0 && from >= b - 0.01 {
                    from = a;
                }
            }
        }
        from
    }

    pub(super) fn play(&mut self) {
        let Some(engine) = &self.engine else {
            self.set_status(StatusKind::Warning, "No audio output device, so playback is unavailable. You can keep editing and export to WAV.");
            return;
        };
        let from = self.play_start(self.transport.position);
        match engine.play(
            &self.project,
            from,
            self.transport.looping,
            self.transport.loop_region,
        ) {
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
                let from = self.play_start(step);
                if let Err(err) = engine.play(
                    &self.project,
                    from,
                    self.transport.looping,
                    self.transport.loop_region,
                ) {
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
                let from = self.play_start(pos);
                let _ = engine.play(
                    &self.project,
                    from,
                    self.transport.looping,
                    self.transport.loop_region,
                );
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
            if v >= CLIP_LEVEL {
                t.clip_until = Some(Instant::now() + CLIP_HOLD);
            }
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
        self.refresh_bar(ctx);
        let mut name_buf = std::mem::take(&mut self.transport.name_buf);
        let mut act = BarAction::default();
        let frame = Frame::new()
            .fill(pal.panel)
            .inner_margin(Margin::symmetric(BAR_PAD, 8))
            .stroke(Stroke::new(1.0_f32, pal.border_soft));
        egui::TopBottomPanel::top("transport")
            .exact_height(TRANSPORT_H)
            .frame(frame)
            .show(ctx, |ui| {
                let plan = Plan::choose(ui.available_width());
                act = self.bar_ui(ui, pal, plan, &mut name_buf);
            });
        self.transport.name_buf = name_buf;
        self.apply_bar_action(act);
    }

    /// Keeps the bar's cached texts current and schedules the repaints its
    /// timed visuals (tap readout, clip warning) need once the app is idle.
    fn refresh_bar(&mut self, ctx: &Context) {
        let t = &mut self.transport;
        let c = &mut t.cache;
        if c.tips.is_none() {
            c.tips = Some(Tips::new(ctx));
        }
        let step = t.position.max(0.0).floor() as u64;
        if c.step != Some(step) {
            c.step = Some(step);
            c.position = ops::format_position(t.position).trim_start().to_owned();
            c.bar_line = format!("bar {}", c.position);
        }
        let bpm = self.project.bpm;
        let total = self.project.total_steps();
        let now_s = ops::steps_to_seconds(t.position, bpm);
        let total_s = ops::steps_to_seconds(total, bpm);
        let key = ((now_s * 10.0) as u64, (total_s * 10.0) as u64);
        if c.time_key != Some(key) {
            c.time_key = Some(key);
            c.time_now = ops::format_time(now_s);
            c.time_pair = format!("{} / {}", c.time_now, ops::format_time(total_s));
        }
        let loop_key = (
            t.looping,
            t.loop_region.map(|(a, b)| (a.to_bits(), b.to_bits())),
        );
        if c.loop_key != Some(loop_key) {
            c.loop_key = Some(loop_key);
            let shortcut = ctx.format_shortcut(&SC_LOOP);
            c.loop_tip = match t.loop_region {
                Some((a, b)) => format!(
                    "Loop bars {}–{} ({shortcut}) · drag the ruler's top strip to move it, ✕ on the band clears it",
                    arranger::bar_number(a),
                    arranger::bar_number_end(b),
                ),
                None => format!(
                    "Loop the whole song ({shortcut}) · drag the ruler's top strip to loop a section"
                ),
            };
        }
        let state = self.history.state();
        if c.history_key != Some(state) {
            c.history_key = Some(state);
            let undo = ctx.format_shortcut(&SC_UNDO);
            let redo = ctx.format_shortcut(&SC_REDO);
            c.undo_tip = match self.history.undo_label() {
                Some(label) => format!("Undo {label} ({undo})"),
                None => format!("Nothing to undo ({undo})"),
            };
            c.redo_tip = match self.history.redo_label() {
                Some(label) => format!("Redo {label} ({redo})"),
                None => format!("Nothing to redo ({redo})"),
            };
        }
        // The name field mirrors the project unless it is being typed in.
        let editing = ctx.memory(|m| m.has_focus(Id::new(NAME_FIELD)));
        if !editing && t.name_buf != self.project.name {
            t.name_buf.clone_from(&self.project.name);
        }
        let now = Instant::now();
        if let Some((_, at)) = t.tap_bpm {
            let age = now.duration_since(at);
            if age < TAP_FEEDBACK {
                ctx.request_repaint_after(TAP_FEEDBACK - age + Duration::from_millis(20));
            }
        }
        if let Some(until) = t.clip_until {
            if until > now {
                ctx.request_repaint_after(until - now + Duration::from_millis(20));
            } else {
                t.clip_until = None;
            }
        }
    }

    /// Draws the whole bar for `plan` and reports what was pressed or
    /// edited; the caller applies it (the closure only holds `&self`).
    fn bar_ui(&self, ui: &mut Ui, pal: Palette, plan: Plan, name_buf: &mut String) -> BarAction {
        let mut act = BarAction::default();
        #[cfg(test)]
        probe_reset(ui);
        let t = &self.transport;
        let cache = &t.cache;
        let tips = cache.tips.as_ref();
        let tip = |pick: fn(&Tips) -> &str| tips.map_or("", pick);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            // --- song: mark + editable name
            let (mark, _) = ui.allocate_exact_size(Vec2::splat(LOGO), Sense::hover());
            paint_brand_mark(ui.painter(), mark, pal);
            #[cfg(test)]
            probe(ui, mark);
            ui.add_space(theme::GAP_S);
            let block = fixed_block(ui, plan.name_w, |ui| {
                kicker(ui, pal, "SONG");
                let resp = ui.add_sized(
                    vec2(plan.name_w, theme::CONTROL_M),
                    egui::TextEdit::singleline(name_buf)
                        .id(Id::new(NAME_FIELD))
                        .font(FontId::proportional(theme::TEXT_TITLE))
                        .hint_text("Song name")
                        .char_limit(80)
                        .margin(Margin::symmetric(8, 4))
                        .vertical_align(Align::Center),
                );
                if resp.changed() {
                    act.rename = true;
                }
                resp.on_hover_ui(|ui| {
                    ui.label(&self.project.name);
                    theme::hint(ui, pal, "Song name — also the default file name");
                });
            });
            #[cfg(test)]
            probe(ui, block);
            let _ = block;
            bar_sep(ui, pal, plan);

            // --- transport buttons
            let gap = theme::GAP_XS;
            let button = |ui: &mut Ui, icon: Icon, on: bool, color, text: &str| -> bool {
                let resp = theme::icon_button(ui, pal, icon, plan.btn, on, color, text);
                #[cfg(test)]
                probe(ui, resp.rect);
                resp.clicked()
            };
            act.to_start = button(ui, Icon::ToStart, false, pal.accent, tip(|t| &t.to_start));
            ui.add_space(gap);
            let play = theme::play_button(ui, pal, t.playing, plan.play);
            #[cfg(test)]
            probe(ui, play.rect);
            act.play = play.clicked();
            ui.add_space(gap);
            act.stop = button(ui, Icon::Stop, false, pal.accent, tip(|t| &t.stop));
            ui.add_space(gap);
            let time = ui.input(|i| i.time);
            let record_tip = if t.recording {
                tip(|t| &t.recording)
            } else {
                tip(|t| &t.record)
            };
            let record = record_button(ui, pal, time, t.recording, t.playing, plan.btn, record_tip);
            #[cfg(test)]
            probe(ui, record.rect);
            act.record = record.clicked();
            ui.add_space(gap);
            act.looping = button(ui, Icon::Loop, t.looping, pal.accent, &cache.loop_tip);
            ui.add_space(gap);
            act.metronome = button(
                ui,
                Icon::Metronome,
                t.metronome,
                pal.link,
                tip(|t| &t.metronome),
            );
            bar_sep(ui, pal, plan);

            // --- position
            let resp = self.position_display(ui, pal, plan.pos_w);
            #[cfg(test)]
            probe(ui, resp.rect);
            act.toggle_time = resp.clicked();
            bar_sep(ui, pal, plan);

            // --- tempo + tap
            let tempo_block_w = plan.tempo_w + gap + plan.tap_w;
            let block = fixed_block(ui, tempo_block_w, |ui| {
                let now = Instant::now();
                let fresh = t
                    .tap_bpm
                    .filter(|(_, at)| now.duration_since(*at) < TAP_FEEDBACK);
                let collecting = fresh.is_none()
                    && t.taps.len() < 3
                    && t.taps
                        .last()
                        .is_some_and(|last| now.duration_since(*last) < TAP_GAP);
                if let Some((bpm, _)) = fresh {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("TAPPED  {bpm:.1}"))
                                .text_style(theme::kicker())
                                .color(pal.accent),
                        )
                        .selectable(false)
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                } else if collecting {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("TAP  {}/3", t.taps.len()))
                                .text_style(theme::kicker())
                                .color(pal.text_dim),
                        )
                        .selectable(false)
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                } else {
                    kicker(ui, pal, "TEMPO");
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    let mut bpm = self.project.bpm;
                    let resp = ui.add_sized(
                        vec2(plan.tempo_w, theme::CONTROL_M),
                        egui::DragValue::new(&mut bpm)
                            .range(ops::MIN_BPM..=ops::MAX_BPM)
                            .speed(0.25)
                            .fixed_decimals(1)
                            .suffix(" bpm"),
                    );
                    if resp.changed() {
                        act.bpm = Some(bpm.clamp(ops::MIN_BPM, ops::MAX_BPM));
                    }
                    resp.on_hover_text(
                        "Tempo in beats per minute. Drag, or double-click to type (30–300).",
                    );
                    let tap = ui
                        .allocate_ui_with_layout(
                            vec2(plan.tap_w, theme::CONTROL_M),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                theme::toggle_button(
                                    ui,
                                    pal,
                                    "Tap",
                                    fresh.is_some(),
                                    theme::CONTROL_M,
                                    tip(|t| &t.tap),
                                )
                            },
                        )
                        .inner;
                    act.tap = tap.clicked();
                });
            });
            #[cfg(test)]
            probe(ui, block);
            let _ = block;

            // --- swing
            if plan.swing {
                ui.add_space(theme::GAP_M);
                let block_w = plan.swing_w + gap + plan.swing_readout;
                let block = fixed_block(ui, block_w, |ui| {
                    kicker(ui, pal, "SWING");
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        let mut swing = self.project.swing;
                        let resp = theme::slider_with(
                            ui,
                            pal,
                            &mut swing,
                            0.0..=ops::MAX_SWING,
                            plan.swing_w,
                            pal.accent,
                            Some(0.0),
                        );
                        if resp.changed() {
                            act.swing = Some(swing.clamp(0.0, ops::MAX_SWING));
                        }
                        resp.on_hover_ui(|ui| {
                            ui.label(format!(
                                "Swing delays every other 16th note: {:.0}%",
                                self.project.swing / ops::MAX_SWING * 100.0
                            ));
                            theme::hint(ui, pal, "Double-click to reset");
                        });
                        readout(
                            ui,
                            pal,
                            plan.swing_readout,
                            &format!("{:.0}%", self.project.swing / ops::MAX_SWING * 100.0),
                        );
                    });
                });
                #[cfg(test)]
                probe(ui, block);
                let _ = block;
            }

            // --- right side, laid out from the edge inward
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let master_w = plan.master_total();
                let block = fixed_block(ui, master_w, |ui| {
                    kicker(ui, pal, "MASTER");
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let mut vol = self.project.master_volume;
                        let resp = theme::slider_with(
                            ui,
                            pal,
                            &mut vol,
                            0.0..=1.0,
                            plan.master_w,
                            pal.accent,
                            Some(0.8),
                        );
                        if resp.changed() {
                            act.master = Some(vol.clamp(0.0, 1.0));
                        }
                        resp.on_hover_ui(|ui| {
                            ui.label(format!(
                                "Master volume {:.0}%",
                                self.project.master_volume * 100.0
                            ));
                            theme::hint(ui, pal, "Double-click to reset");
                        });
                        ui.add_space(gap);
                        readout(
                            ui,
                            pal,
                            plan.master_readout,
                            &format!("{:.0}%", self.project.master_volume * 100.0),
                        );
                        ui.add_space(theme::GAP_S);
                        let clipping = t.clip_until.is_some_and(|until| until > Instant::now());
                        master_meter(ui, pal, plan.meter_w, t.peak, clipping);
                    });
                });
                #[cfg(test)]
                probe(ui, block);
                let _ = block;
                bar_sep(ui, pal, plan);
                let slot = self.save_status(ui, pal, plan);
                #[cfg(test)]
                probe(ui, slot);
                let _ = slot;
                ui.add_space(theme::GAP_M);
                let redo = ui
                    .add_enabled_ui(self.history.can_redo(), |ui| {
                        theme::icon_button(
                            ui,
                            pal,
                            Icon::Redo,
                            plan.undo,
                            false,
                            pal.accent,
                            &cache.redo_tip,
                        )
                    })
                    .inner;
                #[cfg(test)]
                probe(ui, redo.rect);
                act.redo = redo.clicked();
                ui.add_space(gap);
                let undo = ui
                    .add_enabled_ui(self.history.can_undo(), |ui| {
                        theme::icon_button(
                            ui,
                            pal,
                            Icon::Undo,
                            plan.undo,
                            false,
                            pal.accent,
                            &cache.undo_tip,
                        )
                    })
                    .inner;
                #[cfg(test)]
                probe(ui, undo.rect);
                act.undo = undo.clicked();
            });
        });
        act
    }

    /// "● Unsaved" / "Saved" with a tooltip; just the icon in the tight tier.
    /// Returns the slot it occupies.
    fn save_status(&self, ui: &mut Ui, pal: Palette, plan: Plan) -> Rect {
        let dirty = self.dirty();
        let (text, ink) = if dirty {
            ("Unsaved", pal.warn)
        } else if self.doc.path.is_some() {
            ("Saved", pal.text_faint)
        } else {
            ("Not saved", pal.text_faint)
        };
        let slot = fixed_row(ui, plan.status_w, |ui| {
            ui.spacing_mut().item_spacing.x = theme::GAP_S;
            if !plan.status_text {
                ui.add_space((plan.status_w - theme::ICON_S) * 0.5);
            }
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(theme::ICON_S), Sense::hover());
            if dirty || self.doc.path.is_none() {
                ui.painter().circle_filled(rect.center(), 4.0, ink);
            } else {
                theme::paint_icon(ui.painter(), rect, Icon::Check, ink);
            }
            if plan.status_text {
                ui.add(
                    egui::Label::new(RichText::new(text).size(theme::TEXT_SMALL + 1.0).color(ink))
                        .selectable(false)
                        .wrap_mode(egui::TextWrapMode::Extend),
                );
            }
        });
        let rect = slot.response.rect;
        slot.response.on_hover_ui(|ui| {
            ui.label(if dirty {
                "Unsaved changes".to_owned()
            } else {
                match &self.doc.path {
                    Some(path) => format!("All changes saved to {}", path.display()),
                    None => "This song has not been saved to a file yet".to_owned(),
                }
            });
            theme::hint(
                ui,
                pal,
                &format!("Save with {}", ui.ctx().format_shortcut(&super::SC_SAVE)),
            );
        });
        rect
    }

    /// Bar.beat.sixteenth and elapsed time in a sunken display. Click swaps
    /// which one is big.
    fn position_display(&self, ui: &mut Ui, pal: Palette, width: f32) -> Response {
        let t = &self.transport;
        let c = &t.cache;
        let (rect, resp) = ui.allocate_exact_size(vec2(width, POS_H), Sense::click());
        if ui.is_rect_visible(rect) {
            let hot = resp.hovered();
            let p = ui.painter();
            p.rect(
                rect,
                theme::RADIUS_M,
                pal.inset,
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
            let (big, small, tag) = if t.time_first {
                (&c.time_now, &c.bar_line, "TIME")
            } else {
                (&c.position, &c.time_pair, "BAR")
            };
            p.text(
                pos2(rect.left() + 10.0, rect.top() + 4.0),
                Align2::LEFT_TOP,
                big,
                FontId::monospace(19.0),
                if t.playing { pal.accent } else { pal.text },
            );
            p.text(
                pos2(rect.left() + 10.0, rect.bottom() - 5.0),
                Align2::LEFT_BOTTOM,
                small,
                FontId::monospace(10.0),
                pal.text_faint,
            );
            p.text(
                pos2(rect.right() - 8.0, rect.top() + 6.0),
                Align2::RIGHT_TOP,
                tag,
                theme::strong_font(ui, 9.0),
                if hot { pal.text_dim } else { pal.text_faint },
            );
        }
        resp.on_hover_ui(|ui| {
            ui.label("Playhead position");
            theme::hint(ui, pal, "bar.beat.sixteenth and elapsed / song length");
            theme::hint(ui, pal, "Click to swap which one is big");
        })
    }

    /// Applies what the bar reported this frame.
    fn apply_bar_action(&mut self, act: BarAction) {
        if act.to_start {
            self.seek(0.0);
        }
        if act.play {
            self.toggle_play();
        }
        if act.stop {
            self.stop();
        }
        if act.record {
            self.toggle_record();
        }
        if act.looping {
            self.toggle_loop();
        }
        if act.metronome {
            self.toggle_metronome();
        }
        if act.toggle_time {
            self.transport.time_first = !self.transport.time_first;
        }
        if act.undo {
            self.undo();
        }
        if act.redo {
            self.redo();
        }
        if act.tap {
            self.tap_tempo();
        }
        if act.rename {
            let name = self.transport.name_buf.clone();
            self.edit("Rename song", Some("song-name"), |p| p.name = name);
        }
        if let Some(bpm) = act.bpm {
            self.edit("Change tempo", Some("bpm"), |p| p.bpm = bpm);
        }
        if let Some(swing) = act.swing {
            self.edit("Change swing", Some("swing"), |p| p.swing = swing);
        }
        if let Some(vol) = act.master {
            self.edit("Master volume", Some("master"), |p| p.master_volume = vol);
        }
    }
}

// ----- bar layout ---------------------------------------------------------------

/// Transport bar height.
pub(super) const TRANSPORT_H: f32 = 60.0;
/// Horizontal padding inside the bar.
const BAR_PAD: i8 = 12;
/// Height of a kicker + control block (kicker, 2 px gap, 28 px control).
const BLOCK_H: f32 = 40.0;
/// Height of the position display.
const POS_H: f32 = 44.0;
/// Edge of the brand mark.
const LOGO: f32 = 28.0;
/// Room kept free between the left and right groups.
const MIN_FLEX: f32 = 16.0;
/// Peak that counts as clipping on the master meter.
const CLIP_LEVEL: f32 = 0.99;
/// Id of the song-name text field.
const NAME_FIELD: &str = "cadence-transport-song-name";

/// Every width the bar uses at one size tier. The widest tier whose
/// [`Plan::width`] fits the window is chosen; the widths are exact, so the
/// left and right groups can never run into each other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Plan {
    /// Square transport buttons and the play button.
    btn: f32,
    play: f32,
    /// Undo / redo buttons.
    undo: f32,
    name_w: f32,
    pos_w: f32,
    tempo_w: f32,
    tap_w: f32,
    swing_w: f32,
    swing_readout: f32,
    master_w: f32,
    master_readout: f32,
    meter_w: f32,
    /// The save-status slot; `status_text` off shows only its icon.
    status_w: f32,
    status_text: bool,
    /// Space on each side of a divider.
    sep_pad: f32,
    swing: bool,
}

const ROOMY: Plan = Plan {
    btn: 32.0,
    play: 44.0,
    undo: 28.0,
    name_w: 176.0,
    pos_w: 148.0,
    tempo_w: 84.0,
    tap_w: 48.0,
    swing_w: 100.0,
    swing_readout: 36.0,
    master_w: 112.0,
    master_readout: 40.0,
    meter_w: 96.0,
    status_w: 84.0,
    status_text: true,
    sep_pad: 12.0,
    swing: true,
};
const NORMAL: Plan = Plan {
    name_w: 150.0,
    pos_w: 136.0,
    tempo_w: 80.0,
    swing_w: 80.0,
    swing_readout: 34.0,
    master_w: 96.0,
    master_readout: 38.0,
    meter_w: 72.0,
    sep_pad: 10.0,
    ..ROOMY
};
const TIGHT: Plan = Plan {
    btn: 28.0,
    play: 40.0,
    name_w: 112.0,
    pos_w: 124.0,
    tempo_w: 72.0,
    swing_w: 56.0,
    swing_readout: 30.0,
    master_w: 64.0,
    master_readout: 34.0,
    meter_w: 56.0,
    status_w: 16.0,
    status_text: false,
    sep_pad: 8.0,
    ..NORMAL
};
/// Below the smallest window the app allows: swing leaves the bar.
const MINIMAL: Plan = Plan {
    name_w: 96.0,
    swing: false,
    ..TIGHT
};

impl Plan {
    /// The widest tier that fits `available` px.
    pub(super) fn choose(available: f32) -> Plan {
        [ROOMY, NORMAL, TIGHT]
            .into_iter()
            .find(|plan| plan.width() <= available)
            .unwrap_or(MINIMAL)
    }

    fn sep(&self) -> f32 {
        1.0 + 2.0 * self.sep_pad
    }

    fn buttons(&self) -> f32 {
        5.0 * self.btn + self.play + 5.0 * theme::GAP_XS
    }

    fn master_total(&self) -> f32 {
        self.master_w + theme::GAP_XS + self.master_readout + theme::GAP_S + self.meter_w
    }

    fn left(&self) -> f32 {
        LOGO + theme::GAP_S
            + self.name_w
            + self.sep()
            + self.buttons()
            + self.sep()
            + self.pos_w
            + self.sep()
            + self.tempo_w
            + theme::GAP_XS
            + self.tap_w
            + if self.swing {
                theme::GAP_M + self.swing_w + theme::GAP_XS + self.swing_readout
            } else {
                0.0
            }
    }

    fn right(&self) -> f32 {
        self.undo * 2.0
            + theme::GAP_XS
            + theme::GAP_M
            + self.status_w
            + self.sep()
            + self.master_total()
    }

    /// Width of everything plus the room kept between the two groups.
    pub(super) fn width(&self) -> f32 {
        self.left() + self.right() + MIN_FLEX
    }
}

/// What the bar reported this frame.
#[derive(Debug, Default)]
struct BarAction {
    to_start: bool,
    play: bool,
    stop: bool,
    record: bool,
    looping: bool,
    metronome: bool,
    toggle_time: bool,
    undo: bool,
    redo: bool,
    tap: bool,
    rename: bool,
    bpm: Option<f64>,
    swing: Option<f64>,
    master: Option<f32>,
}

/// Tooltips that never change while the app runs.
#[derive(Debug)]
struct Tips {
    to_start: String,
    stop: String,
    record: String,
    recording: String,
    metronome: String,
    tap: String,
}

impl Tips {
    fn new(ctx: &Context) -> Self {
        Tips {
            to_start: theme::shortcut_tip(ctx, "Return to start", &SC_HOME),
            stop: "Stop — press again to return to the start".to_owned(),
            record: format!(
                "Record ({}) — perform into the selected track's pattern from the Keyboard tab or the Z X C… keys",
                ctx.format_shortcut(&SC_RECORD)
            ),
            recording: format!(
                "Recording ({}) — notes you play land in the selected track's pattern: live while playing, step by step while stopped. Click to stop.",
                ctx.format_shortcut(&SC_RECORD)
            ),
            metronome: "Metronome click".to_owned(),
            tap: "Tap tempo — click along with the music; three taps set the tempo".to_owned(),
        }
    }
}

/// A `width` × [`BLOCK_H`] slot whose content is stacked from the top; the
/// slot is exactly that wide whatever the content does.
fn fixed_block(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui)) -> Rect {
    ui.allocate_ui_with_layout(vec2(width, BLOCK_H), Layout::top_down(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        add(ui);
    })
    .response
    .rect
}

/// A `width` × [`BLOCK_H`] slot whose content is centered vertically.
fn fixed_row<R>(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    ui.allocate_ui_with_layout(
        vec2(width, BLOCK_H),
        Layout::left_to_right(Align::Center),
        add,
    )
}

/// Small tracked caps label above a control; never wraps or selects.
fn kicker(ui: &mut Ui, pal: Palette, text: &str) {
    ui.add(
        egui::Label::new(theme::kicker_text(pal, text))
            .selectable(false)
            .wrap_mode(egui::TextWrapMode::Extend),
    );
}

/// Value text in a fixed-width slot next to a slider.
fn readout(ui: &mut Ui, pal: Palette, width: f32, text: &str) {
    ui.allocate_ui_with_layout(
        vec2(width, theme::CONTROL_S),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(text)
                        .size(theme::TEXT_SMALL + 1.0)
                        .color(pal.text_dim),
                )
                .selectable(false)
                .wrap_mode(egui::TextWrapMode::Extend),
            );
        },
    );
}

/// Divider with room on both sides, 28 px tall.
fn bar_sep(ui: &mut Ui, pal: Palette, plan: Plan) {
    ui.add_space(plan.sep_pad);
    theme::divider_v(ui, pal, theme::CONTROL_M);
    ui.add_space(plan.sep_pad);
}

/// The Cadence mark: three bars on an accent tile.
pub(super) fn paint_brand_mark(painter: &egui::Painter, rect: Rect, pal: Palette) {
    let unit = rect.width() / 30.0;
    painter.rect_filled(rect, 8.0 * unit, pal.accent);
    let ink = theme::ink_on(pal.accent);
    for (i, h) in [0.45f32, 0.8, 0.6].iter().enumerate() {
        let x = rect.left() + (8.0 + i as f32 * 6.0) * unit;
        let bottom = rect.bottom() - 7.0 * unit;
        painter.rect_filled(
            Rect::from_min_max(
                pos2(x, bottom - h * 14.0 * unit),
                pos2(x + 3.5 * unit, bottom),
            ),
            1.5 * unit,
            ink,
        );
    }
}

/// Record: a red dot on a button body. Dim red when unarmed, solid red with
/// a tinted body when armed, pulsing while live recording rolls.
fn record_button(
    ui: &mut Ui,
    pal: Palette,
    time: f64,
    armed: bool,
    rolling: bool,
    size: f32,
    tooltip: &str,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, armed, "Record")
    });
    if ui.is_rect_visible(rect) {
        let down = resp.is_pointer_button_down_on();
        let hovered = resp.hovered();
        let (fill, border, dot) = if armed {
            let pulse = if rolling {
                0.5 + 0.5 * (time * std::f64::consts::TAU / 1.2).sin() as f32
            } else {
                1.0
            };
            (
                theme::mix(pal.raised, pal.danger, if down { 0.34 } else { 0.22 }),
                theme::mix(pal.raised, pal.danger, 0.7),
                theme::mix(theme::mix(pal.danger, pal.panel, 0.35), pal.danger, pulse),
            )
        } else {
            (
                if down {
                    pal.control_active
                } else if hovered {
                    pal.control_hover
                } else {
                    pal.control
                },
                if hovered {
                    theme::mix(pal.control_border, pal.text, 0.3)
                } else {
                    pal.control_border
                },
                theme::mix(pal.danger, pal.text_dim, if hovered { 0.0 } else { 0.25 }),
            )
        };
        let p = ui.painter();
        p.rect(
            rect,
            theme::RADIUS_M,
            fill,
            Stroke::new(1.0_f32, border),
            StrokeKind::Inside,
        );
        let side = (size * 0.57).round().max(10.0);
        theme::paint_icon(
            p,
            Rect::from_center_size(rect.center(), Vec2::splat(side)),
            Icon::Record,
            dot,
        );
    }
    resp.on_hover_ui(|ui| {
        ui.set_max_width(280.0);
        ui.label(tooltip);
    })
}

/// Level in 0..1 on a −54 dB … 0 dB scale, so quiet material still shows.
fn meter_position(level: f32) -> f32 {
    if level <= 0.0 {
        return 0.0;
    }
    ((20.0 * level.log10() + METER_FLOOR_DB) / METER_FLOOR_DB).clamp(0.0, 1.0)
}

const METER_FLOOR_DB: f32 = 54.0;

/// Two thin LED-style lanes (left over right) with a clip light at the end.
fn master_meter(ui: &mut Ui, pal: Palette, width: f32, peak: [f32; 2], clipping: bool) {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, METER_H), Sense::hover());
    if ui.is_rect_visible(rect) {
        let m = pal.mixer;
        let p = ui.painter();
        p.rect_filled(rect, 3.0, m.well);
        let inner = rect.shrink(2.0);
        let lane_h = (inner.height() - 2.0) * 0.5;
        let step = 4.0;
        let count = ((inner.width() + 1.0) / step).floor().max(1.0) as usize;
        for (lane, level) in peak.iter().enumerate() {
            let top = inner.top() + lane as f32 * (lane_h + 2.0);
            let lit = (meter_position(*level) * count as f32).round() as usize;
            for i in 0..count {
                let at = i as f32 / count as f32;
                let color = if i >= lit {
                    m.meter_off
                } else if clipping && i + 1 == count {
                    m.meter_clip
                } else if at < 0.62 {
                    m.meter_low
                } else if at < 0.86 {
                    m.meter_mid
                } else {
                    m.meter_high
                };
                let x = inner.left() + i as f32 * step;
                p.rect_filled(
                    Rect::from_min_size(pos2(x, top), vec2(step - 1.0, lane_h)),
                    0.5,
                    color,
                );
            }
        }
    }
    resp.on_hover_ui(|ui| {
        let db = |level: f32| {
            if level <= 0.0005 {
                "−∞ dB".to_owned()
            } else {
                format!("{:.1} dB", 20.0 * level.log10())
            }
        };
        ui.label(format!(
            "Master output   L {}   R {}",
            db(peak[0]),
            db(peak[1])
        ));
        if clipping {
            ui.colored_label(pal.danger, "Clipping — turn the master down");
        }
    });
}

/// Height of the master meter.
const METER_H: f32 = 12.0;

#[cfg(test)]
fn probe_id() -> Id {
    Id::new("transport-bar-probe")
}

#[cfg(test)]
fn probe_reset(ui: &Ui) {
    ui.ctx()
        .data_mut(|d| d.insert_temp(probe_id(), Vec::<Rect>::new()));
}

/// Records the rect of a bar item so tests can check the layout.
#[cfg(test)]
fn probe(ui: &Ui, rect: Rect) {
    ui.ctx().data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<Rect>>(probe_id())
            .push(rect);
    });
}

/// Rects of the items the bar placed in the last frame (tests).
#[cfg(test)]
fn probed(ctx: &Context) -> Vec<Rect> {
    ctx.data(|d| d.get_temp::<Vec<Rect>>(probe_id()).unwrap_or_default())
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

    #[test]
    fn play_start_clamps_into_the_loop_region() {
        let mut app = CadenceApp::headless(crate::model::Project::empty()); // 8 bars = 128 steps
        app.transport.looping = true;
        app.transport.loop_region = Some((32.0, 64.0));
        assert_eq!(app.play_start(80.0), 32.0, "past the end → region start");
        assert_eq!(app.play_start(64.0), 32.0);
        assert_eq!(app.play_start(48.0), 48.0, "inside the region");
        assert_eq!(
            app.play_start(10.0),
            10.0,
            "before it: the lead-in plays once"
        );
        app.transport.looping = false;
        assert_eq!(app.play_start(80.0), 80.0, "no clamp while looping is off");
        app.transport.looping = true;
        app.transport.loop_region = None;
        assert_eq!(app.play_start(200.0), 0.0, "past the song end rewinds");
        // A region outliving a shrunk song is ignored, not clamped into noise.
        app.project.bars = 1;
        app.transport.loop_region = Some((32.0, 64.0));
        assert_eq!(app.play_start(200.0), 0.0);
    }

    /// Frames of the real app at `width` × 860; returns the context and app.
    fn run_bar(width: f32, setup: impl FnOnce(&mut CadenceApp)) -> (Context, CadenceApp) {
        let ctx = Context::default();
        let mut app = CadenceApp::headless(crate::model::Project::demo());
        setup(&mut app);
        for n in 0..4 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 860.0))),
                focused: true,
                time: Some(f64::from(n) / 60.0),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| app.frame(ctx));
        }
        (ctx, app)
    }

    /// Every item of the bar stays inside the window and clear of every
    /// other item, at every width the app can be resized to, up to 4K.
    #[test]
    fn bar_items_never_overlap_or_leave_the_window() {
        // From the narrowest plan (below the 1100 px minimum window) to 4K.
        for width in [
            MINIMAL.width().ceil() + 24.0,
            1100.0,
            1150.0,
            1240.0,
            1300.0,
            1360.0,
            1440.0,
            1600.0,
            1920.0,
            2560.0,
        ] {
            for dirty in [false, true] {
                let (ctx, _) = run_bar(width, |app| {
                    if dirty {
                        app.edit("Rename", None, |p| p.name.push('!'));
                    }
                    app.transport.recording = dirty;
                    app.transport.peak = [0.9, 0.5];
                });
                let items = probed(&ctx);
                assert!(items.len() >= 14, "{width}: only {} items", items.len());
                for (i, a) in items.iter().enumerate() {
                    assert!(
                        a.left() >= -0.01 && a.right() <= width + 0.01,
                        "{width}: item {i} {a:?} leaves the window"
                    );
                    assert!(
                        a.top() >= 28.0 && a.bottom() <= 28.0 + TRANSPORT_H,
                        "{width}: item {i} {a:?} leaves the bar"
                    );
                    for (j, b) in items.iter().enumerate().skip(i + 1) {
                        assert!(
                            !a.shrink(0.01).intersects(b.shrink(0.01)),
                            "{width}: items {i} {a:?} and {j} {b:?} overlap"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn plans_fit_their_window_and_degrade_in_order() {
        for plan in [ROOMY, NORMAL, TIGHT, MINIMAL] {
            assert!((plan.width() - plan.left() - plan.right() - MIN_FLEX).abs() < 0.01);
        }
        // The designed windows: default 1360 wide, the 1100 minimum, 1080p.
        assert!(
            Plan::choose(1100.0).width() <= 1100.0,
            "the minimum window gets a plan that fits"
        );
        assert!(Plan::choose(1360.0).width() <= 1360.0);
        assert_eq!(Plan::choose(1920.0), ROOMY);
        // Wider windows never get a smaller plan.
        let widths = [1100.0, 1200.0, 1300.0, 1400.0, 1500.0, 2000.0];
        let sizes: Vec<f32> = widths.iter().map(|w| Plan::choose(*w).width()).collect();
        assert!(sizes.windows(2).all(|w| w[0] <= w[1]), "{sizes:?}");
        // Swing stays reachable at every size the app allows.
        assert!(Plan::choose(1100.0).swing);
        assert!(!MINIMAL.swing);
    }

    #[test]
    fn meter_scale_is_in_decibels() {
        assert_eq!(meter_position(0.0), 0.0);
        assert_eq!(meter_position(1.0), 1.0);
        assert!(meter_position(0.0001) < 0.05, "−80 dB is off the scale");
        // −18 dBFS lights a third of the meter: it used to be 12%.
        let eighteen = meter_position(10f32.powf(-18.0 / 20.0));
        assert!((0.6..0.7).contains(&eighteen), "{eighteen}");
        assert!(meter_position(0.5) > meter_position(0.25));
    }

    #[test]
    fn display_texts_rebuild_only_when_they_change() {
        let (ctx, mut app) = run_bar(1360.0, |_| {});
        assert_eq!(app.transport.cache.position, "1.1.1");
        app.transport.position = 3.4;
        app.refresh_bar(&ctx);
        let before = app.transport.cache.position.as_ptr();
        assert_eq!(app.transport.cache.position, "1.1.4");
        // Same sixteenth: the cached string is kept, not rebuilt.
        app.transport.position = 3.9;
        app.refresh_bar(&ctx);
        assert_eq!(app.transport.cache.position.as_ptr(), before);
        app.transport.position = 4.0;
        app.refresh_bar(&ctx);
        assert_eq!(app.transport.cache.position, "1.2.1");
        assert!(app.transport.cache.time_pair.contains(" / "));
        app.transport.time_first = true;
        assert_eq!(app.transport.cache.bar_line, "bar 1.2.1");
    }

    #[test]
    fn name_field_mirrors_the_song_and_renames_through_undo() {
        let (ctx, mut app) = run_bar(1360.0, |_| {});
        assert_eq!(app.transport.name_buf, app.project.name);
        app.transport.name_buf = "Night drive".to_owned();
        app.apply_bar_action(BarAction {
            rename: true,
            ..Default::default()
        });
        assert_eq!(app.project.name, "Night drive");
        assert!(app.history.can_undo());
        app.refresh_bar(&ctx);
        assert_eq!(app.transport.name_buf, "Night drive");
        app.undo();
        app.refresh_bar(&ctx);
        assert_eq!(app.transport.name_buf, app.project.name);
        assert_ne!(app.project.name, "Night drive");
    }

    #[test]
    fn tempo_swing_and_master_edits_are_undoable() {
        let (_, mut app) = run_bar(1360.0, |_| {});
        app.apply_bar_action(BarAction {
            bpm: Some(133.0),
            swing: Some(0.3),
            master: Some(0.5),
            ..Default::default()
        });
        assert_eq!(app.project.bpm, 133.0);
        assert_eq!(app.project.swing, 0.3);
        assert_eq!(app.project.master_volume, 0.5);
        assert!(app.history.can_undo());
    }

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
