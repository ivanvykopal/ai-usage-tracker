use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use chrono::Local;
use usage_tracker::codex::CodexCollector;
use usage_tracker::collector::{Collector, ProcessContext};

fn build_fake_codex_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("utt-codex-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let now = Local::now();
    let day_dir = root
        .join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    fs::create_dir_all(&day_dir).unwrap();
    fs::write(
        day_dir.join("rollout-codex-1.jsonl"),
        include_str!("fixtures/codex_rollout.jsonl"),
    )
    .unwrap();
    root
}

#[test]
fn collects_recent_codex_rollout_with_tokens_and_project() {
    let root = build_fake_codex_root();
    let procs = HashMap::new();
    let kids = HashMap::new();
    let ports = HashMap::new();
    let ctx = ProcessContext {
        procs: &procs,
        children: &kids,
        ports: &ports,
        wsl: &HashMap::new(),
    };

    let mut c = CodexCollector::new(root.join("sessions"));
    let sessions = c.collect(&ctx);
    assert_eq!(sessions.len(), 1, "exactly one recent rollout expected");
    let s = &sessions[0];
    assert_eq!(s.agent_cli, "codex");
    assert_eq!(s.session_id, "codex-1");
    assert_eq!(s.project_name, "webapp");
    assert_eq!(s.model, "gpt-5-codex");
    // Codex reports cumulative totals per token_count event (not deltas), so
    // the last event wins: input 3000 - cached 500 = 2500, output 120.
    assert_eq!(s.total_input_tokens, 2500);
    assert_eq!(s.total_output_tokens, 120);
    assert_eq!(s.total_cache_read, 500);
    // task_complete fired last, and the one function_call/-_output pair closed.
    assert_eq!(s.status, usage_tracker::model::SessionStatus::Done);
}

#[test]
fn pending_function_call_marks_session_executing() {
    let root = std::env::temp_dir().join(format!("utt-codex-pending-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let now = Local::now();
    let day_dir = root
        .join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    fs::create_dir_all(&day_dir).unwrap();
    fs::write(
        day_dir.join("rollout-codex-2.jsonl"),
        concat!(
            "{\"type\":\"session_meta\",\"session_id\":\"codex-2\",\"cwd\":\"/webapp\",\"model\":\"gpt-5-codex\"}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"build it\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"call_id\":\"call-1\",\"name\":\"shell\",\"arguments\":\"{}\"}}\n",
        ),
    )
    .unwrap();

    let procs = HashMap::new();
    let kids = HashMap::new();
    let ports = HashMap::new();
    let ctx = ProcessContext { procs: &procs, children: &kids, ports: &ports, wsl: &HashMap::new() };
    let mut c = CodexCollector::new(root.join("sessions"));
    let sessions = c.collect(&ctx);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].status, usage_tracker::model::SessionStatus::Executing);
    assert_eq!(sessions[0].current_task, "shell");
}

#[test]
fn usage_limits_survive_after_rollout_goes_stale() {
    let root = std::env::temp_dir().join(format!("utt-codex-usage-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let now = Local::now();
    let day_dir = root
        .join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    fs::create_dir_all(&day_dir).unwrap();
    let rollout_path = day_dir.join("rollout-codex-3.jsonl");
    fs::write(
        &rollout_path,
        concat!(
            "{\"type\":\"session_meta\",\"session_id\":\"codex-3\",\"cwd\":\"/webapp\",\"model\":\"gpt-5-codex\"}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"output_tokens\":10,\"cached_input_tokens\":0},\"last_token_usage\":{\"input_tokens\":100},\"model_context_window\":400000},\"rate_limits\":{\"primary\":{\"window_minutes\":300,\"used_percent\":12.0,\"resets_at\":1700003600},\"secondary\":{\"window_minutes\":10080,\"used_percent\":5.0,\"resets_at\":1700600000}}}}\n",
        ),
    )
    .unwrap();

    let procs = HashMap::new();
    let kids = HashMap::new();
    let ports = HashMap::new();
    let ctx = ProcessContext { procs: &procs, children: &kids, ports: &ports, wsl: &HashMap::new() };
    let mut c = CodexCollector::new(root.join("sessions"));

    // First tick: rollout is fresh, session present, usage limits populated.
    let sessions = c.collect(&ctx);
    assert_eq!(sessions.len(), 1);
    let rl = c.usage_limits().expect("usage limits after first tick");
    assert_eq!(rl.five_hour_pct, Some(12.0));
    assert_eq!(rl.seven_day_pct, Some(5.0));

    // Make the rollout look stale (older than RECENT_AGE_SECS) so the next
    // collect() drops the session entirely.
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
    let old_ft = filetime::FileTime::from_system_time(old);
    filetime::set_file_mtime(&rollout_path, old_ft).unwrap();

    let sessions = c.collect(&ctx);
    assert!(sessions.is_empty(), "stale rollout must drop the session");
    let rl_after = c.usage_limits().expect("usage limits must survive session ending");
    assert_eq!(rl_after.five_hour_pct, Some(12.0));
}

#[test]
fn usage_limits_populated_from_recent_rollout_even_when_not_treated_as_active_session() {
    // A rollout aged just past RECENT_AGE_SECS (so it's not surfaced as a
    // live session) should still seed usage_limits on the very first tick —
    // this covers "app just launched, no session started yet today."
    let root = std::env::temp_dir().join(format!("utt-codex-usage-scan-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let now = Local::now();
    let day_dir = root
        .join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    fs::create_dir_all(&day_dir).unwrap();
    let rollout_path = day_dir.join("rollout-codex-4.jsonl");
    fs::write(
        &rollout_path,
        concat!(
            "{\"type\":\"session_meta\",\"session_id\":\"codex-4\",\"cwd\":\"/webapp\",\"model\":\"gpt-5-codex\"}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"output_tokens\":10,\"cached_input_tokens\":0},\"last_token_usage\":{\"input_tokens\":100},\"model_context_window\":400000},\"rate_limits\":{\"primary\":{\"window_minutes\":300,\"used_percent\":33.0,\"resets_at\":1700003600}}}}\n",
        ),
    )
    .unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
    let old_ft = filetime::FileTime::from_system_time(old);
    filetime::set_file_mtime(&rollout_path, old_ft).unwrap();

    let procs = HashMap::new();
    let kids = HashMap::new();
    let ports = HashMap::new();
    let ctx = ProcessContext { procs: &procs, children: &kids, ports: &ports, wsl: &HashMap::new() };
    let mut c = CodexCollector::new(root.join("sessions"));

    let sessions = c.collect(&ctx);
    assert!(sessions.is_empty(), "stale rollout is not a live session");
    let rl = c
        .usage_limits()
        .expect("usage limits should be seeded from the day's most recent rollout even without a live session");
    assert_eq!(rl.five_hour_pct, Some(33.0));
}

#[test]
fn monthly_only_rate_limit_is_not_misclassified_as_weekly() {
    // Codex's free plan reports a single ~30-day window as `primary` with no
    // `secondary` at all — this must land in `monthly_pct`, not `seven_day_pct`.
    let root = std::env::temp_dir().join(format!("utt-codex-monthly-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let now = Local::now();
    let day_dir = root
        .join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    fs::create_dir_all(&day_dir).unwrap();
    fs::write(
        day_dir.join("rollout-codex-5.jsonl"),
        concat!(
            "{\"type\":\"session_meta\",\"session_id\":\"codex-5\",\"cwd\":\"/webapp\",\"model\":\"gpt-5-codex\"}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"output_tokens\":10,\"cached_input_tokens\":0},\"last_token_usage\":{\"input_tokens\":100},\"model_context_window\":400000},\"rate_limits\":{\"limit_id\":\"codex\",\"primary\":{\"used_percent\":38.0,\"window_minutes\":43200,\"resets_at\":1783509651},\"secondary\":null}}}\n",
        ),
    )
    .unwrap();

    let procs = HashMap::new();
    let kids = HashMap::new();
    let ports = HashMap::new();
    let ctx = ProcessContext { procs: &procs, children: &kids, ports: &ports, wsl: &HashMap::new() };
    let mut c = CodexCollector::new(root.join("sessions"));

    let sessions = c.collect(&ctx);
    assert_eq!(sessions.len(), 1);
    let rl = c.usage_limits().expect("usage limits should be populated");
    assert_eq!(rl.monthly_pct, Some(38.0));
    assert_eq!(rl.monthly_resets_at, Some(1783509651));
    assert_eq!(rl.five_hour_pct, None, "monthly window must not be misclassified as five_hour");
    assert_eq!(rl.seven_day_pct, None, "monthly window must not be misclassified as seven_day");
}
