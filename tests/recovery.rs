//! Recovery uses a process-wide data directory, so the whole lifecycle runs in
//! one test (this file is its own test binary/process).

use std::fs;

use cadence_studio::model::Project;
use cadence_studio::persistence::{
    clear_recovery, data_dir, load_recovery, recovery_path, save_recovery, DATA_DIR_ENV,
};

#[test]
fn recovery_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var(DATA_DIR_ENV, dir.path());
    assert_eq!(data_dir(), dir.path());
    let path = recovery_path();
    assert!(path.starts_with(dir.path()));
    assert_eq!(path.extension().unwrap(), "cadence");

    // Nothing saved yet: no recovery offer, no error.
    assert_eq!(load_recovery().unwrap(), None);
    clear_recovery().unwrap();

    // Save creates the folder; load returns the exact project.
    let mut project = Project::demo();
    project.name = "Unsaved work".into();
    save_recovery(&project).unwrap();
    assert_eq!(load_recovery().unwrap(), Some(project.clone()));

    // Autosave overwrites in place without leaving temp files.
    project.bpm = 140.0;
    save_recovery(&project).unwrap();
    assert_eq!(load_recovery().unwrap(), Some(project.clone()));
    let names: Vec<_> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "unexpected files: {names:?}");

    // Invalid projects are not autosaved and do not clobber the good autosave.
    let mut bad = project.clone();
    bad.bars = 0;
    assert!(save_recovery(&bad).is_err());
    assert_eq!(load_recovery().unwrap(), Some(project.clone()));

    // A damaged autosave is never loaded, is moved aside, and is not offered again.
    fs::write(&path, b"{\"version\": 1, \"tracks\": [").unwrap();
    let err = load_recovery().unwrap_err();
    assert!(err.contains("damaged"), "{err}");
    assert!(!path.exists());
    assert!(path.with_file_name("autosave.damaged.cadence").exists());
    assert_eq!(load_recovery().unwrap(), None);

    // Clearing removes the autosave.
    save_recovery(&project).unwrap();
    assert!(path.exists());
    clear_recovery().unwrap();
    assert!(!path.exists());
    assert_eq!(load_recovery().unwrap(), None);

    std::env::remove_var(DATA_DIR_ENV);
}
