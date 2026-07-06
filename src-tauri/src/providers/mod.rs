//! One module per supported AI-assistant CLI. Each module exposes a
//! `build(cfg, home_dirs) -> Option<Box<dyn Collector>>` function with the
//! same signature; adding a new CLI means adding a module here and one more
//! entry in `ALL` — nothing else in the app needs to change.
pub mod claude;
pub mod codex;
pub mod hermes;

use crate::collector::Collector;
use crate::config::Config;
use crate::home::HomeDir;

pub struct ProviderEntry {
    pub key: &'static str,
    pub label: &'static str,
    pub build: fn(&Config, &[HomeDir]) -> Option<Box<dyn Collector>>,
}

pub static ALL: &[ProviderEntry] = &[
    ProviderEntry {
        key: "claude",
        label: "Claude Code",
        build: claude::build,
    },
    ProviderEntry {
        key: "codex",
        label: "Codex CLI",
        build: codex::build,
    },
    ProviderEntry {
        key: "hermes",
        label: "Hermes",
        build: hermes::build,
    },
];

/// Build one collector per enabled, detected provider. A provider whose
/// `build` returns `None` (not enabled in config, or no matching directory
/// found on disk) contributes nothing — matches the current behavior of
/// `lib.rs::build_collectors` before this refactor.
pub fn build_collectors(cfg: &Config, home_dirs: &[HomeDir]) -> Vec<Box<dyn Collector>> {
    ALL.iter()
        .filter(|p| cfg.enabled_agents.iter().any(|a| a == p.key))
        .filter_map(|p| (p.build)(cfg, home_dirs))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_keys_are_unique() {
        let mut keys: Vec<&str> = ALL.iter().map(|p| p.key).collect();
        let before = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), before, "duplicate provider key in ALL");
    }

    #[test]
    fn disabled_provider_yields_no_collector() {
        let cfg = Config {
            enabled_agents: vec![],
            ..crate::config::default_config()
        };
        let collectors = build_collectors(&cfg, &[]);
        assert!(collectors.is_empty());
    }
}
