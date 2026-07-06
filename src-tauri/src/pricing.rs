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
    // Anthropic's 4.5 generation flipped the naming order from
    // "claude-{generation}-{family}" (e.g. claude-3-5-haiku) to
    // "claude-{family}-4-5" (e.g. claude-haiku-4-5) — that string is not a
    // substring of the older "claude-3-5-haiku" rule above, so without this
    // entry Haiku 4.5 silently matched nothing and always priced at $0.
    ("claude-haiku-4-5", Rates { input_per_mtok: 1.0, output_per_mtok: 5.0, cache_read_per_mtok: 0.1, cache_write_per_mtok: 1.25 }),
    // Per platform.claude.com/docs/en/about-claude/pricing: Opus 4.5/4.6/4.7/4.8
    // all price the same ($5/$25), well below the generic "claude-opus-4"
    // entry above (which is really the Opus 4/4.1-era $15/$75 rate). Each
    // needs its own entry — "claude-opus-4-8" is a longer/more-specific match
    // than "claude-opus-4", so max_by_key picks it correctly, but there's no
    // single prefix that covers 4.5/4.6/4.7/4.8 without also over-matching.
    ("claude-opus-4-5", Rates { input_per_mtok: 5.0, output_per_mtok: 25.0, cache_read_per_mtok: 0.5, cache_write_per_mtok: 6.25 }),
    ("claude-opus-4-6", Rates { input_per_mtok: 5.0, output_per_mtok: 25.0, cache_read_per_mtok: 0.5, cache_write_per_mtok: 6.25 }),
    ("claude-opus-4-7", Rates { input_per_mtok: 5.0, output_per_mtok: 25.0, cache_read_per_mtok: 0.5, cache_write_per_mtok: 6.25 }),
    ("claude-opus-4-8", Rates { input_per_mtok: 5.0, output_per_mtok: 25.0, cache_read_per_mtok: 0.5, cache_write_per_mtok: 6.25 }),
    // Introductory pricing, in effect through 2026-08-31 per the pricing page
    // (standard $3/$15/$0.3/$3.75 takes over 2026-09-01 — update this entry
    // then, since this table has no time-based logic of its own).
    ("claude-sonnet-5", Rates { input_per_mtok: 2.0, output_per_mtok: 10.0, cache_read_per_mtok: 0.2, cache_write_per_mtok: 2.5 }),
    ("claude-fable-5", Rates { input_per_mtok: 10.0, output_per_mtok: 50.0, cache_read_per_mtok: 1.0, cache_write_per_mtok: 12.5 }),
    // Retained for older sessions that may still report these ids, but
    // OpenAI's current lineup (per developers.openai.com/api/docs/pricing)
    // has moved past "gpt-5"/"gpt-5-codex" to the 5.3/5.4/5.5 generation
    // below — those don't share a punctuation-compatible prefix with these
    // older entries (dot vs hyphen), so without their own rows they'd
    // silently fall through to this bucket's stale price instead of $0,
    // which is a quieter, easier-to-miss version of the same bug.
    ("gpt-5-codex", Rates { input_per_mtok: 1.25, output_per_mtok: 10.0, cache_read_per_mtok: 0.125, cache_write_per_mtok: 1.25 }),
    ("gpt-5", Rates { input_per_mtok: 1.25, output_per_mtok: 10.0, cache_read_per_mtok: 0.125, cache_write_per_mtok: 1.25 }),
    ("gpt-4o", Rates { input_per_mtok: 2.5, output_per_mtok: 10.0, cache_read_per_mtok: 1.25, cache_write_per_mtok: 2.5 }),
    // Current OpenAI lineup. OpenAI doesn't charge a cache-write premium (cache
    // creation is free/automatic, unlike Anthropic's 1.25x), so cache_write
    // equals the base input rate here — matching the convention already used
    // for gpt-5-codex/gpt-5/gpt-4o above.
    ("gpt-5.3-codex", Rates { input_per_mtok: 1.75, output_per_mtok: 14.0, cache_read_per_mtok: 0.175, cache_write_per_mtok: 1.75 }),
    ("gpt-5.4-mini", Rates { input_per_mtok: 0.75, output_per_mtok: 4.5, cache_read_per_mtok: 0.075, cache_write_per_mtok: 0.75 }),
    ("gpt-5.4-nano", Rates { input_per_mtok: 0.20, output_per_mtok: 1.25, cache_read_per_mtok: 0.02, cache_write_per_mtok: 0.20 }),
    ("gpt-5.4", Rates { input_per_mtok: 2.5, output_per_mtok: 15.0, cache_read_per_mtok: 0.25, cache_write_per_mtok: 2.5 }),
    ("gpt-5.5", Rates { input_per_mtok: 5.0, output_per_mtok: 30.0, cache_read_per_mtok: 0.5, cache_write_per_mtok: 5.0 }),
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

    #[test]
    fn haiku_4_5_matches_its_own_entry_not_zero() {
        // The 4.5 generation renamed models to "claude-{family}-4-5", which
        // is not a substring of the older "claude-3-5-haiku" rule — this
        // pins that Haiku 4.5 has its own entry instead of silently costing $0.
        let rates = rates_for_model("claude-haiku-4-5-20251001").unwrap();
        assert!((rates.input_per_mtok - 1.0).abs() < 1e-9);
        assert!((rates.output_per_mtok - 5.0).abs() < 1e-9);
    }

    #[test]
    fn opus_4_5_through_4_8_use_their_own_entries_not_the_stale_generic_opus_4_rate() {
        // "claude-opus-4-8" (and 4-5/4-6/4-7) contain "claude-opus-4" as a
        // substring, so without their own entries they'd silently match the
        // older, much cheaper "claude-opus-4" rule (from the 4.0/4.1 era)
        // instead of the current $5/$25 rate shared by the whole 4.5+ line.
        for id in ["claude-opus-4-5", "claude-opus-4-6", "claude-opus-4-7", "claude-opus-4-8"] {
            let rates = rates_for_model(id).unwrap_or_else(|| panic!("no rates for {id}"));
            assert!((rates.input_per_mtok - 5.0).abs() < 1e-9, "{id} input");
            assert!((rates.output_per_mtok - 25.0).abs() < 1e-9, "{id} output");
        }
    }

    #[test]
    fn sonnet_5_and_fable_5_price_correctly() {
        // Sonnet 5 is under introductory pricing ($2/$10) through 2026-08-31
        // per platform.claude.com/docs/en/about-claude/pricing — not yet the
        // post-introductory $3/$15 rate.
        let sonnet5 = rates_for_model("claude-sonnet-5").unwrap();
        assert!((sonnet5.input_per_mtok - 2.0).abs() < 1e-9);
        assert!((sonnet5.output_per_mtok - 10.0).abs() < 1e-9);

        let fable5 = rates_for_model("claude-fable-5").unwrap();
        assert!((fable5.input_per_mtok - 10.0).abs() < 1e-9);
        assert!((fable5.output_per_mtok - 50.0).abs() < 1e-9);
    }

    #[test]
    fn current_openai_lineup_prices_correctly() {
        let codex = rates_for_model("gpt-5.3-codex").unwrap();
        assert!((codex.input_per_mtok - 1.75).abs() < 1e-9);
        assert!((codex.output_per_mtok - 14.0).abs() < 1e-9);

        let flagship = rates_for_model("gpt-5.4").unwrap();
        assert!((flagship.input_per_mtok - 2.5).abs() < 1e-9);
        assert!((flagship.output_per_mtok - 15.0).abs() < 1e-9);
    }

    #[test]
    fn gpt_5_4_mini_and_nano_win_over_the_generic_gpt_5_4_entry() {
        // "gpt-5.4-mini"/"gpt-5.4-nano" contain "gpt-5.4" as a substring —
        // without their own (longer, more specific) entries they'd silently
        // price at the flagship 5.4 rate instead of their much cheaper own.
        let mini = rates_for_model("gpt-5.4-mini").unwrap();
        assert!((mini.input_per_mtok - 0.75).abs() < 1e-9);

        let nano = rates_for_model("gpt-5.4-nano").unwrap();
        assert!((nano.input_per_mtok - 0.20).abs() < 1e-9);
    }
}
