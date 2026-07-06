use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Waiting,
    Thinking,
    Executing,
    Done,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    pub agent_cli: String,
    pub pid: u32,
    pub session_id: String,
    pub cwd: String,
    pub project_name: String,
    pub started_at: i64,
    pub status: SessionStatus,
    pub model: String,
    pub context_percent: f64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read: u64,
    pub total_cache_create: u64,
    pub turn_count: u32,
    pub current_task: String,
    pub mem_mb: u64,
    /// USD cost estimate for this session's tokens, or `None` if the model
    /// isn't in `pricing::TABLE`. Populated by `App::tick` after `collect()`,
    /// not by individual collectors — see `pricing::estimate_cost_usd`.
    pub cost_usd: Option<f64>,
}

/// Account-level usage-limit windows, ported from abtop's `RateLimitInfo`.
/// Account-level (not session-level), so these live on `Snapshot.usage_limits`
/// rather than on individual sessions. For Claude this comes from Anthropic's
/// usage API (with a hook-file fallback); for Codex it's parsed live from
/// `token_count` events.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RateLimitInfo {
    pub source: String,
    pub five_hour_pct: Option<f64>,
    pub five_hour_resets_at: Option<u64>,
    pub seven_day_pct: Option<f64>,
    pub seven_day_resets_at: Option<u64>,
    /// Monthly quota window. Codex's free plan reports only this window
    /// (`window_minutes` ~43200 / 30 days, no five-hour or weekly window at
    /// all) — paid plans may report five-hour/weekly instead or as well.
    pub monthly_pct: Option<f64>,
    pub monthly_resets_at: Option<u64>,
    pub updated_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub sessions: Vec<AgentSession>,
    pub total_tokens: u64,
    pub by_agent_tokens: HashMap<String, u64>,
    pub by_status: HashMap<SessionStatus, u32>,
    pub usage_limits: BTreeMap<String, RateLimitInfo>,
    pub total_cost_usd: f64,
    pub by_agent_cost_usd: HashMap<String, f64>,
}

pub fn build_snapshot(
    sessions: Vec<AgentSession>,
    usage_limits: BTreeMap<String, RateLimitInfo>,
) -> Snapshot {
    let mut total_tokens: u64 = 0;
    let mut by_agent_tokens: HashMap<String, u64> = HashMap::new();
    let mut by_status: HashMap<SessionStatus, u32> = HashMap::new();
    let mut total_cost_usd: f64 = 0.0;
    let mut by_agent_cost_usd: HashMap<String, f64> = HashMap::new();
    // Pre-populate all statuses to ensure every variant has an entry
    by_status.insert(SessionStatus::Waiting, 0);
    by_status.insert(SessionStatus::Thinking, 0);
    by_status.insert(SessionStatus::Executing, 0);
    by_status.insert(SessionStatus::Done, 0);
    by_status.insert(SessionStatus::Unknown, 0);
    for s in &sessions {
        let t = s.total_input_tokens + s.total_output_tokens;
        total_tokens += t;
        *by_agent_tokens.entry(s.agent_cli.clone()).or_insert(0) += t;
        *by_status.entry(s.status).or_insert(0) += 1;
        if let Some(cost) = s.cost_usd {
            total_cost_usd += cost;
            *by_agent_cost_usd.entry(s.agent_cli.clone()).or_insert(0.0) += cost;
        }
    }
    Snapshot {
        sessions,
        total_tokens,
        by_agent_tokens,
        by_status,
        usage_limits,
        total_cost_usd,
        by_agent_cost_usd,
    }
}
