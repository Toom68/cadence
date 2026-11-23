//! In-app updates: the background check, the status-bar pill and the
//! Update dialog. The network, signature and installer work lives in
//! [`crate::update`]; this module only decides *when* and asks the user.
//!
//! Flow: an automatic check (release builds, at every launch and once a day
//! while open, can be turned off) or Help → Check for Updates finds a newer release → a pill
//! appears in the status bar → the dialog downloads and verifies it → the
//! user picks Restart & Update or Install When I Quit. Quitting goes through
//! the normal unsaved-changes guard, and the installer starts only once
//! the window is really closing.

use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, TryRecvError};
use egui::{Context, OpenUrl, RichText, Ui, ViewportCommand};

use super::dialogs::{self, Dialog};
use super::theme::{self, Palette};
use super::{CadenceApp, StatusKind};
use crate::update::{self, HandoffOutcome, InstallMode, Prepared, Release, Transfer, UpdatePrefs};
use crate::APP_VERSION;

/// Where the update flow stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum Phase {
    /// No check made (or a quiet automatic check found nothing).
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release),
    /// Verified and unpacked; waiting for Restart & Update or quit.
    Ready(Release),
    Failed(String),
}

enum Msg {
    Checked(Result<Option<Release>, String>),
    Downloaded(Result<Prepared, String>),
}

/// The app's update state (never saved except [`UpdatePrefs`]).
#[derive(Default)]
pub(super) struct UpdateState {
    pub(super) phase: Phase,
    /// Whether the running check was asked for (Help menu) — automatic
    /// checks stay silent on failures and on skipped versions.
    manual: bool,
    pub(super) prefs: UpdatePrefs,
    pub(super) mode: Option<InstallMode>,
    /// The verified update and the version it installs.
    prepared: Option<(Prepared, String)>,
    /// The running download's progress and Cancel switch.
    transfer: Option<Arc<Transfer>>,
    /// The last update handed off didn't land (version, message): shown
    /// at the top of the Update dialog until it is closed.
    notice: Option<(String, String)>,
    /// Install as the window closes; `relaunch` reopens Cadence afterwards.
    pub(super) install_on_quit: Option<bool>,
    rx: Option<Receiver<Msg>>,
    /// Unix time of this session's latest automatic check (`None` until the
    /// launch check starts).
    last_attempt: Option<u64>,
}

impl UpdateState {
    /// The saved settings and this copy's install mode (real app only).
    pub(super) fn load() -> Self {
        UpdateState {
            prefs: update::load_prefs(),
            mode: Some(update::install_mode()),
            ..Default::default()
        }
    }

    fn busy(&self) -> bool {
        matches!(self.phase, Phase::Checking | Phase::Downloading(_))
    }

    /// Download progress: (bytes so far, expected total if known).
    fn progress(&self) -> Option<(u64, Option<u64>)> {
        self.transfer.as_ref().map(|t| t.progress())
    }

    fn cancelling(&self) -> bool {
        self.transfer.as_ref().is_some_and(|t| t.is_cancelled())
    }
}

/// `12.3 MB` style sizes for the progress line.
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// "42%" when the size is known, else the bytes so far.
fn progress_text(done: u64, total: Option<u64>) -> String {
    match total {
        Some(total) if total > 0 => format!(
            "{}% — {} of {}",
            (done.min(total) * 100 / total),
            megabytes(done),
            megabytes(total)
        ),
        _ => format!("{} downloaded", megabytes(done)),
    }
}

/// What the user is told when the previous run's update didn't install.
fn unfinished_message(version: &str) -> String {
    format!(
        "The update to Cadence {version} didn't finish — download it from {}",
        update::notes_url(version)
    )
}

/// Runs `job` on a worker thread and wakes the UI with its answer.
fn spawn(ctx: &Context, job: impl FnOnce() -> Msg + Send + 'static) -> Option<Receiver<Msg>> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("cadence-update".into())
        .spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        })
        .ok()?;
    Some(rx)
}

impl CadenceApp {
    /// Automatic checks, run every frame: one at launch, then once a day
    /// while Cadence stays open (an idle window wakes itself when the next
    /// one is due). Release builds only, never when the user turned them
    /// off, and never while an update is downloading or waiting to install.
    pub(super) fn auto_check_updates(&mut self, ctx: &Context) {
        if !update::auto_check_allowed()
            || !matches!(
                self.update.phase,
                Phase::Idle | Phase::UpToDate | Phase::Available(_) | Phase::Failed(_)
            )
        {
            return;
        }
        let now = update::now_secs();
        match self
            .update
            .prefs
            .next_check_in(now, self.update.last_attempt)
        {
            Some(0) => {
                self.update.last_attempt = Some(now);
                self.start_update_check(ctx, false);
            }
            Some(wait) => ctx.request_repaint_after(std::time::Duration::from_secs(wait)),
            None => {}
        }
    }

    /// At launch: did the update the previous run handed off land? Says so
    /// in the status line; a failed one also opens the Update dialog once
    /// (unless another dialog, like crash recovery, is up).
    pub(super) fn review_previous_update(&mut self) {
        if !self.io_enabled {
            return;
        }
        let mode = self
            .update
            .mode
            .clone()
            .unwrap_or_else(update::install_mode);
        let outcome = update::review_handoff(&mode);
        self.apply_handoff_outcome(outcome);
    }

    fn apply_handoff_outcome(&mut self, outcome: Option<HandoffOutcome>) {
        match outcome {
            Some(HandoffOutcome::Installed { version }) => {
                self.set_status(
                    StatusKind::Success,
                    format!("Cadence updated to {version}."),
                );
            }
            Some(HandoffOutcome::Unfinished { version, first }) => {
                let message = unfinished_message(&version);
                self.set_status(StatusKind::Warning, message.clone());
                if first && matches!(self.dialog, Dialog::None) {
                    self.dialog = Dialog::Update;
                }
                self.update.notice = Some((version, message));
            }
            None => {}
        }
    }

    /// Saves the update settings — never from headless tests.
    pub(super) fn store_update_prefs(&self) {
        if self.io_enabled {
            update::save_prefs(&self.update.prefs);
        }
    }

    /// Help → Check for Updates…: checks now and shows the answer.
    pub(super) fn check_updates_now(&mut self, ctx: &Context) {
        self.dialog = Dialog::Update;
        if !matches!(self.update.phase, Phase::Downloading(_) | Phase::Ready(_)) {
            self.start_update_check(ctx, true);
        }
    }

    fn start_update_check(&mut self, ctx: &Context, manual: bool) {
        if self.update.busy() {
            self.update.manual |= manual;
            return;
        }
        if !self.io_enabled {
            self.update.phase = Phase::Failed("Updates are off in this session.".into());
            return;
        }
        self.update.manual = manual;
        self.update.phase = Phase::Checking;
        self.update.rx = spawn(ctx, || {
            update::clean_staging();
            Msg::Checked(update::check())
        });
    }

    fn start_update_download(&mut self, ctx: &Context, release: Release) {
        let mode = self
            .update
            .mode
            .clone()
            .unwrap_or_else(|| InstallMode::Manual(String::new()));
        self.update.phase = Phase::Downloading(release.clone());
        let transfer = Arc::new(Transfer::new(release.size));
        self.update.transfer = Some(Arc::clone(&transfer));
        self.update.rx = spawn(ctx, move || {
            Msg::Downloaded(
                update::download(&release, &transfer).and_then(|file| update::prepare(&mode, file)),
            )
        });
    }

    /// Cancel in the Update dialog: curl is killed and the partial file
    /// removed on the worker thread; its answer puts the offer back.
    fn cancel_update_download(&mut self) {
        if let Some(transfer) = &self.update.transfer {
            transfer.cancel();
        }
    }

    /// Picks up a finished check or download. Called once per frame.
    pub(super) fn poll_updates(&mut self) {
        let Some(rx) = &self.update.rx else {
            return;
        };
        let msg = match rx.try_recv() {
            Ok(msg) => msg,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.update.rx = None;
                self.update.transfer = None;
                self.update.phase = Phase::Failed("The update check stopped unexpectedly.".into());
                return;
            }
        };
        self.update.rx = None;
        let cancelled = self.update.cancelling();
        self.update.transfer = None;
        let manual = self.update.manual;
        match msg {
            Msg::Checked(result) => {
                if result.is_ok() {
                    self.update.prefs.last_check = update::now_secs();
                    self.store_update_prefs();
                }
                self.update.phase = match result {
                    Ok(Some(release)) => {
                        let skipped =
                            self.update.prefs.skipped.as_deref() == Some(release.version.as_str());
                        if skipped && !manual {
                            Phase::Idle
                        } else {
                            if !manual {
                                self.set_status(
                                    StatusKind::Info,
                                    format!(
                                        "Cadence {} is available — click the update button in the status bar.",
                                        release.version
                                    ),
                                );
                            }
                            Phase::Available(release)
                        }
                    }
                    Ok(None) if manual => Phase::UpToDate,
                    Err(err) if manual => Phase::Failed(err),
                    _ => Phase::Idle,
                };
            }
            Msg::Downloaded(Ok(prepared)) => {
                if let Phase::Downloading(release) = std::mem::take(&mut self.update.phase) {
                    self.set_status(
                        StatusKind::Success,
                        format!("Cadence {} is downloaded and verified.", release.version),
                    );
                    self.update.prepared = Some((prepared, release.version.clone()));
                    self.update.phase = Phase::Ready(release);
                }
            }
            Msg::Downloaded(Err(err)) if cancelled || err == update::CANCELLED => {
                if let Phase::Downloading(release) = std::mem::take(&mut self.update.phase) {
                    self.update.phase = Phase::Available(release);
                }
                self.set_status(StatusKind::Info, "Update download cancelled.");
            }
            Msg::Downloaded(Err(err)) => {
                self.set_status(StatusKind::Warning, format!("Update not installed: {err}"));
                self.update.phase = Phase::Failed(err);
            }
        }
    }

    /// Starts the installer if the user asked for one; called as the window
    /// is about to close. Returns false (and keeps Cadence open with an
    /// error) when the installer can't even start.
    pub(super) fn launch_pending_update(&mut self) -> bool {
        let Some(relaunch) = self.update.install_on_quit.take() else {
            return true;
        };
        let Some((prepared, version)) = self.update.prepared.take() else {
            return true;
        };
        match update::launch(&prepared, &version, relaunch) {
            Ok(()) => true,
            Err(err) => {
                self.update.prepared = Some((prepared, version));
                self.set_status(StatusKind::Error, format!("Update not installed: {err}"));
                false
            }
        }
    }

    /// Status-bar pill text while an update needs attention.
    fn update_pill(&self) -> Option<String> {
        match &self.update.phase {
            Phase::Available(r) => Some(format!("Update {} available", r.version)),
            Phase::Downloading(_) => Some(match self.update.progress() {
                Some((done, Some(total))) if total > 0 => {
                    format!("Downloading update… {}%", done.min(total) * 100 / total)
                }
                _ => "Downloading update…".to_owned(),
            }),
            Phase::Ready(_) if self.update.install_on_quit.is_some() => {
                Some("Update installs when you quit".to_owned())
            }
            Phase::Ready(_) => Some("Update ready — restart".to_owned()),
            _ => None,
        }
    }

    /// The pill's compact label for a crowded status bar.
    fn update_pill_short(&self) -> Option<String> {
        match &self.update.phase {
            Phase::Available(r) => Some(format!("Update {}", r.version)),
            Phase::Downloading(_) => Some(match self.update.progress() {
                Some((done, Some(total))) if total > 0 => {
                    format!("Update {}%", done.min(total) * 100 / total)
                }
                _ => "Updating…".to_owned(),
            }),
            Phase::Ready(_) if self.update.install_on_quit.is_some() => {
                Some("Update on quit".to_owned())
            }
            Phase::Ready(_) => Some("Restart to update".to_owned()),
            _ => None,
        }
    }

    fn update_pill_label(&self, short: bool) -> Option<String> {
        if short {
            self.update_pill_short()
        } else {
            self.update_pill()
        }
    }

    /// Room the pill takes in the status bar (0 when there is none), with
    /// its leading gap and dot.
    pub(super) fn update_status_width(&self, ui: &Ui, short: bool) -> f32 {
        self.update_pill_label(short).map_or(0.0, |label| {
            theme::GAP_M
                + theme::text_button_width(ui, &label, theme::CONTROL_S)
                + 8.0
                + 2.0 * ui.spacing().item_spacing.x
        })
    }

    /// The status-bar pill (right-to-left layout: call before the song
    /// summary). `short` picks the compact label when space is tight.
    pub(super) fn update_status_item(&mut self, ui: &mut Ui, pal: Palette, short: bool) {
        let Some(label) = self.update_pill_label(short) else {
            return;
        };
        if matches!(self.update.phase, Phase::Downloading(_)) {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        ui.add_space(theme::GAP_M);
        let tip = match self.update_pill().filter(|_| short) {
            Some(full) => format!("{full} · Open the update details"),
            None => "Open the update details".to_owned(),
        };
        if theme::text_button(ui, pal, &label, theme::CONTROL_S, &tip).clicked() {
            self.dialog = Dialog::Update;
        }
        theme::color_dot(ui, pal.accent, 8.0);
    }

    /// The Update dialog: state, the one sensible next step, and the
    /// automatic-check preference.
    pub(super) fn update_dialog(&mut self, ctx: &Context) -> bool {
        let pal = self.palette();
        let phase = self.update.phase.clone();
        let mode = self
            .update
            .mode
            .clone()
            .unwrap_or_else(|| InstallMode::Manual(String::new()));
        let mut auto = self.update.prefs.auto_check;
        let mut closed = false;
        let mut action = None;
        let progress = self.update.progress();
        let cancelling = self.update.cancelling();
        let notice = self
            .update
            .notice
            .clone()
            .filter(|_| !matches!(phase, Phase::Ready(_)));
        if matches!(phase, Phase::Downloading(_)) {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        let resp = dialogs::show_dialog(ctx, "cadence-update", pal, dialogs::W_COMPACT, |ui| {
            let title = match &phase {
                Phase::Idle | Phase::Checking => "Checking for updates…".to_owned(),
                Phase::UpToDate => "Cadence is up to date".to_owned(),
                Phase::Failed(_) => "Couldn't update Cadence".to_owned(),
                Phase::Available(r) => format!("Cadence {} is available", r.version),
                Phase::Downloading(r) => format!("Downloading Cadence {}…", r.version),
                Phase::Ready(r) => format!("Cadence {} is ready to install", r.version),
            };
            dialogs::header(ui, pal, "UPDATES", &title);
            ui.add_space(theme::GAP_S);
            let body = |ui: &mut Ui, text: &str| {
                ui.label(RichText::new(text).color(pal.text_dim));
            };
            if let Some((version, message)) = &notice {
                ui.label(RichText::new(message.as_str()).color(pal.warn));
                ui.hyperlink_to(
                    format!("Cadence {version} release page"),
                    update::notes_url(version),
                );
                ui.add_space(theme::GAP_S);
            }
            match &phase {
                Phase::Downloading(_) => {
                    let (done, total) = progress.unwrap_or((0, None));
                    match total {
                        Some(total) if total > 0 => {
                            let fraction = (done as f32 / total as f32).clamp(0.0, 1.0);
                            ui.add(egui::ProgressBar::new(fraction).desired_height(8.0));
                        }
                        _ => {
                            ui.spinner();
                        }
                    }
                    let text = if cancelling {
                        "Cancelling…".to_owned()
                    } else {
                        format!(
                            "{} · You can keep working meanwhile.",
                            progress_text(done, total)
                        )
                    };
                    body(ui, &text);
                }
                Phase::Idle | Phase::Checking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        body(ui, "You can keep working meanwhile.");
                    });
                }
                Phase::UpToDate => body(
                    ui,
                    &format!("You have the newest version, Cadence {APP_VERSION}."),
                ),
                Phase::Failed(err) => body(ui, err),
                Phase::Available(_) => {
                    body(ui, &format!("You have Cadence {APP_VERSION}."));
                    match &mode {
                        InstallMode::Manual(reason) if !reason.is_empty() => {
                            ui.add_space(theme::GAP_XS);
                            theme::hint(ui, pal, reason);
                        }
                        _ => theme::hint(
                            ui,
                            pal,
                            "The download is checked against Cadence's release signature before anything installs.",
                        ),
                    }
                }
                Phase::Ready(_) => body(
                    ui,
                    "Cadence will close, update and reopen. If the song has unsaved changes \
                     you can save them first.",
                ),
            }
            let notes = match &phase {
                Phase::Available(r) | Phase::Downloading(r) | Phase::Ready(r) => r.notes.as_deref(),
                _ => None,
            };
            if let Some(notes) = notes {
                ui.add_space(theme::GAP_S);
                ui.label(RichText::new("What's new").strong());
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("cadence-update-notes")
                        .max_height(160.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(notes).color(pal.text_dim)).wrap(),
                            );
                        });
                });
            }
            ui.add_space(theme::GAP_M);
            ui.checkbox(&mut auto, "Check for updates automatically");
            theme::hint(
                ui,
                pal,
                "When it opens and once a day while open, Cadence asks github.com for the \
                 newest version number. Nothing about you or your songs is sent.",
            );
            dialogs::button_row(ui, |ui| match &phase {
                Phase::Available(r) => {
                    let primary = if mode.is_automatic() {
                        "Download & Install"
                    } else {
                        "Open Download Page"
                    };
                    if theme::primary_button(ui, pal, primary).clicked() {
                        action = Some(Action::Download(r.clone()));
                    }
                    if theme::secondary_button(ui, "What's New").clicked() {
                        action = Some(Action::Open(update::notes_url(&r.version)));
                    }
                    if theme::secondary_button(ui, "Skip This Version").clicked() {
                        action = Some(Action::Skip(r.version.clone()));
                    }
                }
                Phase::Ready(_) => {
                    if theme::primary_button(ui, pal, "Restart & Update").clicked() {
                        action = Some(Action::Install { relaunch: true });
                    }
                    if theme::secondary_button(ui, "Install When I Quit").clicked() {
                        action = Some(Action::Install { relaunch: false });
                    }
                }
                Phase::Downloading(_) => {
                    if theme::primary_button(ui, pal, "Close").clicked() {
                        closed = true;
                    }
                    if theme::secondary_button(ui, "Cancel Download").clicked() && !cancelling {
                        action = Some(Action::Cancel);
                    }
                }
                Phase::Failed(_) => {
                    if theme::primary_button(ui, pal, "Close").clicked() {
                        closed = true;
                    }
                    if theme::secondary_button(ui, "Try Again").clicked() {
                        action = Some(Action::Retry);
                    }
                    if theme::secondary_button(ui, "Download Page").clicked() {
                        action = Some(Action::Open(update::download_page()));
                    }
                }
                _ => {
                    if theme::primary_button(ui, pal, "Close").clicked() {
                        closed = true;
                    }
                }
            });
        });
        if auto != self.update.prefs.auto_check {
            self.update.prefs.auto_check = auto;
            self.store_update_prefs();
        }
        match action {
            Some(Action::Download(release)) if mode.is_automatic() => {
                self.start_update_download(ctx, release);
            }
            Some(Action::Download(_)) => {
                ctx.open_url(OpenUrl::new_tab(update::download_page()));
                closed = true;
            }
            Some(Action::Open(url)) => ctx.open_url(OpenUrl::new_tab(url)),
            Some(Action::Skip(version)) => {
                if self
                    .update
                    .notice
                    .as_ref()
                    .is_some_and(|(v, _)| *v == version)
                {
                    self.update.notice = None;
                    if self.io_enabled {
                        update::forget_handoff();
                    }
                }
                self.update.prefs.skipped = Some(version);
                self.store_update_prefs();
                self.update.phase = Phase::Idle;
                closed = true;
            }
            Some(Action::Install { relaunch }) => {
                self.update.install_on_quit = Some(relaunch);
                closed = true;
                if relaunch {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                } else {
                    self.set_status(
                        StatusKind::Info,
                        "The update installs when you quit Cadence.",
                    );
                }
            }
            Some(Action::Retry) => self.start_update_check(ctx, true),
            Some(Action::Cancel) => self.cancel_update_download(),
            None => {}
        }
        let keep = !closed && !resp.should_close();
        if !keep {
            self.update.notice = None;
        }
        keep
    }
}

enum Action {
    Download(Release),
    Open(String),
    Skip(String),
    Install { relaunch: bool },
    Retry,
    Cancel,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Project;
    use egui::RawInput;

    fn release() -> Release {
        Release {
            version: "9.9.9".into(),
            asset: Some("Cadence-9.9.9-windows-x64-setup.exe".into()),
            ..Release::default()
        }
    }

    fn frame(app: &mut CadenceApp, ctx: &Context) {
        let _ = ctx.run(RawInput::default(), |ctx| app.frame(ctx));
    }

    #[test]
    fn headless_sessions_never_go_online() {
        let mut app = CadenceApp::headless(Project::demo());
        let ctx = Context::default();
        app.check_updates_now(&ctx);
        assert!(matches!(app.update.phase, Phase::Failed(_)));
        assert!(app.update.rx.is_none());
        assert!(matches!(app.dialog, Dialog::Update));
        frame(&mut app, &ctx);
        assert!(matches!(app.dialog, Dialog::Update), "dialog stays up");
    }

    #[test]
    fn available_update_shows_a_pill_and_dialog() {
        let mut app = CadenceApp::headless(Project::demo());
        let ctx = Context::default();
        app.update.mode = Some(InstallMode::WindowsSetup);
        app.update.phase = Phase::Available(release());
        assert_eq!(app.update_pill().as_deref(), Some("Update 9.9.9 available"));
        app.dialog = Dialog::Update;
        for _ in 0..3 {
            frame(&mut app, &ctx);
        }
        assert!(matches!(app.dialog, Dialog::Update));
    }

    #[test]
    fn installer_waits_for_an_explicit_choice() {
        let mut app = CadenceApp::headless(Project::demo());
        app.update.phase = Phase::Ready(release());
        assert_eq!(app.update_pill().as_deref(), Some("Update ready — restart"));
        // Nothing chosen: closing launches nothing.
        assert!(app.launch_pending_update());
        app.update.install_on_quit = Some(false);
        assert_eq!(
            app.update_pill().as_deref(),
            Some("Update installs when you quit")
        );
        // Chosen but nothing prepared (download state lost): still a clean quit.
        assert!(app.launch_pending_update());
        assert_eq!(app.update.install_on_quit, None);
    }

    #[test]
    fn quiet_checks_ignore_skipped_versions_and_failures() {
        let mut app = CadenceApp::headless(Project::demo());
        app.update.prefs.skipped = Some("9.9.9".into());
        let (tx, rx) = crossbeam_channel::bounded(1);
        tx.send(Msg::Checked(Ok(Some(release())))).unwrap();
        app.update.rx = Some(rx);
        app.poll_updates();
        assert_eq!(app.update.phase, Phase::Idle);

        let (tx, rx) = crossbeam_channel::bounded(1);
        tx.send(Msg::Checked(Err("offline".into()))).unwrap();
        app.update.rx = Some(rx);
        app.poll_updates();
        assert_eq!(app.update.phase, Phase::Idle);

        // Asked for explicitly, the same answers are shown.
        app.update.manual = true;
        let (tx, rx) = crossbeam_channel::bounded(1);
        tx.send(Msg::Checked(Ok(Some(release())))).unwrap();
        app.update.rx = Some(rx);
        app.poll_updates();
        assert_eq!(app.update.phase, Phase::Available(release()));
    }

    #[test]
    fn download_shows_progress_notes_and_cancels_cleanly() {
        let mut app = CadenceApp::headless(Project::demo());
        let ctx = Context::default();
        let release = Release {
            size: Some(1_000_000),
            notes: Some("Faster startup.\n- Fixed the mixer".repeat(40)),
            ..release()
        };
        app.update.mode = Some(InstallMode::WindowsSetup);
        app.update.phase = Phase::Downloading(release.clone());
        let transfer = Arc::new(Transfer::new(release.size));
        app.update.transfer = Some(Arc::clone(&transfer));
        assert_eq!(app.update_pill().as_deref(), Some("Downloading update… 0%"));
        app.dialog = Dialog::Update;
        for _ in 0..3 {
            frame(&mut app, &ctx);
        }
        assert!(matches!(app.dialog, Dialog::Update));

        app.cancel_update_download();
        assert!(transfer.is_cancelled());
        assert!(app.update.cancelling());
        frame(&mut app, &ctx);
        // The worker answers; the offer comes back without an error.
        let (tx, rx) = crossbeam_channel::bounded(1);
        tx.send(Msg::Downloaded(Err(update::CANCELLED.into())))
            .unwrap();
        app.update.rx = Some(rx);
        app.poll_updates();
        assert_eq!(app.update.phase, Phase::Available(release));
        assert!(app.update.transfer.is_none());
        frame(&mut app, &ctx);
    }

    #[test]
    fn progress_reads_as_percent_or_megabytes() {
        assert_eq!(
            progress_text(250_000, Some(1_000_000)),
            "25% — 0.2 MB of 1.0 MB"
        );
        assert_eq!(progress_text(2_500_000, None), "2.5 MB downloaded");
        assert!(progress_text(5, Some(1)).starts_with("100%"));
    }

    #[test]
    fn an_unfinished_update_is_reported_with_the_release_page() {
        let mut app = CadenceApp::headless(Project::demo());
        let ctx = Context::default();
        app.apply_handoff_outcome(Some(HandoffOutcome::Unfinished {
            version: "9.9.9".into(),
            first: true,
        }));
        assert!(matches!(app.dialog, Dialog::Update));
        let (version, message) = app.update.notice.clone().unwrap();
        assert_eq!(version, "9.9.9");
        assert!(message.contains("didn't finish"), "{message}");
        assert!(message.contains(&update::notes_url("9.9.9")), "{message}");
        assert!(app.status.as_ref().is_some_and(|s| s.text == message));
        frame(&mut app, &ctx);
        assert!(matches!(app.dialog, Dialog::Update));

        // Later launches only use the status line.
        let mut app = CadenceApp::headless(Project::demo());
        app.apply_handoff_outcome(Some(HandoffOutcome::Unfinished {
            version: "9.9.9".into(),
            first: false,
        }));
        assert!(matches!(app.dialog, Dialog::None));
        assert!(app.status.is_some());

        let mut app = CadenceApp::headless(Project::demo());
        app.apply_handoff_outcome(Some(HandoffOutcome::Installed {
            version: "9.9.9".into(),
        }));
        assert!(app
            .status
            .as_ref()
            .is_some_and(|s| s.text == "Cadence updated to 9.9.9."));
        assert!(app.update.notice.is_none());
    }
}
