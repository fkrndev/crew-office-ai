//! Fallback-only model → price table (USD per million tokens) — port of
//! `pricing.ts`.
//!
//! The LIVE telemetry path does NOT use this: Claude Code emits a
//! pre-computed `cost_usd` per `api_request`, and the collector trusts
//! Claude's own figure. This table exists for OFFLINE estimation —
//! proxy-bridge `CostSample` rows and any token-count-only path that must
//! turn usage into dollars.

/// USD per million tokens for one model family.
#[derive(Clone, Copy)]
pub struct ModelPrice {
    pub input_per_m: f64,
    pub output_per_m: f64,
    pub cache_read_per_m: f64,
    pub cache_write_per_m: f64,
}

// Anthropic list prices, USD/M tokens. Approximate, fallback-only.
const OPUS: ModelPrice = ModelPrice {
    input_per_m: 15.0,
    output_per_m: 75.0,
    cache_read_per_m: 1.5,
    cache_write_per_m: 18.75,
};
const SONNET: ModelPrice = ModelPrice {
    input_per_m: 3.0,
    output_per_m: 15.0,
    cache_read_per_m: 0.3,
    cache_write_per_m: 3.75,
};
const HAIKU: ModelPrice = ModelPrice {
    input_per_m: 0.8,
    output_per_m: 4.0,
    cache_read_per_m: 0.08,
    cache_write_per_m: 1.0,
};
const DEFAULT_PRICE: ModelPrice = SONNET;

/// Strip Claude Code's variant suffix so `claude-opus-4-8[1m]` and the base
/// `claude-opus-4-8` resolve to the same family.
pub fn normalize_model(model: &str) -> &str {
    let m = model.trim();
    match m.rfind('[').filter(|_| m.ends_with(']')) {
        Some(i) => m[..i].trim_end(),
        None => m,
    }
}

/// Resolve a model id to its price row by family, falling back to Sonnet.
pub fn price_for(model: &str) -> ModelPrice {
    let m = normalize_model(model).to_lowercase();
    if m.contains("opus") {
        OPUS
    } else if m.contains("haiku") {
        HAIKU
    } else if m.contains("sonnet") {
        SONNET
    } else {
        DEFAULT_PRICE
    }
}

/// Estimate USD cost for a token split — the offline/reconciler path
/// (used by the transcript reconciler; callers priced per-token-split
/// inline use price_for directly).
#[allow(dead_code)]
pub fn estimate_cost_usd(
    model: &str,
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
) -> f64 {
    let p = price_for(model);
    (input / 1_000_000.0) * p.input_per_m
        + (output / 1_000_000.0) * p.output_per_m
        + (cache_read / 1_000_000.0) * p.cache_read_per_m
        + (cache_write / 1_000_000.0) * p.cache_write_per_m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_suffix_strips_and_families_match() {
        assert_eq!(normalize_model("claude-opus-4-8[1m]"), "claude-opus-4-8");
        assert_eq!(normalize_model(" claude-opus-4-8 "), "claude-opus-4-8");
        let (o, s, h, x) = (
            price_for("claude-opus-4-8[1m]"),
            price_for("claude-sonnet-4-5"),
            price_for("claude-haiku-3-5"),
            price_for("qwen3-coder"),
        );
        assert_eq!(o.input_per_m, 15.0);
        assert_eq!(s.input_per_m, 3.0);
        assert_eq!(h.input_per_m, 0.8);
        assert_eq!(x.input_per_m, 3.0, "unknown falls back to Sonnet");
    }

    #[test]
    fn cost_estimate_matches_table() {
        let c = estimate_cost_usd("claude-sonnet-4-5", 1_000_000.0, 1_000_000.0, 0.0, 0.0);
        assert!((c - 18.0).abs() < 1e-9);
    }
}
