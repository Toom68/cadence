//! Hardware MIDI controller input.
//!
//! The GUI plays and records incoming notes exactly like the computer
//! keyboard. Everything that decides *what* a message means is a pure
//! function or small state machine here ([`parse_message`], [`NoteTracker`],
//! [`plan_connections`], the preference format), so it is tested without
//! hardware. Device I/O lives behind [`MidiInputHandle`]: one background
//! thread owns every `midir` connection, rescans the ports every few
//! seconds (faster while Preferences is open, and at once when the port
//! count changes) to follow hot-plugging, and
//! pushes parsed messages into a bounded channel the GUI drains each frame.
//! Headless sessions and tests never create a handle, so they never open
//! a MIDI client.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};

/// Controller number of the sustain (damper) pedal.
pub const CC_SUSTAIN: u8 = 64;

/// Messages kept between two GUI frames before new ones are dropped. A fast
/// player produces well under a hundred per second; this only overflows
/// when the UI is frozen for many seconds.
const CHANNEL_CAPACITY: usize = 4096;

/// Port rescan interval while Preferences (the device picker) is open.
const SCAN_FAST: Duration = Duration::from_millis(1000);
/// Port rescan interval otherwise — enough to notice an unplugged or
/// re-plugged controller within a few seconds.
const SCAN_SLOW: Duration = Duration::from_millis(3000);

/// Between full rescans the thread checks just the port *count* this often
/// and rescans at once when it changed. A controller unplugged and plugged
/// back into the same socket keeps its port id on some systems (CoreMIDI's
/// unique id, WinMM's interface path), so a full rescan that missed the gap
/// would see an unchanged port and keep a connection the backend has
/// already dropped; noticing the dip in the count makes the rescan close it
/// and reopen the port when it returns. An absence shorter than this
/// interval (faster than USB re-enumeration) can still go unseen.
const PRESENCE_POLL: Duration = Duration::from_millis(250);

/// Client name Cadence registers with the system MIDI service.
const CLIENT_NAME: &str = "Cadence";

// ----- messages -------------------------------------------------------------

/// The channel-voice messages Cadence acts on. Everything else (pitch bend,
/// other controllers, aftertouch, clock, sysex) is ignored by
/// [`parse_message`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiMessage {
    /// Key pressed, `velocity` 1–127.
    NoteOn {
        channel: u8,
        pitch: u8,
        velocity: u8,
    },
    /// Key released (a Note On with velocity 0 is a Note Off too).
    NoteOff { channel: u8, pitch: u8 },
    /// Sustain pedal (CC 64): down at values ≥ 64.
    Sustain { channel: u8, down: bool },
}

/// Parses one complete MIDI message as delivered by `midir`. Returns `None`
/// for anything Cadence ignores or for malformed bytes.
pub fn parse_message(bytes: &[u8]) -> Option<MidiMessage> {
    let (&status, data) = bytes.split_first()?;
    if status & 0x80 == 0 || status >= 0xF0 {
        return None;
    }
    let channel = status & 0x0F;
    let byte = |i: usize| data.get(i).copied().filter(|b| b & 0x80 == 0);
    match status & 0xF0 {
        0x80 => Some(MidiMessage::NoteOff {
            channel,
            pitch: byte(0)?,
        }),
        0x90 => {
            let (pitch, velocity) = (byte(0)?, byte(1)?);
            Some(if velocity == 0 {
                MidiMessage::NoteOff { channel, pitch }
            } else {
                MidiMessage::NoteOn {
                    channel,
                    pitch,
                    velocity,
                }
            })
        }
        0xB0 if byte(0)? == CC_SUSTAIN => Some(MidiMessage::Sustain {
            channel,
            down: byte(1)? >= 64,
        }),
        _ => None,
    }
}

/// MIDI velocity (1–127) as the 0–1 note velocity Cadence stores.
pub fn velocity_unit(velocity: u8) -> f32 {
    f32::from(velocity.min(127)) / 127.0
}

// ----- held notes and the sustain pedal ------------------------------------

/// What the GUI should do with the selected track in response to MIDI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NoteAction {
    /// Sound (and, when armed, record) `pitch` at `velocity` (0–1).
    Start { pitch: u8, velocity: f32 },
    /// End the note `pitch` started.
    End { pitch: u8 },
}

/// Turns raw note/pedal messages into [`NoteAction`]s: a held pitch that is
/// struck again ends before it restarts (so every `Start` is paired with one
/// `End`), and while the sustain pedal is down releases are deferred until
/// the pedal comes up. Notes are keyed by pitch alone, so the same pitch
/// from two channels or two devices behaves like one key.
#[derive(Debug, Default)]
pub struct NoteTracker {
    /// Pitches whose `Start` has not been matched by an `End` yet.
    sounding: BTreeSet<u8>,
    /// Of those, the ones whose key is already up (held only by the pedal).
    sustained: BTreeSet<u8>,
    pedal: bool,
}

impl NoteTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the sustain pedal is down.
    pub fn pedal_down(&self) -> bool {
        self.pedal
    }

    /// Pitches currently sounding (held by a key or the pedal).
    pub fn sounding(&self) -> impl Iterator<Item = u8> + '_ {
        self.sounding.iter().copied()
    }

    /// Feeds one message; returns the actions it causes, in order.
    pub fn handle(&mut self, message: MidiMessage) -> Vec<NoteAction> {
        match message {
            MidiMessage::NoteOn {
                pitch, velocity, ..
            } => {
                let mut out = Vec::with_capacity(2);
                if self.sounding.contains(&pitch) {
                    out.push(NoteAction::End { pitch });
                }
                self.sustained.remove(&pitch);
                self.sounding.insert(pitch);
                out.push(NoteAction::Start {
                    pitch,
                    velocity: velocity_unit(velocity),
                });
                out
            }
            MidiMessage::NoteOff { pitch, .. } => {
                if !self.sounding.contains(&pitch) {
                    return Vec::new();
                }
                if self.pedal {
                    self.sustained.insert(pitch);
                    return Vec::new();
                }
                self.sounding.remove(&pitch);
                vec![NoteAction::End { pitch }]
            }
            MidiMessage::Sustain { down, .. } => {
                self.pedal = down;
                if down {
                    return Vec::new();
                }
                let released = std::mem::take(&mut self.sustained);
                for p in &released {
                    self.sounding.remove(p);
                }
                released
                    .into_iter()
                    .map(|pitch| NoteAction::End { pitch })
                    .collect()
            }
        }
    }

    /// Ends everything still sounding and lifts the pedal — used when a
    /// device disconnects or the input preference changes, so no recorded
    /// note waits forever for a release that cannot arrive.
    pub fn release_all(&mut self) -> Vec<NoteAction> {
        self.pedal = false;
        self.sustained.clear();
        std::mem::take(&mut self.sounding)
            .into_iter()
            .map(|pitch| NoteAction::End { pitch })
            .collect()
    }
}

// ----- preference -----------------------------------------------------------

/// Which MIDI inputs Cadence listens to.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum MidiInputPref {
    /// MIDI input off.
    None,
    /// Every input port, including ones plugged in later (the default).
    #[default]
    All,
    /// One port, by its display name; reconnects when it comes back. With
    /// several identical devices the name carries a `#2`-style number from
    /// enumeration order, which only identifies the same unit within a
    /// session (see [`plan_connections`]).
    Device(String),
}

impl MidiInputPref {
    /// Short label for pickers and status messages.
    pub fn label(&self) -> String {
        match self {
            Self::None => "None".to_owned(),
            Self::All => "All inputs".to_owned(),
            Self::Device(name) => name.clone(),
        }
    }

    /// File contents for [`save_pref_in`]: `none`, `all` or `port:<name>`.
    pub fn to_file(&self) -> String {
        match self {
            Self::None => "none".to_owned(),
            Self::All => "all".to_owned(),
            Self::Device(name) => format!("port:{name}"),
        }
    }

    /// Parses [`Self::to_file`] output. Unknown or damaged contents fall
    /// back to the default (all inputs), like a missing file.
    pub fn from_file(text: &str) -> Self {
        let line = text.lines().next().unwrap_or("").trim();
        match line {
            "none" => Self::None,
            "all" => Self::All,
            _ => match line.strip_prefix("port:").map(str::trim) {
                Some(name) if !name.is_empty() => Self::Device(name.to_owned()),
                _ => Self::All,
            },
        }
    }
}

/// File remembering the MIDI input pick, beside `audio-device.txt`.
pub fn pref_path_in(dir: &Path) -> PathBuf {
    dir.join("midi-input.txt")
}

/// The saved MIDI input pick in `dir` (the default when none is saved).
pub fn load_pref_in(dir: &Path) -> MidiInputPref {
    std::fs::read_to_string(pref_path_in(dir))
        .map(|text| MidiInputPref::from_file(&text))
        .unwrap_or_default()
}

/// Persists the MIDI input pick in `dir`. Best effort — a failed write only
/// means the choice doesn't survive a restart. The default is stored as
/// "no file".
pub fn save_pref_in(dir: &Path, pref: &MidiInputPref) {
    let path = pref_path_in(dir);
    if *pref == MidiInputPref::default() {
        let _ = std::fs::remove_file(path);
    } else if std::fs::create_dir_all(dir).is_ok() {
        let _ = std::fs::write(path, pref.to_file());
    }
}

/// [`load_pref_in`] for the user data dir.
pub fn load_pref() -> MidiInputPref {
    load_pref_in(&crate::persistence::data_dir())
}

/// [`save_pref_in`] for the user data dir.
pub fn save_pref(pref: &MidiInputPref) {
    save_pref_in(&crate::persistence::data_dir(), pref);
}

// ----- port bookkeeping -----------------------------------------------------

/// Strips the ` client:port` address ALSA appends to port names
/// (`"Keystation:Keystation MIDI 1 20:0"`): the numbers change when a
/// device is re-plugged, and the saved preference must still match.
pub fn strip_alsa_address(name: &str) -> &str {
    let Some((head, tail)) = name.rsplit_once(' ') else {
        return name;
    };
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match tail.split_once(':') {
        Some((c, p)) if numeric(c) && numeric(p) && !head.is_empty() => head,
        _ => name,
    }
}

/// Makes port display names unique: a second identical controller becomes
/// `"Name #2"`, a third `"Name #3"`, in enumeration order — so a number
/// can move to another unit when they are re-plugged in a different order.
pub fn unique_names(names: &[String]) -> Vec<String> {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    names
        .iter()
        .map(|name| {
            let n = seen.entry(name.as_str()).or_insert(0);
            *n += 1;
            if *n == 1 {
                name.clone()
            } else {
                format!("{name} #{n}")
            }
        })
        .collect()
}

/// One input port as the scanner sees it: unique display name plus the
/// backend's port id (which changes when a device is re-plugged on some
/// systems, forcing a fresh connection).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortEntry {
    pub name: String,
    pub id: String,
}

/// Why a rescan closes a live connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    /// The port is gone (unplugged).
    Unplugged,
    /// The port came back under a new id (re-plugged between two scans);
    /// it is reopened in the same rescan.
    Replugged,
    /// The port is still there but the preference no longer selects it.
    Deselected,
}

/// Connection changes a rescan should make.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionPlan {
    /// Open connections to close (names), with the reason.
    pub close: Vec<(String, CloseReason)>,
    /// Ports to open (names), in enumeration order.
    pub open: Vec<String>,
    /// Open connections to keep whose port is now listed under another
    /// name, `(old, new)` — an identical device came or went, so the
    /// `#2` numbering shifted.
    pub rename: Vec<(String, String)>,
}

/// Index of the only entry matching `pred`, if exactly one does.
fn only_index(list: &[PortEntry], pred: impl Fn(&PortEntry) -> bool) -> Option<usize> {
    let mut hits = list.iter().enumerate().filter(|(_, e)| pred(e));
    let (first, _) = hits.next()?;
    hits.next().is_none().then_some(first)
}

/// Where the connected port `c` is in `available` now: the same name and
/// id, or — when its id identifies one port in both lists — the same id
/// under a renumbered name.
fn current_index(c: &PortEntry, available: &[PortEntry], connected: &[PortEntry]) -> Option<usize> {
    available.iter().position(|a| a == c).or_else(|| {
        only_index(connected, |e| e.id == c.id)?;
        only_index(available, |a| a.id == c.id)
    })
}

/// Decides which ports to close and open so the live connections match
/// `pref` given the ports `available` now. A connected port that vanished,
/// or whose id changed (re-plugged), is closed; with [`MidiInputPref::All`]
/// every available port is opened, with a device pick only that one.
///
/// `pinned` is the port id a device pick last connected to. Identical
/// devices are told apart only by their `#2` numbering, which follows
/// enumeration order and so can shift when one is re-plugged; while the
/// pinned unit is listed it stays the pick whatever it is called now, and
/// only otherwise is the exact saved name matched. Ids are not saved, so
/// after a restart (or on a backend whose ids change on re-plug) the
/// saved `"Name #2"` simply means the second such device enumerated.
pub fn plan_connections(
    pref: &MidiInputPref,
    available: &[PortEntry],
    connected: &[PortEntry],
    pinned: Option<&str>,
) -> ConnectionPlan {
    let target = match pref {
        MidiInputPref::Device(pick) => pinned
            .and_then(|id| only_index(available, |a| a.id == id))
            .or_else(|| available.iter().position(|a| &a.name == pick)),
        _ => None,
    };
    let wanted = |i: usize| match pref {
        MidiInputPref::None => false,
        MidiInputPref::All => true,
        MidiInputPref::Device(_) => target == Some(i),
    };
    let mut plan = ConnectionPlan::default();
    let mut kept = BTreeSet::new();
    let mut gone = Vec::new();
    for c in connected {
        match current_index(c, available, connected) {
            Some(i) if wanted(i) && kept.insert(i) => {
                if available[i].name != c.name {
                    plan.rename
                        .push((c.name.clone(), available[i].name.clone()));
                }
            }
            Some(_) => plan.close.push((c.name.clone(), CloseReason::Deselected)),
            None => gone.push(c),
        }
    }
    for (i, a) in available.iter().enumerate() {
        if wanted(i) && !kept.contains(&i) {
            plan.open.push(a.name.clone());
        }
    }
    for c in gone {
        let reason = if plan.open.contains(&c.name) {
            CloseReason::Replugged
        } else {
            CloseReason::Unplugged
        };
        plan.close.push((c.name.clone(), reason));
    }
    plan
}

// ----- device I/O ------------------------------------------------------------

/// What the input thread reports to the GUI.
#[derive(Clone, Debug, PartialEq)]
pub enum MidiInputEvent {
    /// A message from the connection numbered `conn` (unique for the life
    /// of the input thread; a reopened port gets a new number).
    Message { conn: u64, message: MidiMessage },
    /// A port was opened. `initial` marks the connections made right at
    /// startup, which the GUI doesn't announce.
    Connected { name: String, initial: bool },
    /// An open port was closed. Unless the preference deselected it, any
    /// note it was holding can no longer be released by the device.
    ///
    /// The connection's callback may still deliver a message or two after
    /// this report; the GUI ignores messages from a closed `conn`.
    Disconnected {
        name: String,
        conn: u64,
        reason: CloseReason,
    },
    /// A port could not be opened (reported once until it succeeds).
    Failed { name: String, error: String },
    /// The system MIDI service could not be reached at all.
    Unavailable(String),
}

enum Command {
    SetPref(MidiInputPref),
    FastScan(bool),
}

/// Port lists the input thread publishes after each scan.
#[derive(Clone, Debug, Default)]
struct Shared {
    ports: Vec<String>,
    connected: Vec<String>,
    scanned: bool,
    /// Why MIDI input cannot work at all this session, if it cannot.
    unavailable: Option<String>,
}

/// Owns the background MIDI input thread. Dropping it stops the thread and
/// closes every connection.
pub struct MidiInputHandle {
    events: Receiver<MidiInputEvent>,
    commands: Sender<Command>,
    shared: Arc<Mutex<Shared>>,
    fast_scan: bool,
    _thread: Option<JoinHandle<()>>,
}

impl MidiInputHandle {
    /// Starts the input thread listening per `pref`. `wake` is called (from
    /// the MIDI thread) whenever something arrives, to request a repaint.
    pub fn spawn(pref: MidiInputPref, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, events) = bounded(CHANNEL_CAPACITY);
        let (commands, rx) = crossbeam_channel::unbounded();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let thread_shared = Arc::clone(&shared);
        let (thread_tx, thread_wake) = (tx.clone(), Arc::clone(&wake));
        let thread = match std::thread::Builder::new()
            .name("cadence-midi-in".to_owned())
            .spawn(move || run(pref, rx, thread_tx, thread_wake, thread_shared))
        {
            Ok(thread) => Some(thread),
            Err(err) => {
                // Otherwise Preferences would say "Looking for MIDI
                // devices…" forever.
                mark_unavailable(
                    &shared,
                    &tx,
                    &*wake,
                    format!("could not start the MIDI input thread ({err})"),
                );
                None
            }
        };
        Self {
            events,
            commands,
            shared,
            fast_scan: false,
            _thread: thread,
        }
    }

    /// Everything that arrived since the last call, oldest first.
    pub fn drain(&self) -> Vec<MidiInputEvent> {
        self.events.try_iter().collect()
    }

    /// Switches which ports are open; takes effect within a moment.
    pub fn set_pref(&self, pref: MidiInputPref) {
        let _ = self.commands.send(Command::SetPref(pref));
    }

    /// Rescans faster while a device picker is showing.
    pub fn set_fast_scan(&mut self, fast: bool) {
        if self.fast_scan != fast {
            self.fast_scan = fast;
            let _ = self.commands.send(Command::FastScan(fast));
        }
    }

    /// Input port names from the latest scan.
    pub fn ports(&self) -> Vec<String> {
        self.lock().ports.clone()
    }

    /// Names of the ports currently open.
    pub fn connected(&self) -> Vec<String> {
        self.lock().connected.clone()
    }

    /// Whether the first port scan has finished.
    pub fn scanned(&self) -> bool {
        self.lock().scanned
    }

    /// Why MIDI input cannot work this session (no MIDI service, or the
    /// input thread could not start); `None` while it works.
    pub fn unavailable(&self) -> Option<String> {
        self.lock().unavailable.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Reports one event to the GUI and wakes it. Notices use a blocking-free
/// send too: if the channel is full the GUI is frozen and will rescan state
/// from [`Shared`] anyway.
fn emit(tx: &Sender<MidiInputEvent>, wake: &(dyn Fn() + Send + Sync), event: MidiInputEvent) {
    match tx.try_send(event) {
        Ok(()) => wake(),
        Err(TrySendError::Full(_)) => wake(),
        Err(TrySendError::Disconnected(_)) => {}
    }
}

/// Records that MIDI input cannot work this session and tells the GUI.
fn mark_unavailable(
    shared: &Mutex<Shared>,
    tx: &Sender<MidiInputEvent>,
    wake: &(dyn Fn() + Send + Sync),
    reason: String,
) {
    let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
    s.scanned = true;
    s.unavailable = Some(reason.clone());
    drop(s);
    emit(tx, wake, MidiInputEvent::Unavailable(reason));
}

/// An open input port.
struct Live {
    entry: PortEntry,
    conn: u64,
    _conn: midir::MidiInputConnection<()>,
}

/// The input thread: scan, reconcile, wait for a command or the next scan.
fn run(
    mut pref: MidiInputPref,
    commands: Receiver<Command>,
    tx: Sender<MidiInputEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    shared: Arc<Mutex<Shared>>,
) {
    let scanner = match midir::MidiInput::new(CLIENT_NAME) {
        Ok(scanner) => scanner,
        Err(err) => {
            mark_unavailable(&shared, &tx, &*wake, err.to_string());
            return;
        }
    };
    let mut live: Vec<Live> = Vec::new();
    // Port id of the unit a device pick follows (see `plan_connections`).
    let mut pinned: Option<String> = None;
    let mut next_conn: u64 = 1;
    let mut failed: BTreeSet<String> = BTreeSet::new();
    let mut fast = false;
    let mut initial = true;
    let mut next_scan = Instant::now();
    let mut scanned_count: Option<usize> = None;
    loop {
        pump_system_events();
        let count = scanner.port_count();
        if Instant::now() >= next_scan || rescan_for_count(scanned_count, count) {
            // The count as checked, not the listed ports: backends may skip
            // a port they fail to describe, which must not look like a
            // change on every check.
            scanned_count = Some(count);
            scan(
                &scanner,
                &pref,
                &mut live,
                &mut pinned,
                &mut next_conn,
                &mut failed,
                initial,
                &tx,
                &wake,
                &shared,
            );
            initial = false;
            next_scan = Instant::now() + if fast { SCAN_FAST } else { SCAN_SLOW };
        }
        let wait = next_scan
            .saturating_duration_since(Instant::now())
            .min(PRESENCE_POLL);
        match commands.recv_timeout(wait) {
            Ok(Command::SetPref(p)) => {
                pref = p;
                pinned = None;
                failed.clear();
                next_scan = Instant::now();
            }
            Ok(Command::FastScan(f)) => {
                fast = f;
                if f {
                    next_scan = Instant::now();
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Whether the port count seen by the quick presence check differs from the
/// one checked right before the last full scan (see [`PRESENCE_POLL`]).
pub fn rescan_for_count(scanned: Option<usize>, count: usize) -> bool {
    scanned.is_some_and(|n| n != count)
}

/// Lists the ports, applies [`plan_connections`] and publishes the result.
#[allow(clippy::too_many_arguments)]
fn scan(
    scanner: &midir::MidiInput,
    pref: &MidiInputPref,
    live: &mut Vec<Live>,
    pinned: &mut Option<String>,
    next_conn: &mut u64,
    failed: &mut BTreeSet<String>,
    initial: bool,
    tx: &Sender<MidiInputEvent>,
    wake: &Arc<dyn Fn() + Send + Sync>,
    shared: &Arc<Mutex<Shared>>,
) {
    let ports = scanner.ports();
    let raw: Vec<String> = ports
        .iter()
        .map(|p| {
            let name = scanner
                .port_name(p)
                .unwrap_or_else(|_| "MIDI input".to_owned());
            display_name(&name).to_owned()
        })
        .collect();
    let names = unique_names(&raw);
    let available: Vec<PortEntry> = names
        .iter()
        .zip(&ports)
        .map(|(name, port)| PortEntry {
            name: name.clone(),
            id: port.id(),
        })
        .collect();
    let connected: Vec<PortEntry> = live.iter().map(|l| l.entry.clone()).collect();
    let plan = plan_connections(pref, &available, &connected, pinned.as_deref());
    for (name, reason) in &plan.close {
        let Some(i) = live.iter().position(|l| &l.entry.name == name) else {
            continue;
        };
        let conn = live.remove(i).conn;
        // Every close is reported, a re-plug included: the GUI must end the
        // notes the old connection was holding.
        emit(
            tx,
            &**wake,
            MidiInputEvent::Disconnected {
                name: name.clone(),
                conn,
                reason: *reason,
            },
        );
    }
    // Closes went first, so every old name is still that of one live entry.
    let renames: Vec<(usize, &String)> = plan
        .rename
        .iter()
        .filter_map(|(old, new)| Some((live.iter().position(|l| &l.entry.name == old)?, new)))
        .collect();
    for (i, new) in renames {
        live[i].entry.name = new.clone();
    }
    for name in &plan.open {
        let Some(i) = available.iter().position(|a| &a.name == name) else {
            continue;
        };
        let id = *next_conn;
        match open_port(&ports[i], name, id, tx, wake) {
            Ok(conn) => {
                *next_conn += 1;
                if matches!(pref, MidiInputPref::Device(_)) {
                    *pinned = Some(available[i].id.clone());
                }
                failed.remove(name);
                live.push(Live {
                    entry: available[i].clone(),
                    conn: id,
                    _conn: conn,
                });
                emit(
                    tx,
                    &**wake,
                    MidiInputEvent::Connected {
                        name: name.clone(),
                        initial,
                    },
                );
            }
            Err(error) => {
                if failed.insert(name.clone()) {
                    emit(
                        tx,
                        &**wake,
                        MidiInputEvent::Failed {
                            name: name.clone(),
                            error,
                        },
                    );
                }
            }
        }
    }
    let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
    let changed = s.ports != names || !s.scanned;
    s.ports = names;
    s.connected = live.iter().map(|l| l.entry.name.clone()).collect();
    s.scanned = true;
    drop(s);
    if changed {
        wake();
    }
}

/// Opens one port with its own client; the callback parses on the MIDI
/// thread and forwards only the messages Cadence uses.
fn open_port(
    port: &midir::MidiInputPort,
    name: &str,
    conn: u64,
    tx: &Sender<MidiInputEvent>,
    wake: &Arc<dyn Fn() + Send + Sync>,
) -> Result<midir::MidiInputConnection<()>, String> {
    let mut input = midir::MidiInput::new(CLIENT_NAME).map_err(|e| e.to_string())?;
    // Clock, active sensing and sysex would only wake the UI for nothing.
    input.ignore(midir::Ignore::All);
    let tx = tx.clone();
    let wake = Arc::clone(wake);
    input
        .connect(
            port,
            &format!("{CLIENT_NAME} input ({name})"),
            move |_stamp, bytes, _| {
                if let Some(message) = parse_message(bytes) {
                    emit(&tx, &*wake, MidiInputEvent::Message { conn, message });
                }
            },
            (),
        )
        .map_err(|e| e.to_string())
}

/// Port name as shown and saved: ALSA's changing address suffix removed.
fn display_name(raw: &str) -> &str {
    if cfg!(target_os = "linux") {
        strip_alsa_address(raw)
    } else {
        raw
    }
}

/// CoreMIDI delivers device add/remove notifications to the run loop of
/// the thread that created the first MIDI client — this thread, which has
/// no run loop of its own. Without pumping it, the port list stays frozen
/// at its startup state and hot-plugging is never seen.
#[cfg(target_os = "macos")]
fn pump_system_events() {
    use std::ffi::c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after: u8) -> i32;
    }
    // SAFETY: `kCFRunLoopDefaultMode` is an immutable CoreFoundation
    // constant, and running the current thread's run loop for zero seconds
    // only processes already-pending sources on this thread.
    unsafe {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.0, 0);
    }
}

/// WinMM and ALSA re-enumerate devices on every port listing.
#[cfg(not(target_os = "macos"))]
fn pump_system_events() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(pitch: u8, velocity: u8) -> MidiMessage {
        MidiMessage::NoteOn {
            channel: 0,
            pitch,
            velocity,
        }
    }
    fn off(pitch: u8) -> MidiMessage {
        MidiMessage::NoteOff { channel: 0, pitch }
    }
    fn pedal(down: bool) -> MidiMessage {
        MidiMessage::Sustain { channel: 0, down }
    }
    fn start(pitch: u8, velocity: u8) -> NoteAction {
        NoteAction::Start {
            pitch,
            velocity: velocity_unit(velocity),
        }
    }
    fn end(pitch: u8) -> NoteAction {
        NoteAction::End { pitch }
    }

    #[test]
    fn parses_notes_and_the_sustain_pedal() {
        assert_eq!(parse_message(&[0x90, 60, 100]), Some(on(60, 100)));
        assert_eq!(
            parse_message(&[0x93, 61, 1]),
            Some(MidiMessage::NoteOn {
                channel: 3,
                pitch: 61,
                velocity: 1
            })
        );
        assert_eq!(parse_message(&[0x80, 60, 64]), Some(off(60)));
        assert_eq!(parse_message(&[0x90, 60, 0]), Some(off(60)), "vel 0");
        assert_eq!(parse_message(&[0xB0, 64, 127]), Some(pedal(true)));
        assert_eq!(parse_message(&[0xB0, 64, 64]), Some(pedal(true)));
        assert_eq!(parse_message(&[0xB0, 64, 63]), Some(pedal(false)));
        assert_eq!(parse_message(&[0xB0, 64, 0]), Some(pedal(false)));
    }

    #[test]
    fn ignores_everything_else_and_malformed_bytes() {
        for bytes in [
            &[][..],
            &[0x90],
            &[0x90, 60],
            &[0x90, 0x80, 10],
            &[0x90, 60, 0xFF],
            &[60, 100],
            &[0xB0, 1, 20],   // mod wheel
            &[0xB0, 64],      // truncated pedal
            &[0xE0, 0, 64],   // pitch bend
            &[0xA0, 60, 10],  // poly aftertouch
            &[0xC0, 5],       // program change
            &[0xD0, 30],      // channel pressure
            &[0xF8],          // clock
            &[0xFE],          // active sensing
            &[0xF0, 1, 0xF7], // sysex
        ] {
            assert_eq!(parse_message(bytes), None, "{bytes:?}");
        }
    }

    #[test]
    fn velocity_maps_to_unit_range() {
        assert_eq!(velocity_unit(127), 1.0);
        assert_eq!(velocity_unit(0), 0.0);
        assert!((velocity_unit(64) - 0.504).abs() < 0.001);
        assert_eq!(velocity_unit(200), 1.0);
    }

    #[test]
    fn notes_start_and_end_with_their_keys() {
        let mut t = NoteTracker::new();
        assert_eq!(t.handle(on(60, 90)), vec![start(60, 90)]);
        assert_eq!(t.handle(on(64, 40)), vec![start(64, 40)]);
        assert_eq!(t.handle(off(60)), vec![end(60)]);
        assert_eq!(t.handle(off(60)), vec![], "double release is ignored");
        assert_eq!(t.handle(off(70)), vec![], "never-started key");
        assert_eq!(t.sounding().collect::<Vec<_>>(), vec![64]);
    }

    #[test]
    fn restriking_a_held_pitch_ends_it_first() {
        let mut t = NoteTracker::new();
        t.handle(on(60, 90));
        assert_eq!(t.handle(on(60, 50)), vec![end(60), start(60, 50)]);
        assert_eq!(t.handle(off(60)), vec![end(60)]);
    }

    #[test]
    fn sustain_defers_releases_until_the_pedal_lifts() {
        let mut t = NoteTracker::new();
        t.handle(on(60, 90));
        assert_eq!(t.handle(pedal(true)), vec![]);
        assert!(t.pedal_down());
        assert_eq!(t.handle(off(60)), vec![], "held by the pedal");
        t.handle(on(64, 80));
        assert_eq!(t.handle(off(64)), vec![]);
        t.handle(on(67, 70)); // still held by its key at pedal-up
        assert_eq!(t.handle(pedal(false)), vec![end(60), end(64)]);
        assert_eq!(t.sounding().collect::<Vec<_>>(), vec![67]);
        assert_eq!(t.handle(off(67)), vec![end(67)], "no pedal any more");
    }

    #[test]
    fn restriking_a_sustained_pitch_retriggers_it() {
        let mut t = NoteTracker::new();
        t.handle(pedal(true));
        t.handle(on(60, 90));
        t.handle(off(60));
        assert_eq!(t.handle(on(60, 30)), vec![end(60), start(60, 30)]);
        // The re-struck key is down again, so pedal-up must not end it.
        assert_eq!(t.handle(pedal(false)), vec![]);
        assert_eq!(t.handle(off(60)), vec![end(60)]);
    }

    #[test]
    fn release_all_ends_held_and_sustained_notes() {
        let mut t = NoteTracker::new();
        t.handle(pedal(true));
        t.handle(on(60, 90));
        t.handle(off(60));
        t.handle(on(62, 90));
        assert_eq!(t.release_all(), vec![end(60), end(62)]);
        assert!(!t.pedal_down());
        assert_eq!(t.handle(off(62)), vec![]);
        assert_eq!(t.release_all(), vec![]);
    }

    #[test]
    fn pref_round_trips_through_its_file_format() {
        for pref in [
            MidiInputPref::None,
            MidiInputPref::All,
            MidiInputPref::Device("Keystation 49 MK3".to_owned()),
            MidiInputPref::Device("port: odd:name".to_owned()),
        ] {
            assert_eq!(MidiInputPref::from_file(&pref.to_file()), pref);
        }
        assert_eq!(MidiInputPref::from_file(""), MidiInputPref::All);
        assert_eq!(MidiInputPref::from_file("garbage"), MidiInputPref::All);
        assert_eq!(MidiInputPref::from_file("port:  "), MidiInputPref::All);
        assert_eq!(MidiInputPref::from_file("none\n"), MidiInputPref::None);
        assert_eq!(MidiInputPref::default(), MidiInputPref::All);
    }

    #[test]
    fn pref_saves_and_loads_in_a_dir() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("data");
        assert_eq!(load_pref_in(&sub), MidiInputPref::All, "missing file");
        let pick = MidiInputPref::Device("Launchkey Mini".to_owned());
        save_pref_in(&sub, &pick);
        assert_eq!(load_pref_in(&sub), pick);
        save_pref_in(&sub, &MidiInputPref::None);
        assert_eq!(load_pref_in(&sub), MidiInputPref::None);
        save_pref_in(&sub, &MidiInputPref::All);
        assert!(!pref_path_in(&sub).exists(), "the default is no file");
        assert_eq!(load_pref_in(&sub), MidiInputPref::All);
    }

    #[test]
    fn alsa_addresses_are_stripped() {
        assert_eq!(
            strip_alsa_address("Keystation:Keystation MIDI 1 20:0"),
            "Keystation:Keystation MIDI 1"
        );
        assert_eq!(
            strip_alsa_address("Midi Through:Port-0 14:0"),
            "Midi Through:Port-0"
        );
        assert_eq!(strip_alsa_address("IAC Driver Bus 1"), "IAC Driver Bus 1");
        assert_eq!(strip_alsa_address("Ratio 1:2x"), "Ratio 1:2x");
        assert_eq!(strip_alsa_address("12:0"), "12:0");
        assert_eq!(strip_alsa_address(""), "");
    }

    #[test]
    fn duplicate_port_names_are_numbered() {
        let names: Vec<String> = ["Pad", "Keys", "Pad", "Pad"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            unique_names(&names),
            vec!["Pad", "Keys", "Pad #2", "Pad #3"]
        );
    }

    fn entry(name: &str, id: &str) -> PortEntry {
        PortEntry {
            name: name.to_owned(),
            id: id.to_owned(),
        }
    }

    #[test]
    fn plan_follows_the_preference() {
        let avail = [entry("A", "1"), entry("B", "2")];
        let all = plan_connections(&MidiInputPref::All, &avail, &[], None);
        assert_eq!(all.open, vec!["A", "B"]);
        assert!(all.close.is_empty());

        let none = plan_connections(&MidiInputPref::None, &avail, &avail, None);
        assert_eq!(
            none.close,
            vec![
                ("A".to_owned(), CloseReason::Deselected),
                ("B".to_owned(), CloseReason::Deselected)
            ]
        );
        assert!(none.open.is_empty());

        let pick = MidiInputPref::Device("B".to_owned());
        let one = plan_connections(&pick, &avail, &[entry("A", "1")], None);
        assert_eq!(one.close, vec![("A".to_owned(), CloseReason::Deselected)]);
        assert_eq!(one.open, vec!["B"]);

        let steady = plan_connections(&MidiInputPref::All, &avail, &avail, None);
        assert_eq!(steady, ConnectionPlan::default(), "nothing to do");
    }

    #[test]
    fn an_unavailable_service_is_reported_not_left_scanning() {
        let (tx, events) = bounded(4);
        let (commands, _rx) = crossbeam_channel::unbounded();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let handle = MidiInputHandle {
            events,
            commands,
            shared: Arc::clone(&shared),
            fast_scan: false,
            _thread: None,
        };
        assert!(!handle.scanned() && handle.unavailable().is_none());
        let woken = std::sync::atomic::AtomicBool::new(false);
        let wake = || woken.store(true, std::sync::atomic::Ordering::Relaxed);
        mark_unavailable(&shared, &tx, &wake, "no thread".to_owned());
        assert!(handle.scanned(), "Preferences stops saying \"Looking…\"");
        assert_eq!(handle.unavailable().as_deref(), Some("no thread"));
        assert_eq!(
            handle.drain(),
            vec![MidiInputEvent::Unavailable("no thread".to_owned())]
        );
        assert!(woken.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn a_dip_in_the_port_count_triggers_a_rescan() {
        assert!(!rescan_for_count(None, 2), "the first scan is on schedule");
        assert!(!rescan_for_count(Some(2), 2));
        assert!(rescan_for_count(Some(2), 1), "unplugged");
        assert!(rescan_for_count(Some(1), 2), "plugged back in");
    }

    #[test]
    fn plan_handles_hot_plugging() {
        let pick = MidiInputPref::Device("Keys".to_owned());
        // Unplugged: the live connection is closed, nothing opens.
        let gone = plan_connections(&pick, &[], &[entry("Keys", "7")], None);
        assert_eq!(
            gone.close,
            vec![("Keys".to_owned(), CloseReason::Unplugged)]
        );
        assert!(gone.open.is_empty());
        // Missing at startup: nothing to do until it shows up.
        assert_eq!(
            plan_connections(&pick, &[entry("Other", "1")], &[], None),
            ConnectionPlan::default()
        );
        // Back again: reconnects.
        let back = plan_connections(&pick, &[entry("Keys", "9")], &[], None);
        assert_eq!(back.open, vec!["Keys"]);
        // Re-plugged between two scans under a new id: reopen.
        let replug = plan_connections(&pick, &[entry("Keys", "9")], &[entry("Keys", "7")], None);
        assert_eq!(
            replug.close,
            vec![("Keys".to_owned(), CloseReason::Replugged)],
            "reported, so held notes are released"
        );
        assert_eq!(replug.open, vec!["Keys"]);
        // All inputs picks up a newly plugged device and drops a gone one.
        let all = plan_connections(
            &MidiInputPref::All,
            &[entry("A", "1"), entry("C", "3")],
            &[entry("A", "1"), entry("B", "2")],
            None,
        );
        assert_eq!(all.close, vec![("B".to_owned(), CloseReason::Unplugged)]);
        assert_eq!(all.open, vec!["C"]);
    }

    #[test]
    fn identical_devices_keep_their_connections_when_renumbered() {
        // Two "X" units; the first is unplugged, so the second is now
        // listed as plain "X". Its connection is kept, just renamed.
        let plan = plan_connections(
            &MidiInputPref::All,
            &[entry("X", "B")],
            &[entry("X", "A"), entry("X #2", "B")],
            None,
        );
        assert_eq!(plan.close, vec![("X".to_owned(), CloseReason::Unplugged)]);
        assert_eq!(plan.rename, vec![("X #2".to_owned(), "X".to_owned())]);
        assert!(plan.open.is_empty());
        // Shared ids (a backend that cannot tell them apart) fall back to
        // exact names.
        let shared = plan_connections(
            &MidiInputPref::All,
            &[entry("X", "0")],
            &[entry("X", "0"), entry("X #2", "0")],
            None,
        );
        assert_eq!(
            shared.close,
            vec![("X #2".to_owned(), CloseReason::Unplugged)]
        );
        assert!(shared.rename.is_empty() && shared.open.is_empty());
    }

    #[test]
    fn a_device_pick_follows_its_unit_not_the_numbering() {
        let pick = MidiInputPref::Device("X #2".to_owned());
        // Unit B was picked as "X #2"; re-plugged, it now enumerates first.
        let swapped = [entry("X", "B"), entry("X #2", "A")];
        let back = plan_connections(&pick, &swapped, &[], Some("B"));
        assert_eq!(back.open, vec!["X"], "B, not the unit now called X #2");
        let kept = plan_connections(&pick, &swapped, &[entry("X #2", "B")], Some("B"));
        assert_eq!(kept.rename, vec![("X #2".to_owned(), "X".to_owned())]);
        assert!(kept.close.is_empty() && kept.open.is_empty());
        // With the pinned unit absent the exact saved name decides: only
        // the other unit is left, and it is not called "X #2".
        let alone = plan_connections(&pick, &[entry("X", "A")], &[], Some("B"));
        assert_eq!(alone, ConnectionPlan::default());
        // Without a pin (fresh launch) the saved name is matched as is.
        let fresh = plan_connections(&pick, &swapped, &[], None);
        assert_eq!(fresh.open, vec!["X #2"]);
        // A pin from another unit never overrides "None".
        let off = plan_connections(&MidiInputPref::None, &swapped, &[], Some("B"));
        assert_eq!(off, ConnectionPlan::default());
    }
}
