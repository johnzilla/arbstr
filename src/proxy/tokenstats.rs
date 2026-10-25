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
    /// tokenstats' own feed status from `/health` (`ok`, `degraded`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feed_status: Option<String>,
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
            feed_status: None,
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
        self.inner.read().unwrap_or_else(|e| e.into_inner()).clone()
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
    /// `ok` | `degraded` | `down` | `starting` | `offline` (tokenstats ≥ Oct 2026).
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    feed: Option<HealthFeed>,
}

#[derive(Debug, Default, Deserialize)]
struct HealthFeed {
    #[serde(default)]
    reasons: Vec<String>,
}

/// Centralized catalogs: one shared key per source, always polled by tokenstats.
const REFERENCE_SOURCES: &[&str] = &["openrouter", "orcarouter"];
/// Source names tokenstats publishes; any other bare key is a node provider_id.
const KNOWN_SOURCES: &[&str] = &["openrouter", "orcarouter", "routstr", "nostr"];
/// Ask for the whole (filtered) board; tokenstats caps this itself.
const QUOTE_LIMIT: usize = 20_000;

fn is_reference_source(source: &str) -> bool {
    REFERENCE_SOURCES
        .iter()
        .any(|s| s.eq_ignore_ascii_case(source))
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
    /// Endpoint catalog-fetch success rate over 24h (tokenstats ≥ Oct 2026;
    /// persisted, so steadier than `/api/nodes` 1h `reliability`).
    #[serde(default)]
    pub reliability_24h: Option<f64>,
    /// Node's `/v1/info` npub matches its announcing Nostr key.
    #[serde(default)]
    pub identity_verified: Option<bool>,
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
    unverified_identity: bool,
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

/// Why a feed-advertised endpoint may not receive a key, or None if it may.
///
/// The feed decides market endpoint URLs, and routing a request there sends
/// the resolved key as a bearer token. So new market endpoints must be https
/// and on an operator-approved host (`[tokenstats] allowed_hosts`).
fn endpoint_refusal(endpoint: &str, allowed_hosts: &[String]) -> Option<(&'static str, String)> {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return Some((
            "insecure_endpoint",
            format!("Endpoint {endpoint} is not a valid URL"),
        ));
    };
    if url.scheme() != "https" {
        return Some((
            "insecure_endpoint",
            format!("Endpoint {endpoint} is not https; keys are never sent in cleartext"),
        ));
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let allowed = allowed_hosts.iter().any(|h| {
        let h = h.trim().trim_start_matches("*.").to_ascii_lowercase();
        !h.is_empty() && (host == h || host.ends_with(&format!(".{h}")))
    });
    if allowed {
        None
    } else {
        Some((
            "endpoint_not_allowed",
            format!(
                "Host {host} is not in [tokenstats] allowed_hosts; add it to route here \
                 with your configured key"
            ),
        ))
    }
}

/// A usable quote price: present, finite, non-negative and within the cap.
fn valid_price(price: Option<f64>, cap: f64) -> Option<f64> {
    price.filter(|p| p.is_finite() && *p >= 0.0 && *p <= cap)
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
    if is_reference_source(source) {
        let src = source.to_ascii_lowercase();
        format!(
            "Set [tokenstats.keys] {src} = \"${{{}_API_KEY}}\" (shared for all {src} quotes)",
            src.to_ascii_uppercase()
        )
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
) -> (Vec<QuoteGroup>, usize, usize) {
    let mut map: HashMap<(String, String, String), QuoteGroup> = HashMap::new();
    let mut stale_count = 0usize;
    let mut invalid_price_count = 0usize;

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

        // Prefer the quote's own 24h figure; fall back to /api/nodes (older
        // tokenstats). Unknown reliability is not trusted, except for the
        // reference catalogs on a tokenstats that doesn't report it yet.
        let rel = q
            .reliability_24h
            .or_else(|| reliability.get(q.provider_id.as_str()).copied());
        let low_rel = match rel {
            Some(r) => r < cfg.min_reliability,
            None => !is_reference_source(&q.source),
        };
        let unverified = cfg.require_verified_identity
            && !is_reference_source(&q.source)
            && q.identity_verified != Some(true);

        // A missing, negative or absurd price is dropped, not read as free:
        // the cheapest quote wins routing, and the highest sets reserves
        let (Some(price_in), Some(price_out)) = (
            valid_price(q.price_in_sats, cfg.max_price_sats_per_1m),
            valid_price(q.price_out_sats, cfg.max_price_sats_per_1m),
        ) else {
            invalid_price_count += 1;
            continue;
        };
        let rates = ModelRates::new(price_in, price_out, 0.0);

        let key = (q.source.clone(), q.provider_id.clone(), endpoint.clone());
        let g = map.entry(key).or_insert_with(|| QuoteGroup {
            source: q.source.clone(),
            provider_id: q.provider_id.clone(),
            provider: q.provider.clone(),
            endpoint,
            models: HashMap::new(),
            reliability: rel,
            low_reliability: low_rel,
            unverified_identity: unverified,
        });
        g.models.insert(q.model.clone(), rates);
        if low_rel {
            g.low_reliability = true;
        }
        if unverified {
            g.unverified_identity = true;
        }
        if g.reliability.is_none() {
            g.reliability = rel;
        }
    }

    (
        map.into_values().collect(),
        stale_count,
        invalid_price_count,
    )
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
    // Unknown stays unknown (it used to default to 1.0, i.e. trusted).
    let reliability: HashMap<&str, f64> = nodes
        .iter()
        .filter_map(|n| Some((n.provider_id.as_str(), n.reliability?)))
        .collect();

    let (groups, stale_count, invalid_price_count) = build_groups(quotes, &reliability, cfg, now);

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
            let hint = match group.reliability {
                Some(r) => format!(
                    "Node reliability {:.0}% (24h) is below min_reliability ({:.0}%). \
                     Lower [tokenstats].min_reliability or wait for healthier polls.",
                    r * 100.0,
                    cfg.min_reliability * 100.0
                ),
                None => "Node reliability is unknown (tokenstats hasn't polled it yet); \
                         it becomes routable once it has a track record."
                    .to_string(),
            };
            skipped.push(SkipEntry {
                source: group.source.clone(),
                provider: group.provider.clone(),
                provider_id: group.provider_id.clone(),
                endpoint: Some(group.endpoint.clone()),
                models_count: group.models.len(),
                reason: "low_reliability".to_string(),
                hint,
                reliability: group.reliability,
            });
            continue;
        }

        if group.unverified_identity {
            skipped.push(SkipEntry {
                source: group.source.clone(),
                provider: group.provider.clone(),
                provider_id: group.provider_id.clone(),
                endpoint: Some(group.endpoint.clone()),
                models_count: group.models.len(),
                reason: "unverified_identity".to_string(),
                hint: "require_verified_identity is on and tokenstats could not confirm \
                       this node's npub matches its Nostr announcement."
                    .to_string(),
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

        // New market endpoint: must be https on an allowed host before any key
        // is attached to it
        if let Some((reason, hint)) = endpoint_refusal(&group.endpoint, &cfg.allowed_hosts) {
            skipped.push(SkipEntry {
                source: group.source.clone(),
                provider: group.provider.clone(),
                provider_id: group.provider_id.clone(),
                endpoint: Some(group.endpoint.clone()),
                models_count: group.models.len(),
                reason: reason.to_string(),
                hint,
                reliability: group.reliability,
            });
            continue;
        }

        // ...and require a key
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

    if invalid_price_count > 0 {
        skipped.push(SkipEntry {
            source: "*".to_string(),
            provider: "*".to_string(),
            provider_id: "*".to_string(),
            endpoint: None,
            models_count: invalid_price_count,
            reason: "invalid_price".to_string(),
            hint: format!(
                "{invalid_price_count} quote(s) had a missing, negative or non-finite price, \
                 or one above max_price_sats_per_1m={}",
                cfg.max_price_sats_per_1m
            ),
            reliability: None,
        });
    }

    let market_count = market_providers.len();
    static_out.extend(market_providers);
    (static_out, skipped, market_count)
}

fn fmt_reasons(reasons: &[String]) -> String {
    if reasons.is_empty() {
        String::new()
    } else {
        format!(": {}", reasons.join("; "))
    }
}

/// Which tokenstats sources to fetch, as a comma list (`None` = all).
///
/// arbstr can only route where it has a key, and only overlays rates onto
/// static providers whose endpoint matches a quote, so there is no point
/// downloading the whole board (~8k quotes, mostly Routstr) every poll. An
/// explicit `[tokenstats] source` wins; otherwise sources come from
/// `[tokenstats.keys]` (a bare key that isn't a source name is a node
/// provider_id, i.e. routstr) plus reference catalogs matching a static
/// provider's host. With neither, fetch everything (observability only).
fn quote_sources(cfg: &TokenstatsConfig, static_providers: &[ProviderConfig]) -> Option<String> {
    if let Some(src) = cfg
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(src.to_string());
    }
    let mut sources = std::collections::BTreeSet::new();
    for (key, value) in &cfg.keys {
        if value.is_empty() {
            continue;
        }
        let key = key.to_ascii_lowercase();
        let source = match key.split_once(':') {
            Some((src, _)) => src.to_string(),
            None if KNOWN_SOURCES.contains(&key.as_str()) => key,
            None => "routstr".to_string(),
        };
        sources.insert(source);
    }
    for p in static_providers {
        let host = reqwest::Url::parse(&p.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
            .unwrap_or_default();
        for (domain, source) in [
            ("openrouter.ai", "openrouter"),
            ("orcarouter.ai", "orcarouter"),
        ] {
            if host == domain || host.ends_with(&format!(".{domain}")) {
                sources.insert(source.to_string());
            }
        }
    }
    (!sources.is_empty()).then(|| sources.into_iter().collect::<Vec<_>>().join(","))
}

/// `/api/quotes` with server-side quality filters: chat models only (arbstr
/// proxies chat completions), fresher than `stale_after_secs`, and no price
/// outliers (the cheapest quote wins routing, so a node priced 1000x too low
/// would otherwise attract traffic). Local checks stay as defense in depth.
fn quotes_url(
    base: &str,
    cfg: &TokenstatsConfig,
    sources: Option<&str>,
) -> Result<reqwest::Url, String> {
    let limit = QUOTE_LIMIT.to_string();
    let max_age = cfg.stale_after_secs.to_string();
    let ratio = cfg.blend_ratio.to_string();
    let mut params = vec![
        ("limit", limit.as_str()),
        ("kind", "chat"),
        ("max_age_secs", max_age.as_str()),
        ("exclude_outliers", "1"),
    ];
    if let Some(src) = sources {
        params.push(("source", src));
    }
    if (cfg.blend_ratio - 3.0).abs() > f64::EPSILON {
        params.push(("ratio", ratio.as_str()));
    }
    reqwest::Url::parse_with_params(&format!("{base}/api/quotes"), &params)
        .map_err(|e| e.to_string())
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
        feed_status: None,
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
                let reasons = h.feed.map(|f| f.reasons).unwrap_or_default();
                status.feed_status = h.status.clone();
                match h.status.as_deref() {
                    // No trustworthy data: same as unreachable.
                    Some(s @ ("down" | "starting")) => {
                        status.last_error = Some(format!("tokenstats feed status {s}"));
                        status.notes.push(format!(
                            "tokenstats feed is {s} — keeping last provider set \
                             (static + previous market){}",
                            fmt_reasons(&reasons)
                        ));
                        market.set(status);
                        return;
                    }
                    Some("degraded") => status.notes.push(format!(
                        "tokenstats feed is degraded{}",
                        fmt_reasons(&reasons)
                    )),
                    Some("offline") => status.notes.push(
                        "tokenstats is serving a snapshot (polling disabled); \
                         quotes older than stale_after_secs are dropped"
                            .to_string(),
                    ),
                    _ => {}
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

    let sources = quote_sources(cfg, static_providers);
    let quotes_url = match quotes_url(base, cfg, sources.as_deref()) {
        Ok(u) => u,
        Err(e) => {
            status.last_error = Some(format!("bad tokenstats url: {e}"));
            market.set(status);
            return;
        }
    };
    match &sources {
        Some(s) if cfg.source.is_none() => status.notes.push(format!(
            "Fetching sources {s} (from [tokenstats.keys] and static provider hosts); \
             set [tokenstats] source to widen"
        )),
        _ => {}
    }

    let quotes: Vec<Quote> = match client
        .get(quotes_url)
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
            reliability_24h: None,
            identity_verified: None,
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
            allowed_hosts: vec!["openrouter.ai".to_string(), "node.example.com".to_string()],
            max_price_sats_per_1m: 1_000_000.0,
            require_verified_identity: false,
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
        let (providers, _, _) = merge_quotes_into_providers(&static_p, &[], &[], &cfg, Utc::now());
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

    fn node_cfg() -> TokenstatsConfig {
        let mut keys = HashMap::new();
        keys.insert("node-a".to_string(), "cashu".to_string());
        cfg_with_keys(keys)
    }

    fn node_quote(rel_24h: Option<f64>, identity: Option<bool>) -> Quote {
        let mut q = sample_quote(
            "routstr",
            "node-a",
            "gpt-4o",
            "https://node.example.com",
            50.0,
        );
        q.reliability_24h = rel_24h;
        q.identity_verified = identity;
        q
    }

    #[test]
    fn quote_reliability_24h_wins_over_nodes_1h() {
        let nodes = vec![NodeInfo {
            provider_id: "node-a".to_string(),
            reliability: Some(0.1),
        }];
        let quotes = vec![node_quote(Some(0.95), None)];
        let (_, skipped, market) =
            merge_quotes_into_providers(&[], &quotes, &nodes, &node_cfg(), Utc::now());
        assert_eq!(market, 1, "{skipped:?}");
    }

    #[test]
    fn unknown_node_reliability_is_not_trusted() {
        // Used to default to 1.0; now a node with no track record waits.
        let nodes = vec![NodeInfo {
            provider_id: "node-a".to_string(),
            reliability: None,
        }];
        let quotes = vec![node_quote(None, None)];
        let (_, skipped, market) =
            merge_quotes_into_providers(&[], &quotes, &nodes, &node_cfg(), Utc::now());
        assert_eq!(market, 0);
        assert_eq!(skipped[0].reason, "low_reliability");
        assert!(skipped[0].hint.contains("unknown"), "{}", skipped[0].hint);
    }

    #[test]
    fn reference_catalogs_ok_without_reliability_but_not_when_low() {
        let cfg = openrouter_cfg();
        let mut q = sample_quote(
            "openrouter",
            "openrouter",
            "m",
            "https://openrouter.ai/api/v1",
            5.0,
        );
        // Older tokenstats: no figure at all → still routable.
        assert_eq!(merge(&cfg, vec![q.clone()]).2, 1);
        // Reported and below the floor → skipped like any node.
        q.reliability_24h = Some(0.2);
        let (_, skipped, market) = merge(&cfg, vec![q]);
        assert_eq!(market, 0);
        assert_eq!(skipped[0].reason, "low_reliability");
    }

    #[test]
    fn require_verified_identity_skips_unverified_nodes() {
        let mut cfg = node_cfg();
        cfg.require_verified_identity = true;
        for (identity, routable) in [(Some(true), 1), (Some(false), 0), (None, 0)] {
            let quotes = vec![node_quote(Some(1.0), identity)];
            let (_, skipped, market) =
                merge_quotes_into_providers(&[], &quotes, &[], &cfg, Utc::now());
            assert_eq!(market, routable, "{identity:?}");
            if routable == 0 {
                assert_eq!(skipped[0].reason, "unverified_identity");
            }
        }
        // Reference catalogs have no Nostr identity to verify.
        let mut cfg = openrouter_cfg();
        cfg.require_verified_identity = true;
        let q = sample_quote(
            "openrouter",
            "openrouter",
            "m",
            "https://openrouter.ai/api/v1",
            5.0,
        );
        assert_eq!(merge(&cfg, vec![q]).2, 1);
    }

    fn static_provider(url: &str) -> ProviderConfig {
        let mut p = openrouter_cfg_static();
        p.url = url.to_string();
        p
    }

    fn openrouter_cfg_static() -> ProviderConfig {
        let quotes = vec![sample_quote(
            "openrouter",
            "openrouter",
            "m",
            "https://openrouter.ai/api/v1",
            5.0,
        )];
        merge(&openrouter_cfg(), quotes).0.remove(0)
    }

    #[test]
    fn sources_come_from_keys_static_hosts_or_explicit_config() {
        let mut keys = HashMap::new();
        keys.insert("openrouter".to_string(), "k".to_string());
        keys.insert("node-a".to_string(), "cashu".to_string());
        keys.insert("orcarouter:orcarouter".to_string(), "k".to_string());
        keys.insert("unset".to_string(), String::new());
        let cfg = cfg_with_keys(keys);
        assert_eq!(
            quote_sources(&cfg, &[]).as_deref(),
            Some("openrouter,orcarouter,routstr")
        );

        let bare = cfg_with_keys(HashMap::new());
        assert_eq!(quote_sources(&bare, &[]), None, "no keys: fetch everything");
        let statics = [static_provider("https://openrouter.ai/api/v1")];
        assert_eq!(
            quote_sources(&bare, &statics).as_deref(),
            Some("openrouter")
        );

        let mut explicit = cfg.clone();
        explicit.source = Some("routstr".to_string());
        assert_eq!(
            quote_sources(&explicit, &statics).as_deref(),
            Some("routstr")
        );
    }

    #[test]
    fn quotes_url_carries_quality_filters() {
        let mut cfg = openrouter_cfg();
        cfg.stale_after_secs = 300;
        cfg.blend_ratio = 4.0;
        let url = quotes_url("https://tokenstats.ai", &cfg, Some("openrouter,routstr")).unwrap();
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(url.path(), "/api/quotes");
        assert_eq!(q["kind"], "chat");
        assert_eq!(q["max_age_secs"], "300");
        assert_eq!(q["exclude_outliers"], "1");
        assert_eq!(q["source"], "openrouter,routstr");
        assert_eq!(q["ratio"], "4");
        assert_eq!(q["limit"], QUOTE_LIMIT.to_string());
        let all = quotes_url("https://tokenstats.ai", &openrouter_cfg(), None).unwrap();
        assert!(!all.query().unwrap().contains("source="));
        assert!(!all.query().unwrap().contains("ratio="));
    }

    #[test]
    fn orcarouter_key_hint_is_shared_like_openrouter() {
        let hint = key_hint("orcarouter", "orcarouter");
        assert!(
            hint.contains("orcarouter = \"${ORCAROUTER_API_KEY}\""),
            "{hint}"
        );
    }

    #[test]
    fn health_response_parses_new_and_old_shapes() {
        let new: HealthResponse = serde_json::from_str(
            r#"{"ok":true,"quotes":10,"status":"degraded","feed":{"reasons":["openrouter catalog is stale"]}}"#,
        )
        .unwrap();
        assert_eq!(new.status.as_deref(), Some("degraded"));
        assert_eq!(new.feed.unwrap().reasons.len(), 1);
        let old: HealthResponse = serde_json::from_str(r#"{"ok":true,"quotes":10}"#).unwrap();
        assert!(old.status.is_none() && old.feed.is_none());
    }

    fn openrouter_cfg() -> TokenstatsConfig {
        let mut keys = HashMap::new();
        keys.insert("openrouter".to_string(), "sk-or-secret".to_string());
        cfg_with_keys(keys)
    }

    fn merge(
        cfg: &TokenstatsConfig,
        quotes: Vec<Quote>,
    ) -> (Vec<ProviderConfig>, Vec<SkipEntry>, usize) {
        merge_quotes_into_providers(&[], &quotes, &[], cfg, Utc::now())
    }

    #[test]
    fn source_key_not_sent_to_unlisted_host() {
        // A feed advertising an "openrouter" quote at an attacker host must not
        // get the openrouter key attached
        let quotes = vec![sample_quote(
            "openrouter",
            "openrouter",
            "gpt-4o",
            "https://attacker.example/v1",
            1.0,
        )];
        let (providers, skipped, market) = merge(&openrouter_cfg(), quotes);
        assert_eq!(market, 0);
        assert!(providers.is_empty());
        assert_eq!(skipped[0].reason, "endpoint_not_allowed");
        assert!(skipped[0].hint.contains("allowed_hosts"));
    }

    #[test]
    fn host_match_is_exact_or_subdomain() {
        let cfg = openrouter_cfg();
        for (endpoint, ok) in [
            ("https://openrouter.ai/api/v1", true),
            ("https://api.openrouter.ai/v1", true),
            ("https://OpenRouter.AI/api/v1", true),
            ("https://openrouter.ai.evil.com/v1", false),
            ("https://evilopenrouter.ai/v1", false),
            ("https://openrouter.ai@evil.com/v1", false),
        ] {
            let (_, skipped, market) = merge(
                &cfg,
                vec![sample_quote(
                    "openrouter",
                    "openrouter",
                    "gpt-4o",
                    endpoint,
                    10.0,
                )],
            );
            assert_eq!(market == 1, ok, "{endpoint}: {skipped:?}");
        }
    }

    #[test]
    fn http_endpoint_never_gets_a_key() {
        let quotes = vec![sample_quote(
            "openrouter",
            "openrouter",
            "gpt-4o",
            "http://openrouter.ai/api/v1",
            10.0,
        )];
        let (providers, skipped, _) = merge(&openrouter_cfg(), quotes);
        assert!(providers.is_empty());
        assert_eq!(skipped[0].reason, "insecure_endpoint");
    }

    #[test]
    fn empty_allowlist_admits_no_market_endpoints() {
        let mut cfg = openrouter_cfg();
        cfg.allowed_hosts.clear();
        let quotes = vec![sample_quote(
            "openrouter",
            "openrouter",
            "gpt-4o",
            "https://openrouter.ai/api/v1",
            10.0,
        )];
        let (_, skipped, market) = merge(&cfg, quotes);
        assert_eq!(market, 0);
        assert_eq!(skipped[0].reason, "endpoint_not_allowed");
    }

    #[test]
    fn bad_prices_are_dropped_not_free() {
        let cfg = openrouter_cfg();
        let ep = "https://openrouter.ai/api/v1";
        let mut missing = sample_quote("openrouter", "openrouter", "m-missing", ep, 10.0);
        missing.price_out_sats = None;
        let mut negative = sample_quote("openrouter", "openrouter", "m-negative", ep, 10.0);
        negative.price_in_sats = Some(-5.0);
        let nan = sample_quote("openrouter", "openrouter", "m-nan", ep, f64::NAN);
        let huge = sample_quote("openrouter", "openrouter", "m-huge", ep, 5_000_000.0);
        let ok = sample_quote("openrouter", "openrouter", "m-ok", ep, 10.0);

        let (providers, skipped, market) = merge(&cfg, vec![missing, negative, nan, huge, ok]);
        assert_eq!(market, 1);
        assert_eq!(providers[0].models, vec!["m-ok".to_string()]);
        let invalid = skipped
            .iter()
            .find(|s| s.reason == "invalid_price")
            .unwrap();
        assert_eq!(invalid.models_count, 4);
    }

    #[test]
    fn zero_price_is_valid() {
        let mut q = sample_quote(
            "openrouter",
            "openrouter",
            "free-model",
            "https://openrouter.ai/api/v1",
            0.0,
        );
        q.price_in_sats = Some(0.0);
        let (providers, _, market) = merge(&openrouter_cfg(), vec![q]);
        assert_eq!(market, 1);
        assert_eq!(providers[0].rates_for("free-model").output_rate, 0.0);
    }

    #[test]
    fn price_cap_bounds_overlay_on_static_provider() {
        // The feed can't push a static provider's rates past the cap either
        // (those rates feed frontier_rates, i.e. every reserve)
        let static_p = vec![ProviderConfig {
            name: "or".to_string(),
            url: "https://openrouter.ai/api/v1".to_string(),
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 100.0,
            output_rate: 300.0,
            base_fee: 0.0,
            model_rates: HashMap::new(),
            tier: Tier::Standard,
            auto_discover: false,
            source: None,
            provider_id: None,
        }];
        let quotes = vec![sample_quote(
            "openrouter",
            "openrouter",
            "gpt-4o",
            "https://openrouter.ai/api/v1",
            9e12,
        )];
        let (providers, skipped, _) =
            merge_quotes_into_providers(&static_p, &quotes, &[], &openrouter_cfg(), Utc::now());
        assert_eq!(providers[0].output_rate, 300.0);
        assert!(skipped.iter().any(|s| s.reason == "invalid_price"));
    }
}
