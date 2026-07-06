use crate::model::RateLimitInfo;
use serde::Deserialize;
use std::path::Path;

/// Claude Code itself doesn't expose 5h/weekly usage in the transcript, so
/// account-level limits are read from a small JSON file a user-configured
/// StatusLine hook is expected to write. Filename and schema match abtop's
/// (`five_hour`/`seven_day` with `used_percentage`/`resets_at`) so a hook
/// already set up for abtop works here too.
pub const CLAUDE_RATE_FILE: &str = "abtop-rate-limits.json";

#[derive(Debug, Deserialize)]
struct RateLimitFile {
    #[serde(default)]
    source: String,
    #[serde(default)]
    five_hour: Option<WindowInfo>,
    #[serde(default)]
    seven_day: Option<WindowInfo>,
    #[serde(default)]
    updated_at: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct WindowInfo {
    #[serde(default)]
    used_percentage: f64,
    #[serde(default)]
    resets_at: u64,
}

/// Reads and parses a rate-limit file. Returns `None` on any I/O or parse
/// error, or if both windows are absent (a malformed/placeholder file).
pub fn read_rate_limit_file(path: &Path, default_source: &str) -> Option<RateLimitInfo> {
    let content = std::fs::read_to_string(path).ok()?;
    let file: RateLimitFile = serde_json::from_str(&content).ok()?;

    if file.five_hour.is_none() && file.seven_day.is_none() {
        return None;
    }

    let source = if file.source.is_empty() {
        default_source.to_string()
    } else {
        file.source
    };

    Some(RateLimitInfo {
        source,
        five_hour_pct: file.five_hour.as_ref().map(|w| w.used_percentage),
        five_hour_resets_at: file.five_hour.as_ref().map(|w| w.resets_at),
        seven_day_pct: file.seven_day.as_ref().map(|w| w.used_percentage),
        seven_day_resets_at: file.seven_day.as_ref().map(|w| w.resets_at),
        monthly_pct: None,
        monthly_resets_at: None,
        updated_at: file.updated_at,
        ..Default::default()
    })
}
