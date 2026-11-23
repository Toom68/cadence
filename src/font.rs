//! SoundFont (`.sf2`) instrument support.
//!
//! General MIDI sounds come from SoundFont files — real sampled instruments,
//! unlike the built-in synthesizers and JSON packs. Two kinds of fonts are
//! loaded:
//!
//! - **Bundled** fonts ship with the app ([`bundled_fonts_dirs`]): release
//!   packages put the General MIDI font [`BUILTIN_FONT_NAME`] in a
//!   `soundfonts/` folder next to the executable (Windows) or in
//!   `Cadence.app/Contents/Resources/soundfonts/` (macOS). They are read-only,
//!   listed first and labelled "(built-in)" ([`FontLibrary::display_name`]).
//! - **User** fonts are `.sf2` files dropped into the user font directory
//!   ([`user_fonts_dir`]).
//!
//! Every file becomes one "font" that can play any of the 128 General MIDI
//! programs ([`GM_PROGRAM_NAMES`]).
//!
//! Songs reference a font by *name* ([`crate::model::FontRef`]): the file stem
//! (e.g. `GeneralUser` for `GeneralUser.sf2`, or `Cadence GM` for the bundled
//! font) plus a GM program number. Because the bundled file name is fixed, a
//! song using the built-in font plays on any machine with Cadence installed.
//! The audio engine resolves names to parsed fonts through [`font_key`], a
//! stable 64-bit hash of the normalized name, so the model never has to carry
//! file handles or paths.
//!
//! Loading mirrors [`crate::packs`]: [`load_library`] scans the bundled dirs
//! and [`user_fonts_dir`] for `*.sf2` files (size-capped, sorted, non-fatal
//! per file) and returns a [`FontLibrary`] the caller shares with the audio
//! engine via `AudioEngine::set_fonts` / `Renderer::set_fonts`. Parsing is
//! done on the caller's thread — never on the audio thread. Parsed bundled
//! fonts are cached for the life of the process, so re-scanning the user
//! folder doesn't re-read the ~30 MB built-in font.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use rustysynth::SoundFont;

/// Environment variable overriding the user font directory directly.
/// Intended for tests, portable installs and diagnostics.
pub const FONTS_DIR_ENV: &str = "CADENCE_FONTS_DIR";

/// Environment variable naming the directory of fonts that ship with the app,
/// replacing the discovery next to the executable ([`bundled_fonts_dirs`]).
/// Developers point it at a downloaded font; tests point it at an empty or
/// temporary folder so the result never depends on what is installed.
pub const BUNDLED_FONTS_DIR_ENV: &str = "CADENCE_SOUNDFONT_DIR";

/// Name (file stem) of the General MIDI SoundFont shipped with Cadence —
/// `Cadence GM.sf2` in the bundled `soundfonts/` folder. Songs store this name,
/// so it must never change; the font behind it (currently GeneralUser GS, see
/// `packaging/soundfont/README.md`) may.
pub const BUILTIN_FONT_NAME: &str = "Cadence GM";

/// Folder name holding bundled fonts, next to the executable (Windows,
/// portable zip, Linux) or in the macOS bundle's `Contents/Resources`.
pub const BUNDLED_FONTS_SUBDIR: &str = "soundfonts";

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

/// The 16 General MIDI program families, in standard GM bank order: programs
/// 0..=127 group into families of [`GM_FAMILY_SIZE`] (8) following
/// [`GM_PROGRAM_NAMES`].
pub const GM_FAMILIES: [&str; 16] = [
    "Piano",
    "Chromatic Percussion",
    "Organ",
    "Guitar",
    "Bass",
    "Strings",
    "Ensemble",
    "Brass",
    "Reed",
    "Pipe",
    "Synth Lead",
    "Synth Pad",
    "Synth FX",
    "Ethnic",
    "Percussive",
    "Sound FX",
];

/// Programs per GM family (128 programs / 16 families).
pub const GM_FAMILY_SIZE: usize = 8;

/// Display name of the GM family containing `program` (0..=127); out-of-range
/// values clamp into the last family like [`gm_program_name`].
pub fn gm_family(program: u8) -> &'static str {
    GM_FAMILIES[(program.min(127) as usize) / GM_FAMILY_SIZE]
}

/// The full GM catalog for listing UIs: every family with its 8 programs as
/// `(program number, display name)` pairs, in standard GM order. Shared by
/// the instrument library's SoundFont bank and the program pickers so their
/// grouping can never drift apart.
pub fn gm_program_catalog() -> Vec<(&'static str, Vec<(u8, &'static str)>)> {
    GM_FAMILIES
        .iter()
        .enumerate()
        .map(|(family, name)| {
            let programs = GM_PROGRAM_NAMES
                .iter()
                .enumerate()
                .skip(family * GM_FAMILY_SIZE)
                .take(GM_FAMILY_SIZE)
                .map(|(program, label)| (program as u8, *label))
                .collect();
            (*name, programs)
        })
        .collect()
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

/// Directories searched for fonts that ship with Cadence, in priority order
/// (the first file with a given name wins; later copies are skipped
/// silently).
///
/// - `$CADENCE_SOUNDFONT_DIR`, when set, is the only bundled directory.
/// - Otherwise `<exe dir>/soundfonts` (Windows installer, portable zip,
///   Linux) and, for an executable in `Cadence.app/Contents/MacOS`,
///   `Contents/Resources/soundfonts`.
/// - Developer builds (debug, or a binary under this crate's directory, as
///   `cargo run --release` produces) without either folder use the
///   repo-local cache `.cache/soundfonts` that `scripts/fetch-soundfont.sh`
///   fills. A packaged app never falls back to it, so its own copy is the
///   one that gets tested.
///
/// Missing directories are fine: they just contribute no fonts.
pub fn bundled_fonts_dirs() -> Vec<PathBuf> {
    let exe = std::env::current_exe().ok().map(|exe| {
        // Resolve symlinks (e.g. a `cadence` link on PATH) so the bundle
        // layout is found. Windows canonical paths are verbatim (`\\?\`)
        // and would defeat the dev-cache prefix check, so keep them as is.
        if cfg!(windows) {
            exe
        } else {
            exe.canonicalize().unwrap_or(exe)
        }
    });
    resolve_bundled_dirs(
        &|key| std::env::var_os(key),
        exe.as_deref(),
        Path::new(env!("CARGO_MANIFEST_DIR")),
        cfg!(debug_assertions),
        &|dir| dir.is_dir(),
    )
}

fn resolve_bundled_dirs(
    env: &dyn Fn(&str) -> Option<OsString>,
    exe: Option<&Path>,
    repo: &Path,
    debug_build: bool,
    is_dir: &dyn Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    if let Some(dir) = env(BUNDLED_FONTS_DIR_ENV).filter(|dir| !dir.is_empty()) {
        return vec![PathBuf::from(dir)];
    }
    let mut dirs = Vec::new();
    if let Some(exe_dir) = exe.and_then(Path::parent) {
        dirs.push(exe_dir.join(BUNDLED_FONTS_SUBDIR));
        if exe_dir.ends_with("Contents/MacOS") {
            if let Some(contents) = exe_dir.parent() {
                dirs.push(contents.join("Resources").join(BUNDLED_FONTS_SUBDIR));
            }
        }
    }
    let in_repo = exe.is_some_and(|exe| exe.starts_with(repo));
    if (debug_build || in_repo) && !dirs.iter().any(|dir| is_dir(dir)) {
        dirs.push(repo.join(".cache").join(BUNDLED_FONTS_SUBDIR));
    }
    dirs
}

/// Opens the user font directory in the platform file manager, creating it
/// first so `.sf2` files can be dropped straight in. Best-effort: returns the
/// opened directory or a displayable error when no opener could be spawned.
/// Desktop only — callers gate this on `io_enabled` like other native
/// integrations (headless tests never spawn a process).
pub fn reveal_user_fonts_dir() -> Result<PathBuf, String> {
    let dir = user_fonts_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    #[cfg(target_os = "windows")]
    {
        // `explorer` reports a nonzero exit code even when it opens the
        // folder, so any successful spawn counts.
        match std::process::Command::new("explorer").arg(&dir).status() {
            Ok(_) => Ok(dir),
            Err(e) => Err(format!("Could not open {}: {e}", dir.display())),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        match std::process::Command::new(opener).arg(&dir).status() {
            Ok(status) if status.success() => Ok(dir),
            Ok(_) => Err(format!(
                "The file manager could not open {}.",
                dir.display()
            )),
            Err(e) => Err(format!("Could not open {}: {e}", dir.display())),
        }
    }
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
/// `fonts` lists the bundled fonts first (search-path order), then the user
/// fonts sorted by name, for deterministic display; `by_key` maps
/// [`font_key`] to the parsed font for the audio engine. Files that fail to
/// parse or exceed [`MAX_FONT_BYTES`] are skipped and reported in `errors`.
#[derive(Clone, Debug, Default)]
pub struct FontLibrary {
    pub fonts: Vec<FontInfo>,
    pub by_key: HashMap<u64, Arc<SoundFont>>,
    /// User font directory the library was loaded from (for display and
    /// diagnostics; bundled fonts live in [`Self::bundled_dirs`]).
    pub dir: PathBuf,
    /// Bundled font directories that were searched.
    pub bundled_dirs: Vec<PathBuf>,
    /// [`font_key`]s of the fonts that ship with the app (read-only).
    pub builtin: HashSet<u64>,
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

    /// Whether the font called `name` ships with the app (as opposed to a
    /// user-installed `.sf2`).
    pub fn is_builtin(&self, name: &str) -> bool {
        self.builtin.contains(&font_key(name))
    }

    /// The label UIs show for font `name`: bundled fonts get a
    /// " (built-in)" suffix (e.g. "Cadence GM (built-in)"); songs keep
    /// storing the bare name.
    pub fn display_name(&self, name: &str) -> String {
        if self.is_builtin(name) {
            format!("{} (built-in)", name.trim())
        } else {
            name.to_owned()
        }
    }

    /// Whether any font ships with the app in this install.
    pub fn has_builtin(&self) -> bool {
        !self.builtin.is_empty()
    }

    fn insert(&mut self, name: String, font: Arc<SoundFont>, builtin: bool) {
        let key = font_key(&name);
        let presets = font.get_presets().len();
        self.fonts.push(FontInfo { name, key, presets });
        self.by_key.insert(key, font);
        if builtin {
            self.builtin.insert(key);
        }
    }
}

/// Loads the bundled fonts ([`bundled_fonts_dirs`]) and every `*.sf2` in
/// [`user_fonts_dir`]. Never fails as a whole; per-file problems land in
/// [`FontLibrary::errors`].
pub fn load_library() -> FontLibrary {
    load_library_with(&bundled_fonts_dirs(), &user_fonts_dir())
}

/// Like [`load_library`] but reads only the user fonts in `dir` (no bundled
/// fonts). A missing directory yields an empty library.
pub fn load_library_from(dir: &Path) -> FontLibrary {
    load_library_with(&[], dir)
}

/// Loads the bundled fonts from `bundled` (searched in order; the first file
/// with a given name wins) followed by the user fonts in `user`. A user font
/// whose name a bundled font already uses is skipped and reported, so a song
/// referencing the built-in font always gets the copy that ships with the
/// app. Missing directories yield no fonts and no errors.
pub fn load_library_with(bundled: &[PathBuf], user: &Path) -> FontLibrary {
    let mut library = FontLibrary {
        dir: user.to_path_buf(),
        bundled_dirs: bundled.to_vec(),
        ..FontLibrary::default()
    };
    for dir in bundled {
        for path in font_files(dir) {
            let Some(name) = font_name(&path) else {
                continue;
            };
            if library.by_key.contains_key(&font_key(&name)) {
                // An earlier bundled dir already provides it (e.g. the dev
                // cache next to a packaged copy): not a problem.
                continue;
            }
            match load_bundled_font(&path) {
                Ok(font) => library.insert(name, font, true),
                Err(err) => library
                    .errors
                    .push(format!("{} (built-in): {err}", path.display())),
            }
        }
    }
    let first_user = library.fonts.len();
    for path in font_files(user) {
        let origin = path.display().to_string();
        let Some(name) = font_name(&path) else {
            library
                .errors
                .push(format!("{origin}: file name has no usable stem"));
            continue;
        };
        let key = font_key(&name);
        if library.by_key.contains_key(&key) {
            let why = if library.builtin.contains(&key) {
                "a built-in font already uses this name; rename the file to use it"
            } else {
                "duplicate font name; keeping the first file"
            };
            library.errors.push(format!("{origin}: \"{name}\": {why}"));
            continue;
        }
        match load_font_file(&path) {
            Ok(font) => library.insert(name, font, false),
            Err(err) => library.errors.push(format!("{origin}: {err}")),
        }
    }
    library.fonts[first_user..].sort_by(|a, b| a.name.cmp(&b.name));
    library
}

/// The `*.sf2` files directly inside `dir`, sorted; empty when `dir` is
/// missing or unreadable.
fn font_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_font = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case(FONT_FILE_EXT));
            if is_font && path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// A font's name: its file stem — predictable and stable, so songs keep
/// working if the font's internal name changes.
fn font_name(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.trim().is_empty())
}

/// Parsed bundled fonts, keyed by path and invalidated by size/mtime. The
/// bundled folder is read-only, so this saves re-parsing the built-in font
/// every time the user folder changes.
type BundledCache = HashMap<PathBuf, (u64, Option<SystemTime>, Arc<SoundFont>)>;

fn load_bundled_font(path: &Path) -> Result<Arc<SoundFont>, String> {
    static CACHE: OnceLock<Mutex<BundledCache>> = OnceLock::new();
    let meta = std::fs::metadata(path).map_err(|e| format!("could not read file info: {e}"))?;
    let stamp = (meta.len(), meta.modified().ok());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((len, modified, font)) = cache.lock().ok().and_then(|c| c.get(path).cloned()) {
        if (len, modified) == stamp {
            return Ok(font);
        }
    }
    let font = load_font_file(path)?;
    if let Ok(mut cache) = cache.lock() {
        cache.insert(path.to_path_buf(), (stamp.0, stamp.1, font.clone()));
    }
    Ok(font)
}

/// Parses one `.sf2` file (size-capped).
fn load_font_file(path: &Path) -> Result<Arc<SoundFont>, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("could not read file info: {e}"))?;
    if meta.len() > MAX_FONT_BYTES {
        return Err(format!(
            "file is too large ({} MB, limit {} MB)",
            meta.len() / (1024 * 1024),
            MAX_FONT_BYTES / (1024 * 1024)
        ));
    }
    let file = File::open(path).map_err(|e| format!("could not open: {e}"))?;
    let font = SoundFont::new(&mut BufReader::new(file))
        .map_err(|e| format!("not a readable SoundFont: {e}"))?;
    Ok(Arc::new(font))
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
    fn gm_catalog_groups_cover_all_128_programs() {
        let catalog = gm_program_catalog();
        assert_eq!(catalog.len(), 16, "16 GM families");
        assert_eq!(catalog[0].0, "Piano");
        assert_eq!(catalog[15].0, "Sound FX");
        let mut seen = Vec::new();
        for (family, programs) in &catalog {
            assert_eq!(programs.len(), GM_FAMILY_SIZE, "{family} has 8 programs");
            for (program, name) in programs {
                assert_eq!(*name, GM_PROGRAM_NAMES[*program as usize]);
                assert_eq!(gm_family(*program), *family, "program {program}");
                seen.push(*program);
            }
        }
        assert_eq!(seen.len(), 128);
        seen.sort();
        assert_eq!(seen, (0u8..128).collect::<Vec<u8>>(), "no gaps or dupes");
        // Family boundaries: the first and last program of each family.
        for (i, family) in GM_FAMILIES.iter().enumerate() {
            let first = (i * GM_FAMILY_SIZE) as u8;
            assert_eq!(gm_family(first), *family);
            assert_eq!(gm_family(first + 7), *family);
        }
        assert_eq!(gm_family(128), "Sound FX", "out-of-range clamps");
        // Spot checks for the instrument library's category headers.
        assert_eq!(gm_family(0), "Piano");
        assert_eq!(gm_family(24), "Guitar");
        assert_eq!(gm_family(40), "Strings");
        assert_eq!(gm_family(56), "Brass");
        assert_eq!(gm_family(80), "Synth Lead");
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
    fn bundled_dirs_resolution() {
        let repo = Path::new("/src/cadence");
        let none = |_key: &str| None;
        let nothing = |_dir: &Path| false;
        let resolve = |env: &dyn Fn(&str) -> Option<OsString>, exe: Option<&str>, debug| {
            resolve_bundled_dirs(env, exe.map(Path::new), repo, debug, &nothing)
        };
        // The env override replaces discovery entirely.
        let env = |key: &str| match key {
            BUNDLED_FONTS_DIR_ENV => Some(OsString::from("/tmp/sf-override")),
            _ => None,
        };
        let mac = "/Applications/Cadence.app/Contents/MacOS/cadence";
        assert_eq!(
            resolve(&env, Some(mac), true),
            vec![PathBuf::from("/tmp/sf-override")]
        );
        // An empty override is ignored.
        let empty = |key: &str| (key == BUNDLED_FONTS_DIR_ENV).then(OsString::new);
        assert_eq!(
            resolve(&empty, Some(mac), false),
            resolve(&none, Some(mac), false)
        );
        // macOS bundle: next to the binary, then Contents/Resources.
        assert_eq!(
            resolve(&none, Some(mac), false),
            vec![
                PathBuf::from("/Applications/Cadence.app/Contents/MacOS/soundfonts"),
                PathBuf::from("/Applications/Cadence.app/Contents/Resources/soundfonts"),
            ]
        );
        // Windows install / portable zip / Linux: next to the binary only.
        let installed = "/opt/cadence/cadence";
        assert_eq!(
            resolve(&none, Some(installed), false),
            vec![PathBuf::from("/opt/cadence/soundfonts")]
        );
        // Debug builds also read the repo-local dev cache...
        let dev_cache = PathBuf::from("/src/cadence/.cache/soundfonts");
        assert_eq!(
            resolve(&none, Some(installed), true).last(),
            Some(&dev_cache)
        );
        // ...as do release binaries built inside the repo (`cargo run --release`).
        let cargo_run = "/src/cadence/target/release/cadence";
        assert_eq!(
            resolve(&none, Some(cargo_run), false),
            vec![
                PathBuf::from("/src/cadence/target/release/soundfonts"),
                dev_cache.clone(),
            ]
        );
        // A packaged app inside the repo (dist/Cadence.app) with its own
        // soundfonts folder never falls back to the dev cache.
        let dist = Path::new("/src/cadence/dist/Cadence.app/Contents/MacOS/cadence");
        let resources =
            PathBuf::from("/src/cadence/dist/Cadence.app/Contents/Resources/soundfonts");
        let has_resources = |dir: &Path| dir == resources.as_path();
        let dirs = resolve_bundled_dirs(&none, Some(dist), repo, true, &has_resources);
        assert_eq!(dirs.last(), Some(&resources));
        assert!(!dirs.contains(&dev_cache));
        // No executable path: only the dev cache, and only in debug builds.
        assert!(resolve(&none, None, false).is_empty());
        assert_eq!(resolve(&none, None, true), vec![dev_cache]);
    }

    #[test]
    fn packaging_pin_installs_the_builtin_font_name() {
        // The fetch scripts install the font under SOUNDFONT_FILE; its stem is
        // the name songs store, so it must match BUILTIN_FONT_NAME.
        let pin = include_str!("../packaging/soundfont/pin.txt");
        let value = |key: &str| {
            pin.lines()
                .find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))
                .map(str::trim)
                .unwrap_or_else(|| panic!("{key} missing from pin.txt"))
        };
        let file = Path::new(value("SOUNDFONT_FILE"));
        assert_eq!(file.extension().and_then(|e| e.to_str()), Some("sf2"));
        assert_eq!(font_name(file).as_deref(), Some(BUILTIN_FONT_NAME));
        let sha = value("SOUNDFONT_SHA256");
        assert_eq!(sha.len(), 64);
        assert!(sha.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(value("SOUNDFONT_URL").starts_with("https://"));
        let bytes: u64 = value("SOUNDFONT_BYTES").parse().unwrap();
        assert!(
            bytes <= MAX_FONT_BYTES,
            "the loader must accept the shipped font"
        );
    }

    #[test]
    fn display_name_marks_builtin_fonts() {
        let mut library = FontLibrary::empty();
        assert_eq!(library.display_name(BUILTIN_FONT_NAME), BUILTIN_FONT_NAME);
        library.builtin.insert(font_key(BUILTIN_FONT_NAME));
        assert!(library.has_builtin());
        assert!(library.is_builtin(BUILTIN_FONT_NAME));
        assert_eq!(
            library.display_name(BUILTIN_FONT_NAME),
            "Cadence GM (built-in)"
        );
        assert_eq!(library.display_name("GeneralUser"), "GeneralUser");
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
