// Release builds on Windows are GUI-subsystem apps (no console window).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cadence_studio::model::{Instrument, Project};
use cadence_studio::{audio, font, packs, persistence, update, APP_NAME, APP_VERSION};

const USAGE: &str = "\
Cadence - native music sequencer

USAGE:
    cadence [SONG.cadence]                 Open the app (optionally with a song)
    cadence --render-demo OUTPUT.wav       Render the built-in demo song to WAV
    cadence --render SONG.cadence OUT.wav  Render a song file to WAV
    cadence --render-stems SONG.cadence OUT_DIR
                                           Render each audible track to its own WAV
    cadence --check SONG.cadence           Validate a song file and print a summary

INSTRUMENT PACKS (JSON synth presets, no rebuild needed):
    cadence --new-pack ID OUTPUT.json      Write a starter pack template (e.g. ID = my-synths)
    cadence --validate-pack PACK.json      Check a pack and report the first bad field
    cadence --install-pack PACK.json       Validate, then install into the user pack folder
    cadence --list-instruments             List built-in, pack and SoundFont instruments

SOUNDFONTS (.sf2 sampled General MIDI, no rebuild needed):
    Drop .sf2 files into the user font folder shown by --list-instruments
    (or $CADENCE_FONTS_DIR), then pick a font + GM program for a track.

    Pipeline: --new-pack -> edit JSON -> --validate-pack -> --install-pack ->
    \"Reload Packs\" in the app (or restart) -> add the instrument from the library.

OTHER:
    cadence --check-update                 Ask the release feed for a newer version
    cadence --version                      Print the version
    cadence --help                         Print this help
";

#[derive(Debug, PartialEq)]
enum Command {
    Gui(Option<PathBuf>),
    RenderDemo(PathBuf),
    Render(PathBuf, PathBuf),
    RenderStems(PathBuf, PathBuf),
    Check(PathBuf),
    NewPack(String, PathBuf),
    ValidatePack(PathBuf),
    InstallPack(PathBuf),
    ListInstruments,
    CheckUpdate,
    Help,
    Version,
}

fn parse_args(args: Vec<OsString>) -> Result<Command, String> {
    // macOS may pass a Finder process serial number when launched from Finder.
    let mut args: Vec<OsString> = args
        .into_iter()
        .filter(|arg| !arg.to_string_lossy().starts_with("-psn_"))
        .collect();
    let Some(first) = args.first().map(|arg| arg.to_string_lossy().into_owned()) else {
        return Ok(Command::Gui(None));
    };
    let mut rest = args.split_off(1).into_iter().map(PathBuf::from);
    let mut take = |what: &str| {
        rest.next()
            .ok_or_else(|| format!("{first} requires {what}.\n\n{USAGE}"))
    };
    let command = match first.as_str() {
        "-h" | "--help" => Command::Help,
        "-V" | "--version" => Command::Version,
        "--render-demo" => Command::RenderDemo(take("an output .wav path")?),
        "--render" => {
            let song = take("a song file")?;
            Command::Render(song, take("an output .wav path")?)
        }
        "--render-stems" => {
            let song = take("a song file")?;
            Command::RenderStems(song, take("an output directory")?)
        }
        "--check" => Command::Check(take("a song file")?),
        "--new-pack" => {
            let id = take("a pack id")?.to_string_lossy().into_owned();
            Command::NewPack(id, take("an output .json path")?)
        }
        "--validate-pack" => Command::ValidatePack(take("a pack .json file")?),
        "--install-pack" => Command::InstallPack(take("a pack .json file")?),
        "--list-instruments" => Command::ListInstruments,
        "--check-update" => Command::CheckUpdate,
        flag if flag.starts_with('-') => {
            return Err(format!("Unknown option {flag}.\n\n{USAGE}"));
        }
        _ => Command::Gui(Some(args.remove(0).into())),
    };
    if let Some(extra) = rest.next() {
        return Err(format!(
            "Unexpected argument {}.\n\n{USAGE}",
            extra.display()
        ));
    }
    Ok(command)
}

fn main() -> ExitCode {
    let command = match parse_args(std::env::args_os().skip(1).collect()) {
        Ok(command) => command,
        Err(message) => {
            attach_console();
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    if !matches!(command, Command::Gui(_)) {
        attach_console();
    }
    let result = match command {
        Command::Gui(path) => run_gui(path),
        Command::Help => {
            print!("{USAGE}");
            Ok(())
        }
        Command::Version => {
            println!("{APP_NAME} {APP_VERSION}");
            Ok(())
        }
        Command::RenderDemo(out) => render(&Project::demo(), &out),
        Command::Render(song, out) => load_song(&song).and_then(|project| {
            if same_file(&song, &out) {
                Err("The output file must be different from the song file.".into())
            } else {
                render(&project, &out)
            }
        }),
        Command::RenderStems(song, dir) => {
            load_song(&song).and_then(|project| render_stems(&project, &dir))
        }
        Command::Check(song) => check(&song),
        Command::NewPack(id, out) => new_pack(&id, &out),
        Command::ValidatePack(path) => validate_pack(&path),
        Command::InstallPack(path) => install_pack(&path),
        Command::ListInstruments => {
            list_instruments();
            Ok(())
        }
        Command::CheckUpdate => check_update(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run_gui(path: Option<PathBuf>) -> Result<(), String> {
    if let Some(path) = path {
        cadence_studio::set_launch_path(std::path::absolute(&path).unwrap_or(path));
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_app_id("cadence")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([1100.0, 760.0]);
    match eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")) {
        Ok(icon) => viewport = viewport.with_icon(icon),
        Err(err) => eprintln!("warning: could not load app icon: {err}"),
    }
    let options = eframe::NativeOptions {
        viewport,
        centered: true,
        persistence_path: storage_path(std::env::var_os(persistence::DATA_DIR_ENV)),
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(cadence_studio::CadenceApp::new(cc)))),
    )
    .map_err(|err| format!("Could not start the {APP_NAME} window: {err}"))
}

/// Where eframe keeps its own storage (the selected theme, egui memory).
/// With `CADENCE_DATA_DIR` set it lives beside the other app data, so test
/// instances never share state with a real install; otherwise `None` keeps
/// eframe's per-user default.
fn storage_path(data_dir: Option<OsString>) -> Option<PathBuf> {
    data_dir
        .filter(|dir| !dir.is_empty())
        .map(|dir| PathBuf::from(dir).join("app.ron"))
}

fn seconds_per_step(project: &Project) -> f64 {
    60.0 / project.bpm / 4.0
}

fn render(project: &Project, out: &Path) -> Result<(), String> {
    project
        .validate()
        .map_err(|err| format!("Cannot render an invalid project: {err}"))?;
    warn_missing_audio(project);
    let duration = project.total_steps() * seconds_per_step(project);
    eprintln!(
        "Rendering \"{}\" ({:.1} s) to {}",
        project.name,
        duration,
        out.display()
    );
    let started = std::time::Instant::now();
    let mut last_shown = -1i32;
    let mut stderr = std::io::stderr();
    audio::export_wav(project, out, |progress| {
        let percent = (progress.clamp(0.0, 1.0) * 100.0).floor() as i32;
        if percent / 10 != last_shown / 10 {
            last_shown = percent;
            let _ = write!(stderr, "\r  {percent:3}%");
            let _ = stderr.flush();
        }
    })?;
    let bytes = std::fs::metadata(out).map(|meta| meta.len()).unwrap_or(0);
    eprintln!(
        "\r  100%\nWrote {} ({:.1} MB) in {:.2} s",
        out.display(),
        bytes as f64 / (1024.0 * 1024.0),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn render_stems(project: &Project, dir: &Path) -> Result<(), String> {
    project
        .validate()
        .map_err(|err| format!("Cannot render an invalid project: {err}"))?;
    warn_missing_audio(project);
    eprintln!(
        "Rendering stems of \"{}\" ({} tracks) to {}",
        project.name,
        project.tracks.len(),
        dir.display()
    );
    // The font catalog is shared across stems so .sf2 files are parsed once;
    // it is only loaded when the song actually references one.
    let fonts = if project.tracks.iter().any(|t| t.font.is_some()) {
        font::load_library()
    } else {
        font::FontLibrary::empty()
    };
    let started = std::time::Instant::now();
    let mut last_shown = -1i32;
    let mut stderr = std::io::stderr();
    let written = audio::export_stems(
        project,
        &fonts,
        dir,
        audio::EXPORT_SAMPLE_RATE,
        |progress| {
            let percent = (progress.clamp(0.0, 1.0) * 100.0).floor() as i32;
            if percent / 10 != last_shown / 10 {
                last_shown = percent;
                let _ = write!(stderr, "\r  {percent:3}%");
                let _ = stderr.flush();
            }
        },
    )?;
    eprintln!("\r  100%");
    for path in &written {
        println!("  {}", path.display());
    }
    for (index, track) in project.tracks.iter().enumerate() {
        if let Some(reason) = audio::stem_skip_reason(project, index) {
            println!("  skipped \"{}\" ({reason})", track.name);
        }
    }
    eprintln!(
        "Wrote {} stem{} to {} in {:.2} s",
        written.len(),
        if written.len() == 1 { "" } else { "s" },
        dir.display(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Loads and validates a song like the app's Open does: audio this
/// computer's store lacks is picked up from the song's `<name> Audio`
/// folder (written by "Save Copy with Audio…") or the song's own folder.
fn load_song(song: &Path) -> Result<Project, String> {
    let project = persistence::load_project(song)?;
    let found = persistence::import_song_audio(&project, song);
    if !found.staged.is_empty() {
        eprintln!(
            "Found {} audio file{} beside the song and added {} to the audio store.",
            found.staged.len(),
            if found.staged.len() == 1 { "" } else { "s" },
            if found.staged.len() == 1 {
                "it"
            } else {
                "them"
            },
        );
    }
    Ok(project)
}

fn check(song: &Path) -> Result<(), String> {
    let project = load_song(song)?;
    let events = project.events(false);
    let audible = project.events(true);
    println!("OK: {}", song.display());
    println!("  name:     {}", project.name);
    println!("  version:  {}", project.version);
    println!(
        "  tempo:    {} bpm, swing {:.0}%",
        project.bpm,
        project.swing * 100.0
    );
    println!(
        "  length:   {} bars ({:.1} s)",
        project.bars,
        project.total_steps() * seconds_per_step(&project)
    );
    println!("  tracks:   {}", project.tracks.len());
    for (index, track) in project.tracks.iter().enumerate() {
        let notes: usize = track.patterns.iter().map(|p| p.notes.len()).sum();
        let mut flags = String::new();
        if track.mute {
            flags.push_str(" [muted]");
        }
        if track.solo {
            flags.push_str(" [solo]");
        }
        println!(
            "    {:>2}. {:<20} {:<6} {} patterns, {} clips, {} notes{}",
            index + 1,
            track.name,
            track.instrument.name(),
            track.patterns.len(),
            track.clips.len(),
            notes,
            flags
        );
    }
    println!(
        "  events:   {} scheduled, {} audible",
        events.len(),
        audible.len()
    );
    let missing_audio = persistence::missing_audio_refs(&project);
    if !missing_audio.is_empty() {
        println!(
            "  WARNING: {} audio clip file{} not in this PC's audio store; \
             those clips play and export as silence: {}",
            missing_audio.len(),
            if missing_audio.len() == 1 { "" } else { "s" },
            missing_audio.join(", ")
        );
    }
    Ok(())
}

/// stderr warning for audio clips whose files are not in this PC's audio
/// store — the render itself stays silent on them, so say it up front.
fn warn_missing_audio(project: &Project) {
    let missing = persistence::missing_audio_refs(project);
    if !missing.is_empty() {
        eprintln!(
            "WARNING: {} audio clip file{} missing from the audio store; those \
             clips will be silent: {}",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" },
            missing.join(", ")
        );
    }
}

/// "my-synths" / "retro.chip_kit" -> "My Synths" / "Retro Chip Kit".
fn pack_display_name(id: &str) -> String {
    id.split(['-', '_', '.'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn new_pack(id: &str, out: &Path) -> Result<(), String> {
    if out.exists() {
        return Err(format!(
            "{} already exists; choose a new file name.",
            out.display()
        ));
    }
    let pack = packs::template_pack(id, &pack_display_name(id))?;
    persistence::write_file_atomic(out, packs::pack_to_json(&pack)?.as_bytes())?;
    println!(
        "Created pack \"{}\" ({}) with {} instrument(s) at {}",
        pack.name,
        pack.id,
        pack.instruments.len(),
        out.display()
    );
    println!(
        "Next: edit it, then run `cadence --validate-pack {0}` and `cadence --install-pack {0}`.",
        out.display()
    );
    Ok(())
}

fn print_pack(pack: &packs::InstrumentPack) {
    println!("  pack:        {} ({})", pack.name, pack.id);
    println!("  instruments: {}", pack.instruments.len());
    for preset in &pack.instruments {
        println!("    {:<32} {}", preset.id, preset.name);
    }
}

fn validate_pack(path: &Path) -> Result<(), String> {
    let pack = packs::load_pack(path)?;
    println!("OK: {}", path.display());
    print_pack(&pack);
    Ok(())
}

fn install_pack(path: &Path) -> Result<(), String> {
    let pack = packs::load_pack(path)?;
    let installed = packs::install_pack(path)?;
    println!("Installed: {}", installed.display());
    print_pack(&pack);
    println!("Use \"Reload Packs\" in Cadence (or restart it) to add these instruments.");
    Ok(())
}

fn list_instruments() {
    println!("Built-in instruments:");
    for instrument in Instrument::ALL {
        println!("  {:<32} {}", instrument.name(), instrument.description());
    }
    let catalog = packs::load_catalog();
    println!("Pack instruments ({}):", catalog.instruments.len());
    for preset in &catalog.instruments {
        println!("  {:<32} {}", preset.id, preset.name);
    }
    println!("User pack folder: {}", packs::user_packs_dir().display());
    if !catalog.errors.is_empty() {
        eprintln!("Pack problems ({}):", catalog.errors.len());
        for err in &catalog.errors {
            eprintln!("  {err}");
        }
    }
    let fonts = font::load_library();
    println!("SoundFont instruments ({}):", fonts.fonts.len());
    for info in &fonts.fonts {
        println!(
            "  {:<32} {} presets, GM programs 0-127",
            fonts.display_name(&info.name),
            info.presets
        );
    }
    if !fonts.has_builtin() {
        println!(
            "  (no built-in SoundFont found; searched: {})",
            fonts
                .bundled_dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!(
        "User font folder: {} (drop .sf2 files here)",
        fonts.dir.display()
    );
    if !fonts.errors.is_empty() {
        eprintln!("Font problems ({}):", fonts.errors.len());
        for err in &fonts.errors {
            eprintln!("  {err}");
        }
    }
}

fn check_update() -> Result<(), String> {
    match update::install_mode() {
        update::InstallMode::Manual(reason) => println!("one-click install: no — {reason}"),
        _ => println!("one-click install: yes"),
    }
    if update::update_key().is_none() {
        println!("note: this build has no release key, so it can't verify updates");
    }
    match update::check()? {
        Some(release) => {
            println!(
                "{APP_NAME} {} is available (this is {APP_VERSION}): {}",
                release.version,
                update::notes_url(&release.version)
            );
            match &release.asset {
                Some(asset) => println!("  download for this computer: {asset}"),
                None => println!("  that release has no download for this computer"),
            }
        }
        None => println!("{APP_NAME} {APP_VERSION} is up to date."),
    }
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// GUI-subsystem Windows builds have no console; reattach to the parent
/// terminal so CLI modes can print. No-op elsewhere.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Console::{
            AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE,
        };
        // Keep inherited handles (e.g. `cadence --check x > log.txt`).
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        if out.is_null() || out == INVALID_HANDLE_VALUE {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(OsString::from).collect())
    }

    #[test]
    fn parses_cli_modes() {
        assert_eq!(parse(&[]), Ok(Command::Gui(None)));
        assert_eq!(parse(&["-psn_0_12345"]), Ok(Command::Gui(None)));
        assert_eq!(
            parse(&["song.cadence"]),
            Ok(Command::Gui(Some("song.cadence".into())))
        );
        assert_eq!(
            parse(&["--render-demo", "out.wav"]),
            Ok(Command::RenderDemo("out.wav".into()))
        );
        assert_eq!(
            parse(&["--render", "a.cadence", "b.wav"]),
            Ok(Command::Render("a.cadence".into(), "b.wav".into()))
        );
        assert_eq!(
            parse(&["--render-stems", "a.cadence", "stems"]),
            Ok(Command::RenderStems("a.cadence".into(), "stems".into()))
        );
        assert_eq!(
            parse(&["--check", "a.cadence"]),
            Ok(Command::Check("a.cadence".into()))
        );
        assert_eq!(
            parse(&["--new-pack", "my-synths", "p.json"]),
            Ok(Command::NewPack("my-synths".into(), "p.json".into()))
        );
        assert_eq!(
            parse(&["--validate-pack", "p.json"]),
            Ok(Command::ValidatePack("p.json".into()))
        );
        assert_eq!(
            parse(&["--install-pack", "p.json"]),
            Ok(Command::InstallPack("p.json".into()))
        );
        assert_eq!(parse(&["--list-instruments"]), Ok(Command::ListInstruments));
        assert_eq!(parse(&["--check-update"]), Ok(Command::CheckUpdate));
        assert_eq!(parse(&["--help"]), Ok(Command::Help));
        assert_eq!(parse(&["--version"]), Ok(Command::Version));
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse(&["--render"]).is_err());
        assert!(parse(&["--render", "a.cadence"]).is_err());
        assert!(parse(&["--render-stems"]).is_err());
        assert!(parse(&["--render-stems", "a.cadence"]).is_err());
        assert!(parse(&["--render-stems", "a.cadence", "d", "x"]).is_err());
        assert!(parse(&["--render-demo"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["--check", "a", "b"]).is_err());
        assert!(parse(&["a.cadence", "b.cadence"]).is_err());
        assert!(parse(&["--new-pack", "id"]).is_err());
        assert!(parse(&["--validate-pack"]).is_err());
        assert!(parse(&["--list-instruments", "x"]).is_err());
    }

    #[test]
    fn eframe_storage_follows_the_data_dir_override() {
        assert_eq!(storage_path(None), None);
        assert_eq!(storage_path(Some(OsString::new())), None);
        assert_eq!(
            storage_path(Some(OsString::from("data"))),
            Some(PathBuf::from("data").join("app.ron"))
        );
    }

    #[test]
    fn pack_names_from_ids() {
        assert_eq!(pack_display_name("my-synths"), "My Synths");
        assert_eq!(pack_display_name("retro.chip_kit"), "Retro Chip Kit");
        assert_eq!(pack_display_name("8-bit"), "8 Bit");
    }
}
