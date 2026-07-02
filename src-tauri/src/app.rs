use crate::collector::{Collector, ProcessContext};
use crate::model::{build_snapshot, Snapshot};
use crate::process::{self, ProcessSnapshot};
use std::collections::HashMap;

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
}

impl App {
    pub fn new(collectors: Vec<Box<dyn Collector>>) -> Self {
        Self::new_with_wsl_distros(collectors, Vec::new())
    }

    pub fn new_with_wsl_distros(collectors: Vec<Box<dyn Collector>>, wsl_distros: Vec<String>) -> Self {
        Self {
            collectors,
            wsl_distros,
        }
    }

    /// Refresh every collector against the current process state and return
    /// an aggregated `Snapshot`.
    pub fn tick(&mut self) -> Snapshot {
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
        for c in &mut self.collectors {
            // Catch a collector panic so one agent can't take down the tick loop.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.collect(&ctx)));
            if let Ok(s) = result {
                sessions.extend(s);
            }
            // On panic: skip this collector this tick, keep going.
        }
        // Dedupe by (agent_cli, session_id); last one wins.
        sessions.sort_by(|a, b| {
            a.agent_cli
                .cmp(&b.agent_cli)
                .then_with(|| a.session_id.cmp(&b.session_id))
        });
        sessions.dedup_by(|a, b| a.agent_cli == b.agent_cli && a.session_id == b.session_id);
        build_snapshot(sessions)
    }
}
