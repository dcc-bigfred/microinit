//! Tests for config path filtering (inotify event relevance).

use std::path::Path;

use microinit::config_watch::is_relevant_path;

#[test]
fn relevant_json_and_known_names() {
    assert!(is_relevant_path(Path::new("/data/etc/microinit.json")));
    assert!(is_relevant_path(Path::new(
        "/data/etc/microinit.services.enabled-override.json"
    )));
    assert!(is_relevant_path(Path::new("/data/etc/microinit.d")));
    assert!(is_relevant_path(Path::new(
        "/data/etc/microinit.d/services/a/x.json"
    )));
}

#[test]
fn ignores_editor_temps_and_non_json() {
    assert!(!is_relevant_path(Path::new("/data/etc/microinit.json.tmp")));
    assert!(!is_relevant_path(Path::new(
        "/data/etc/.microinit.json.swp"
    )));
    assert!(!is_relevant_path(Path::new("/data/etc/microinit.json~")));
    assert!(!is_relevant_path(Path::new("/data/etc/readme.txt")));
}
