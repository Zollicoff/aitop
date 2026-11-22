//! Converting and downloading LiteLLM's public price list.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::{cache_path, Price, PriceFile, Rates, BUILTIN};

const LITELLM_URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// Convert LiteLLM's `model_prices_and_context_window.json` into our format,
/// keeping only the Anthropic and OpenAI models these tools use.
pub fn from_litellm(json: &str, as_of: &str) -> Result<PriceFile> {
    let all: BTreeMap<String, serde_json::Value> = serde_json::from_str(json).context("not a LiteLLM price file")?;
    let mut models = BTreeMap::new();
    let per_m = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_f64()).map(|x| round(x * 1e6));
    for (key, v) in &all {
        // Provider-prefixed duplicates (`bedrock/…`, `azure/…`) are skipped.
        if key.contains('/') {
            continue;
        }
        let name = key.as_str();
        let wanted = ["claude-", "gpt-", "o1", "o3", "o4", "codex-"].iter().any(|p| name.starts_with(p));
        let mode = v.get("mode").and_then(|m| m.as_str()).unwrap_or("chat");
        if !wanted || !(mode == "chat" || mode == "responses") {
            continue;
        }
        let (Some(input), Some(output)) = (per_m(v, "input_cost_per_token"), per_m(v, "output_cost_per_token")) else {
            continue;
        };
        // The long-context tier starts where the list says: 200K tokens for
        // some models, 272K for others.
        let threshold = v
            .as_object()
            .into_iter()
            .flat_map(|o| o.keys())
            .filter_map(|k| {
                k.strip_prefix("input_cost_per_token_above_")?.strip_suffix("k_tokens")?.parse::<u64>().ok()
            })
            .min();
        let above = threshold.map(|n| format!("_above_{n}k_tokens")).unwrap_or_default();
        let long = |k: &str| threshold.and_then(|_| per_m(v, &format!("{k}{above}")));
        // A speed tier's prices; a cache price it does not list keeps the
        // normal tier's ratio to the input price.
        let standard = Rates {
            input,
            output,
            cache_read: per_m(v, "cache_read_input_token_cost").unwrap_or(input),
            cache_write: per_m(v, "cache_creation_input_token_cost").unwrap_or(input),
        };
        let tier = |suffix: &str, speed: &str| -> Option<Rates> {
            let s = format!("{suffix}{speed}");
            let tier_input = per_m(v, &format!("input_cost_per_token{s}"))?;
            let scaled = |k: &str, normal: f64| {
                per_m(v, &format!("{k}{s}")).unwrap_or(if standard.input > 0.0 {
                    round(normal / standard.input * tier_input)
                } else {
                    tier_input
                })
            };
            Some(Rates {
                input: tier_input,
                output: per_m(v, &format!("output_cost_per_token{s}"))?,
                cache_read: scaled("cache_read_input_token_cost", standard.cache_read),
                cache_write: scaled("cache_creation_input_token_cost", standard.cache_write),
            })
        };
        let long_tier = |speed: &str| threshold.and_then(|_| tier(&above, speed));
        // Some models charge a multiple for fast mode rather than listing its prices.
        let fast_multiplier =
            v.get("provider_specific_entry").and_then(|e| e.get("fast")).and_then(|m| m.as_f64()).filter(|m| *m > 0.0);
        let price = Price {
            input,
            output,
            cache_read: per_m(v, "cache_read_input_token_cost"),
            cache_write: per_m(v, "cache_creation_input_token_cost"),
            cache_write_1h: per_m(v, "cache_creation_input_token_cost_above_1hr"),
            input_above_200k: long("input_cost_per_token"),
            output_above_200k: long("output_cost_per_token"),
            cache_read_above_200k: long("cache_read_input_token_cost"),
            fast_multiplier,
            long_context_tokens: threshold.filter(|n| *n != 200).map(|n| n * 1000),
            fast: tier("", "_priority").filter(|_| fast_multiplier.is_none()),
            fast_long: long_tier("_priority").filter(|_| fast_multiplier.is_none()),
            ultrafast: tier("", "_ultrafast"),
            ultrafast_long: long_tier("_ultrafast"),
        };
        models.insert(name.to_string(), price);
    }
    if models.is_empty() {
        bail!("no usable models found in price file");
    }
    Ok(PriceFile { as_of: as_of.to_string(), source: LITELLM_URL.to_string(), models })
}

fn round(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

/// Keep fast-mode multipliers from the built-in table where the list says
/// nothing about a model's fast mode.
fn keep_fast_multipliers(file: &mut PriceFile) {
    let Ok(builtin) = serde_json::from_str::<PriceFile>(BUILTIN) else { return };
    for (k, p) in builtin.models {
        if let (Some(m), Some(e)) = (p.fast_multiplier, file.models.get_mut(&k)) {
            if e.fast_multiplier.is_none() && e.fast.is_none() {
                e.fast_multiplier = Some(m);
            }
        }
    }
}

/// Bumped when the conversion changes, so tables converted before are redone.
const T3_FORMAT: u32 = 2;

/// The price list T3 Code keeps in `rates` (a LiteLLM list it refreshes by
/// itself), in our format. The list is several megabytes, so the converted
/// table is kept in `converted` and reused until T3 writes a new list.
pub fn from_t3(rates: &Path, converted: &Path) -> Option<PriceFile> {
    #[derive(serde::Deserialize)]
    struct Rates<'a> {
        #[serde(rename = "fetchedAtMs", default)]
        fetched_at_ms: i64,
        #[serde(borrow)]
        document: &'a serde_json::value::RawValue,
    }
    let meta = std::fs::metadata(rates).ok()?;
    let written = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
    // Names the exact list the converted table was made from.
    let source = format!("{} ({written}, {} bytes, format {T3_FORMAT})", rates.display(), meta.len());
    if let Some(f) = std::fs::read_to_string(converted).ok().and_then(|t| serde_json::from_str::<PriceFile>(&t).ok()) {
        if f.source == source {
            return Some(f);
        }
    }
    let text = std::fs::read_to_string(rates).ok()?;
    let parsed: Rates = serde_json::from_str(&text).ok()?;
    let as_of = chrono::DateTime::from_timestamp_millis(parsed.fetched_at_ms)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
        .unwrap_or_default();
    let mut file = from_litellm(parsed.document.get(), &as_of).ok()?;
    keep_fast_multipliers(&mut file);
    file.source = source;
    // Best effort: without it the list is converted again next time.
    if let Some(dir) = converted.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string(&file) {
        let _ = std::fs::write(converted, json);
    }
    Some(file)
}

/// Download fresh prices into the cache file.
pub fn update(output: Option<&Path>) -> Result<(PathBuf, usize)> {
    let body = ureq::get(LITELLM_URL)
        .call()
        .context("download failed")?
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_string()
        .context("download failed")?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut file = from_litellm(&body, &today)?;
    keep_fast_multipliers(&mut file);
    let path = output.map(Path::to_path_buf).unwrap_or_else(cache_path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let n = file.models.len();
    std::fs::write(&path, serde_json::to_string_pretty(&file)? + "\n")?;
    Ok((path, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn litellm_conversion() {
        let j = r#"{
          "claude-opus-5-5": {"input_cost_per_token": 4e-6, "output_cost_per_token": 2e-5, "cache_read_input_token_cost": 2e-7, "mode": "chat", "litellm_provider": "anthropic"},
          "gemini/gemini-3-pro-preview": {"input_cost_per_token": 2e-6, "output_cost_per_token": 1.2e-5, "mode": "chat"},
          "gpt-5.3-codex": {"input_cost_per_token": 1.75e-6, "output_cost_per_token": 1.4e-5, "mode": "responses"},
          "bedrock/whatever": {"input_cost_per_token": 1e-6, "output_cost_per_token": 1e-6},
          "text-embedding-3": {"input_cost_per_token": 1e-6, "output_cost_per_token": 0, "mode": "embedding"},
          "sample_spec": {}
        }"#;
        let f = from_litellm(j, "2026-01-01").unwrap();
        assert_eq!(f.models.len(), 2);
        assert_eq!(f.models["claude-opus-5-5"].cache_read, Some(0.2));
        assert_eq!(f.models["gpt-5.3-codex"].output, 14.0);
        assert!(!f.models.contains_key("gemini-3-pro-preview"));
    }

    #[test]
    fn speed_and_context_tiers() {
        let j = r#"{
          "gpt-6.2-astra": {"mode": "responses",
            "input_cost_per_token": 8e-6, "output_cost_per_token": 4e-5, "cache_read_input_token_cost": 4e-7,
            "input_cost_per_token_above_272k_tokens": 1.6e-5, "output_cost_per_token_above_272k_tokens": 6e-5,
            "cache_read_input_token_cost_above_272k_tokens": 8e-7,
            "input_cost_per_token_priority": 1.6e-5, "output_cost_per_token_priority": 8e-5,
            "cache_read_input_token_cost_priority": 8e-7,
            "input_cost_per_token_above_272k_tokens_priority": 3.2e-5, "output_cost_per_token_above_272k_tokens_priority": 1.2e-4,
            "input_cost_per_token_ultrafast": 2.4e-5, "output_cost_per_token_ultrafast": 1.2e-4},
          "claude-opus-9": {"input_cost_per_token": 5e-6, "output_cost_per_token": 2.5e-5,
            "input_cost_per_token_above_200k_tokens": 1e-5, "provider_specific_entry": {"fast": 6}}
        }"#;
        let f = from_litellm(j, "2026-01-01").unwrap();
        let g = f.models["gpt-6.2-astra"];
        assert_eq!(
            (g.long_context_tokens, g.input_above_200k, g.cache_read_above_200k),
            (Some(272_000), Some(16.0), Some(0.8))
        );
        assert_eq!(g.fast, Some(Rates { input: 16.0, output: 80.0, cache_read: 0.8, cache_write: 16.0 }));
        // No cache price listed for the long fast tier: the normal ratio (0.4 : 8) of its input price.
        assert_eq!(g.fast_long, Some(Rates { input: 32.0, output: 120.0, cache_read: 1.6, cache_write: 32.0 }));
        assert_eq!(g.ultrafast.map(|r| (r.input, r.cache_read)), Some((24.0, 1.2)));
        assert_eq!(g.ultrafast_long, None);
        let c = f.models["claude-opus-9"];
        assert_eq!(
            (c.fast_multiplier, c.fast, c.long_context_tokens, c.input_above_200k),
            (Some(6.0), None, None, Some(10.0))
        );
    }

    #[test]
    fn t3_price_list() {
        let dir = tempfile::tempdir().unwrap();
        let (rates, converted) = (dir.path().join("usage-model-rates.json"), dir.path().join("cache/pricing-t3.json"));
        assert!(from_t3(&rates, &converted).is_none(), "no T3 list, no layer");
        let list = |output: f64| {
            format!(
                r#"{{"fetchedAtMs":1795158054818,"document":{{"gpt-6.2-astra":{{"input_cost_per_token":8e-6,"output_cost_per_token":{output},"mode":"responses"}},"claude-opus-4-6":{{"input_cost_per_token":5e-6,"output_cost_per_token":2.5e-5,"mode":"chat"}}}}}}"#
            )
        };
        std::fs::write(&rates, list(4e-5)).unwrap();
        let f = from_t3(&rates, &converted).unwrap();
        assert_eq!((f.models["gpt-6.2-astra"].input, f.models["gpt-6.2-astra"].output), (8.0, 40.0));
        assert!(f.as_of.starts_with("2026-11-"), "{}", f.as_of);
        let builtin: PriceFile = serde_json::from_str(BUILTIN).unwrap();
        assert_eq!(f.models["claude-opus-4-6"].fast_multiplier, builtin.models["claude-opus-4-6"].fast_multiplier);
        // The converted table is reused until T3 writes a new list.
        assert!(converted.exists());
        assert_eq!(from_t3(&rates, &converted).unwrap().source, f.source);
        std::fs::write(&rates, list(6e-5) + " ").unwrap();
        assert_eq!(from_t3(&rates, &converted).unwrap().models["gpt-6.2-astra"].output, 60.0);
        std::fs::write(&rates, "not json at all").unwrap();
        assert!(from_t3(&rates, &converted).is_none());
    }
}
