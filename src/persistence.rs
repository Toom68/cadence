//! Safe project persistence and crash recovery.
//!
//! Projects are stored as pretty-printed JSON with the `.cadence` extension.
//! Every write goes to a uniquely named temporary file in the destination
//! directory, is flushed to disk, and then atomically renamed over the target,
//! so an interrupted or failed save never corrupts an existing song.
//! Loading enforces a size cap and full model validation, so damaged or hostile
//! files are rejected with a readable error instead of being half-loaded.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::model::Project;

/// File extension (without the dot) used for Cadence songs.
pub const PROJECT_EXTENSION: &str = crate::model::PROJECT_EXTENSION;

/// Largest project file Cadence will read or write.
pub const MAX_PROJECT_BYTES: u64 = 20 * 1024 * 1024;

/// Environment variable that overrides the application data directory.
/// Intended for tests, portable installs and diagnostics.
pub const DATA_DIR_ENV: &str = "CADENCE_DATA_DIR";

const RECOVERY_FILE: &str = "autosave.cadence";
const DAMAGED_RECOVERY_FILE: &str = "autosave.damaged.cadence";
/// Plain-text sidecar naming the song file the autosave belongs to.
const RECOVERY_SOURCE_FILE: &str = "autosave.source";

/// Parses and validates a project from JSON text.
pub fn project_from_json(text: &str) -> Result<Project, String> {
    if text.len() as u64 > MAX_PROJECT_BYTES {
        return Err(too_large_message(text.len() as u64));
    }
    let project: Project = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|err| format!("This is not a valid Cadence project: {err}"))?;
    project
        .validate()
        .map_err(|err| format!("The project contains invalid data: {err}"))?;
    Ok(project)
}

/// Validates and serializes a project to pretty JSON.
pub fn project_to_json(project: &Project) -> Result<String, String> {
    project
        .validate()
        .map_err(|err| format!("Cannot save an invalid project: {err}"))?;
    let mut text = serde_json::to_string_pretty(project)
        .map_err(|err| format!("Could not serialize the project: {err}"))?;
    text.push('\n');
    if text.len() as u64 > MAX_PROJECT_BYTES {
        return Err(too_large_message(text.len() as u64));
    }
    Ok(text)
}

/// Loads a `.cadence` project, enforcing the size cap and model validation.
pub fn load_project(path: &Path) -> Result<Project, String> {
    let shown = path.display();
    let meta = fs::metadata(path).map_err(|err| format!("Could not open {shown}: {err}"))?;
    if !meta.is_file() {
        return Err(format!("{shown} is not a file."));
    }
    if meta.len() > MAX_PROJECT_BYTES {
        return Err(too_large_message(meta.len()));
    }
    let file = File::open(path).map_err(|err| format!("Could not open {shown}: {err}"))?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    // Read at most one byte past the cap so a file growing after the metadata
    // check is still rejected without unbounded memory use.
    file.take(MAX_PROJECT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("Could not read {shown}: {err}"))?;
    if bytes.len() as u64 > MAX_PROJECT_BYTES {
        return Err(too_large_message(bytes.len() as u64));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("{shown} is not a Cadence project (it is not UTF-8 text)."))?;
    project_from_json(&text).map_err(|err| format!("{shown}: {err}"))
}

/// Saves a project atomically. On any error the existing file is untouched.
pub fn save_project(project: &Project, path: &Path) -> Result<(), String> {
    write_file_atomic(path, project_to_json(project)?.as_bytes())
}

/// Returns `path` with the `.cadence` extension appended if it is missing.
pub fn with_project_extension(path: &Path) -> PathBuf {
    let has_ext = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(PROJECT_EXTENSION));
    if has_ext {
        path.to_path_buf()
    } else {
        let mut name: OsString = path.as_os_str().to_owned();
        name.push(".");
        name.push(PROJECT_EXTENSION);
        PathBuf::from(name)
    }
}

/// Directory for Cadence's local application data (autosave etc.).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV).filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    directories::ProjectDirs::from("studio", "Cadence", "Cadence")
        .map(|dirs| dirs.data_local_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("cadence-studio"))
}

/// Folder holding the crash-recovery autosave.
pub fn recovery_dir() -> PathBuf {
    data_dir().join("recovery")
}

/// Location of the crash-recovery autosave.
pub fn recovery_path() -> PathBuf {
    recovery_dir().join(RECOVERY_FILE)
}

// ----- audio store ---------------------------------------------------------

/// Directory holding imported WAV files referenced by
/// [`crate::model::AudioClip::file`]. Staged names are bare
/// `<content-hash>.wav` filenames, so projects record a tiny safe reference
/// while the bytes live beside the autosave data.
pub fn audio_dir() -> PathBuf {
    data_dir().join("audio")
}

/// Largest WAV file the audio store accepts — also the read cap when a
/// clip's file is decoded for playback.
pub const MAX_AUDIO_FILE_BYTES: u64 = 48 * 1024 * 1024;

/// Accepted WAV sample-rate range, matching the rates the renderer can run at.
pub const MIN_AUDIO_SAMPLE_RATE: u32 = 8_000;
pub const MAX_AUDIO_SAMPLE_RATE: u32 = 384_000;

/// Decoded-frame cap per channel; the file-byte cap binds first for normal
/// content, this only fences pathological high-rate files.
const MAX_AUDIO_FRAMES: usize = 32 * 1024 * 1024;

/// What a staged file contributes to an `AudioClip`: the store file name
/// plus the decoded source duration.
#[derive(Clone, Debug, PartialEq)]
pub struct StagedAudio {
    /// Store-relative file name (the value [`crate::model::AudioClip::file`]
    /// stores — never a path).
    pub file: String,
    /// Source duration in seconds; the clip's timeline `length` derives
    /// from `seconds / project.seconds_per_step()` at import time.
    pub seconds: f64,
}

/// Copies `source` into the audio store under a content-hash name and
/// returns the clip reference. The source is decoded first so a bogus pick
/// never leaves a dead file in the store; re-importing identical content
/// reuses the staged copy instead of duplicating it.
pub fn stage_audio_file(source: &Path) -> Result<StagedAudio, String> {
    stage_audio_file_into(&audio_dir(), source)
}

/// The store-parameterized core of [`stage_audio_file`] — tests point it at
/// a temp directory, production callers go through [`audio_dir`].
fn stage_audio_file_into(dir: &Path, source: &Path) -> Result<StagedAudio, String> {
    let shown = source.display();
    let meta = fs::metadata(source).map_err(|err| format!("Could not open {shown}: {err}"))?;
    if !meta.is_file() {
        return Err(format!("{shown} is not a file."));
    }
    if meta.len() > MAX_AUDIO_FILE_BYTES {
        return Err(format!(
            "{shown} is too large ({:.1} MB). Audio clips are limited to {} MB.",
            meta.len() as f64 / (1024.0 * 1024.0),
            MAX_AUDIO_FILE_BYTES / (1024 * 1024)
        ));
    }
    let file = File::open(source).map_err(|err| format!("Could not open {shown}: {err}"))?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_AUDIO_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("Could not read {shown}: {err}"))?;
    if bytes.len() as u64 > MAX_AUDIO_FILE_BYTES {
        return Err(format!("{shown} is too large."));
    }
    let decoded = decode_wav_f32(&bytes).map_err(|err| format!("{shown}: {err}"))?;
    let name = format!("{:016x}.wav", audio_content_hash(&bytes));
    fs::create_dir_all(dir)
        .map_err(|err| format!("Could not create the audio store {}: {err}", dir.display()))?;
    let dest = dir.join(&name);
    if !dest.is_file() {
        write_file_atomic(&dest, &bytes)?;
    }
    Ok(StagedAudio {
        file: name,
        seconds: decoded.seconds,
    })
}

/// Store-relative names of audio files `project` references that are not
/// staged in `dir`. Used to warn when a song travels between machines: a
/// missing clip file plays and exports as silence.
pub fn missing_audio_refs_in(project: &crate::model::Project, dir: &Path) -> Vec<String> {
    let mut missing: Vec<String> = project
        .tracks
        .iter()
        .flat_map(|track| track.audio_clips.iter().map(|clip| clip.file.clone()))
        .filter(|name| {
            fs::metadata(dir.join(name))
                .map(|m| !m.is_file())
                .unwrap_or(true)
        })
        .collect();
    missing.sort();
    missing.dedup();
    missing
}

/// [`missing_audio_refs_in`] for the user audio store.
pub fn missing_audio_refs(project: &crate::model::Project) -> Vec<String> {
    missing_audio_refs_in(project, &audio_dir())
}

/// Reads a staged file by its store-relative `name` for decoding. Returns
/// `None` for anything unsafe, missing, unreadable or oversize — a missing
/// clip file is a documented tolerance (the clip plays silence), never a
/// load failure.
pub fn read_audio_file(name: &str) -> Option<Vec<u8>> {
    if !crate::model::audio_ref_name_ok(name) {
        return None;
    }
    let path = audio_dir().join(name);
    let meta = fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_AUDIO_FILE_BYTES {
        return None;
    }
    let file = File::open(&path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_AUDIO_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_AUDIO_FILE_BYTES).then_some(bytes)
}

/// FNV-1a over the whole file, seeded with its length: the staged name
/// changes whenever the content does, so a name always means one file.
fn audio_content_hash(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ bytes.len() as u64;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A decoded WAV file: normalized stereo `f32` at the source sample rate.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedWav {
    /// Source sample rate in Hz.
    pub sample_rate: u32,
    /// Source length in seconds (`frames / sample_rate`, capped).
    pub seconds: f64,
    /// Left channel (the source itself when the file is mono).
    pub left: Vec<f32>,
    /// Right channel (identical to `left` when the file is mono).
    pub right: Vec<f32>,
}

/// Decodes WAV bytes into normalized stereo `f32`. Integer PCM scales by
/// bit depth (with the unsigned-8-bit correction hound leaves out), float
/// data is clamped, mono duplicates into both channels and wider layouts
/// fold even channels left / odd right. Anything malformed is an `Err`,
/// never a panic; decoding stops at [`MAX_AUDIO_FRAMES`] so a pathological
/// file cannot allocate unboundedly.
pub fn decode_wav_f32(bytes: &[u8]) -> Result<DecodedWav, String> {
    let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes))
        .map_err(|err| format!("Not a WAV file: {err}"))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    if channels == 0 {
        return Err("The WAV file has no audio channels.".to_string());
    }
    if !(MIN_AUDIO_SAMPLE_RATE..=MAX_AUDIO_SAMPLE_RATE).contains(&spec.sample_rate) {
        return Err(format!(
            "Unsupported sample rate {} Hz ({}–{} Hz only).",
            spec.sample_rate, MIN_AUDIO_SAMPLE_RATE, MAX_AUDIO_SAMPLE_RATE
        ));
    }
    let (left, right) = match spec.sample_format {
        hound::SampleFormat::Float => decode_channels(&mut reader, channels, |s: f32| s)?,
        hound::SampleFormat::Int => {
            if !(8..=32).contains(&spec.bits_per_sample) {
                return Err(format!(
                    "Unsupported bit depth {} (8–32 bit PCM or float WAV only).",
                    spec.bits_per_sample
                ));
            }
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            // RIFF stores ≤ 8-bit PCM unsigned, but hound reads it as
            // signed — re-offset so real 8-bit files decode correctly.
            let unsigned = spec.bits_per_sample <= 8;
            decode_channels(&mut reader, channels, move |s: i32| {
                let v = if unsigned { (s & 0xFF) - 128 } else { s };
                v as f32 * scale
            })?
        }
    };
    if left.is_empty() {
        return Err("The WAV file contains no audio.".to_string());
    }
    Ok(DecodedWav {
        sample_rate: spec.sample_rate,
        seconds: left.len() as f64 / spec.sample_rate as f64,
        left,
        right,
    })
}

/// Per-channel decode loop behind [`decode_wav_f32`]: folds arbitrary
/// channel counts into stereo — even channel indices lean left, odd lean
/// right, each side normalized by its channel count — and duplicates mono
/// into both channels. Stops at [`MAX_AUDIO_FRAMES`].
fn decode_channels<S: hound::Sample>(
    reader: &mut hound::WavReader<impl std::io::Read>,
    channels: usize,
    mut to_f32: impl FnMut(S) -> f32,
) -> Result<(Vec<f32>, Vec<f32>), String> {
    let left_n = channels.div_ceil(2);
    let right_n = (channels / 2).max(1);
    let mut left = Vec::new();
    let mut right = Vec::new();
    let (mut acc_l, mut acc_r) = (0.0f32, 0.0f32);
    let mut chan = 0usize;
    for sample in reader.samples::<S>() {
        let v = to_f32(sample.map_err(|err| format!("bad audio data: {err}"))?).clamp(-1.5, 1.5);
        if chan.is_multiple_of(2) {
            acc_l += v;
        } else {
            acc_r += v;
        }
        chan += 1;
        if chan == channels {
            left.push((acc_l / left_n as f32).clamp(-1.0, 1.0));
            right.push((acc_r / right_n as f32).clamp(-1.0, 1.0));
            acc_l = 0.0;
            acc_r = 0.0;
            chan = 0;
            if left.len() >= MAX_AUDIO_FRAMES {
                break;
            }
        }
    }
    if channels == 1 {
        right = left.clone();
    }
    Ok((left, right))
}

/// The crash-recovery autosave as offered at launch: the project plus which
/// song file it was autosaved from and when.
#[derive(Clone, Debug, PartialEq)]
pub struct RecoveredWork {
    /// The autosaved project (already validated).
    pub project: Project,
    /// The `.cadence` file the work belonged to; `None` for an untitled song.
    pub source: Option<PathBuf>,
    /// When the autosave was written (the file's modification time).
    pub saved_at: Option<std::time::SystemTime>,
}

/// Writes the crash-recovery autosave atomically.
pub fn save_recovery(project: &Project) -> Result<(), String> {
    save_recovery_in(&recovery_dir(), project, None)
}

/// Writes the autosave into `dir`, recording `source` (the song file the work
/// belongs to) in a small `autosave.source` text file beside it. The
/// `.cadence` autosave itself is a plain project file.
pub fn save_recovery_in(
    dir: &Path,
    project: &Project,
    source: Option<&Path>,
) -> Result<(), String> {
    fs::create_dir_all(dir)
        .map_err(|err| format!("Could not create recovery folder {}: {err}", dir.display()))?;
    save_project(project, &dir.join(RECOVERY_FILE))?;
    let sidecar = dir.join(RECOVERY_SOURCE_FILE);
    // The source label is a convenience: failing to record it never fails
    // the autosave, but a stale label is removed rather than left behind.
    match source.and_then(Path::to_str) {
        Some(text) => {
            if write_file_atomic(&sidecar, text.as_bytes()).is_err() {
                let _ = fs::remove_file(&sidecar);
            }
        }
        None => {
            let _ = fs::remove_file(&sidecar);
        }
    }
    Ok(())
}

/// Loads the autosave if present. Returns `Ok(None)` when there is none.
///
/// A damaged autosave is never loaded: it is moved aside to
/// `autosave.damaged.cadence` (so it is not offered again on every launch) and
/// an error describing the problem is returned.
pub fn load_recovery() -> Result<Option<Project>, String> {
    Ok(load_recovery_in(&recovery_dir())?.map(|work| work.project))
}

/// [`load_recovery`] for an explicit recovery folder, with the source file
/// and autosave time.
pub fn load_recovery_in(dir: &Path) -> Result<Option<RecoveredWork>, String> {
    let path = dir.join(RECOVERY_FILE);
    let meta = match fs::symlink_metadata(&path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("Could not check {}: {err}", path.display())),
        Ok(meta) => meta,
    };
    match load_project(&path) {
        Ok(project) => {
            let source = fs::read(dir.join(RECOVERY_SOURCE_FILE))
                .ok()
                .filter(|bytes| bytes.len() <= 32 * 1024)
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty())
                .map(PathBuf::from);
            Ok(Some(RecoveredWork {
                project,
                source,
                saved_at: meta.modified().ok(),
            }))
        }
        Err(err) => {
            let damaged = path.with_file_name(DAMAGED_RECOVERY_FILE);
            let moved = fs::rename(&path, &damaged).is_ok();
            let _ = fs::remove_file(dir.join(RECOVERY_SOURCE_FILE));
            let note = if moved {
                format!(" It was moved to {}.", damaged.display())
            } else {
                String::new()
            };
            Err(format!(
                "The recovery autosave is damaged and was not loaded.{note} ({err})"
            ))
        }
    }
}

/// Deletes the autosave (after a clean save or an explicit discard).
pub fn clear_recovery() -> Result<(), String> {
    clear_recovery_in(&recovery_dir())
}

/// [`clear_recovery`] for an explicit recovery folder.
pub fn clear_recovery_in(dir: &Path) -> Result<(), String> {
    let path = dir.join(RECOVERY_FILE);
    let _ = fs::remove_file(dir.join(RECOVERY_SOURCE_FILE));
    match fs::remove_file(&path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(format!(
            "Could not remove recovery file {}: {err}",
            path.display()
        )),
        _ => Ok(()),
    }
}

fn too_large_message(bytes: u64) -> String {
    format!(
        "The project is too large ({:.1} MB). Cadence projects are limited to {} MB.",
        bytes as f64 / (1024.0 * 1024.0),
        MAX_PROJECT_BYTES / (1024 * 1024)
    )
}

/// Writes `bytes` to a temp file beside `path`, syncs it and renames it over
/// `path`. `std::fs::rename` replaces existing files atomically on both Unix
/// (`rename(2)`) and Windows (`MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`).
/// On error the temp file is removed and any existing `path` is untouched.
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.is_dir() {
        return Err(format!("{} is a folder, not a file.", path.display()));
    }
    write_atomic(path, bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let shown = path.display();
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("{shown} is not a valid file name."))?;
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    if !dir.is_dir() {
        return Err(format!("The folder {} does not exist.", dir.display()));
    }

    let mut tmp_name = OsString::from(".");
    tmp_name.push(file_name);
    tmp_name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = dir.join(tmp_name);

    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if let Ok(meta) = fs::metadata(path) {
            // Keep the original file's permissions (e.g. read-only stays an error below).
            if meta.permissions().readonly() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "the existing file is read-only",
                ));
            }
            let _ = fs::set_permissions(&tmp, meta.permissions());
        }
        fs::rename(&tmp, path)?;
        sync_dir(&dir);
        Ok(())
    })();

    result.map_err(|err| {
        let _ = fs::remove_file(&tmp);
        format!("Could not save {shown}: {err}")
    })
}

#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Ok(handle) = File::open(dir) {
        let _ = handle.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_is_appended_once() {
        assert_eq!(
            with_project_extension(Path::new("song")),
            PathBuf::from("song.cadence")
        );
        assert_eq!(
            with_project_extension(Path::new("a/song.CADENCE")),
            PathBuf::from("a/song.CADENCE")
        );
        assert_eq!(
            with_project_extension(Path::new("my.song")),
            PathBuf::from("my.song.cadence")
        );
    }

    #[test]
    fn oversize_text_is_rejected_before_parsing() {
        let text = " ".repeat(MAX_PROJECT_BYTES as usize + 1);
        assert!(project_from_json(&text).unwrap_err().contains("too large"));
    }

    // ----- audio store ------------------------------------------------------

    /// Writes a tiny 16-bit mono WAV (one quarter-note of a 220 Hz sine at
    /// 44.1 kHz) to `dir/name` and returns its path plus byte content.
    fn write_test_wav(dir: &Path, name: &str) -> (PathBuf, Vec<u8>) {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 44_100,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
            for i in 0..11_025u32 {
                let t = i as f32 / spec.sample_rate as f32;
                let s = (t * 220.0 * std::f32::consts::TAU).sin() * 0.6;
                w.write_sample((s * i16::MAX as f32) as i16).unwrap();
            }
            w.finalize().unwrap();
        }
        let bytes = cursor.into_inner();
        let path = dir.join(name);
        fs::write(&path, &bytes).unwrap();
        (path, bytes)
    }

    /// Unique throwaway directory under the system temp dir.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cadence-audio-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stage_audio_file_copies_and_reports_duration() {
        let src = temp_dir("src");
        let store = temp_dir("store");
        let (wav, _bytes) = write_test_wav(&src, "Take One.wav");

        let staged = stage_audio_file_into(&store, &wav).unwrap();
        assert!(
            crate::model::audio_ref_name_ok(&staged.file),
            "{:?}",
            staged.file
        );
        assert!(staged.file.ends_with(".wav"));
        assert!((staged.seconds - 0.25).abs() < 0.01, "{}", staged.seconds);
        // The store holds the exact bytes under the hashed name.
        let dest = store.join(&staged.file);
        assert_eq!(fs::read(&dest).unwrap(), fs::read(&wav).unwrap());

        // Re-staging identical content hits the same file without rewriting.
        let again = stage_audio_file_into(&store, &wav).unwrap();
        assert_eq!(again.file, staged.file);
        assert_eq!(fs::read_dir(&store).unwrap().count(), 1);

        // Rejects a non-WAV and never leaves a store entry behind.
        let bogus = src.join("nope.wav");
        fs::write(&bogus, b"not a wav").unwrap();
        assert!(stage_audio_file_into(&store, &bogus).is_err());
        assert_eq!(fs::read_dir(&store).unwrap().count(), 1);
        assert!(stage_audio_file_into(&store, &src.join("missing.wav")).is_err());
        assert!(
            stage_audio_file_into(&store, &src).is_err(),
            "a dir is not a file"
        );

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&store);
    }

    #[test]
    fn missing_audio_refs_lists_only_absent_staged_files() {
        let store = temp_dir("refs");
        let src = temp_dir("refs-src");
        let (wav, _bytes) = write_test_wav(&src, "Take.wav");
        let staged = stage_audio_file_into(&store, &wav).unwrap();

        let mut project = crate::model::Project::demo();
        let mut track = crate::model::create_audio_track("Audio");
        track.audio_clips.push(crate::model::AudioClip::new(
            &staged.file,
            "Take",
            0.0,
            16.0,
        ));
        track
            .audio_clips
            .push(crate::model::AudioClip::new("gone.wav", "Gone", 16.0, 16.0));
        track.audio_clips.push(crate::model::AudioClip::new(
            "gone.wav", "Gone 2", 32.0, 16.0,
        ));
        project.tracks.push(track);

        let missing = missing_audio_refs_in(&project, &store);
        assert_eq!(missing, vec!["gone.wav".to_owned()]);

        let _ = fs::remove_dir_all(&store);
        let _ = fs::remove_dir_all(&src);
    }

    #[test]
    fn decode_wav_f32_handles_formats_and_garbage() {
        // 16-bit mono: identical channels, normalized peak, right duration.
        let mut cursor = std::io::Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
            for i in 0..8_000i16 {
                w.write_sample((i / 4).saturating_mul(16)).unwrap();
            }
            w.finalize().unwrap();
        }
        let d = decode_wav_f32(&cursor.into_inner()).unwrap();
        assert_eq!(d.sample_rate, 8_000);
        assert_eq!(d.left.len(), 8_000);
        assert_eq!(d.left, d.right);
        assert!((d.seconds - 1.0).abs() < 0.001);
        assert!(d.left.iter().all(|v| (-1.0..=1.0).contains(v)));
        assert!(d.left.iter().any(|v| v.abs() > 0.5));

        // 32-bit float stereo keeps left and right independent.
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: 44_100,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
            for _ in 0..441 {
                w.write_sample(-0.9f32).unwrap();
                w.write_sample(0.9f32).unwrap();
            }
            w.finalize().unwrap();
        }
        let d = decode_wav_f32(&cursor.into_inner()).unwrap();
        assert_eq!(d.left.len(), 441);
        assert!(d.left.iter().all(|v| *v < 0.0));
        assert!(d.right.iter().all(|v| *v > 0.0));

        // Garbage never decodes.
        assert!(decode_wav_f32(b"").is_err());
        assert!(decode_wav_f32(b"RIFFxxxxWAVE").is_err());
        assert!(decode_wav_f32(&vec![0u8; 4096]).is_err());
    }

    #[test]
    fn read_audio_file_fences_the_store() {
        // Everything that is not a plain staged name fails fast — before any
        // filesystem access — and a missing-but-valid name reads as `None`.
        for bad in ["", ".", "..", "../x.wav", "a/b.wav", "b\\c.wav", "x y.wav"] {
            assert!(read_audio_file(bad).is_none(), "{bad}");
        }
        assert!(read_audio_file("0000000000000000.wav").is_none());
    }
}
