//! Model pricing (micro-USD per token) for the durable cost ledger.
//!
//! Rates are USD per 1M tokens (as published); we store cost in **micro-USD**
//! (1e-6 USD) integers to avoid float drift. Unknown/local models cost 0.

/// (input_usd_per_million, output_usd_per_million) for known models.
fn rate_per_million(model: &str) -> Option<(f64, f64)> {
    let m = model.to_ascii_lowercase();
    let r = match m.as_str() {
        "gpt-4o-mini" => (0.15, 0.60),
        "gpt-4o" => (2.50, 10.00),
        "gpt-4.1-mini" => (0.40, 1.60),
        "text-embedding-3-small" => (0.02, 0.0),
        "text-embedding-3-large" => (0.13, 0.0),
        _ => return None,
    };
    Some(r)
}

/// Cost in micro-USD for a call, rounded to the nearest micro-USD.
pub fn cost_micro_usd(model: &str, input_tokens: u64, output_tokens: u64) -> i64 {
    let Some((in_rate, out_rate)) = rate_per_million(model) else {
        return 0;
    };
    // usd = tokens/1e6 * rate; micro = usd * 1e6 = tokens * rate.
    let micro = input_tokens as f64 * in_rate + output_tokens as f64 * out_rate;
    micro.round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt4o_mini_cost_math() {
        // 1M input + 1M output = $0.15 + $0.60 = $0.75 = 750_000 micro-USD.
        assert_eq!(cost_micro_usd("gpt-4o-mini", 1_000_000, 1_000_000), 750_000);
        // 1000 input tokens = $0.00015 = 150 micro-USD.
        assert_eq!(cost_micro_usd("gpt-4o-mini", 1000, 0), 150);
    }

    #[test]
    fn unknown_and_local_models_are_free() {
        assert_eq!(cost_micro_usd("llama3.1", 1000, 1000), 0);
        assert_eq!(cost_micro_usd("hash-placeholder-v1", 5, 5), 0);
    }
}
