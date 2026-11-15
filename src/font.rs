//! SoundFont (`.sf2`) instrument support.
//!
//! General MIDI sounds are provided by user-installed SoundFont files — real
//! sampled instruments, unlike the built-in synthesizers and JSON packs. Drop
//! `.sf2` files into the user font directory ([`user_fonts_dir`]); every file
//! becomes one "font" that can play any of the 128 General MIDI programs
//! ([`GM_PROGRAM_NAMES`]).
//!
//! Songs reference a font by *name* ([`crate::model::FontRef`]): the file stem
//! (e.g. `GeneralUser` for `GeneralUser.sf2`) plus a GM program number. The
//! audio engine resolves names to parsed fonts through [`font_key`], a stable
//! 64-bit hash of the normalized name, so the model never has to carry file
//! handles or paths.
//!
//! Loading mirrors [`crate::packs`]: [`load_library`] scans
//! [`user_fonts_dir`] for `*.sf2` files (size-capped, sorted, non-fatal per
//! file) and returns a [`FontLibrary`] the caller shares with the audio
//! engine via `AudioEngine::set_fonts` / `Renderer::set_fonts`. Parsing is
//! done on the caller's thread — never on the audio thread.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustysynth::SoundFont;

/// Environment variable overriding the user font directory directly.
/// Intended for tests, portable installs and diagnostics.
pub const FONTS_DIR_ENV: &str = "CADENCE_FONTS_DIR";

/// Largest `.sf2` file Cadence will read. SoundFonts embed all their samples,
/// so they run from a few hundred KB to ~100 MB; 256 MB is generous headroom.
pub const MAX_FONT_BYTES: u64 = 256 * 1024 * 1024;

const FONT_FILE_EXT: &str = "sf2";

/// The 128 General MIDI program names, indexed by program number (0 = Acoustic
/// Grand Piano, 127 = Gunshot). This is the standard GM mapping every
/// SoundFont aims for; Cadence uses it for display names since individual
/// fonts may label their presets differently.
pub const GM_PROGRAM_NAMES: [&str; 128] = [
    "Acoustic Grand Piano",
    "Bright Acoustic Piano",
    "Electric Grand Piano",
    "Honky-tonk Piano",
    "Electric Piano 1",
    "Electric Piano 2",
    "Harpsichord",
    "Clavinet",
    "Celesta",
    "Glockenspiel",
    "Music Box",
    "Vibraphone",
    "Marimba",
    "Xylophone",
    "Tubular Bells",
    "Dulcimer",
    "Drawbar Organ",
    "Percussive Organ",
    "Rock Organ",
    "Church Organ",
    "Reed Organ",
    "Accordion",
    "Harmonica",
    "Tango Accordion",
    "Acoustic Guitar (nylon)",
    "Acoustic Guitar (steel)",
    "Electric Guitar (jazz)",
    "Electric Guitar (clean)",
    "Electric Guitar (muted)",
    "Overdriven Guitar",
    "Distortion Guitar",
    "Guitar Harmonics",
    "Acoustic Bass",
    "Electric Bass (finger)",
    "Electric Bass (pick)",
    "Fretless Bass",
    "Slap Bass 1",
    "Slap Bass 2",
    "Synth Bass 1",
    "Synth Bass 2",
    "Violin",
    "Viola",
    "Cello",
    "Contrabass",
    "Tremolo Strings",
    "Pizzicato Strings",
    "Orchestral Harp",
    "Timpani",
    "String Ensemble 1",
    "String Ensemble 2",
    "SynthStrings 1",
    "SynthStrings 2",
    "Choir Aahs",
    "Voice Oohs",
    "Synth Voice",
    "Orchestra Hit",
    "Trumpet",
    "Trombone",
    "Tuba",
    "Muted Trumpet",
    "French Horn",
    "Brass Section",
    "Synth Brass 1",
    "Synth Brass 2",
    "Soprano Sax",
    "Alto Sax",
    "Tenor Sax",
    "Baritone Sax",
    "Oboe",
    "English Horn",
    "Bassoon",
    "Clarinet",
    "Piccolo",
    "Flute",
    "Recorder",
    "Pan Flute",
    "Blown Bottle",
    "Shakuhachi",
    "Whistle",
    "Ocarina",
    "Lead 1 (square)",
    "Lead 2 (sawtooth)",
    "Lead 3 (calliope)",
    "Lead 4 (chiff)",
    "Lead 5 (charang)",
    "Lead 6 (voice)",
    "Lead 7 (fifths)",
    "Lead 8 (bass+lead)",
    "Pad 1 (new age)",
    "Pad 2 (warm)",
    "Pad 3 (polysynth)",
    "Pad 4 (choir)",
    "Pad 5 (bowed)",
    "Pad 6 (metallic)",
    "Pad 7 (halo)",
    "Pad 8 (sweep)",
    "FX 1 (rain)",
    "FX 2 (soundtrack)",
    "FX 3 (crystal)",
    "FX 4 (atmosphere)",
    "FX 5 (brightness)",
    "FX 6 (goblins)",
    "FX 7 (echoes)",
    "FX 8 (sci-fi)",
    "Sitar",
    "Banjo",
    "Shamisen",
    "Koto",
    "Kalimba",
    "Bagpipe",
    "Fiddle",
    "Shanai",
    "Tinkle Bell",
    "Agogo",
    "Steel Drums",
    "Woodblock",
    "Taiko Drum",
    "Melodic Tom",
    "Synth Drum",
    "Reverse Cymbal",
    "Guitar Fret Noise",
    "Breath Noise",
    "Seashore",
    "Bird Tweet",
    "Telephone Ring",
    "Helicopter",
    "Applause",
    "Gunshot",
];

/// Display name of GM `program` (0..=127); out-of-range values clamp to the
/// nearest program so unvalidated data still shows something sensible.
pub fn gm_program_name(program: u8) -> &'static str {
    GM_PROGRAM_NAMES[(program.min(127)) as usize]
}

/// Stable 64-bit key for a font name, shared by the model (which stores the
/// name) and the engine (which looks fonts up by key).
///
/// The name is normalized by trimming surrounding whitespace; everything else
/// — including case — is significant, matching file-system semantics. The key
/// is only ever used in memory (map keys, `Sound::Font`), never persisted, so
/// it does not need to survive Rust version changes: FNV-1a is used anyway for
/// determinism and simplicity.
pub fn font_key(name: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in name.trim().as_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Directory holding user-installed SoundFonts (`*.sf2` files).
///
/// Resolution order: `$CADENCE_FONTS_DIR`, then `$CADENCE_DATA_DIR/fonts`,
/// then the platform application data directory (`.../Cadence/fonts`).
pub fn user_fonts_dir() -> PathBuf {
    resolve_fonts_dir(&|key| std::env::var_os(key))
}

fn resolve_fonts_dir(env: &dyn Fn(&str) -> Option<OsString>) -> PathBuf {
    if let Some(dir) = env(FONTS_DIR_ENV).filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(dir) = env(crate::packs::DATA_DIR_ENV).filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir).join("fonts");
    }
    directories::ProjectDirs::from("studio", "Cadence", "Cadence")
        .map(|dirs| dirs.data_local_dir().join("fonts"))
        .unwrap_or_else(|| std::env::temp_dir().join("cadence-studio").join("fonts"))
}

/// Display metadata for one installed font (no audio data).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontInfo {
    /// Font name: the `.sf2` file stem (e.g. `GeneralUser`). This is the name
    /// songs store in [`crate::model::FontRef`].
    pub name: String,
    /// [`font_key`] of `name`.
    pub key: u64,
    /// Number of presets the font provides.
    pub presets: usize,
}

/// Every SoundFont currently known to the app plus non-fatal load problems.
///
/// `fonts` is sorted by name for deterministic display; `by_key` maps
/// [`font_key`] to the parsed font for the audio engine. Files that fail to
/// parse or exceed [`MAX_FONT_BYTES`] are skipped and reported in `errors`.
#[derive(Clone, Debug, Default)]
pub struct FontLibrary {
    pub fonts: Vec<FontInfo>,
    pub by_key: HashMap<u64, Arc<SoundFont>>,
    /// Directory the library was loaded from (for display/diagnostics).
    pub dir: PathBuf,
    pub errors: Vec<String>,
}

impl FontLibrary {
    /// An empty library: every font reference falls back to the track's
    /// built-in/preset sound.
    pub fn empty() -> Self {
        FontLibrary::default()
    }

    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }

    /// The parsed font for `key`, if installed.
    pub fn get(&self, key: u64) -> Option<&Arc<SoundFont>> {
        self.by_key.get(&key)
    }
}

/// Loads every `*.sf2` in [`user_fonts_dir`]. Never fails as a whole;
/// per-file problems land in [`FontLibrary::errors`].
pub fn load_library() -> FontLibrary {
    load_library_from(&user_fonts_dir())
}

/// Like [`load_library`] but reads fonts from `dir` instead of
/// [`user_fonts_dir`]. A missing directory yields an empty library.
pub fn load_library_from(dir: &Path) -> FontLibrary {
    let mut library = FontLibrary {
        dir: dir.to_path_buf(),
        ..FontLibrary::default()
    };
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_font = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case(FONT_FILE_EXT));
            if is_font {
                files.push(path);
            }
        }
    }
    files.sort();
    for path in files {
        let origin = path.display().to_string();
        match load_font_file(&path) {
            Ok((name, font)) => {
                let key = font_key(&name);
                if library.by_key.contains_key(&key) {
                    library.errors.push(format!(
                        "{origin}: duplicate font name \"{name}\"; keeping the first file"
                    ));
                    continue;
                }
                let presets = font.get_presets().len();
                library.fonts.push(FontInfo { name, key, presets });
                library.by_key.insert(key, font);
            }
            Err(err) => library.errors.push(format!("{origin}: {err}")),
        }
    }
    library.fonts.sort_by(|a, b| a.name.cmp(&b.name));
    // `by_key` was filled in file order; `fonts` is display order. Rebuild the
    // info list in the same sorted order is unnecessary — FontInfo carries its
    // own key, so lookups stay correct.
    library
}

/// Parses one `.sf2` file. The returned name is the file stem — predictable
/// and stable, so songs keep working if the font's internal name changes.
fn load_font_file(path: &Path) -> Result<(String, Arc<SoundFont>), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("could not read file info: {e}"))?;
    if meta.len() > MAX_FONT_BYTES {
        return Err(format!(
            "file is too large ({} MB, limit {} MB)",
            meta.len() / (1024 * 1024),
            MAX_FONT_BYTES / (1024 * 1024)
        ));
    }
    let mut file = File::open(path).map_err(|e| format!("could not open: {e}"))?;
    let font = SoundFont::new(&mut file).map_err(|e| format!("not a readable SoundFont: {e}"))?;
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.trim().is_empty())
        .ok_or_else(|| "file name has no usable stem".to_string())?;
    Ok((name, Arc::new(font)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gm_program_table_is_complete_and_standard() {
        assert_eq!(GM_PROGRAM_NAMES.len(), 128);
        assert_eq!(GM_PROGRAM_NAMES[0], "Acoustic Grand Piano");
        assert_eq!(GM_PROGRAM_NAMES[24], "Acoustic Guitar (nylon)");
        assert_eq!(GM_PROGRAM_NAMES[32], "Acoustic Bass");
        assert_eq!(GM_PROGRAM_NAMES[56], "Trumpet");
        assert_eq!(GM_PROGRAM_NAMES[127], "Gunshot");
        // Every entry is non-empty and unique-ish enough for display.
        assert!(GM_PROGRAM_NAMES.iter().all(|n| !n.is_empty()));
        assert_eq!(gm_program_name(0), "Acoustic Grand Piano");
        assert_eq!(gm_program_name(200), "Gunshot", "clamps to 127");
    }

    #[test]
    fn font_key_is_stable_and_normalized() {
        let a = font_key("GeneralUser");
        assert_eq!(a, font_key("GeneralUser"));
        assert_eq!(a, font_key("  GeneralUser  "), "trims whitespace");
        assert_ne!(a, font_key("generaluser"), "case is significant");
        assert_ne!(a, font_key("OtherFont"));
        // FNV-1a of the empty/whitespace name is the offset basis.
        assert_eq!(font_key(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(font_key("   "), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn fonts_dir_resolution() {
        // CADENCE_FONTS_DIR wins outright.
        let env = |key: &str| match key {
            FONTS_DIR_ENV => Some(OsString::from("/tmp/fonts-override")),
            crate::packs::DATA_DIR_ENV => Some(OsString::from("/tmp/data-override")),
            _ => None,
        };
        assert_eq!(
            resolve_fonts_dir(&env),
            PathBuf::from("/tmp/fonts-override")
        );
        // CADENCE_DATA_DIR gets a /fonts suffix.
        let env = |key: &str| match key {
            crate::packs::DATA_DIR_ENV => Some(OsString::from("/tmp/data-override")),
            _ => None,
        };
        assert_eq!(
            resolve_fonts_dir(&env),
            PathBuf::from("/tmp/data-override/fonts")
        );
        // Default branch always ends in "fonts".
        let none = |_key: &str| None;
        assert!(resolve_fonts_dir(&none).ends_with("fonts"));
    }

    #[test]
    fn missing_dir_yields_empty_library() {
        let library = load_library_from(Path::new("/definitely/not/a/real/dir"));
        assert!(library.fonts.is_empty());
        assert!(library.by_key.is_empty());
        assert!(library.errors.is_empty());
        assert!(library.is_empty());
    }

    #[test]
    fn non_sf2_files_are_ignored() {
        let dir = std::env::temp_dir().join(format!("cadence-font-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
        std::fs::write(dir.join("fake.sf2"), b"not a soundfont").unwrap();
        let library = load_library_from(&dir);
        // The bogus .sf2 is reported, the .txt is ignored entirely.
        assert!(library.fonts.is_empty());
        assert_eq!(library.errors.len(), 1);
        assert!(library.errors[0].contains("fake.sf2"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
