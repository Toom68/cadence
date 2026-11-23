//! Open Recent persists in the process-wide data directory, so the whole
//! check runs as one test in its own binary.

use std::fs;

use cadence_studio::persistence::{
    data_dir, load_recent_files, push_recent_file, save_recent_files, DATA_DIR_ENV,
    MAX_RECENT_FILES,
};

#[test]
fn recent_files_persist_in_the_data_dir() {
    let data = tempfile::tempdir().unwrap();
    std::env::set_var(DATA_DIR_ENV, data.path().join("app"));
    assert!(data_dir().starts_with(data.path()));
    assert!(load_recent_files().is_empty());

    let songs = tempfile::tempdir().unwrap();
    let mut list = Vec::new();
    for i in 0..(MAX_RECENT_FILES + 3) {
        let path = songs.path().join(format!("song {i}.cadence"));
        fs::write(&path, "{}").unwrap();
        push_recent_file(&mut list, &path);
    }
    // The data folder is created on the first save.
    save_recent_files(&list).unwrap();
    let loaded = load_recent_files();
    assert_eq!(loaded.len(), MAX_RECENT_FILES);
    assert_eq!(loaded, list);
    assert!(loaded[0].ends_with(format!("song {}.cadence", MAX_RECENT_FILES + 2)));
    assert!(data.path().join("app").join("recent-files.txt").is_file());

    save_recent_files(&[]).unwrap();
    assert!(load_recent_files().is_empty());
}
