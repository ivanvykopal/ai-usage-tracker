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
