use std::fs;
use usage_tracker::config::{default_config, load_config, save_config};

#[test]
fn missing_file_returns_defaults() {
    let cfg = load_config(std::path::Path::new("/nonexistent/config.toml"));
    assert_eq!(cfg.poll_interval_ms, 1000);
    assert!((cfg.opacity - 1.0).abs() < 0.001);
    assert_eq!(cfg.hotkey, "Ctrl+Shift+Space");
    assert_eq!(cfg.enabled_agents, vec!["claude", "codex", "hermes"]);
}

#[test]
fn round_trip_preserves_values() {
    let dir = tempfile_dir("round_trip");
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
    let dir = tempfile_dir("corrupt_file");
    let path = dir.join("config.toml");
    fs::write(&path, "this is = = not valid toml {{{").unwrap();
    let cfg = load_config(&path);
    assert_eq!(cfg.poll_interval_ms, 1000); // default
}

fn tempfile_dir(test_name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("utt-{}-{}", std::process::id(), test_name));
    let _ = fs::create_dir_all(&dir);
    dir
}

#[test]
fn claude_usage_enabled_defaults_true() {
    let cfg = load_config(std::path::Path::new("/nonexistent/config.toml"));
    assert!(cfg.claude_usage_enabled);
}

#[test]
fn claude_usage_enabled_can_be_disabled_and_round_trips() {
    let dir = tempfile_dir("claude_usage_disabled");
    let path = dir.join("config.toml");
    let mut cfg = default_config();
    cfg.claude_usage_enabled = false;
    save_config(&path, &cfg).unwrap();
    let loaded = load_config(&path);
    assert!(!loaded.claude_usage_enabled);
}

#[test]
fn old_config_file_without_claude_usage_enabled_defaults_true() {
    let dir = tempfile_dir("legacy_config");
    let path = dir.join("config.toml");
    fs::write(
        &path,
        r#"
poll_interval_ms = 1000
opacity = 1.0
hotkey = "Ctrl+Shift+Space"
enabled_agents = ["claude", "codex"]
"#,
    )
    .unwrap();
    let cfg = load_config(&path);
    assert!(cfg.claude_usage_enabled, "missing field in an old config file must default to true, not fail to parse");
}
