//! In-app updates: the background check, the status-bar pill and the
//! Update dialog. The network, signature and installer work lives in
//! [`crate::update`]; this module only decides *when* and asks the user.
//!
//! Flow: a launch-time check (release builds, at most once a day, can be
//! turned off) or Help → Check for Updates finds a newer release → a pill
//! appears in the status bar → the dialog downloads and verifies it → the
//! user picks Restart & Update or Install When I Quit. Quitting goes through
//! the normal unsaved-changes guard, and the installer starts only once
//! the window is really closing.

use crossbeam_channel::{Receiver, TryRecvError};
use egui::{Context, OpenUrl, RichText, Ui, ViewportCommand};

use super::dialogs::{self, Dialog};
use super::theme::{self, Palette};
use super::{CadenceApp, StatusKind};
use crate::update::{self, InstallMode, Prepared, Release, UpdatePrefs};
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
    prepared: Option<Prepared>,
    /// Install as the window closes; `relaunch` reopens Cadence afterwards.
    pub(super) install_on_quit: Option<bool>,
    rx: Option<Receiver<Msg>>,
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
    /// Launch-time check: release builds only, at most once a day, never
    /// when the user turned automatic checks off.
    pub(super) fn auto_check_updates(&mut self, ctx: &Context) {
        if update::auto_check_allowed() && self.update.prefs.check_due(update::now_secs()) {
            self.start_update_check(ctx, false);
        }
    }

    /// Saves the update settings — never from headless tests.
    fn store_update_prefs(&self) {
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
        self.update.rx = spawn(ctx, move || {
            Msg::Downloaded(
                update::download(&release).and_then(|file| update::prepare(&mode, file)),
            )
        });
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
                self.update.phase = Phase::Failed("The update check stopped unexpectedly.".into());
                return;
            }
        };
        self.update.rx = None;
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
                    self.update.phase = Phase::Ready(release);
                }
                self.update.prepared = Some(prepared);
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
        let Some(prepared) = self.update.prepared.take() else {
            return true;
        };
        match update::launch(&prepared, relaunch) {
            Ok(()) => true,
            Err(err) => {
                self.update.prepared = Some(prepared);
                self.set_status(StatusKind::Error, format!("Update not installed: {err}"));
                false
            }
        }
    }

    /// Status-bar pill text while an update needs attention.
    fn update_pill(&self) -> Option<String> {
        match &self.update.phase {
            Phase::Available(r) => Some(format!("Update {} available", r.version)),
            Phase::Downloading(_) => Some("Downloading update…".to_owned()),
            Phase::Ready(_) if self.update.install_on_quit.is_some() => {
                Some("Update installs when you quit".to_owned())
            }
            Phase::Ready(_) => Some("Update ready — restart".to_owned()),
            _ => None,
        }
    }

    /// The status-bar pill (right-to-left layout: call before the song summary).
    pub(super) fn update_status_item(&mut self, ui: &mut Ui, pal: Palette) {
        let Some(label) = self.update_pill() else {
            return;
        };
        ui.add_space(theme::GAP_M);
        if theme::text_button(ui, pal, &label, theme::CONTROL_S, "Open the update details")
            .clicked()
        {
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
            match &phase {
                Phase::Idle | Phase::Checking | Phase::Downloading(_) => {
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
            ui.add_space(theme::GAP_M);
            ui.checkbox(&mut auto, "Check for updates automatically");
            theme::hint(
                ui,
                pal,
                "At most once a day, Cadence asks github.com for the newest version number. \
                 Nothing about you or your songs is sent.",
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
            None => {}
        }
        !closed && !resp.should_close()
    }
}

enum Action {
    Download(Release),
    Open(String),
    Skip(String),
    Install { relaunch: bool },
    Retry,
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
}
