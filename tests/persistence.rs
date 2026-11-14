mod common;

use std::fs;
use std::path::{Path, PathBuf};

use cadence_studio::model::Project;
use cadence_studio::persistence::{
    load_project, project_from_json, project_to_json, save_project, with_project_extension,
    MAX_PROJECT_BYTES, PROJECT_EXTENSION,
};

fn song_path(dir: &Path) -> PathBuf {
    dir.join(format!("song.{PROJECT_EXTENSION}"))
}

fn dir_entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn demo_round_trips_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let path = song_path(dir.path());
    let demo = Project::demo();
    save_project(&demo, &path).unwrap();
    let loaded = load_project(&path).unwrap();
    assert_eq!(loaded, demo);
    assert_eq!(dir_entries(dir.path()), vec!["song.cadence".to_string()]);
}

#[test]
fn empty_and_tiny_projects_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    for (index, project) in [Project::empty(), common::tiny_project()]
        .into_iter()
        .enumerate()
    {
        let path = dir.path().join(format!("p{index}.cadence"));
        save_project(&project, &path).unwrap();
        assert_eq!(load_project(&path).unwrap(), project);
    }
}

#[test]
fn saved_file_is_readable_pretty_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = song_path(dir.path());
    let demo = Project::demo();
    save_project(&demo, &path).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with('{') && text.ends_with("}\n"));
    assert!(text.lines().count() > 10 && text.contains("\"tracks\""));
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(value.get("bpm").is_some());
    assert_eq!(project_from_json(&text).unwrap(), demo);
}

#[test]
fn overwrite_replaces_contents_and_leaves_no_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = song_path(dir.path());
    save_project(&Project::demo(), &path).unwrap();
    let mut edited = Project::demo();
    edited.name = "Edited".into();
    edited.bpm = 128.0;
    for _ in 0..3 {
        save_project(&edited, &path).unwrap();
    }
    assert_eq!(load_project(&path).unwrap(), edited);
    assert_eq!(dir_entries(dir.path()), vec!["song.cadence".to_string()]);
}

#[test]
fn invalid_project_is_never_written_and_original_survives() {
    let dir = tempfile::tempdir().unwrap();
    let path = song_path(dir.path());
    save_project(&Project::demo(), &path).unwrap();
    let before = fs::read(&path).unwrap();

    for mutate in [
        (|p: &mut Project| p.bpm = 1000.0) as fn(&mut Project),
        |p| p.bpm = f64::NAN,
        |p| p.swing = 5.0,
        |p| p.bars = 0,
        |p| p.master_volume = f32::INFINITY,
        |p| {
            let track = p.tracks[0].clone();
            p.tracks = vec![track; 33];
        },
    ] {
        let mut bad = Project::demo();
        mutate(&mut bad);
        assert!(
            save_project(&bad, &path).is_err(),
            "invalid project was saved"
        );
        assert!(project_to_json(&bad).is_err());
    }
    assert_eq!(fs::read(&path).unwrap(), before, "original was modified");
    assert_eq!(dir_entries(dir.path()), vec!["song.cadence".to_string()]);
}

#[test]
fn save_into_missing_folder_or_onto_folder_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope").join("song.cadence");
    assert!(save_project(&Project::demo(), &missing).is_err());
    assert!(!missing.exists());

    let folder = dir.path().join("folder.cadence");
    fs::create_dir(&folder).unwrap();
    assert!(save_project(&Project::demo(), &folder).is_err());
    assert!(folder.is_dir());
}

#[test]
fn read_only_original_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let path = song_path(dir.path());
    save_project(&Project::demo(), &path).unwrap();
    let before = fs::read(&path).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&path, perms.clone()).unwrap();

    let mut edited = Project::demo();
    edited.name = "Should not land".into();
    assert!(save_project(&edited, &path).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(dir_entries(dir.path()), vec!["song.cadence".to_string()]);

    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&path, perms).unwrap();
}

#[test]
fn damaged_files_are_rejected_with_messages() {
    let dir = tempfile::tempdir().unwrap();
    let good = project_to_json(&Project::demo()).unwrap();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        ("garbage", b"not json at all".to_vec()),
        ("truncated", good.as_bytes()[..good.len() / 2].to_vec()),
        ("binary", vec![0xff, 0xfe, 0x00, 0x81, 0x7b]),
        ("wrong shape", br#"{"hello": "world"}"#.to_vec()),
        ("array", b"[1, 2, 3]".to_vec()),
        ("trailing junk", format!("{good} }}").into_bytes()),
    ];
    for (name, bytes) in cases {
        let path = dir.path().join(format!("{name}.cadence"));
        fs::write(&path, &bytes).unwrap();
        let err = load_project(&path).expect_err(name);
        assert!(!err.is_empty(), "{name}: empty error message");
        assert_eq!(
            fs::read(&path).unwrap(),
            bytes,
            "{name}: load modified the file"
        );
    }
}

#[test]
fn out_of_range_values_in_json_fail_validation() {
    let mut value: serde_json::Value =
        serde_json::from_str(&project_to_json(&Project::demo()).unwrap()).unwrap();
    value["bpm"] = serde_json::json!(5.0);
    assert!(project_from_json(&value.to_string()).is_err());

    let mut value: serde_json::Value =
        serde_json::from_str(&project_to_json(&Project::demo()).unwrap()).unwrap();
    value["tracks"][0]["volume"] = serde_json::json!(7.5);
    assert!(project_from_json(&value.to_string()).is_err());
}

#[test]
fn utf8_bom_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let path = song_path(dir.path());
    let demo = Project::demo();
    let mut bytes = "\u{feff}".as_bytes().to_vec();
    bytes.extend(project_to_json(&demo).unwrap().into_bytes());
    fs::write(&path, bytes).unwrap();
    assert_eq!(load_project(&path).unwrap(), demo);
}

#[test]
fn size_cap_is_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.cadence");
    let file = fs::File::create(&path).unwrap();
    file.set_len(MAX_PROJECT_BYTES + 1).unwrap();
    drop(file);
    let err = load_project(&path).unwrap_err();
    assert!(err.contains("too large"), "{err}");
}

#[test]
fn missing_file_and_directory_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load_project(&dir.path().join("missing.cadence")).is_err());
    assert!(load_project(dir.path()).is_err());
}

#[test]
fn extension_helper() {
    assert_eq!(
        with_project_extension(Path::new("/tmp/My Song")),
        PathBuf::from("/tmp/My Song.cadence")
    );
    assert_eq!(
        with_project_extension(Path::new("x.cadence")),
        PathBuf::from("x.cadence")
    );
}
