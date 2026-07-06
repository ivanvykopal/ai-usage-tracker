use crate::collector::{Collector, ProcessContext};
use crate::model::{build_snapshot, Snapshot};
use crate::process::{self, ProcessSnapshot};
use std::collections::{HashMap, HashSet};

/// Owns the collectors and runs the tick loop that refreshes them into a
/// `Snapshot`. Collectors run sequentially (they share the sysinfo view),
/// matching abtop's single-threaded `App`. A collector that panics is caught
/// and skipped for that tick — one broken agent never blanks the panel.
pub struct App {
    collectors: Vec<Box<dyn Collector>>,
    /// WSL distros to poll for a process snapshot each tick, so sessions
    /// discovered under `\\wsl$\<distro>\...` (native Windows only) can have
    /// their pid liveness checked against the right VM.
    wsl_distros: Vec<String>,
    /// (agent_cli, session_id) -> (status as of last tick, ms timestamp that
    /// status was first observed). Drives the `stalled` flag.
    stall_tracker: HashMap<(String, String), (crate::model::SessionStatus, i64)>,
}

impl App {
    pub fn new(collectors: Vec<Box<dyn Collector>>) -> Self {
        Self::new_with_wsl_distros(collectors, Vec::new())
    }

    pub fn new_with_wsl_distros(collectors: Vec<Box<dyn Collector>>, wsl_distros: Vec<String>) -> Self {
        Self {
            collectors,
            wsl_distros,
            stall_tracker: HashMap::new(),
        }
    }

    /// Reconciles the running set of collectors against a freshly-built list
    /// — used when the user toggles which providers are enabled from the
    /// in-app settings panel, so a change takes effect on the very next tick
    /// without an app restart. `new_collectors` is expected to contain a
    /// fresh instance per currently-enabled provider (from
    /// `providers::build_collectors`), but a provider whose enabled state
    /// didn't change keeps its *existing* running instance instead of
    /// swapping in the fresh one — a fresh `ClaudeCollector`, for example,
    /// starts its usage-API poller over from an empty state, so blindly
    /// replacing an unchanged collector would transiently blank its usage
    /// data for no reason every time an unrelated provider is toggled.
    pub fn set_collectors(&mut self, new_collectors: Vec<Box<dyn Collector>>) {
        let mut old_by_name: HashMap<String, Box<dyn Collector>> = self
            .collectors
            .drain(..)
            .map(|c| (c.name().to_string(), c))
            .collect();
        self.collectors = new_collectors
            .into_iter()
            .map(|new_c| {
                old_by_name
                    .remove(new_c.name())
                    .unwrap_or(new_c)
            })
            .collect();
    }

    /// Refresh every collector against the current process state and return
    /// an aggregated `Snapshot`.
    pub fn tick(&mut self) -> Snapshot {
        let now_ms = chrono::Utc::now().timestamp_millis();
        // Default threshold when called from the live tick loop; callers that
        // need testability can use tick_with_threshold directly.
        self.tick_with_threshold(180, now_ms)
    }

    pub fn tick_with_threshold(&mut self, stall_alert_secs: u64, now_ms: i64) -> Snapshot {
        let ps = process::snapshot();
        let wsl: HashMap<String, ProcessSnapshot> = self
            .wsl_distros
            .iter()
            .map(|d| (d.clone(), process::wsl_snapshot(d)))
            .collect();
        let ctx = ProcessContext {
            procs: &ps.procs,
            children: &ps.children,
            ports: &ps.ports_by_pid,
            wsl: &wsl,
        };
        let mut sessions = Vec::new();
        let mut usage_limits = std::collections::BTreeMap::new();
        for c in &mut self.collectors {
            // Catch a collector panic so one agent can't take down the tick loop.
            let name = c.name().to_string();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.collect(&ctx)));
            if let Ok(s) = result {
                sessions.extend(s);
            }
            // On panic: skip this collector this tick, keep going.
            if let Ok(Some(rl)) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.usage_limits())) {
                usage_limits.insert(name, rl);
            }
        }
        // Populate cost estimates from the pricing table.
        for s in &mut sessions {
            s.cost_usd = crate::pricing::estimate_cost_usd(
                &s.model,
                s.total_input_tokens,
                s.total_output_tokens,
                s.total_cache_read,
                s.total_cache_create,
            );
        }
        if stall_alert_secs > 0 {
            let threshold_ms = (stall_alert_secs as i64) * 1000;
            let mut still_present: HashSet<(String, String)> = HashSet::new();
            for s in &mut sessions {
                let key = (s.agent_cli.clone(), s.session_id.clone());
                still_present.insert(key.clone());
                let is_active_status = matches!(
                    s.status,
                    crate::model::SessionStatus::Thinking | crate::model::SessionStatus::Executing
                );
                let entry = self.stall_tracker.entry(key).or_insert((s.status, now_ms));
                if entry.0 != s.status {
                    *entry = (s.status, now_ms);
                }
                s.stalled = is_active_status && (now_ms - entry.1) >= threshold_ms;
            }
            self.stall_tracker.retain(|k, _| still_present.contains(k));
        }

        // Dedupe by (agent_cli, session_id); last one wins.
        sessions.sort_by(|a, b| {
            a.agent_cli
                .cmp(&b.agent_cli)
                .then_with(|| a.session_id.cmp(&b.session_id))
        });
        sessions.dedup_by(|a, b| a.agent_cli == b.agent_cli && a.session_id == b.session_id);
        build_snapshot(sessions, usage_limits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentSession, SessionStatus};

    struct FakeCollector;
    impl Collector for FakeCollector {
        fn name(&self) -> &str {
            "fake"
        }
        fn collect(&mut self, _ctx: &ProcessContext) -> Vec<AgentSession> {
            vec![AgentSession {
                agent_cli: "claude".into(),
                pid: 0,
                session_id: "s1".into(),
                cwd: String::new(),
                project_name: String::new(),
                started_at: 0,
                status: SessionStatus::Waiting,
                model: "claude-sonnet-4-20250514".into(),
                context_percent: 0.0,
                total_input_tokens: 1_000_000,
                total_output_tokens: 0,
                total_cache_read: 0,
                total_cache_create: 0,
                turn_count: 0,
                current_task: String::new(),
                mem_mb: 0,
                cost_usd: None,
                stalled: false,
            }]
        }
    }

    #[test]
    fn tick_populates_cost_usd_from_pricing_table() {
        let mut app = App::new(vec![Box::new(FakeCollector)]);
        let snapshot = app.tick();
        assert_eq!(snapshot.sessions.len(), 1);
        let cost = snapshot.sessions[0].cost_usd.expect("cost should be Some for a known model");
        assert!((cost - 3.0).abs() < 1e-9);
    }

    #[test]
    fn session_stuck_in_thinking_past_threshold_is_flagged_stalled() {
        struct StuckCollector;
        impl Collector for StuckCollector {
            fn name(&self) -> &str { "stuck" }
            fn collect(&mut self, _ctx: &ProcessContext) -> Vec<AgentSession> {
                vec![AgentSession { status: SessionStatus::Thinking, session_id: "s1".into(), agent_cli: "claude".into(), ..blank_session() }]
            }
        }
        let mut app = App::new(vec![Box::new(StuckCollector)]);
        let first = app.tick_with_threshold(60, 0);
        assert!(!first.sessions[0].stalled, "should not be stalled on first sighting");
        let second = app.tick_with_threshold(60, 61_000);
        assert!(second.sessions[0].stalled, "should be stalled after 61s in the same status");
    }

    #[test]
    fn session_that_changes_status_resets_the_stall_timer() {
        struct FlippingCollector { thinking: bool }
        impl Collector for FlippingCollector {
            fn name(&self) -> &str { "flip" }
            fn collect(&mut self, _ctx: &ProcessContext) -> Vec<AgentSession> {
                let status = if self.thinking { SessionStatus::Thinking } else { SessionStatus::Executing };
                self.thinking = !self.thinking;
                vec![AgentSession { status, session_id: "s1".into(), agent_cli: "claude".into(), ..blank_session() }]
            }
        }
        let mut app = App::new(vec![Box::new(FlippingCollector { thinking: true })]);
        app.tick_with_threshold(60, 0);
        let second = app.tick_with_threshold(60, 61_000);
        assert!(!second.sessions[0].stalled, "status changed, so the timer should have reset");
    }

    #[test]
    fn set_collectors_preserves_unchanged_collector_instances() {
        // Mimics ClaudeCollector holding live state (e.g. a usage-API poller
        // handle) that a same-named replacement wouldn't have yet — toggling
        // an unrelated provider must not reset it.
        struct StatefulCollector { tag: &'static str, hits: u32 }
        impl Collector for StatefulCollector {
            fn name(&self) -> &str { self.tag }
            fn collect(&mut self, _ctx: &ProcessContext) -> Vec<AgentSession> {
                self.hits += 1;
                vec![AgentSession { session_id: self.hits.to_string(), agent_cli: self.tag.into(), ..blank_session() }]
            }
        }
        let mut app = App::new(vec![Box::new(StatefulCollector { tag: "claude", hits: 5 })]);
        // Simulate rebuilding collectors after toggling an unrelated provider:
        // a brand-new "claude" instance with reset state is offered...
        app.set_collectors(vec![Box::new(StatefulCollector { tag: "claude", hits: 0 })]);
        // ...but the original, stateful instance should still be the one running.
        let snapshot = app.tick();
        assert_eq!(snapshot.sessions[0].session_id, "6", "expected the original collector's hit-count to continue, not reset to 1");
    }

    #[test]
    fn set_collectors_drops_disabled_and_adds_newly_enabled() {
        struct NamedCollector(&'static str);
        impl Collector for NamedCollector {
            fn name(&self) -> &str { self.0 }
            fn collect(&mut self, _ctx: &ProcessContext) -> Vec<AgentSession> {
                vec![AgentSession { session_id: self.0.into(), agent_cli: self.0.into(), ..blank_session() }]
            }
        }
        let mut app = App::new(vec![Box::new(NamedCollector("claude")), Box::new(NamedCollector("codex"))]);
        // codex gets disabled, hermes gets newly enabled, claude is unchanged.
        app.set_collectors(vec![Box::new(NamedCollector("claude")), Box::new(NamedCollector("hermes"))]);
        let snapshot = app.tick();
        let names: std::collections::HashSet<_> = snapshot.sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(names, std::collections::HashSet::from(["claude", "hermes"]));
    }

    fn blank_session() -> AgentSession {
        AgentSession {
            agent_cli: String::new(), pid: 0, session_id: String::new(), cwd: String::new(),
            project_name: String::new(), started_at: 0, status: SessionStatus::Waiting,
            model: String::new(), context_percent: 0.0, total_input_tokens: 0, total_output_tokens: 0,
            total_cache_read: 0, total_cache_create: 0, turn_count: 0, current_task: String::new(),
            mem_mb: 0, cost_usd: None, stalled: false,
        }
    }
}
