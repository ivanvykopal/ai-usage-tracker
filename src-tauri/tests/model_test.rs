use usage_tracker::model::{build_snapshot, AgentSession, RateLimitInfo, SessionStatus};
use std::collections::{BTreeMap, HashMap};

fn session(agent: &str, pid: u32, inp: u64, out: u64, status: SessionStatus) -> AgentSession {
    AgentSession {
        agent_cli: agent.into(), pid, session_id: format!("{agent}-{pid}"),
        cwd: "/proj".into(), project_name: "proj".into(), started_at: 0,
        status, model: "m".into(), context_percent: 0.0,
        total_input_tokens: inp, total_output_tokens: out, total_cache_read: 0,
        total_cache_create: 0,
        turn_count: 0, current_task: String::new(), mem_mb: 0,
    }
}

#[test]
fn snapshot_aggregates_tokens_by_agent() {
    let sessions = vec![
        session("claude", 1, 100, 10, SessionStatus::Executing),
        session("claude", 2, 200, 20, SessionStatus::Waiting),
        session("codex", 3, 50, 5, SessionStatus::Thinking),
    ];
    let snap = build_snapshot(sessions, BTreeMap::new());
    assert_eq!(snap.total_tokens, 100 + 10 + 200 + 20 + 50 + 5);
    let mut want = HashMap::new();
    want.insert("claude".to_string(), 100 + 10 + 200 + 20);
    want.insert("codex".to_string(), 50 + 5);
    assert_eq!(snap.by_agent_tokens, want);
}

#[test]
fn snapshot_counts_by_status() {
    let sessions = vec![
        session("claude", 1, 0, 0, SessionStatus::Executing),
        session("claude", 2, 0, 0, SessionStatus::Executing),
        session("codex", 3, 0, 0, SessionStatus::Waiting),
    ];
    let snap = build_snapshot(sessions, BTreeMap::new());
    assert_eq!(snap.by_status.get(&SessionStatus::Executing), Some(&2));
    assert_eq!(snap.by_status.get(&SessionStatus::Waiting), Some(&1));
    assert_eq!(snap.by_status.get(&SessionStatus::Thinking), Some(&0));
}

#[test]
fn snapshot_serializes_to_json() {
    let snap = build_snapshot(vec![session("claude", 1, 1, 1, SessionStatus::Waiting)], BTreeMap::new());
    let json = serde_json::to_string(&snap).unwrap();
    assert!(json.contains("\"agent_cli\":\"claude\""));
    assert!(json.contains("\"total_tokens\":2"));
}

#[test]
fn snapshot_carries_usage_limits_map() {
    let mut limits = BTreeMap::new();
    limits.insert(
        "claude".to_string(),
        RateLimitInfo {
            source: "claude".into(),
            five_hour_pct: Some(42.0),
            five_hour_resets_at: Some(1_700_000_000),
            seven_day_pct: None,
            seven_day_resets_at: None,
            updated_at: Some(1_700_000_000),
        },
    );
    let snap = build_snapshot(Vec::new(), limits.clone());
    assert_eq!(snap.usage_limits, limits);
}
