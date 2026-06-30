use crate::model::AgentSession;
use crate::process::ProcInfo;
use std::collections::HashMap;

/// A read-only view of the process state passed to each collector on a tick.
/// Collectors use it to attribute sessions to live PIDs, read memory, and
/// detect active descendants for status heuristics.
pub struct ProcessContext<'a> {
    pub procs: &'a HashMap<u32, ProcInfo>,
    pub children: &'a HashMap<u32, Vec<u32>>,
    pub ports: &'a HashMap<u32, Vec<u16>>,
}

/// The single extension point for an AI assistant. Each agent (Claude, Codex,
/// Hermes) implements this to turn local file/process state into
/// `AgentSession`s. A collector that fails should return an empty `Vec` for
/// the tick rather than panicking — `App::tick` additionally catches panics so
/// one broken agent never blanks the panel.
pub trait Collector {
    fn name(&self) -> &str;
    fn collect(&mut self, ctx: &ProcessContext) -> Vec<AgentSession>;
}
