use std::fs;
use usage_tracker::config::{default_config, load_config, save_config};

#[test]
fn missing_file_returns_defaults() {
    let cfg = load_config(std::path::Path::new("/nonexistent/config.toml"));
    assert_eq!(cfg.poll_interval_ms, 1000);
    assert!((cfg.opacity - 0.85).abs() < 0.001);
    assert_eq!(cfg.hotkey, "Ctrl+Shift+Space");
    assert_eq!(cfg.enabled_agents, vec!["claude", "codex", "hermes"]);
}

#[test]
fn round_trip_preserves_values() {
    let dir = tempfile_dir();
    let path = dir.join("config.toml");
    let mut cfg = default_config();
    cfg.poll_interval_ms = 2000;
    cfg.opacity = 0.5;
    cfg.hermes_data_dir = Some(dir.join("hermes-data"));
    save_config(&path, &cfg).unwrap();
    let loaded = load_config(&path);
    assert_eq!(loaded.poll_interval_ms, 2000);
    assert!((loaded.opacity - 0.5).abs() < 0.001);
    assert_eq!(loaded.hermes_data_dir, Some(dir.join("hermes-data")));
}

#[test]
fn corrupt_file_falls_back_to_defaults() {
    let dir = tempfile_dir();
    let path = dir.join("config.toml");
    fs::write(&path, "this is = = not valid toml {{{").unwrap();
    let cfg = load_config(&path);
    assert_eq!(cfg.poll_interval_ms, 1000); // default
}

fn tempfile_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("utt-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    dir
}
