//! Converting and downloading LiteLLM's public price list.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::{cache_path, Price, PriceFile, BUILTIN};

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
        let price = Price {
            input,
            output,
            cache_read: per_m(v, "cache_read_input_token_cost"),
            cache_write: per_m(v, "cache_creation_input_token_cost"),
            cache_write_1h: per_m(v, "cache_creation_input_token_cost_above_1hr"),
            input_above_200k: per_m(v, "input_cost_per_token_above_200k_tokens"),
            output_above_200k: per_m(v, "output_cost_per_token_above_200k_tokens"),
            cache_read_above_200k: per_m(v, "cache_read_input_token_cost_above_200k_tokens"),
            fast_multiplier: None,
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
    // Keep fast-mode multipliers from the built-in table (LiteLLM has none).
    let builtin: PriceFile = serde_json::from_str(BUILTIN)?;
    for (k, p) in builtin.models {
        if let (Some(m), Some(e)) = (p.fast_multiplier, file.models.get_mut(&k)) {
            e.fast_multiplier = Some(m);
        }
    }
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
}
