//! Signed in-app updates.
//!
//! Every tagged release publishes `latest.json` (the newest version and the
//! file to fetch per platform) plus each download with a minisign signature
//! beside it. Cadence fetches the manifest with the operating system's own
//! `curl` — no TLS stack is bundled — downloads its platform's file, and
//! checks it against the public key compiled into the binary. The signature's
//! trusted comment must name exactly that version and file, so a tampered,
//! swapped or replayed download is refused before anything runs.
//!
//! Nothing installs without the user's go-ahead. Windows hands the verified
//! setup to Inno Setup (per-user, no admin prompt); macOS swaps the verified
//! `Cadence.app` into place after the app quits. Portable copies and
//! read-only locations get the download page instead.
//!
//! Both installers finish after Cadence has quit, so a hand-off marker in
//! the data dir lets the next launch report an update that didn't land
//! (and, on macOS, remove the previous bundle kept for a manual rollback
//! once the new version has launched twice). Downloads report progress and
//! can be cancelled ([`Transfer`]); the manifest may carry each file's size
//! and short release notes, both optional.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};

use crate::APP_VERSION;

/// Where releases live — one constant, so the feed can move to a store or
/// CDN that serves the same `latest/download/…` and `download/v…/…` paths.
pub const RELEASES_BASE: &str = "https://github.com/Toom68/cadence/releases";
/// Overrides [`RELEASES_BASE`] (to stage a release); must still be https.
pub const UPDATE_URL_ENV: &str = "CADENCE_UPDATE_URL";
/// When set, the automatic launch-time check never runs.
pub const NO_CHECK_ENV: &str = "CADENCE_NO_UPDATE_CHECK";
/// The release manifest's file name.
pub const MANIFEST_NAME: &str = "latest.json";
/// While Cadence stays open, automatic checks repeat this long after the
/// last successful one (every launch checks as well).
pub const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
/// A failed automatic check (offline) is retried after this long.
pub const RETRY_SECS: u64 = 60 * 60;

/// The release signing public key (minisign format). Until a real key is
/// committed this is a placeholder that fails to parse, and updates fall
/// back to the download page.
const UPDATE_KEY: &str = include_str!("../packaging/update-key.pub");

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 16 * 1024;
const MAX_ASSET_BYTES: u64 = 256 * 1024 * 1024;

// ----- versions and the manifest --------------------------------------------

/// `"1.2.0"` / `"v1.2.0"` → `[1, 2, 0]`. Exactly three numeric parts; anything
/// else (pre-release tags, junk) is not a version Cadence will offer.
pub fn parse_version(s: &str) -> Option<[u64; 3]> {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.');
    let mut out = [0u64; 3];
    for slot in &mut out {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// True when `candidate` is a strictly newer version than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// The manifest key for this build's download, if the platform has one.
pub fn platform_key() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("windows-x64")
    } else if cfg!(target_os = "macos") {
        Some("macos-universal")
    } else {
        None
    }
}

/// A release newer than (or equal to) this build, as the manifest describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// This platform's download file name; `None` when the release has none.
    pub asset: Option<String>,
    /// The download's size in bytes, when the manifest gives it (progress).
    pub size: Option<u64>,
    /// Short plain-text release notes for the Update dialog.
    pub notes: Option<String>,
}

/// `latest.json`. Fields added after 1.2 (`sizes`, `notes`) are optional and
/// read leniently, so a malformed extra never hides an update; older apps
/// ignore them (serde skips unknown fields).
#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    assets: BTreeMap<String, String>,
    /// File name → size in bytes.
    #[serde(default)]
    sizes: serde_json::Value,
    #[serde(default)]
    notes: serde_json::Value,
}

/// Release notes are shown as plain text and capped at this many bytes.
pub const MAX_NOTES_BYTES: usize = 4 * 1024;

/// Plain text only: no control characters but newlines and tabs, CRLF
/// folded, trimmed, and cut to [`MAX_NOTES_BYTES`] on a character boundary.
fn clean_notes(raw: &str) -> Option<String> {
    let text: String = raw
        .replace("\r\n", "\n")
        .chars()
        .map(|c| if c == '\r' { '\n' } else { c })
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.len() <= MAX_NOTES_BYTES {
        return Some(text.to_owned());
    }
    let mut end = MAX_NOTES_BYTES - '…'.len_utf8();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(format!("{}…", text[..end].trim_end()))
}

/// File names are joined onto URLs and a local folder: plain names only.
fn asset_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Parses `latest.json`. The manifest is not trusted for anything that
/// matters: each download's signature binds its own version and name.
pub fn parse_manifest(bytes: &[u8], platform: Option<&str>) -> Result<Release, String> {
    let manifest: Manifest = serde_json::from_slice(bytes)
        .map_err(|err| format!("The update information is unreadable ({err})."))?;
    let version = manifest.version.trim();
    let version = version.strip_prefix('v').unwrap_or(version).to_owned();
    if parse_version(&version).is_none() {
        return Err(format!(
            "The update information names an invalid version \"{}\".",
            manifest.version
        ));
    }
    let asset = platform
        .and_then(|key| manifest.assets.get(key))
        .filter(|name| asset_name_ok(name))
        .cloned();
    let size = asset
        .as_deref()
        .and_then(|name| manifest.sizes.get(name))
        .and_then(serde_json::Value::as_u64)
        .filter(|&n| n > 0 && n <= MAX_ASSET_BYTES);
    let notes = manifest.notes.as_str().and_then(clean_notes);
    Ok(Release {
        version,
        asset,
        size,
        notes,
    })
}

/// The base URL releases are served from.
pub fn releases_base() -> String {
    std::env::var(UPDATE_URL_ENV)
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| url.starts_with("https://"))
        .unwrap_or_else(|| RELEASES_BASE.to_owned())
}

/// The newest release's page — the fallback for manual installs.
pub fn download_page() -> String {
    format!("{}/latest", releases_base())
}

/// Release notes for `version`.
pub fn notes_url(version: &str) -> String {
    format!("{}/tag/v{version}", releases_base())
}

fn manifest_url(base: &str) -> String {
    format!("{base}/latest/download/{MANIFEST_NAME}")
}

fn asset_url(base: &str, version: &str, name: &str) -> String {
    format!("{base}/download/v{version}/{name}")
}

// ----- signatures ------------------------------------------------------------

/// The release signing key compiled into this build, if a real one is.
pub fn update_key() -> Option<PublicKey> {
    PublicKey::decode(UPDATE_KEY.trim()).ok()
}

/// What the release pipeline signs as the trusted comment of each file.
pub fn trusted_comment(version: &str, asset: &str) -> String {
    format!("cadence-update {version} {asset}")
}

/// Checks `bytes` against `signature` (a `.minisig` file's text): signed by
/// `key`, and signed *as* `asset` of `version`.
pub fn verify(
    key: &PublicKey,
    bytes: &[u8],
    signature: &str,
    version: &str,
    asset: &str,
) -> Result<(), String> {
    let sig = Signature::decode(signature.trim())
        .map_err(|err| format!("The update's signature is unreadable ({err})."))?;
    key.verify(bytes, &sig, false)
        .map_err(|_| "The downloaded update failed its signature check.".to_owned())?;
    if sig.trusted_comment() != trusted_comment(version, asset) {
        return Err("The update's signature is for a different file or version.".to_owned());
    }
    Ok(())
}

// ----- network -----------------------------------------------------------------

/// The OS-provided curl (Windows 10 1803+, every macOS): the system TLS
/// stack and certificate store, with nothing extra to ship or patch.
fn curl_program() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let path = PathBuf::from(root).join("System32").join("curl.exe");
            if path.is_file() {
                return path;
            }
        }
        PathBuf::from("curl.exe")
    }
    #[cfg(not(windows))]
    {
        let path = Path::new("/usr/bin/curl");
        if path.is_file() {
            path.to_path_buf()
        } else {
            PathBuf::from("curl")
        }
    }
}

/// Child processes of the GUI app must not flash a console window.
fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// What a cancelled download reports (the UI goes back quietly).
pub const CANCELLED: &str = "The download was cancelled.";

/// A download's progress and its Cancel switch, shared between the worker
/// thread and the UI.
#[derive(Debug, Default)]
pub struct Transfer {
    cancel: AtomicBool,
    received: AtomicU64,
    /// Expected size in bytes; 0 = unknown.
    total: AtomicU64,
}

impl Transfer {
    /// A transfer expecting `total` bytes, when the manifest says.
    pub fn new(total: Option<u64>) -> Self {
        Transfer {
            total: AtomicU64::new(total.unwrap_or(0)),
            ..Transfer::default()
        }
    }

    /// Asks the download to stop: curl is killed and the partial file removed.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Bytes on disk so far and the expected total, if known.
    pub fn progress(&self) -> (u64, Option<u64>) {
        let total = self.total.load(Ordering::Relaxed);
        (
            self.received.load(Ordering::Relaxed),
            (total > 0).then_some(total),
        )
    }

    /// Share done, 0.0..=1.0, when the total is known.
    pub fn fraction(&self) -> Option<f32> {
        let (done, total) = self.progress();
        total.map(|total| (done as f64 / total as f64).clamp(0.0, 1.0) as f32)
    }
}

/// How often a running curl is polled for progress and cancellation.
const POLL: Duration = Duration::from_millis(100);

/// Downloads `url` to `out` over https only (redirects included). With a
/// `transfer`, the file's growing size is reported and a cancel kills curl
/// and deletes the partial file.
fn fetch(
    url: &str,
    out: &Path,
    max_bytes: u64,
    timeout_secs: u32,
    transfer: Option<&Transfer>,
) -> Result<(), String> {
    let mut cmd = Command::new(curl_program());
    cmd.args([
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "15",
        "--retry",
        "2",
    ])
    .arg("--max-time")
    .arg(timeout_secs.to_string())
    .arg("--max-filesize")
    .arg(max_bytes.to_string())
    .arg("--user-agent")
    .arg(format!("Cadence/{APP_VERSION}"))
    .arg("--output")
    .arg(out)
    .arg(url)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    // `--silent --show-error` prints one short line at most, far below a
    // pipe buffer, so reading it after exit cannot deadlock.
    .stderr(Stdio::piped());
    hide_console(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|err| format!("Could not start curl to download the update ({err})."))?;
    let status = loop {
        if transfer.is_some_and(Transfer::is_cancelled) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(out);
            return Err(CANCELLED.to_owned());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(out);
                return Err(format!("The download stopped unexpectedly ({err})."));
            }
        }
        if let (Some(transfer), Ok(meta)) = (transfer, fs::metadata(out)) {
            transfer.received.store(meta.len(), Ordering::Relaxed);
        }
        std::thread::sleep(POLL);
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    if status.success() {
        if let (Some(transfer), Ok(meta)) = (transfer, fs::metadata(out)) {
            transfer.received.store(meta.len(), Ordering::Relaxed);
        }
        return Ok(());
    }
    let _ = fs::remove_file(out);
    let detail = stderr.trim().trim_start_matches("curl: ");
    Err(if detail.contains("error: 404") {
        "No update information is published yet.".to_owned()
    } else {
        format!("Couldn't reach the update server — are you online? ({detail})")
    })
}

/// Reads at most `max` bytes of `path`, refusing anything larger.
fn read_capped(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    let file =
        fs::File::open(path).map_err(|err| format!("Could not read the download ({err})."))?;
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("Could not read the download ({err})."))?;
    if bytes.len() as u64 > max {
        return Err("The download is larger than any Cadence release.".to_owned());
    }
    Ok(bytes)
}

/// Scratch folder for manifests and downloads (per user).
fn staging_dir() -> PathBuf {
    std::env::temp_dir().join("cadence-update")
}

/// Removes leftovers of an earlier update (a finished installer, a manifest).
/// Best effort: a setup that is still running keeps its file locked.
pub fn clean_staging() {
    let _ = fs::remove_dir_all(staging_dir());
}

/// Fetches the manifest; `Some` when it offers a version newer than this one.
pub fn check() -> Result<Option<Release>, String> {
    let dir = staging_dir();
    fs::create_dir_all(&dir)
        .map_err(|err| format!("Could not create {} ({err}).", dir.display()))?;
    let path = dir.join(format!("{MANIFEST_NAME}.{}", std::process::id()));
    fetch(
        &manifest_url(&releases_base()),
        &path,
        MAX_MANIFEST_BYTES,
        30,
        None,
    )?;
    let bytes = read_capped(&path, MAX_MANIFEST_BYTES);
    let _ = fs::remove_file(&path);
    let release = parse_manifest(&bytes?, platform_key())?;
    Ok(is_newer(&release.version, APP_VERSION).then_some(release))
}

/// Downloads this platform's file of `release` and verifies its signature;
/// returns the verified file. A file that fails verification is deleted, as
/// is a partial one when `transfer` is cancelled (the error is [`CANCELLED`]).
pub fn download(release: &Release, transfer: &Transfer) -> Result<PathBuf, String> {
    let name = release
        .asset
        .as_deref()
        .ok_or("That release has no download for this computer.")?;
    let key = update_key().ok_or(
        "This build can't verify updates (it has no release key) — use the download page instead.",
    )?;
    let base = releases_base();
    let dir = staging_dir();
    fs::create_dir_all(&dir)
        .map_err(|err| format!("Could not create {} ({err}).", dir.display()))?;
    let file = dir.join(name);
    let sig_path = dir.join(format!("{name}.minisig"));
    let sig_url = asset_url(&base, &release.version, &format!("{name}.minisig"));
    fetch(&sig_url, &sig_path, MAX_SIGNATURE_BYTES, 60, Some(transfer))?;
    transfer.received.store(0, Ordering::Relaxed);
    let fetched = fetch(
        &asset_url(&base, &release.version, name),
        &file,
        MAX_ASSET_BYTES,
        900,
        Some(transfer),
    );
    if fetched.is_err() || transfer.is_cancelled() {
        let _ = fs::remove_file(&sig_path);
        let _ = fs::remove_file(&file);
        return Err(fetched.err().unwrap_or_else(|| CANCELLED.to_owned()));
    }
    let checked = read_capped(&sig_path, MAX_SIGNATURE_BYTES)
        .and_then(|sig| String::from_utf8(sig).map_err(|_| "The signature is not text.".to_owned()))
        .and_then(|sig| {
            let bytes = read_capped(&file, MAX_ASSET_BYTES)?;
            verify(&key, &bytes, &sig, &release.version, name)
        });
    let _ = fs::remove_file(&sig_path);
    if let Err(err) = checked {
        let _ = fs::remove_file(&file);
        return Err(format!("{err} Nothing was installed."));
    }
    Ok(file)
}

// ----- installing --------------------------------------------------------------

/// How this copy of Cadence can be updated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallMode {
    /// Installed by the Windows setup: run the newer setup when Cadence quits.
    WindowsSetup,
    /// A `Cadence.app` in a writable folder: swap in the new bundle on quit.
    MacBundle(PathBuf),
    /// No in-place update (portable copy, read-only location): the reason.
    Manual(String),
}

impl InstallMode {
    pub fn is_automatic(&self) -> bool {
        !matches!(self, InstallMode::Manual(_))
    }
}

/// Works out how the running executable can be updated.
pub fn install_mode() -> InstallMode {
    match std::env::current_exe() {
        Ok(exe) => install_mode_for(&exe),
        Err(_) => InstallMode::Manual("Cadence couldn't locate its own program file.".into()),
    }
}

fn install_mode_for(exe: &Path) -> InstallMode {
    if cfg!(windows) {
        if exe.with_file_name("unins000.exe").is_file() {
            InstallMode::WindowsSetup
        } else {
            InstallMode::Manual(
                "This copy of Cadence was unzipped rather than installed. Install it once with \
                 the setup from the download page and later updates install themselves."
                    .into(),
            )
        }
    } else if cfg!(target_os = "macos") {
        mac_install_mode(exe)
    } else {
        InstallMode::Manual("One-click updates aren't available on this platform.".into())
    }
}

fn mac_install_mode(exe: &Path) -> InstallMode {
    let Some(bundle) = exe
        .ancestors()
        .nth(3)
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
    else {
        return InstallMode::Manual("Cadence isn't running from an app bundle.".into());
    };
    let shown = bundle.to_string_lossy();
    let movable = "Move Cadence to your Applications folder to turn on one-click updates.";
    if shown.contains("/AppTranslocation/") || shown.starts_with("/Volumes/") {
        return InstallMode::Manual(movable.into());
    }
    let Some(parent) = bundle.parent() else {
        return InstallMode::Manual(movable.into());
    };
    let probe = parent.join(format!(".cadence-write-probe-{}", std::process::id()));
    if fs::create_dir(&probe).is_err() {
        return InstallMode::Manual(format!(
            "Cadence can't write to {}. {movable}",
            parent.display()
        ));
    }
    let _ = fs::remove_dir(&probe);
    InstallMode::MacBundle(bundle.to_path_buf())
}

/// A verified update, unpacked and ready for [`launch`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prepared {
    /// The verified Windows setup executable.
    WindowsSetup(PathBuf),
    /// The new bundle, staged beside the running one so the swap is a rename.
    MacBundle {
        current: PathBuf,
        staged: PathBuf,
        stage: PathBuf,
    },
}

/// Turns a verified download into something [`launch`] can install.
pub fn prepare(mode: &InstallMode, file: PathBuf) -> Result<Prepared, String> {
    match mode {
        InstallMode::WindowsSetup => Ok(Prepared::WindowsSetup(file)),
        InstallMode::MacBundle(current) => {
            let parent = current
                .parent()
                .ok_or("Cadence's folder couldn't be found.")?;
            let stage = parent.join(".cadence-update");
            let _ = fs::remove_dir_all(&stage);
            fs::create_dir_all(&stage)
                .map_err(|err| format!("Could not create {} ({err}).", stage.display()))?;
            let status = Command::new("/usr/bin/ditto")
                .args(["-x", "-k"])
                .arg(&file)
                .arg(&stage)
                .stdin(Stdio::null())
                .status()
                .map_err(|err| format!("Could not unpack the update ({err})."))?;
            let _ = fs::remove_file(&file);
            let staged = stage.join("Cadence.app");
            if !status.success() || !staged.join("Contents/MacOS/cadence").is_file() {
                let _ = fs::remove_dir_all(&stage);
                return Err("The update couldn't be unpacked.".into());
            }
            // Downloads made by Cadence itself carry no quarantine flag; this
            // only guards against an inherited one. Failure is harmless.
            let _ = Command::new("/usr/bin/xattr")
                .args(["-dr", "com.apple.quarantine"])
                .arg(&staged)
                .stdin(Stdio::null())
                .status();
            Ok(Prepared::MacBundle {
                current: current.clone(),
                staged,
                stage,
            })
        }
        InstallMode::Manual(reason) => Err(reason.clone()),
    }
}

/// The previous bundle's name inside the staging folder after a swap. It
/// is kept as a manual rollback until the new version has launched twice.
pub const MAC_BACKUP_NAME: &str = "Cadence (previous).app";

/// Swaps the bundle once Cadence (`$1`) has exited, rolling back if the new
/// one can't be moved in; reopens the app when `$5` is 1. On success the
/// old bundle stays in the staging folder as `Cadence (previous).app` (see
/// [`review_handoff`]); everything else staged is removed.
const MAC_SWAP_SCRIPT: &str = r#"pid="$1"; app="$2"; new="$3"; stage="$4"; relaunch="$5"
i=0
while kill -0 "$pid" 2>/dev/null; do
  sleep 0.2; i=$((i+1)); [ "$i" -gt 600 ] && exit 1
done
old="$stage/Cadence (previous).app"
rm -rf "$old"
if mv "$app" "$old"; then
  if mv "$new" "$app"; then
    find "$stage" -mindepth 1 -maxdepth 1 ! -name "Cadence (previous).app" -exec rm -rf {} +
  else
    mv "$old" "$app"
  fi
fi
[ "$relaunch" = 1 ] && open "$app"
exit 0
"#;

/// Starts the installer for `version` as Cadence quits. `relaunch` reopens
/// Cadence afterwards (Restart & Update) instead of just installing (on
/// quit). A hand-off marker is written first so the next launch can tell
/// whether the update landed ([`review_handoff`]).
pub fn launch(prepared: &Prepared, version: &str, relaunch: bool) -> Result<(), String> {
    let data = crate::persistence::data_dir();
    record_handoff_in(&data, version);
    let started = match prepared {
        Prepared::WindowsSetup(setup) => {
            let mut cmd = Command::new(setup);
            cmd.args([
                "/SILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/SP-",
                "/CLOSEAPPLICATIONS",
            ]);
            if !relaunch {
                cmd.arg("/norelaunch=1");
            }
            cmd.spawn()
                .map(drop)
                .map_err(|err| format!("Could not start the update installer ({err})."))
        }
        Prepared::MacBundle {
            current,
            staged,
            stage,
        } => Command::new("/bin/sh")
            .arg("-c")
            .arg(MAC_SWAP_SCRIPT)
            .arg("sh")
            .arg(std::process::id().to_string())
            .arg(current)
            .arg(staged)
            .arg(stage)
            .arg(if relaunch { "1" } else { "0" })
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(drop)
            .map_err(|err| format!("Could not start the update ({err}).")),
    };
    if started.is_err() {
        clear_handoff_in(&data);
    }
    started
}

// ----- after the hand-off ----------------------------------------------------------

/// `<data dir>/update-pending.json`: written just before an installer or
/// swap starts. A silent setup or a swap script can fail after Cadence has
/// quit, where nobody sees it; the next launch compares its own version
/// with this one and says so.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Handoff {
    /// The version handed to the installer.
    pub version: String,
    /// The version that handed it off.
    pub from: String,
    /// Unix time of the hand-off.
    pub started: u64,
    /// The new version has launched once (the macOS rollback copy is kept
    /// until the launch after that).
    pub launched: bool,
    /// The "didn't finish" dialog has been shown once already.
    pub notified: bool,
}

/// What the previous run's hand-off came to, as this launch sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandoffOutcome {
    /// This is the new version, launched for the first time.
    Installed { version: String },
    /// Still older than the version handed off: the install failed or was
    /// interrupted. `first` the first time this is noticed.
    Unfinished { version: String, first: bool },
}

const HANDOFF_FILE: &str = "update-pending.json";

fn handoff_path(data_dir: &Path) -> PathBuf {
    data_dir.join(HANDOFF_FILE)
}

fn write_handoff_in(data_dir: &Path, handoff: &Handoff) {
    let _ = fs::create_dir_all(data_dir);
    if let Ok(json) = serde_json::to_vec_pretty(handoff) {
        let _ = crate::persistence::write_file_atomic(&handoff_path(data_dir), &json);
    }
}

/// Best effort: without the marker the update still installs, the next
/// launch just can't report a failure.
fn record_handoff_in(data_dir: &Path, version: &str) {
    write_handoff_in(
        data_dir,
        &Handoff {
            version: version.to_owned(),
            from: APP_VERSION.to_owned(),
            started: now_secs(),
            ..Handoff::default()
        },
    );
}

fn read_handoff_in(data_dir: &Path) -> Option<Handoff> {
    let bytes = fs::read(handoff_path(data_dir)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn clear_handoff_in(data_dir: &Path) {
    let _ = fs::remove_file(handoff_path(data_dir));
}

/// What a launch running `running` makes of `handoff`: the outcome to
/// report, the marker to keep (`None` = delete it) and whether the macOS
/// rollback copy can go.
fn review(handoff: &Handoff, running: &str) -> (Option<HandoffOutcome>, Option<Handoff>, bool) {
    if parse_version(&handoff.version).is_none() {
        return (None, None, false);
    }
    let version = handoff.version.clone();
    if is_newer(&version, running) {
        let first = !handoff.notified;
        let keep = Handoff {
            notified: true,
            ..handoff.clone()
        };
        return (
            Some(HandoffOutcome::Unfinished { version, first }),
            Some(keep),
            false,
        );
    }
    if handoff.launched {
        // The second launch of the new version: it works, drop the backup.
        return (None, None, true);
    }
    let keep = Handoff {
        launched: true,
        ..handoff.clone()
    };
    (
        Some(HandoffOutcome::Installed { version }),
        Some(keep),
        false,
    )
}

/// Checks the previous run's hand-off (if any) at launch: updates the
/// marker, removes the macOS rollback copy once the new version has
/// launched twice, and says what to tell the user.
pub fn review_handoff() -> Option<HandoffOutcome> {
    review_handoff_in(&crate::persistence::data_dir(), &install_mode())
}

fn review_handoff_in(data_dir: &Path, mode: &InstallMode) -> Option<HandoffOutcome> {
    let handoff = read_handoff_in(data_dir);
    let Some(handoff) = handoff else {
        // A marker that is unreadable (or absent) is simply dropped.
        clear_handoff_in(data_dir);
        return None;
    };
    let (outcome, keep, drop_backup) = review(&handoff, APP_VERSION);
    match keep {
        Some(keep) => write_handoff_in(data_dir, &keep),
        None => clear_handoff_in(data_dir),
    }
    if drop_backup {
        if let InstallMode::MacBundle(bundle) = mode {
            remove_mac_backup(bundle, std::env::current_exe().ok().as_deref());
        }
    }
    outcome
}

/// The rollback copy a macOS swap leaves beside `bundle`.
pub fn mac_backup_path(bundle: &Path) -> Option<PathBuf> {
    Some(
        bundle
            .parent()?
            .join(".cadence-update")
            .join(MAC_BACKUP_NAME),
    )
}

/// Deletes the rollback copy — never the bundle or executable that is
/// running (`running_exe`), whatever the paths look like — and the
/// then-empty staging folder.
fn remove_mac_backup(bundle: &Path, running_exe: Option<&Path>) {
    let Some(backup) = mac_backup_path(bundle) else {
        return;
    };
    let in_use = bundle.starts_with(&backup)
        || backup.starts_with(bundle)
        || running_exe.is_some_and(|exe| exe.starts_with(&backup));
    if in_use || !backup.is_dir() {
        return;
    }
    let _ = fs::remove_dir_all(&backup);
    if let Some(stage) = backup.parent() {
        let _ = fs::remove_dir(stage);
    }
}

/// Forgets a hand-off (the user skipped that version, say).
pub fn forget_handoff() {
    clear_handoff_in(&crate::persistence::data_dir());
}

// ----- preferences ---------------------------------------------------------------

/// The user's update settings, kept in `<data dir>/updates.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdatePrefs {
    /// Check at every launch and once a day while open (on by default;
    /// Help → Check for Updates always works).
    pub auto_check: bool,
    /// Unix time of the last successful check.
    pub last_check: u64,
    /// A version the user chose to skip — automatic checks stay quiet on it.
    pub skipped: Option<String>,
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        UpdatePrefs {
            auto_check: true,
            last_check: 0,
            skipped: None,
        }
    }
}

impl UpdatePrefs {
    /// Whether a day has passed since the last successful check at unix
    /// time `now` (or the clock went backwards).
    pub fn check_due(&self, now: u64) -> bool {
        self.auto_check && (now < self.last_check || now - self.last_check >= CHECK_INTERVAL_SECS)
    }

    /// Seconds until the next automatic check (0 = now); `None` when
    /// automatic checks are off. `last_attempt` is this session's latest
    /// automatic attempt: `None` means none yet, so every launch checks
    /// right away. Afterwards a check is due a day after the last
    /// successful one, and a failed attempt (offline) waits [`RETRY_SECS`]
    /// instead of retrying every frame.
    pub fn next_check_in(&self, now: u64, last_attempt: Option<u64>) -> Option<u64> {
        if !self.auto_check {
            return None;
        }
        let Some(attempt) = last_attempt else {
            return Some(0);
        };
        let due = if now < self.last_check {
            now
        } else {
            self.last_check.saturating_add(CHECK_INTERVAL_SECS)
        };
        Some(
            due.max(attempt.saturating_add(RETRY_SECS))
                .saturating_sub(now),
        )
    }
}

fn prefs_path() -> PathBuf {
    crate::persistence::data_dir().join("updates.json")
}

/// The saved update settings (defaults when missing or unreadable).
pub fn load_prefs() -> UpdatePrefs {
    fs::read(prefs_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Saves the update settings. Best effort, like the audio-device pick.
pub fn save_prefs(prefs: &UpdatePrefs) {
    if let Ok(json) = serde_json::to_vec_pretty(prefs) {
        let _ = crate::persistence::write_file_atomic(&prefs_path(), &json);
    }
}

/// Unix time now (0 if the clock is before 1970).
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Automatic checks run only in release builds and when not opted out by
/// [`NO_CHECK_ENV`]; development runs never phone home on their own.
pub fn auto_check_allowed() -> bool {
    !cfg!(debug_assertions) && std::env::var_os(NO_CHECK_ENV).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Throwaway test key pair; the secret half was discarded.
    const TEST_KEY: &str = "untrusted comment: minisign public key 930C7E0253A95B03
RWQDW6lTAn4MkxXhGiCMKvesybkEQ6Np/6ABO1QfZa7+j2Y/mqJcFiRr
";
    const OTHER_KEY: &str = "untrusted comment: minisign public key 469417D625FDCBD3
RWTTy/0l1heURgyyqXFdl9LboTZoEkNcrWNjJ8pB+DS42TLw7bmcBs2u
";
    const PAYLOAD: &[u8] = b"cadence update payload fixture";
    const ASSET: &str = "Cadence-9.8.7-windows-x64-setup.exe";
    /// `minisign -S -t "cadence-update 9.8.7 <ASSET>"` over [`PAYLOAD`].
    const SIGNATURE: &str = "untrusted comment: signature from minisign secret key
RUQDW6lTAn4Mk5aQNBJWn0LKtR298FfDGTr8emzWiqNK2h7zRDaMYHybfARvRpoRVjfN/oDOVPHTUF7RVs5ACE89d+9RZzi1ow0=
trusted comment: cadence-update 9.8.7 Cadence-9.8.7-windows-x64-setup.exe
2MO/2Re4WtFvgUH+LPZFlegWLQjiHMlY0AcKxzPiVjhdNdqWndv2vAOJEOi2GPW0506FUoIIYYaTvDQiprZNAA==
";

    fn key(text: &str) -> PublicKey {
        PublicKey::decode(text.trim()).expect("test key parses")
    }

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(parse_version("1.2.3"), Some([1, 2, 3]));
        assert_eq!(parse_version("v10.0.1"), Some([10, 0, 1]));
        for bad in [
            "",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "1.2.3-beta",
            "+1.2.3",
            "1..3",
            " v",
        ] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.2.0", "1.2.0"));
        assert!(!is_newer("1.1.9", "1.2.0"));
        assert!(!is_newer("garbage", "1.2.0"));
    }

    #[test]
    fn manifest_picks_this_platforms_asset() {
        let json = br#"{"version": "v1.3.0", "assets": {
            "windows-x64": "Cadence-1.3.0-windows-x64-setup.exe",
            "macos-universal": "Cadence-1.3.0-macos-universal.zip",
            "evil": "../../etc/passwd"}}"#;
        let win = parse_manifest(json, Some("windows-x64")).unwrap();
        assert_eq!(win.version, "1.3.0");
        assert_eq!(
            win.asset.as_deref(),
            Some("Cadence-1.3.0-windows-x64-setup.exe")
        );
        let mac = parse_manifest(json, Some("macos-universal")).unwrap();
        assert_eq!(
            mac.asset.as_deref(),
            Some("Cadence-1.3.0-macos-universal.zip")
        );
        assert_eq!(parse_manifest(json, Some("evil")).unwrap().asset, None);
        assert_eq!(parse_manifest(json, None).unwrap().asset, None);
        assert!(parse_manifest(br#"{"version": "soon"}"#, None).is_err());
        assert!(parse_manifest(b"<html>", None).is_err());
    }

    #[test]
    fn manifest_extras_are_optional_lenient_and_forward_compatible() {
        // What CI writes since sizes and notes were added.
        let json = br#"{"version": "1.3.0",
            "assets": {"windows-x64": "Cadence-1.3.0-windows-x64-setup.exe"},
            "sizes": {"Cadence-1.3.0-windows-x64-setup.exe": 12345678},
            "notes": "Faster startup.\r\n\n- Fixed \u0007the mixer",
            "some_future_field": {"nested": [1, 2, 3]}}"#;
        let release = parse_manifest(json, Some("windows-x64")).unwrap();
        assert_eq!(release.size, Some(12_345_678));
        assert_eq!(
            release.notes.as_deref(),
            Some("Faster startup.\n\n- Fixed the mixer")
        );
        // The pre-1.3 manifest shape still parses, without extras.
        let old = br#"{"version": "1.3.0", "assets": {}}"#;
        let release = parse_manifest(old, Some("windows-x64")).unwrap();
        assert_eq!((release.size, release.notes), (None, None));
        // Malformed extras are ignored rather than hiding the update.
        let odd = br#"{"version": "1.3.0",
            "assets": {"windows-x64": "Cadence-1.3.0-windows-x64-setup.exe"},
            "sizes": {"Cadence-1.3.0-windows-x64-setup.exe": "big"}, "notes": 7}"#;
        let release = parse_manifest(odd, Some("windows-x64")).unwrap();
        assert_eq!(release.version, "1.3.0");
        assert_eq!((release.size, release.notes), (None, None));
        let odd = br#"{"version": "1.3.0", "sizes": [1], "notes": "   "}"#;
        assert_eq!(parse_manifest(odd, None).unwrap().notes, None);
    }

    #[test]
    fn release_notes_are_capped_on_a_character_boundary() {
        let long = "é".repeat(MAX_NOTES_BYTES);
        let notes = clean_notes(&long).unwrap();
        assert!(notes.len() <= MAX_NOTES_BYTES, "{}", notes.len());
        assert!(notes.ends_with('…'));
        assert_eq!(clean_notes("  ok\tfine \n").as_deref(), Some("ok\tfine"));
        assert_eq!(clean_notes("\u{1b}[31m"), Some("[31m".to_owned()));
        assert_eq!(clean_notes(""), None);
    }

    #[test]
    fn transfer_reports_progress_and_cancels() {
        let t = Transfer::new(Some(200));
        assert_eq!(t.progress(), (0, Some(200)));
        t.received.store(50, Ordering::Relaxed);
        assert_eq!(t.fraction(), Some(0.25));
        t.received.store(999, Ordering::Relaxed);
        assert_eq!(t.fraction(), Some(1.0));
        assert_eq!(Transfer::new(None).fraction(), None);
        assert!(!t.is_cancelled());
        t.cancel();
        assert!(t.is_cancelled());
    }

    /// A cancelled fetch kills curl and removes the partial file. The URL is
    /// loopback, and curl is killed before it could get anywhere anyway.
    #[test]
    fn cancelled_fetch_removes_the_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("partial.exe");
        fs::write(&out, b"half a download").unwrap();
        let transfer = Transfer::new(None);
        transfer.cancel();
        let result = fetch("https://127.0.0.1:9/x", &out, 1024, 5, Some(&transfer));
        if let Err(err) = &result {
            if err.starts_with("Could not start curl") {
                return; // no curl on this machine
            }
        }
        assert_eq!(result, Err(CANCELLED.to_owned()));
        assert!(!out.exists());
    }

    #[test]
    fn handoff_review_reports_failures_and_keeps_the_backup_one_launch() {
        let handoff = Handoff {
            version: "1.3.0".into(),
            from: "1.2.0".into(),
            started: 1,
            ..Handoff::default()
        };
        // Still the old version: the install didn't finish. Said loudly
        // once, quietly afterwards; the marker stays until it lands.
        let (outcome, keep, drop_backup) = review(&handoff, "1.2.0");
        assert_eq!(
            outcome,
            Some(HandoffOutcome::Unfinished {
                version: "1.3.0".into(),
                first: true
            })
        );
        let keep = keep.unwrap();
        assert!(keep.notified && !drop_backup);
        let (outcome, keep, _) = review(&keep, "1.2.0");
        assert_eq!(
            outcome,
            Some(HandoffOutcome::Unfinished {
                version: "1.3.0".into(),
                first: false
            })
        );
        assert!(keep.is_some());
        // The new version's first launch: report it, keep the backup.
        let (outcome, keep, drop_backup) = review(&handoff, "1.3.0");
        assert_eq!(
            outcome,
            Some(HandoffOutcome::Installed {
                version: "1.3.0".into()
            })
        );
        let keep = keep.unwrap();
        assert!(keep.launched && !drop_backup);
        // Its second launch: quietly drop the marker and the backup.
        assert_eq!(review(&keep, "1.3.0"), (None, None, true));
        // A later version (installed by hand) counts as landed too.
        assert!(matches!(
            review(&handoff, "1.4.0").0,
            Some(HandoffOutcome::Installed { .. })
        ));
        // Junk markers are dropped.
        let junk = Handoff {
            version: "../x".into(),
            ..Handoff::default()
        };
        assert_eq!(review(&junk, "1.2.0"), (None, None, false));
    }

    #[test]
    fn handoff_marker_round_trips_and_cleans_up_the_mac_backup() {
        let data = tempfile::tempdir().unwrap();
        let apps = tempfile::tempdir().unwrap();
        let bundle = apps.path().join("Cadence.app");
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        let backup = mac_backup_path(&bundle).unwrap();
        fs::create_dir_all(&backup).unwrap();
        let mode = InstallMode::MacBundle(bundle.clone());
        assert_eq!(review_handoff_in(data.path(), &mode), None);

        // The marker names this very version: the update landed.
        record_handoff_in(data.path(), APP_VERSION);
        let marker = read_handoff_in(data.path()).unwrap();
        assert_eq!(
            (marker.version.as_str(), marker.from.as_str()),
            (APP_VERSION, APP_VERSION)
        );
        assert!(matches!(
            review_handoff_in(data.path(), &mode),
            Some(HandoffOutcome::Installed { .. })
        ));
        assert!(backup.is_dir(), "kept through the first launch");
        assert_eq!(review_handoff_in(data.path(), &mode), None);
        assert!(!backup.exists(), "removed on the next launch");
        assert!(
            !backup.parent().unwrap().exists(),
            "empty staging folder removed"
        );
        assert!(bundle.is_dir(), "the app itself is untouched");
        assert!(read_handoff_in(data.path()).is_none());

        // A newer version than this one was handed off: it didn't finish.
        record_handoff_in(data.path(), "999.0.0");
        assert_eq!(
            review_handoff_in(data.path(), &mode),
            Some(HandoffOutcome::Unfinished {
                version: "999.0.0".into(),
                first: true
            })
        );
        clear_handoff_in(data.path());
        assert!(read_handoff_in(data.path()).is_none());

        // An unreadable marker is dropped.
        fs::write(handoff_path(data.path()), b"{not json").unwrap();
        assert_eq!(review_handoff_in(data.path(), &mode), None);
        assert!(!handoff_path(data.path()).exists());
    }

    #[test]
    fn backup_cleanup_never_touches_the_running_app() {
        let apps = tempfile::tempdir().unwrap();
        let bundle = apps.path().join("Cadence.app");
        fs::create_dir_all(&bundle).unwrap();
        let backup = mac_backup_path(&bundle).unwrap();
        let exe = backup.join("Contents/MacOS/cadence");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, b"").unwrap();
        // Someone launched the backup copy itself: it stays.
        remove_mac_backup(&bundle, Some(&exe));
        assert!(exe.is_file());
        remove_mac_backup(&bundle, Some(&bundle.join("Contents/MacOS/cadence")));
        assert!(!backup.exists());
        assert!(bundle.is_dir());
    }

    #[test]
    fn asset_names_are_plain_file_names() {
        assert!(asset_name_ok("Cadence-1.2.0-macos-universal.zip"));
        for bad in ["", ".hidden", "a/b", "a\\b", "..", "a b", "x?y"] {
            assert!(!asset_name_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn signature_must_match_key_payload_version_and_name() {
        let good = key(TEST_KEY);
        assert_eq!(verify(&good, PAYLOAD, SIGNATURE, "9.8.7", ASSET), Ok(()));
        // CRLF line endings (a .minisig edited on Windows) still verify.
        let crlf = SIGNATURE.replace('\n', "\r\n");
        assert_eq!(verify(&good, PAYLOAD, &crlf, "9.8.7", ASSET), Ok(()));
        assert!(verify(&good, b"tampered payload", SIGNATURE, "9.8.7", ASSET).is_err());
        assert!(verify(&key(OTHER_KEY), PAYLOAD, SIGNATURE, "9.8.7", ASSET).is_err());
        // A genuine signature replayed as another version or file is refused.
        assert!(verify(&good, PAYLOAD, SIGNATURE, "9.9.0", ASSET).is_err());
        assert!(verify(&good, PAYLOAD, SIGNATURE, "9.8.7", "Cadence.zip").is_err());
        assert!(verify(&good, PAYLOAD, "not a signature", "9.8.7", ASSET).is_err());
    }

    #[test]
    fn trusted_comment_format_matches_the_release_pipeline() {
        // .github/workflows/ci.yml signs with exactly this text.
        assert_eq!(
            trusted_comment("1.2.0", "Cadence-1.2.0-windows-x64-setup.exe"),
            "cadence-update 1.2.0 Cadence-1.2.0-windows-x64-setup.exe"
        );
    }

    #[test]
    fn placeholder_key_disables_verified_installs() {
        // The committed key is either a real minisign key or the placeholder;
        // either way it must not panic, and a placeholder must not parse.
        if UPDATE_KEY.contains("placeholder") {
            assert!(update_key().is_none());
        } else {
            assert!(
                update_key().is_some(),
                "packaging/update-key.pub is malformed"
            );
        }
    }

    #[test]
    fn automatic_checks_are_throttled() {
        let mut prefs = UpdatePrefs::default();
        assert!(prefs.check_due(1_000_000));
        prefs.last_check = 1_000_000;
        assert!(!prefs.check_due(1_000_000 + 60));
        assert!(prefs.check_due(1_000_000 + CHECK_INTERVAL_SECS));
        // A clock that jumped backwards doesn't block checks forever.
        assert!(prefs.check_due(10));
        prefs.auto_check = false;
        assert!(!prefs.check_due(1_000_000 + CHECK_INTERVAL_SECS));
    }

    #[test]
    fn checks_at_every_launch_then_daily_with_an_hourly_offline_retry() {
        let day = CHECK_INTERVAL_SECS;
        let now = 10_000_000;
        let mut prefs = UpdatePrefs {
            last_check: now - 60,
            ..UpdatePrefs::default()
        };
        assert_eq!(day, 24 * 60 * 60);
        // Launch: nothing tried this session yet, so check right away even
        // though the last check was a minute ago.
        assert_eq!(prefs.next_check_in(now, None), Some(0));
        // That check succeeded: the next one is due a day later, and an
        // app left open counts down to it.
        prefs.last_check = now;
        assert_eq!(prefs.next_check_in(now, Some(now)), Some(day));
        assert_eq!(prefs.next_check_in(now + day - 5, Some(now)), Some(5));
        assert_eq!(prefs.next_check_in(now + day, Some(now)), Some(0));
        // Offline: the attempt failed (last_check unchanged), so retry after
        // an hour rather than every frame.
        let tried = now + day;
        assert_eq!(prefs.next_check_in(tried, Some(tried)), Some(RETRY_SECS));
        assert_eq!(
            prefs.next_check_in(tried + RETRY_SECS, Some(tried)),
            Some(0)
        );
        // A clock that jumped backwards still checks.
        assert_eq!(
            prefs.next_check_in(now - day, Some(now - day - RETRY_SECS)),
            Some(0)
        );
        // Turned off: never.
        prefs.auto_check = false;
        assert_eq!(prefs.next_check_in(now, None), None);
    }

    #[test]
    fn prefs_round_trip_and_tolerate_unknown_fields() {
        let prefs = UpdatePrefs {
            auto_check: false,
            last_check: 42,
            skipped: Some("1.4.0".into()),
        };
        let json = serde_json::to_string(&prefs).unwrap();
        assert_eq!(serde_json::from_str::<UpdatePrefs>(&json).unwrap(), prefs);
        let partial: UpdatePrefs = serde_json::from_str(r#"{"future": 1}"#).unwrap();
        assert_eq!(partial, UpdatePrefs::default());
    }

    #[test]
    fn urls_follow_the_release_layout() {
        let base = RELEASES_BASE;
        assert_eq!(
            manifest_url(base),
            "https://github.com/Toom68/cadence/releases/latest/download/latest.json"
        );
        assert_eq!(
            asset_url(base, "1.2.0", "Cadence-1.2.0-macos-universal.zip"),
            "https://github.com/Toom68/cadence/releases/download/v1.2.0/Cadence-1.2.0-macos-universal.zip"
        );
    }

    /// The macOS swap script, run for real: the new bundle replaces the old
    /// one, and a missing new bundle leaves the old one in place.
    #[cfg(unix)]
    #[test]
    fn swap_script_replaces_the_bundle_or_rolls_back() {
        let run = |app: &Path, new: &Path, stage: &Path| {
            // A PID that is not running, so the script doesn't wait.
            let status = Command::new("/bin/sh")
                .arg("-c")
                .arg(MAC_SWAP_SCRIPT)
                .args(["sh", "2147483646"])
                .args([app, new, stage])
                .arg("0")
                .status()
                .unwrap();
            assert!(status.success());
        };
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Cadence.app");
        let stage = dir.path().join(".cadence-update");
        let new = stage.join("Cadence.app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("v"), "old").unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("v"), "new").unwrap();
        fs::write(stage.join("leftover.txt"), "x").unwrap();
        run(&app, &new, &stage);
        assert_eq!(fs::read_to_string(app.join("v")).unwrap(), "new");
        // The old bundle stays as a rollback copy; nothing else is left.
        let backup = stage.join(MAC_BACKUP_NAME);
        assert_eq!(mac_backup_path(&app).unwrap(), backup);
        assert_eq!(fs::read_to_string(backup.join("v")).unwrap(), "old");
        let left: Vec<_> = fs::read_dir(&stage)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, vec![std::ffi::OsString::from(MAC_BACKUP_NAME)]);

        // A failed move rolls back; the previous backup is replaced by
        // the bundle that was running, then moved back into place.
        run(&app, &stage.join("missing.app"), &stage);
        assert_eq!(
            fs::read_to_string(app.join("v")).unwrap(),
            "new",
            "rolled back"
        );
        assert!(!backup.exists());

        // A second successful update replaces the older backup.
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("v"), "newer").unwrap();
        run(&app, &new, &stage);
        assert_eq!(fs::read_to_string(app.join("v")).unwrap(), "newer");
        assert_eq!(fs::read_to_string(backup.join("v")).unwrap(), "new");
    }

    #[test]
    fn portable_windows_copies_and_stray_binaries_update_manually() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Cadence.exe");
        fs::write(&exe, b"").unwrap();
        if cfg!(windows) {
            assert!(!install_mode_for(&exe).is_automatic());
            fs::write(dir.path().join("unins000.exe"), b"").unwrap();
            assert_eq!(install_mode_for(&exe), InstallMode::WindowsSetup);
        } else if cfg!(target_os = "macos") {
            assert!(!install_mode_for(&exe).is_automatic());
            let bin = dir.path().join("Cadence.app/Contents/MacOS");
            fs::create_dir_all(&bin).unwrap();
            let exe = bin.join("cadence");
            fs::write(&exe, b"").unwrap();
            assert_eq!(
                install_mode_for(&exe),
                InstallMode::MacBundle(dir.path().join("Cadence.app"))
            );
        }
    }
}
