use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use usage_tracker::claude::{encode_cwd_path, ClaudeCollector};
use usage_tracker::collector::{Collector, ProcessContext};
use usage_tracker::process::ProcInfo;

fn build_fake_claude_root(test_name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("utt-claude-{}-{}", std::process::id(), test_name));
    // clean slate so re-runs don't see stale state
    let _ = fs::remove_dir_all(&root);
    let _ = fs::create_dir_all(root.join("sessions"));
    let enc = encode_cwd_path("/proj");
    let _ = fs::create_dir_all(root.join("projects").join(&enc));
    fs::write(
        root.join("sessions").join("4242.json"),
        include_str!("fixtures/claude_session.json"),
    )
    .unwrap();
    fs::write(
        root.join("projects").join(enc).join("abc-123.jsonl"),
        include_str!("fixtures/claude_transcript.jsonl"),
    )
    .unwrap();
    root
}

fn empty_ctx() -> ProcessContext<'static> {
    use std::sync::OnceLock;
    static EMPTY_PROCS: OnceLock<HashMap<u32, ProcInfo>> = OnceLock::new();
    static EMPTY_KIDS: OnceLock<HashMap<u32, Vec<u32>>> = OnceLock::new();
    static EMPTY_PORTS: OnceLock<HashMap<u32, Vec<u16>>> = OnceLock::new();
    let procs = EMPTY_PROCS.get_or_init(HashMap::new);
    let kids = EMPTY_KIDS.get_or_init(HashMap::new);
    let ports = EMPTY_PORTS.get_or_init(HashMap::new);
    static EMPTY_WSL: OnceLock<HashMap<String, usage_tracker::process::ProcessSnapshot>> = OnceLock::new();
    let wsl = EMPTY_WSL.get_or_init(HashMap::new);
    ProcessContext {
        procs,
        children: kids,
        ports,
        wsl,
    }
}

#[test]
fn encode_cwd_replaces_slash_and_colon() {
    assert_eq!(encode_cwd_path("/proj"), "-proj");
    assert_eq!(encode_cwd_path("C:\\Users\\me"), "C--Users-me");
}

#[test]
fn collects_session_with_accumulated_tokens_and_project_name() {
    let root = build_fake_claude_root("accumulated_tokens");
    // Pretend pid 4242 is alive as a 'claude' process.
    let mut procs = HashMap::new();
    procs.insert(
        4242,
        ProcInfo {
            pid: 4242,
            command: "claude".into(),
            rss_kb: 50_000,
            cpu: 0.0,
            parent_pid: None,
        },
    );
    let kids = HashMap::new();
    let ports = HashMap::new();
    let ctx = ProcessContext {
        procs: &procs,
        children: &kids,
        ports: &ports,
        wsl: &HashMap::new(),
    };

    let mut c = ClaudeCollector::new(root);
    let sessions = c.collect(&ctx);
    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!(s.agent_cli, "claude");
    assert_eq!(s.pid, 4242);
    assert_eq!(s.session_id, "abc-123");
    assert_eq!(s.project_name, "proj");
    assert_eq!(s.model, "claude-sonnet-4-6");
    // input accumulates: 1200 + 7000 = 8200 ; output: 80 + 150 = 230 ; cache_read: 5000 + 5000 = 10000
    assert_eq!(s.total_input_tokens, 8200);
    assert_eq!(s.total_output_tokens, 230);
    assert_eq!(s.total_cache_read, 10000);
    assert_eq!(s.total_cache_create, 300);
}

#[test]
fn synthetic_user_messages_do_not_pin_thinking_status() {
    // The transcript's last line is a synthetic tool_result wrapper; if it
    // were mistaken for a real prompt, the session would read as perpetually
    // "Thinking" even with an alive, idle process and no pending tool call.
    let root = build_fake_claude_root("synthetic_user_messages");
    let mut procs = HashMap::new();
    procs.insert(
        4242,
        ProcInfo {
            pid: 4242,
            command: "claude".into(),
            rss_kb: 50_000,
            cpu: 0.0,
            parent_pid: None,
        },
    );
    let kids = HashMap::new();
    let ports = HashMap::new();
    let ctx = ProcessContext {
        procs: &procs,
        children: &kids,
        ports: &ports,
        wsl: &HashMap::new(),
    };

    let mut c = ClaudeCollector::new(root);
    let sessions = c.collect(&ctx);
    assert_eq!(sessions[0].status, usage_tracker::model::SessionStatus::Waiting);
}

#[test]
fn dead_pid_session_is_dropped() {
    let root = build_fake_claude_root("dead_pid");
    let ctx = empty_ctx(); // no procs → pid 4242 not alive
    let mut c = ClaudeCollector::new(root);
    let sessions = c.collect(&ctx);
    assert!(
        sessions.is_empty(),
        "session whose pid is not alive must be dropped"
    );
}

use usage_tracker::claude::ClaudeUsageSource;
use usage_tracker::model::RateLimitInfo;

#[test]
fn falls_back_to_hook_file_when_api_poller_has_no_data() {
    let root = build_fake_claude_root("hook_fallback");
    fs::write(
        root.join("abtop-rate-limits.json"),
        r#"{"five_hour":{"used_percentage":55.0,"resets_at":1700000000},"seven_day":{"used_percentage":10.0,"resets_at":1700500000}}"#,
    )
    .unwrap();
    let mut procs = HashMap::new();
    procs.insert(
        4242,
        ProcInfo { pid: 4242, command: "claude".into(), rss_kb: 50_000, cpu: 0.0, parent_pid: None },
    );
    let ctx = ProcessContext { procs: &procs, children: &HashMap::new(), ports: &HashMap::new(), wsl: &HashMap::new() };

    let usage_source = ClaudeUsageSource::ApiHandle(std::sync::Arc::new(std::sync::Mutex::new(None)));
    let mut c = usage_tracker::claude::ClaudeCollector::new_multi_with_usage(
        vec![usage_tracker::claude::ConfigDirEntry { dir: root, wsl_distro: None }],
        usage_source,
    );
    let _ = c.collect(&ctx);
    let rl = c.usage_limits().expect("hook file should be used as fallback");
    assert_eq!(rl.five_hour_pct, Some(55.0));
}

#[test]
fn prefers_api_handle_data_over_hook_file() {
    let root = build_fake_claude_root("api_precedence");
    fs::write(
        root.join("abtop-rate-limits.json"),
        r#"{"five_hour":{"used_percentage":55.0,"resets_at":1700000000}}"#,
    )
    .unwrap();
    let mut procs = HashMap::new();
    procs.insert(
        4242,
        ProcInfo { pid: 4242, command: "claude".into(), rss_kb: 50_000, cpu: 0.0, parent_pid: None },
    );
    let ctx = ProcessContext { procs: &procs, children: &HashMap::new(), ports: &HashMap::new(), wsl: &HashMap::new() };

    let api_value = RateLimitInfo {
        source: "claude".into(),
        five_hour_pct: Some(7.0),
        five_hour_resets_at: Some(1),
        seven_day_pct: None,
        seven_day_resets_at: None,
        monthly_pct: None,
        monthly_resets_at: None,
        updated_at: Some(1),
    };
    let usage_source = ClaudeUsageSource::ApiHandle(std::sync::Arc::new(std::sync::Mutex::new(Some(api_value))));
    let mut c = usage_tracker::claude::ClaudeCollector::new_multi_with_usage(
        vec![usage_tracker::claude::ConfigDirEntry { dir: root, wsl_distro: None }],
        usage_source,
    );
    let _ = c.collect(&ctx);
    let rl = c.usage_limits().expect("api handle should provide usage");
    assert_eq!(rl.five_hour_pct, Some(7.0));
}
