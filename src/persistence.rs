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

/// Location of the crash-recovery autosave.
pub fn recovery_path() -> PathBuf {
    data_dir().join("recovery").join(RECOVERY_FILE)
}

/// Writes the crash-recovery autosave atomically.
pub fn save_recovery(project: &Project) -> Result<(), String> {
    let path = recovery_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .map_err(|err| format!("Could not create recovery folder {}: {err}", dir.display()))?;
    }
    save_project(project, &path)
}

/// Loads the autosave if present. Returns `Ok(None)` when there is none.
///
/// A damaged autosave is never loaded: it is moved aside to
/// `autosave.damaged.cadence` (so it is not offered again on every launch) and
/// an error describing the problem is returned.
pub fn load_recovery() -> Result<Option<Project>, String> {
    let path = recovery_path();
    match fs::symlink_metadata(&path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("Could not check {}: {err}", path.display())),
        Ok(_) => {}
    }
    match load_project(&path) {
        Ok(project) => Ok(Some(project)),
        Err(err) => {
            let damaged = path.with_file_name(DAMAGED_RECOVERY_FILE);
            let moved = fs::rename(&path, &damaged).is_ok();
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
    let path = recovery_path();
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
}
