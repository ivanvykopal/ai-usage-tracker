#[derive(Debug, Clone, Copy)]
pub struct Rates {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: f64,
    pub cache_write_per_mtok: f64,
}

/// Published per-million-token USD prices as of the model's release pricing
/// page. Matched by substring against the reported model string (which may
/// carry suffixes like "[1m]" or date stamps), longest/most-specific prefix
/// first so "claude-3-5-haiku" doesn't match the generic "claude-3-5" rule
/// before its own.
const TABLE: &[(&str, Rates)] = &[
    ("claude-opus-4", Rates { input_per_mtok: 15.0, output_per_mtok: 75.0, cache_read_per_mtok: 1.5, cache_write_per_mtok: 18.75 }),
    ("claude-sonnet-4", Rates { input_per_mtok: 3.0, output_per_mtok: 15.0, cache_read_per_mtok: 0.3, cache_write_per_mtok: 3.75 }),
    ("claude-3-5-haiku", Rates { input_per_mtok: 0.8, output_per_mtok: 4.0, cache_read_per_mtok: 0.08, cache_write_per_mtok: 1.0 }),
    ("claude-3-5-sonnet", Rates { input_per_mtok: 3.0, output_per_mtok: 15.0, cache_read_per_mtok: 0.3, cache_write_per_mtok: 3.75 }),
    ("gpt-5-codex", Rates { input_per_mtok: 1.25, output_per_mtok: 10.0, cache_read_per_mtok: 0.125, cache_write_per_mtok: 1.25 }),
    ("gpt-5", Rates { input_per_mtok: 1.25, output_per_mtok: 10.0, cache_read_per_mtok: 0.125, cache_write_per_mtok: 1.25 }),
    ("gpt-4o", Rates { input_per_mtok: 2.5, output_per_mtok: 10.0, cache_read_per_mtok: 1.25, cache_write_per_mtok: 2.5 }),
];

pub fn rates_for_model(model: &str) -> Option<Rates> {
    let m = model.to_ascii_lowercase();
    TABLE
        .iter()
        .filter(|(prefix, _)| m.contains(prefix))
        .max_by_key(|(prefix, _)| prefix.len()) // most specific match wins
        .map(|(_, rates)| *rates)
}

pub fn estimate_cost_usd(model: &str, input: u64, output: u64, cache_read: u64, cache_write: u64) -> Option<f64> {
    let rates = rates_for_model(model)?;
    let mtok = 1_000_000.0;
    Some(
        (input as f64 / mtok) * rates.input_per_mtok
            + (output as f64 / mtok) * rates.output_per_mtok
            + (cache_read as f64 / mtok) * rates.cache_read_per_mtok
            + (cache_write as f64 / mtok) * rates.cache_write_per_mtok,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_model_returns_rates() {
        assert!(rates_for_model("claude-sonnet-4-20250514").is_some());
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(rates_for_model("some-future-model-9000").is_none());
        assert!(estimate_cost_usd("some-future-model-9000", 1000, 1000, 0, 0).is_none());
    }

    #[test]
    fn cost_scales_linearly_with_tokens() {
        let cost = estimate_cost_usd("claude-sonnet-4-20250514", 1_000_000, 0, 0, 0).unwrap();
        assert!((cost - 3.0).abs() < 1e-9);
    }

    #[test]
    fn most_specific_prefix_wins_over_generic_one() {
        // "claude-3-5-haiku" must not be shadowed by a hypothetical shorter
        // "claude-3-5" rule — this test pins that behavior even though the
        // current table has no overlapping short prefix, so a future entry
        // can't silently break haiku pricing.
        let haiku = rates_for_model("claude-3-5-haiku-20241022").unwrap();
        assert!((haiku.input_per_mtok - 0.8).abs() < 1e-9);
    }
}
