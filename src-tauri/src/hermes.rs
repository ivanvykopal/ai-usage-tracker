use crate::collector::{Collector, ProcessContext};
use crate::model::AgentSession;
use std::path::PathBuf;

pub struct HermesCollector {
    _data_dir: PathBuf,
}

impl HermesCollector {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { _data_dir: data_dir }
    }
}

impl Collector for HermesCollector {
    fn name(&self) -> &str {
        "hermes"
    }

    fn collect(&mut self, _ctx: &ProcessContext) -> Vec<AgentSession> {
        // TODO: Implement Hermes data parsing once data format is known
        Vec::new()
    }
}
