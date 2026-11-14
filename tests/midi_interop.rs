//! MIDI exchange integrated with persistence: exported files re-import into
//! valid, saveable projects, and malformed input is rejected cleanly.

use cadence_studio::midi::{export_midi, import_midi};
use cadence_studio::model::Project;
use cadence_studio::persistence::{load_project, save_project};

#[test]
fn demo_midi_round_trip_is_a_valid_saveable_project() {
    let demo = Project::demo();
    let bytes = export_midi(&demo).unwrap();
    assert_eq!(&bytes[0..4], b"MThd");
    assert!(bytes.windows(4).filter(|w| *w == b"MTrk").count() >= 2);

    let imported = import_midi(&bytes).unwrap();
    imported.validate().unwrap();
    assert!(
        (imported.bpm - demo.bpm).abs() < 0.5,
        "tempo lost: {}",
        imported.bpm
    );
    assert!(!imported.tracks.is_empty());
    let original_notes = demo.events(false).len();
    let imported_notes = imported.events(false).len();
    assert!(
        imported_notes * 10 >= original_notes * 9,
        "lost notes: {imported_notes} of {original_notes}"
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("imported.cadence");
    save_project(&imported, &path).unwrap();
    assert_eq!(load_project(&path).unwrap(), imported);
}

#[test]
fn malformed_midi_is_rejected() {
    let cases: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"MThd".to_vec(),
        b"RIFF\0\0\0\0WAVE".to_vec(),
        vec![0u8; 64],
    ];
    for bytes in cases {
        assert!(
            import_midi(&bytes).is_err(),
            "accepted {} bytes of junk",
            bytes.len()
        );
    }
}
