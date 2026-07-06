use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub poll_interval_ms: u64,
    pub opacity: f32,
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    pub hotkey: String,
    pub enabled_agents: Vec<String>,
    pub hermes_data_dir: Option<PathBuf>,
    /// When `true` (default), Claude's 5h/weekly usage is fetched directly
    /// from Anthropic's usage API using the OAuth token already stored by
    /// Claude Code — the one network call this app makes. Set `false` to
    /// keep the app strictly local-only; Claude usage then only populates
    /// via the `abtop-rate-limits.json` hook file, if configured.
    #[serde(default = "default_claude_usage_enabled")]
    pub claude_usage_enabled: bool,
    #[serde(default = "default_history_enabled")]
    pub history_enabled: bool,
    #[serde(default = "default_history_retention_days")]
    pub history_retention_days: u32,
    /// Seconds a session may stay in `Thinking` or `Executing` before it's
    /// flagged `stalled` and (once) notified about. `0` disables the feature.
    #[serde(default = "default_stall_alert_secs")]
    pub stall_alert_secs: u64,
    /// When true, render each session row in a compact form that hides most
    /// per-token meta spans. Defaults to false (current expanded behavior).
    #[serde(default)]
    pub compact_view: bool,
    /// UI theme name. "dark" or "light"; other values fall back to dark on
    /// the frontend. Defaults to "dark".
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Hex accent color applied to the CSS --accent variable. Defaults to
    /// the dark-theme blue.
    #[serde(default = "default_accent_color")]
    pub accent_color: String,
}

fn default_claude_usage_enabled() -> bool {
    true
}

fn default_history_enabled() -> bool {
    true
}

fn default_history_retention_days() -> u32 {
    30
}

fn default_stall_alert_secs() -> u64 {
    180
}

fn default_theme() -> String {
    "dark".to_string()
}

fn default_accent_color() -> String {
    "#6aa0ff".to_string()
}

pub fn default_config() -> Config {
    Config {
        poll_interval_ms: 1000,
        opacity: 1.0,
        window_x: None,
        window_y: None,
        hotkey: "Ctrl+Shift+Space".to_string(),
        enabled_agents: vec!["claude".into(), "codex".into(), "hermes".into()],
        hermes_data_dir: None,
        claude_usage_enabled: true,
        history_enabled: true,
        history_retention_days: 30,
        stall_alert_secs: 180,
        compact_view: false,
        theme: "dark".to_string(),
        accent_color: "#6aa0ff".to_string(),
    }
}

pub fn load_config(path: &Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str::<Config>(&text).unwrap_or_else(|_| default_config()),
        Err(_) => default_config(),
    }
}

pub fn save_config(path: &Path, cfg: &Config) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(path, text)
}
