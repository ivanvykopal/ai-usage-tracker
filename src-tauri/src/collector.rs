use crate::model::AgentSession;
use crate::process::{ProcInfo, ProcessSnapshot};
use std::collections::HashMap;

/// A read-only view of the process state passed to each collector on a tick.
/// Collectors use it to attribute sessions to live PIDs, read memory, and
/// detect active descendants for status heuristics.
pub struct ProcessContext<'a> {
    pub procs: &'a HashMap<u32, ProcInfo>,
    pub children: &'a HashMap<u32, Vec<u32>>,
    pub ports: &'a HashMap<u32, Vec<u16>>,
    /// Process snapshots for WSL distributions, keyed by distro name. Only
    /// populated on native Windows when a WSL-sourced config dir was found;
    /// empty everywhere else. Lets collectors check pid liveness for
    /// sessions whose data came from `\\wsl$\<distro>\...` paths.
    pub wsl: &'a HashMap<String, ProcessSnapshot>,
}

impl<'a> ProcessContext<'a> {
    /// The process/children maps to use for a session sourced from `distro`
    /// (`None` for the native host). Falls back to empty maps if a distro's
    /// snapshot wasn't captured this tick.
    pub fn procs_for(
        &self,
        distro: Option<&str>,
    ) -> (&'a HashMap<u32, ProcInfo>, &'a HashMap<u32, Vec<u32>>) {
        match distro {
            None => (self.procs, self.children),
            Some(d) => match self.wsl.get(d) {
                Some(snap) => (&snap.procs, &snap.children),
                None => (EMPTY_PROCS.get_or_init(HashMap::new), EMPTY_CHILDREN.get_or_init(HashMap::new)),
            },
        }
    }
}

static EMPTY_PROCS: std::sync::OnceLock<HashMap<u32, ProcInfo>> = std::sync::OnceLock::new();
static EMPTY_CHILDREN: std::sync::OnceLock<HashMap<u32, Vec<u32>>> = std::sync::OnceLock::new();

/// Best-effort context-window size (in tokens) for a model name, used to turn
/// a session's last-known context token count into a percentage. There's no
/// API that reports this per-session, so it's a static lookup rather than an
/// exact figure. `used_tokens` lets a base-200K Claude model auto-upgrade to
/// the 1M window: Anthropic lets accounts opt into the 1M beta without it
/// necessarily showing up as a `[1m]` suffix on every model string, so tokens
/// alone crossing 200K is itself evidence the session is running in that mode.
pub fn context_window_for_model(model: &str, configured_model: &str, used_tokens: u64) -> u64 {
    let m = model.to_ascii_lowercase();
    let cm = configured_model.to_ascii_lowercase();
    if m.contains("1m") || cm.contains("1m") {
        return 1_000_000;
    }
    let base = if m.contains("claude") {
        200_000
    } else if m.contains("gpt-5") || m.contains("codex") {
        400_000
    } else if m.contains("gpt-4") {
        128_000
    } else {
        200_000
    };
    if base == 200_000 && used_tokens > 200_000 {
        1_000_000
    } else {
        base
    }
}

/// `used_tokens / context_window_for_model(model, configured_model, used_tokens) * 100`,
/// clamped to `[0, 100]`. Returns `0.0` when `used_tokens` is `0` (nothing
/// parsed yet).
pub fn context_percent_for(model: &str, configured_model: &str, used_tokens: u64) -> f64 {
    if used_tokens == 0 {
        return 0.0;
    }
    let window = context_window_for_model(model, configured_model, used_tokens);
    if window == 0 {
        return 0.0;
    }
    ((used_tokens as f64 / window as f64) * 100.0).clamp(0.0, 100.0)
}

/// The single extension point for an AI assistant. Each agent (Claude, Codex,
/// Hermes) implements this to turn local file/process state into
/// `AgentSession`s. A collector that fails should return an empty `Vec` for
/// the tick rather than panicking — `App::tick` additionally catches panics so
/// one broken agent never blanks the panel.
pub trait Collector: Send {
    fn name(&self) -> &str;
    fn collect(&mut self, ctx: &ProcessContext) -> Vec<AgentSession>;
}
