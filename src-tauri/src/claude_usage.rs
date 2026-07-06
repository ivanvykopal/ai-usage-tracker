use crate::model::RateLimitInfo;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// `~/.claude/.credentials.json`'s shape — only the fields this module needs.
#[derive(Debug, Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OauthSection>,
}

#[derive(Debug, Deserialize)]
struct OauthSection {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
}

/// Reads the OAuth access token Claude Code itself stores after login.
/// Returns `None` if the file is missing, malformed, or the account isn't
/// using OAuth (e.g. Bedrock/Vertex/API-key auth) — callers should fall back
/// to the hook-file mechanism in that case.
pub fn read_access_token(credentials_path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(credentials_path).ok()?;
    let file: CredentialsFile = serde_json::from_str(&text).ok()?;
    file.claude_ai_oauth?.access_token
}

#[derive(Debug, Deserialize)]
struct UsageBucket {
    #[serde(default)]
    utilization: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageApiResponse {
    #[serde(default)]
    five_hour: Option<UsageBucket>,
    #[serde(default)]
    seven_day: Option<UsageBucket>,
}

fn parse_resets_at(s: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp() as u64)
}

/// Parses Anthropic's `GET /api/oauth/usage` response body into a
/// `RateLimitInfo`. Returns `None` if both `five_hour` and `seven_day` are
/// absent (malformed/unexpected response shape) — same "both windows absent
/// means don't trust this" rule the hook-file parser in `rate_limit.rs` uses.
pub fn parse_usage_response(body: &str) -> Option<RateLimitInfo> {
    let parsed: UsageApiResponse = serde_json::from_str(body).ok()?;
    if parsed.five_hour.is_none() && parsed.seven_day.is_none() {
        return None;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(RateLimitInfo {
        source: "claude".to_string(),
        five_hour_pct: parsed.five_hour.as_ref().and_then(|b| b.utilization),
        five_hour_resets_at: parsed
            .five_hour
            .as_ref()
            .and_then(|b| b.resets_at.as_deref())
            .and_then(parse_resets_at),
        seven_day_pct: parsed.seven_day.as_ref().and_then(|b| b.utilization),
        seven_day_resets_at: parsed
            .seven_day
            .as_ref()
            .and_then(|b| b.resets_at.as_deref())
            .and_then(parse_resets_at),
        monthly_pct: None,
        ..Default::default()
        monthly_resets_at: None,
        updated_at: Some(now),
    })
}

const USAGE_API_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const POLL_INTERVAL: Duration = Duration::from_secs(180);
const ERROR_BACKOFF: Duration = Duration::from_secs(30);
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(300);

/// Parses an HTTP `Retry-After` header value (seconds form only — Anthropic's
/// API doesn't use the HTTP-date form) into a backoff duration, falling back
/// to `RATE_LIMIT_BACKOFF` when absent or malformed.
fn parse_retry_after(header: Option<&str>) -> Duration {
    header
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(RATE_LIMIT_BACKOFF)
}

/// Calls Anthropic's usage API once. Returns `Err(retry_after)` on failure —
/// the caller decides how long to wait before the next attempt.
fn fetch_once(token: &str) -> Result<RateLimitInfo, Duration> {
    let response = ureq::get(USAGE_API_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .timeout(Duration::from_secs(5))
        .call();

    match response {
        Ok(resp) => {
            let body = resp.into_string().map_err(|_| ERROR_BACKOFF)?;
            parse_usage_response(&body).ok_or(ERROR_BACKOFF)
        }
        Err(ureq::Error::Status(429, resp)) => {
            Err(parse_retry_after(resp.header("Retry-After")))
        }
        Err(_) => Err(ERROR_BACKOFF),
    }
}

/// Owns the background thread that polls Anthropic's usage API and publishes
/// the latest known `RateLimitInfo` into a shared handle the Claude collector
/// reads each tick. Never blocks the 1s session tick loop.
pub struct ClaudeUsagePoller;

impl ClaudeUsagePoller {
    /// Spawns the polling thread and returns the shared handle immediately.
    /// `credentials_path` is `.claude/.credentials.json` under the home
    /// directory resolved by `lib.rs::resolve_home_dirs`.
    pub fn start(credentials_path: PathBuf) -> Arc<Mutex<Option<RateLimitInfo>>> {
        let handle = Arc::new(Mutex::new(None));
        let thread_handle = handle.clone();
        std::thread::spawn(move || loop {
            let wait = match read_access_token(&credentials_path) {
                Some(token) => match fetch_once(&token) {
                    Ok(rl) => {
                        if let Ok(mut guard) = thread_handle.lock() {
                            *guard = Some(rl);
                        }
                        POLL_INTERVAL
                    }
                    Err(backoff) => backoff,
                },
                // No OAuth token (e.g. Bedrock/Vertex/API-key auth) — nothing
                // to poll; the Claude collector's hook-file fallback covers
                // this case. Recheck periodically in case the user logs in.
                None => POLL_INTERVAL,
            };
            std::thread::sleep(wait);
        });
        handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn reads_access_token_from_credentials_file() {
        let dir = std::env::temp_dir().join(format!("utt-claude-usage-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(".credentials.json");
        std::fs::write(
            &path,
            r#"{"claudeAiOauth":{"accessToken":"sk-test-token","refreshToken":"r"}}"#,
        )
        .unwrap();
        assert_eq!(read_access_token(&path), Some("sk-test-token".to_string()));
    }

    #[test]
    fn missing_oauth_section_returns_none() {
        let dir = std::env::temp_dir().join(format!("utt-claude-usage-noauth-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(".credentials.json");
        std::fs::write(&path, r#"{"someOtherAuth":{}}"#).unwrap();
        assert_eq!(read_access_token(&path), None);
    }

    #[test]
    fn missing_file_returns_none() {
        assert_eq!(read_access_token(Path::new("/nonexistent/.credentials.json")), None);
    }

    #[test]
    fn parses_five_hour_and_seven_day_buckets() {
        let body = r#"{
            "five_hour": {"utilization": 42.0, "resets_at": "2024-11-15T12:00:00Z"},
            "seven_day": {"utilization": 18.5, "resets_at": "2024-11-20T00:00:00Z"}
        }"#;
        let rl = parse_usage_response(body).expect("should parse");
        assert_eq!(rl.source, "claude");
        assert_eq!(rl.five_hour_pct, Some(42.0));
        assert_eq!(rl.seven_day_pct, Some(18.5));
        assert!(rl.five_hour_resets_at.is_some());
        assert!(rl.seven_day_resets_at.is_some());
    }

    #[test]
    fn missing_buckets_returns_none() {
        assert!(parse_usage_response(r#"{"extra_usage": {"is_enabled": false}}"#).is_none());
    }

    #[test]
    fn malformed_json_returns_none() {
        assert!(parse_usage_response("not json").is_none());
    }

    #[test]
    fn retry_after_header_parses_to_seconds() {
        assert_eq!(parse_retry_after(Some("120")), Duration::from_secs(120));
    }

    #[test]
    fn missing_retry_after_uses_default_rate_limit_backoff() {
        assert_eq!(parse_retry_after(None), RATE_LIMIT_BACKOFF);
    }

    #[test]
    fn malformed_retry_after_uses_default_rate_limit_backoff() {
        assert_eq!(parse_retry_after(Some("not-a-number")), RATE_LIMIT_BACKOFF);
    }
}
