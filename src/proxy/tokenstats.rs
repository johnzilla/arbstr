//! tokenstats market feed client and merge logic.
//!
//! Polls `GET /health`, `GET /api/quotes`, and `GET /api/nodes` from a live
//! tokenstats instance, maps quotes into providers with **per-model** rates
//! (sats per 1M tokens), and only marks market providers **routable** when an
//! API key is resolved. Non-routable market entries are retained for
//! observability (`GET /providers` market section).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::config::{ApiKey, ModelRates, ProviderConfig, Tier, TokenstatsConfig};
use crate::router::Router as ProviderRouter;

/// Why a market source/provider is not available for routing.
#[derive(Debug, Clone, Serialize)]
pub struct SkipEntry {
    pub source: String,
    pub provider: String,
    pub provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    pub models_count: usize,
    pub reason: String,
    pub hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reliability: Option<f64>,
}

/// Snapshot of market feed status for API / ops visibility.
#[derive(Debug, Clone, Serialize)]
pub struct MarketStatus {
    pub enabled: bool,
    pub url: Option<String>,
    pub last_poll_at: Option<String>,
    pub last_success_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub quotes_seen: usize,
    pub nodes_seen: usize,
    pub routable_market_providers: usize,
    pub skipped: Vec<SkipEntry>,
    pub notes: Vec<String>,
}

impl Default for MarketStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            url: None,
            last_poll_at: None,
            last_success_at: None,
            last_error: None,
            quotes_seen: 0,
            nodes_seen: 0,
            routable_market_providers: 0,
            skipped: Vec::new(),
            notes: vec![
                "tokenstats feed not configured — using static [[providers]] only".to_string(),
            ],
        }
    }
}

/// Shared market status for handlers.
#[derive(Clone, Default)]
pub struct MarketState {
    inner: Arc<RwLock<MarketStatus>>,
}

impl MarketState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> MarketStatus {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set(&self, status: MarketStatus) {
        match self.inner.write() {
            Ok(mut g) => *g = status,
            Err(p) => *p.into_inner() = status,
        }
    }
}

#[derive(Debug, Deserialize)]
struct HealthResponse {
    ok: Option<bool>,
    quotes: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Quote {
    pub source: String,
    pub provider: String,
    pub provider_id: String,
    pub model: String,
    pub price_in_sats: Option<f64>,
    pub price_out_sats: Option<f64>,
    pub endpoint: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeInfo {
    pub provider_id: String,
    pub reliability: Option<f64>,
}

#[derive(Clone)]
struct QuoteGroup {
    source: String,
    provider_id: String,
    provider: String,
    endpoint: String,
    models: HashMap<String, ModelRates>,
    reliability: Option<f64>,
    low_reliability: bool,
}

/// Normalize an OpenAI-compatible base URL to end with `/v1`.
pub fn normalize_endpoint(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.ends_with("/v1") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

/// Resolve API key for a market provider from tokenstats.keys.
///
/// Lookup order: `provider_id`, source name (e.g. `openrouter`), `source:provider_id`.
pub fn resolve_market_key(
    keys: &HashMap<String, String>,
    source: &str,
    provider_id: &str,
) -> Option<ApiKey> {
    for k in [
        provider_id.to_string(),
        source.to_string(),
        format!("{source}:{provider_id}"),
    ] {
        if let Some(v) = keys.get(&k) {
            if !v.is_empty() {
                return Some(ApiKey::from(v.as_str()));
            }
        }
    }
    None
}

fn key_hint(source: &str, provider_id: &str) -> String {
    if source.eq_ignore_ascii_case("openrouter") {
        "Set [tokenstats.keys] openrouter = \"${OPENROUTER_API_KEY}\" (shared for all OpenRouter quotes)"
            .to_string()
    } else {
        format!(
            "Set [tokenstats.keys] \"{provider_id}\" = \"${{CASHU_OR_API_KEY}}\" \
             (or \"{source}\" for all {source} nodes)"
        )
    }
}

fn sanitize_name(source: &str, provider_id: &str, display: &str) -> String {
    let base = if !display.is_empty() {
        display
    } else {
        provider_id
    };
    let slug: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        format!("{source}-{provider_id}")
    } else {
        format!("{source}-{slug}")
    }
}

fn build_groups(
    quotes: &[Quote],
    reliability: &HashMap<&str, f64>,
    cfg: &TokenstatsConfig,
    now: DateTime<Utc>,
) -> (Vec<QuoteGroup>, usize) {
    let mut map: HashMap<(String, String, String), QuoteGroup> = HashMap::new();
    let mut stale_count = 0usize;

    for q in quotes {
        if let Some(filter) = &cfg.source {
            if !q.source.eq_ignore_ascii_case(filter) {
                continue;
            }
        }

        if let Some(observed) = q.observed_at {
            let age = now.signed_duration_since(observed).num_seconds();
            if age > cfg.stale_after_secs as i64 {
                stale_count += 1;
                continue;
            }
        }

        let Some(endpoint_raw) = q.endpoint.as_deref() else {
            continue;
        };
        let endpoint = normalize_endpoint(endpoint_raw);
        if endpoint.is_empty() {
            continue;
        }

        let rel = reliability.get(q.provider_id.as_str()).copied();
        let low_rel = if q.source.eq_ignore_ascii_case("openrouter") {
            false
        } else {
            rel.map(|r| r < cfg.min_reliability).unwrap_or(false)
        };

        let rates = ModelRates::new(
            q.price_in_sats.unwrap_or(0.0),
            q.price_out_sats.unwrap_or(0.0),
            0.0,
        );

        let key = (q.source.clone(), q.provider_id.clone(), endpoint.clone());
        let g = map.entry(key).or_insert_with(|| QuoteGroup {
            source: q.source.clone(),
            provider_id: q.provider_id.clone(),
            provider: q.provider.clone(),
            endpoint,
            models: HashMap::new(),
            reliability: rel,
            low_reliability: low_rel,
        });
        g.models.insert(q.model.clone(), rates);
        if low_rel {
            g.low_reliability = true;
        }
        if g.reliability.is_none() {
            g.reliability = rel;
        }
    }

    (map.into_values().collect(), stale_count)
}

/// Group quotes into providers; split routable vs skipped by key resolution.
///
/// Static providers are always kept. Matching URLs get model rate overlays from
/// the market. New market endpoints become providers only when a key resolves.
pub fn merge_quotes_into_providers(
    static_providers: &[ProviderConfig],
    quotes: &[Quote],
    nodes: &[NodeInfo],
    cfg: &TokenstatsConfig,
    now: DateTime<Utc>,
) -> (Vec<ProviderConfig>, Vec<SkipEntry>, usize) {
    let reliability: HashMap<&str, f64> = nodes
        .iter()
        .map(|n| (n.provider_id.as_str(), n.reliability.unwrap_or(1.0)))
        .collect();

    let (groups, stale_count) = build_groups(quotes, &reliability, cfg, now);

    let mut static_out: Vec<ProviderConfig> = static_providers.to_vec();
    // Index static_out by normalized URL for overlay
    let static_idx: HashMap<String, usize> = static_out
        .iter()
        .enumerate()
        .map(|(i, p)| (normalize_endpoint(&p.url), i))
        .collect();

    let mut market_providers = Vec::new();
    let mut skipped: Vec<SkipEntry> = Vec::new();

    for group in &groups {
        if group.low_reliability {
            skipped.push(SkipEntry {
                source: group.source.clone(),
                provider: group.provider.clone(),
                provider_id: group.provider_id.clone(),
                endpoint: Some(group.endpoint.clone()),
                models_count: group.models.len(),
                reason: "low_reliability".to_string(),
                hint: format!(
                    "Node reliability {:.0}% is below min_reliability ({:.0}%). \
                     Raise [tokenstats].min_reliability or wait for healthier polls.",
                    group.reliability.unwrap_or(0.0) * 100.0,
                    cfg.min_reliability * 100.0
                ),
                reliability: group.reliability,
            });
            continue;
        }

        // Overlay onto matching static provider
        if let Some(&idx) = static_idx.get(&group.endpoint) {
            let p = &mut static_out[idx];
            for (model, rates) in &group.models {
                p.model_rates.insert(model.clone(), rates.clone());
                if !p.models.iter().any(|m| m == model) {
                    p.models.push(model.clone());
                }
            }
            if let Some(best) = group.models.values().min_by(|a, b| {
                a.blended(cfg.blend_ratio)
                    .partial_cmp(&b.blended(cfg.blend_ratio))
                    .unwrap_or(std::cmp::Ordering::Equal)
            }) {
                p.input_rate = best.input_rate;
                p.output_rate = best.output_rate;
                p.base_fee = best.base_fee;
            }
            if p.source.is_none() {
                p.source = Some(group.source.clone());
            }
            if p.provider_id.is_none() {
                p.provider_id = Some(group.provider_id.clone());
            }
            continue;
        }

        // New market endpoint — require key
        let api_key = resolve_market_key(&cfg.keys, &group.source, &group.provider_id);
        if api_key.is_none() {
            skipped.push(SkipEntry {
                source: group.source.clone(),
                provider: group.provider.clone(),
                provider_id: group.provider_id.clone(),
                endpoint: Some(group.endpoint.clone()),
                models_count: group.models.len(),
                reason: "missing_api_key".to_string(),
                hint: key_hint(&group.source, &group.provider_id),
                reliability: group.reliability,
            });
            continue;
        }

        let default_rates = group
            .models
            .values()
            .min_by(|a, b| {
                a.blended(cfg.blend_ratio)
                    .partial_cmp(&b.blended(cfg.blend_ratio))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
            .unwrap_or_else(|| ModelRates::new(0.0, 0.0, 0.0));

        let models: Vec<String> = group.models.keys().cloned().collect();
        market_providers.push(ProviderConfig {
            name: sanitize_name(&group.source, &group.provider_id, &group.provider),
            url: group.endpoint.clone(),
            api_key,
            models,
            input_rate: default_rates.input_rate,
            output_rate: default_rates.output_rate,
            base_fee: default_rates.base_fee,
            model_rates: group.models.clone(),
            tier: Tier::Standard,
            auto_discover: false,
            source: Some(group.source.clone()),
            provider_id: Some(group.provider_id.clone()),
        });
    }

    if stale_count > 0 {
        skipped.push(SkipEntry {
            source: "*".to_string(),
            provider: "*".to_string(),
            provider_id: "*".to_string(),
            endpoint: None,
            models_count: stale_count,
            reason: "stale_quotes".to_string(),
            hint: format!(
                "{stale_count} quote(s) older than stale_after_secs={} were ignored",
                cfg.stale_after_secs
            ),
            reliability: None,
        });
    }

    let market_count = market_providers.len();
    static_out.extend(market_providers);
    (static_out, skipped, market_count)
}

/// Fetch quotes + nodes from tokenstats and update the router.
pub async fn poll_once(
    client: &Client,
    cfg: &TokenstatsConfig,
    static_providers: &[ProviderConfig],
    router: &ProviderRouter,
    market: &MarketState,
) {
    let base = cfg.url.trim_end_matches('/');
    let prev = market.snapshot();
    let mut status = MarketStatus {
        enabled: true,
        url: Some(base.to_string()),
        last_poll_at: Some(Utc::now().to_rfc3339()),
        last_success_at: prev.last_success_at,
        last_error: None,
        quotes_seen: 0,
        nodes_seen: 0,
        routable_market_providers: 0,
        skipped: Vec::new(),
        notes: Vec::new(),
    };

    match client
        .get(format!("{base}/health"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(h) = resp.json::<HealthResponse>().await {
                if h.ok == Some(false) || h.quotes == Some(0) {
                    status.notes.push(
                        "tokenstats /health reports zero quotes — waiting for catalog data"
                            .to_string(),
                    );
                }
            }
        }
        Ok(resp) => {
            status.last_error = Some(format!("health HTTP {}", resp.status()));
            status.notes.push(
                "tokenstats unhealthy — keeping last provider set (static + previous market)"
                    .to_string(),
            );
            market.set(status);
            return;
        }
        Err(e) => {
            status.last_error = Some(format!("health request failed: {e}"));
            status.notes.push(
                "tokenstats unreachable — keeping last provider set (static + previous market)"
                    .to_string(),
            );
            market.set(status);
            return;
        }
    }

    let mut quotes_url = format!("{base}/api/quotes?limit=2000");
    if let Some(src) = &cfg.source {
        quotes_url.push_str(&format!("&source={src}"));
    }
    if (cfg.blend_ratio - 3.0).abs() > f64::EPSILON {
        quotes_url.push_str(&format!("&ratio={}", cfg.blend_ratio));
    }

    let quotes: Vec<Quote> = match client
        .get(&quotes_url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json().await {
            Ok(q) => q,
            Err(e) => {
                status.last_error = Some(format!("quotes JSON: {e}"));
                market.set(status);
                return;
            }
        },
        Ok(resp) => {
            status.last_error = Some(format!("quotes HTTP {}", resp.status()));
            market.set(status);
            return;
        }
        Err(e) => {
            status.last_error = Some(format!("quotes request failed: {e}"));
            market.set(status);
            return;
        }
    };

    let nodes: Vec<NodeInfo> = match client
        .get(format!("{base}/api/nodes"))
        .timeout(Duration::from_secs(15))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => resp.json().await.unwrap_or_default(),
        _ => Vec::new(),
    };

    status.quotes_seen = quotes.len();
    status.nodes_seen = nodes.len();

    let (providers, skipped, market_count) =
        merge_quotes_into_providers(static_providers, &quotes, &nodes, cfg, Utc::now());

    status.skipped = skipped;
    status.routable_market_providers = market_count;
    status.last_success_at = Some(Utc::now().to_rfc3339());
    status.notes.push(format!(
        "Merged {} total providers ({} static baseline + {} new market). {} skip reason(s).",
        providers.len(),
        static_providers.len(),
        market_count,
        status.skipped.len()
    ));
    if status.skipped.iter().any(|s| s.reason == "missing_api_key") {
        status.notes.push(
            "Sources without keys are observed for pricing but not routable. \
             Configure [tokenstats.keys] to enable routing."
                .to_string(),
        );
    }

    router.replace_providers(providers);
    market.set(status);

    tracing::info!(
        quotes = quotes.len(),
        nodes = nodes.len(),
        market_providers = market_count,
        "tokenstats poll complete"
    );
}

/// Background poll loop. Stops when `cancel` is set true.
pub async fn poll_loop(
    client: Client,
    cfg: TokenstatsConfig,
    static_providers: Vec<ProviderConfig>,
    router: ProviderRouter,
    market: MarketState,
    mut cancel: watch::Receiver<bool>,
) {
    let interval = Duration::from_secs(cfg.poll_interval_secs.max(5));
    poll_once(&client, &cfg, &static_providers, &router, &market).await;

    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                poll_once(&client, &cfg, &static_providers, &router, &market).await;
            }
            _ = cancel.changed() => {
                if *cancel.borrow() {
                    tracing::info!("tokenstats poller shutting down");
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_quote(source: &str, pid: &str, model: &str, endpoint: &str, out: f64) -> Quote {
        Quote {
            source: source.to_string(),
            provider: format!("Provider {pid}"),
            provider_id: pid.to_string(),
            model: model.to_string(),
            price_in_sats: Some(out / 2.0),
            price_out_sats: Some(out),
            endpoint: Some(endpoint.to_string()),
            observed_at: Some(Utc::now()),
        }
    }

    fn cfg_with_keys(keys: HashMap<String, String>) -> TokenstatsConfig {
        TokenstatsConfig {
            url: "http://localhost".to_string(),
            poll_interval_secs: 60,
            min_reliability: 0.5,
            stale_after_secs: 600,
            blend_ratio: 3.0,
            source: None,
            keys,
        }
    }

    #[test]
    fn normalize_adds_v1() {
        assert_eq!(
            normalize_endpoint("https://openrouter.ai/api"),
            "https://openrouter.ai/api/v1"
        );
        assert_eq!(
            normalize_endpoint("https://openrouter.ai/api/v1/"),
            "https://openrouter.ai/api/v1"
        );
    }

    #[test]
    fn missing_key_skips_openrouter() {
        let cfg = cfg_with_keys(HashMap::new());
        let quotes = vec![sample_quote(
            "openrouter",
            "openrouter",
            "gpt-4o",
            "https://openrouter.ai/api/v1",
            100.0,
        )];
        let (providers, skipped, market) =
            merge_quotes_into_providers(&[], &quotes, &[], &cfg, Utc::now());
        assert_eq!(market, 0);
        assert!(providers.is_empty());
        assert_eq!(skipped[0].reason, "missing_api_key");
        assert!(skipped[0].hint.contains("openrouter"));
    }

    #[test]
    fn openrouter_key_makes_routable() {
        let mut keys = HashMap::new();
        keys.insert("openrouter".to_string(), "sk-test".to_string());
        let cfg = cfg_with_keys(keys);
        let quotes = vec![
            sample_quote(
                "openrouter",
                "openrouter",
                "gpt-4o",
                "https://openrouter.ai/api/v1",
                100.0,
            ),
            sample_quote(
                "openrouter",
                "openrouter",
                "claude-3.5-sonnet",
                "https://openrouter.ai/api/v1",
                200.0,
            ),
        ];
        let (providers, skipped, market) =
            merge_quotes_into_providers(&[], &quotes, &[], &cfg, Utc::now());
        assert_eq!(market, 1);
        assert_eq!(providers.len(), 1);
        assert!(skipped.is_empty());
        assert_eq!(providers[0].models.len(), 2);
        assert_eq!(
            providers[0].rates_for("claude-3.5-sonnet").output_rate,
            200.0
        );
        assert!(providers[0].api_key.is_some());
    }

    #[test]
    fn static_providers_always_kept() {
        let static_p = vec![ProviderConfig {
            name: "local".to_string(),
            url: "http://localhost:9337/v1".to_string(),
            api_key: None,
            models: vec!["llama3".to_string()],
            input_rate: 0.0,
            output_rate: 0.0,
            base_fee: 0.0,
            model_rates: HashMap::new(),
            tier: Tier::Local,
            auto_discover: false,
            source: None,
            provider_id: None,
        }];
        let cfg = cfg_with_keys(HashMap::new());
        let (providers, _, _) =
            merge_quotes_into_providers(&static_p, &[], &[], &cfg, Utc::now());
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "local");
    }

    #[test]
    fn low_reliability_skipped() {
        let mut keys = HashMap::new();
        keys.insert("node-a".to_string(), "cashu".to_string());
        let cfg = cfg_with_keys(keys);
        let quotes = vec![sample_quote(
            "routstr",
            "node-a",
            "gpt-4o",
            "https://node.example.com",
            50.0,
        )];
        let nodes = vec![NodeInfo {
            provider_id: "node-a".to_string(),
            reliability: Some(0.1),
        }];
        let (providers, skipped, market) =
            merge_quotes_into_providers(&[], &quotes, &nodes, &cfg, Utc::now());
        assert_eq!(market, 0);
        assert!(providers.is_empty());
        assert_eq!(skipped[0].reason, "low_reliability");
    }
}
