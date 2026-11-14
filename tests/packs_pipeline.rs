//! End-to-end instrument pack pipeline through the real `cadence` binary:
//! `--new-pack` -> `--validate-pack` -> `--install-pack` -> `--list-instruments`,
//! isolated in a temporary `CADENCE_DATA_DIR`.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use cadence_studio::packs;
use cadence_studio::persistence::DATA_DIR_ENV;

struct Sandbox {
    _dir: tempfile::TempDir,
    data: PathBuf,
    work: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let work = dir.path().join("work");
        fs::create_dir_all(&work).unwrap();
        Sandbox {
            _dir: dir,
            data,
            work,
        }
    }

    fn run(&self, args: &[&OsStr]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cadence"))
            .args(args)
            .env(DATA_DIR_ENV, &self.data)
            .env_remove(packs::PACKS_DIR_ENV)
            .output()
            .expect("run cadence binary")
    }

    fn packs_dir(&self) -> PathBuf {
        self.data.join("packs")
    }

    fn installed_files(&self) -> Vec<PathBuf> {
        match fs::read_dir(self.packs_dir()) {
            Ok(entries) => {
                let mut files: Vec<PathBuf> = entries.map(|e| e.unwrap().path()).collect();
                files.sort();
                files
            }
            Err(_) => Vec::new(),
        }
    }

    fn new_pack(&self, id: &str) -> PathBuf {
        let path = self.work.join(format!("{id}.json"));
        let out = self.run(&[OsStr::new("--new-pack"), OsStr::new(id), path.as_os_str()]);
        assert_success(&out, "--new-pack");
        assert!(path.is_file());
        path
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn assert_success(out: &Output, what: &str) {
    assert!(
        out.status.success(),
        "{what} failed ({:?})\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        text(&out.stdout),
        text(&out.stderr)
    );
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn write_json(path: &Path, value: &serde_json::Value) {
    fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

#[test]
fn template_validate_install_and_catalog() {
    let sandbox = Sandbox::new();

    // Bundled packs are available with an empty data dir, without problems.
    let out = sandbox.run(&[OsStr::new("--list-instruments")]);
    assert_success(&out, "--list-instruments (empty)");
    let listing = text(&out.stdout);
    assert!(listing.contains("Built-in instruments:"), "{listing}");
    let bundled = pack_instrument_count(&listing);
    assert!(
        bundled >= 8,
        "expected the bundled 8-bit pack, got {bundled} presets"
    );
    assert!(
        !text(&out.stderr).contains("Pack problems"),
        "{}",
        text(&out.stderr)
    );

    // Scaffold a template and validate it.
    let pack_path = sandbox.new_pack("my-synths");
    let template = packs::load_pack(&pack_path).expect("template must load");
    assert_eq!(template.id, "my-synths");
    assert!(!template.instruments.is_empty());
    let out = sandbox.run(&[OsStr::new("--validate-pack"), pack_path.as_os_str()]);
    assert_success(&out, "--validate-pack");
    assert!(text(&out.stdout).starts_with("OK:"));
    assert!(
        sandbox.installed_files().is_empty(),
        "validate must not install"
    );

    // Install it into the sandboxed user pack folder.
    let out = sandbox.run(&[OsStr::new("--install-pack"), pack_path.as_os_str()]);
    assert_success(&out, "--install-pack");
    let installed = sandbox.installed_files();
    assert_eq!(installed.len(), 1, "{installed:?}");
    assert!(installed[0].starts_with(sandbox.packs_dir()));
    assert!(text(&out.stdout).contains("Installed:"));

    let preset_id = template.instruments[0].id.clone();
    let out = sandbox.run(&[OsStr::new("--list-instruments")]);
    assert_success(&out, "--list-instruments (installed)");
    let listing = text(&out.stdout);
    assert_eq!(listing.matches(preset_id.as_str()).count(), 1, "{listing}");
    assert!(listing.contains(&template.instruments[0].name), "{listing}");
    assert_eq!(
        pack_instrument_count(&listing),
        bundled + template.instruments.len()
    );

    // Re-installing the same pack id replaces it instead of duplicating it.
    let mut value = read_json(&pack_path);
    value["instruments"][0]["name"] = serde_json::json!("Renamed Lead");
    write_json(&pack_path, &value);
    let out = sandbox.run(&[OsStr::new("--install-pack"), pack_path.as_os_str()]);
    assert_success(&out, "--install-pack (replace)");
    assert_eq!(sandbox.installed_files().len(), 1);
    let out = sandbox.run(&[OsStr::new("--list-instruments")]);
    let listing = text(&out.stdout);
    assert_eq!(listing.matches(preset_id.as_str()).count(), 1, "{listing}");
    assert!(listing.contains("Renamed Lead"), "{listing}");
    assert_eq!(
        pack_instrument_count(&listing),
        bundled + template.instruments.len()
    );
}

/// Parses `Pack instruments (N):` from `--list-instruments` output.
fn pack_instrument_count(listing: &str) -> usize {
    let line = listing
        .lines()
        .find(|line| line.starts_with("Pack instruments ("))
        .unwrap_or_else(|| panic!("no pack section in:\n{listing}"));
    line.trim_start_matches("Pack instruments (")
        .trim_end_matches("):")
        .parse()
        .unwrap()
}

#[test]
fn invalid_packs_are_rejected_and_never_installed() {
    let sandbox = Sandbox::new();
    let pack_path = sandbox.new_pack("broken-pack");
    let good = read_json(&pack_path);

    let mut loud = good.clone();
    loud["instruments"][0]["oscillators"][0]["level"] = serde_json::json!(5.0);
    let mut traversal = good.clone();
    traversal["id"] = serde_json::json!("../evil");
    let mut empty_name = good.clone();
    empty_name["instruments"][0]["name"] = serde_json::json!("");
    let mut duplicate = good.clone();
    let first = duplicate["instruments"][0].clone();
    duplicate["instruments"].as_array_mut().unwrap().push(first);

    let cases = [
        ("level", loud),
        ("id", traversal),
        ("name", empty_name),
        ("duplicate", duplicate),
    ];
    for (name, value) in cases {
        let path = sandbox.work.join(format!("bad-{name}.json"));
        write_json(&path, &value);
        for flag in ["--validate-pack", "--install-pack"] {
            let out = sandbox.run(&[OsStr::new(flag), path.as_os_str()]);
            assert_eq!(
                out.status.code(),
                Some(1),
                "{flag} accepted bad pack {name}"
            );
            let err = text(&out.stderr);
            assert!(err.starts_with("error:"), "{name}: {err}");
        }
    }
    let path = sandbox.work.join("garbage.json");
    fs::write(&path, b"{ this is not json").unwrap();
    let out = sandbox.run(&[OsStr::new("--install-pack"), path.as_os_str()]);
    assert_eq!(out.status.code(), Some(1));

    assert!(sandbox.installed_files().is_empty(), "bad pack was written");
    assert!(!sandbox.data.join("evil").exists());
    assert!(!sandbox.data.join("evil.json").exists());
}

#[test]
fn bad_field_errors_point_at_the_field() {
    let sandbox = Sandbox::new();
    let pack_path = sandbox.new_pack("pointer-pack");
    let mut value = read_json(&pack_path);
    value["instruments"][0]["oscillators"][0]["level"] = serde_json::json!(5.0);
    write_json(&pack_path, &value);
    let out = sandbox.run(&[OsStr::new("--validate-pack"), pack_path.as_os_str()]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(
        err.contains("level"),
        "error should name the bad field: {err}"
    );
}

#[test]
fn new_pack_refuses_to_overwrite_and_rejects_bad_ids() {
    let sandbox = Sandbox::new();
    let path = sandbox.new_pack("keep-me");
    let before = fs::read(&path).unwrap();
    let out = sandbox.run(&[
        OsStr::new("--new-pack"),
        OsStr::new("keep-me"),
        path.as_os_str(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(fs::read(&path).unwrap(), before);

    for bad in ["../evil", "Has Spaces", ""] {
        let out_path = sandbox.work.join("bad-id.json");
        let out = sandbox.run(&[
            OsStr::new("--new-pack"),
            OsStr::new(bad),
            out_path.as_os_str(),
        ]);
        assert_eq!(out.status.code(), Some(1), "accepted id {bad:?}");
        assert!(!out_path.exists());
    }
}

#[test]
fn user_pack_dir_follows_data_dir_override() {
    // Only this test touches the in-process environment.
    let dir = tempfile::tempdir().unwrap();
    std::env::remove_var(packs::PACKS_DIR_ENV);
    std::env::set_var(DATA_DIR_ENV, dir.path());
    assert_eq!(packs::user_packs_dir(), dir.path().join("packs"));
    let catalog = packs::load_catalog();
    assert!(catalog.errors.is_empty(), "{:?}", catalog.errors);
    std::env::remove_var(DATA_DIR_ENV);
}
