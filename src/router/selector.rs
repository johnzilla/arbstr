//! Provider selection logic.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use crate::config::{ApiKey, ModelRates, PolicyRule, ProviderConfig, Tier};
use crate::error::{Error, Result};

/// Default blend ratio for ranking (1:3 in:out), matching tokenstats.
pub const DEFAULT_BLEND_RATIO: f64 = 3.0;

/// A provider selected for routing (rates resolved for the requested model).
#[derive(Debug, Clone)]
pub struct SelectedProvider {
    pub name: String,
    pub url: String,
    pub api_key: Option<ApiKey>,
    /// Sats per 1M input tokens for the selected model
    pub input_rate: f64,
    /// Sats per 1M output tokens for the selected model
    pub output_rate: f64,
    /// Base fee per request in sats
    pub base_fee: f64,
    pub tier: Tier,
    /// Market source if known (openrouter, routstr, …)
    pub source: Option<String>,
}

impl SelectedProvider {
    fn from_config(config: &ProviderConfig, model: &str) -> Self {
        let rates = config.rates_for(model);
        Self {
            name: config.name.clone(),
            url: config.url.clone(),
            api_key: config.api_key.clone(),
            input_rate: rates.input_rate,
            output_rate: rates.output_rate,
            base_fee: rates.base_fee,
            tier: config.tier,
            source: config.source.clone(),
        }
    }

    /// Routing score: blended rate (sats/1M) + base_fee (sats/request).
    pub fn routing_score(&self, blend_ratio: f64) -> f64 {
        let rates = ModelRates::new(self.input_rate, self.output_rate, self.base_fee);
        rates.blended(blend_ratio) + self.base_fee
    }
}

/// Router for selecting providers.
///
/// Provider list is behind a lock so the tokenstats poller can hot-swap
/// market-backed providers without restarting the process.
#[derive(Debug, Clone)]
pub struct Router {
    providers: Arc<RwLock<Vec<ProviderConfig>>>,
    policy_rules: Vec<PolicyRule>,
    #[allow(dead_code)]
    // Preserved for future strategy-based dispatch (lowest_latency, round_robin)
    default_strategy: String,
    blend_ratio: f64,
}

impl Router {
    /// Create a new router with the given providers and policies.
    pub fn new(
        providers: Vec<ProviderConfig>,
        policy_rules: Vec<PolicyRule>,
        default_strategy: String,
    ) -> Self {
        Self::with_blend_ratio(providers, policy_rules, default_strategy, DEFAULT_BLEND_RATIO)
    }

    /// Create a router with an explicit blend ratio for cost ranking.
    pub fn with_blend_ratio(
        providers: Vec<ProviderConfig>,
        policy_rules: Vec<PolicyRule>,
        default_strategy: String,
        blend_ratio: f64,
    ) -> Self {
        Self {
            providers: Arc::new(RwLock::new(providers)),
            policy_rules,
            default_strategy,
            blend_ratio: if blend_ratio > 0.0 {
                blend_ratio
            } else {
                DEFAULT_BLEND_RATIO
            },
        }
    }

    /// Replace the full provider list (used by tokenstats merge).
    pub fn replace_providers(&self, providers: Vec<ProviderConfig>) {
        match self.providers.write() {
            Ok(mut guard) => *guard = providers,
            Err(poisoned) => {
                *poisoned.into_inner() = providers;
            }
        }
    }

    /// Snapshot of current providers.
    pub fn providers_snapshot(&self) -> Vec<ProviderConfig> {
        self.providers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Select the best provider for a request.
    pub fn select(
        &self,
        model: &str,
        policy_name: Option<&str>,
        prompt: Option<&str>,
        max_tier: Option<Tier>,
    ) -> Result<SelectedProvider> {
        self.select_candidates(model, policy_name, prompt, max_tier)
            .map(|mut v| v.remove(0))
    }

    /// Select all candidate providers for a request, sorted cheapest-first.
    ///
    /// Sorted by blended workload rate (`(in + r·out)/(1+r)` + base_fee)
    /// with default ratio 3, matching tokenstats.
    pub fn select_candidates(
        &self,
        model: &str,
        policy_name: Option<&str>,
        prompt: Option<&str>,
        max_tier: Option<Tier>,
    ) -> Result<Vec<SelectedProvider>> {
        let policy = self.find_policy(policy_name, prompt);
        let providers = self.providers.read().unwrap_or_else(|e| e.into_inner());

        let mut candidates: Vec<&ProviderConfig> = providers
            .iter()
            .filter(|p| p.models.is_empty() || p.models.iter().any(|m| m == model))
            .collect();

        if candidates.is_empty() {
            return Err(Error::NoProviders {
                model: model.to_string(),
            });
        }

        if let Some(max_tier) = max_tier {
            candidates.retain(|p| p.tier <= max_tier);
            if candidates.is_empty() {
                return Err(Error::NoTierMatch {
                    tier: max_tier,
                    model: model.to_string(),
                });
            }
        }

        if let Some(policy) = &policy {
            candidates = self.apply_policy_constraints(candidates, policy, model)?;
        }

        let ratio = self.blend_ratio;
        candidates.sort_by(|a, b| {
            let sa = a.rates_for(model).blended(ratio) + a.rates_for(model).base_fee;
            let sb = b.rates_for(model).blended(ratio) + b.rates_for(model).base_fee;
            sa.partial_cmp(&sb)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        });

        let mut seen = HashSet::new();
        let unique: Vec<SelectedProvider> = candidates
            .into_iter()
            .filter(|p| seen.insert(p.name.clone()))
            .map(|p| SelectedProvider::from_config(p, model))
            .collect();

        if unique.is_empty() {
            return Err(Error::NoPolicyMatch);
        }

        Ok(unique)
    }

    fn find_policy(&self, policy_name: Option<&str>, prompt: Option<&str>) -> Option<&PolicyRule> {
        if let Some(name) = policy_name {
            if let Some(policy) = self.policy_rules.iter().find(|p| p.name == name) {
                tracing::debug!(policy = %name, "Matched policy by header");
                return Some(policy);
            }
        }

        if let Some(prompt) = prompt {
            let prompt_lower = prompt.to_lowercase();
            for policy in &self.policy_rules {
                if policy
                    .keywords
                    .iter()
                    .any(|kw| prompt_lower.contains(&kw.to_lowercase()))
                {
                    tracing::debug!(policy = %policy.name, "Matched policy by keyword heuristics");
                    return Some(policy);
                }
            }
        }

        None
    }

    fn apply_policy_constraints<'a>(
        &self,
        candidates: Vec<&'a ProviderConfig>,
        policy: &PolicyRule,
        model: &str,
    ) -> Result<Vec<&'a ProviderConfig>> {
        let mut filtered = candidates;

        if !policy.allowed_models.is_empty() && !policy.allowed_models.iter().any(|m| m == model) {
            tracing::warn!(
                model = %model,
                policy = %policy.name,
                "Model not allowed by policy"
            );
            return Err(Error::BadRequest(format!(
                "Model '{}' not allowed by policy '{}'",
                model, policy.name
            )));
        }

        if let Some(max_sats) = policy.max_sats_per_1m_output {
            filtered.retain(|p| p.rates_for(model).output_rate <= max_sats);
        }

        if filtered.is_empty() {
            return Err(Error::NoPolicyMatch);
        }

        Ok(filtered)
    }

    /// Return the most expensive (frontier) rates for a model across all providers.
    ///
    /// Returns `(input_rate, output_rate, base_fee)` in sats per 1M / sats, or `None`.
    pub fn frontier_rates(&self, model: &str) -> Option<(f64, f64, f64)> {
        let providers = self.providers.read().unwrap_or_else(|e| e.into_inner());
        let candidates: Vec<_> = providers
            .iter()
            .filter(|p| p.models.is_empty() || p.models.iter().any(|m| m == model))
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let max_input = candidates
            .iter()
            .map(|p| p.rates_for(model).input_rate)
            .fold(0.0_f64, f64::max);
        let max_output = candidates
            .iter()
            .map(|p| p.rates_for(model).output_rate)
            .fold(0.0_f64, f64::max);
        let max_base = candidates
            .iter()
            .map(|p| p.rates_for(model).base_fee)
            .fold(0.0_f64, f64::max);
        Some((max_input, max_output, max_base))
    }

    /// Get a snapshot of configured providers (compat for callers that want a slice-like view).
    pub fn providers(&self) -> Vec<ProviderConfig> {
        self.providers_snapshot()
    }
}

/// Calculate the actual cost in satoshis for a completed request.
///
/// # Formula
/// `(input_tokens * input_rate + output_tokens * output_rate) / 1_000_000.0 + base_fee`
///
/// Rates are in **sats per 1M tokens** (RIP-05 / tokenstats). Result is `f64`
/// for sub-satoshi precision on cheap models.
pub fn actual_cost_sats(
    input_tokens: u32,
    output_tokens: u32,
    input_rate: f64,
    output_rate: f64,
    base_fee: f64,
) -> f64 {
    let input_cost = input_tokens as f64 * input_rate;
    let output_cost = output_tokens as f64 * output_rate;
    (input_cost + output_cost) / 1_000_000.0 + base_fee
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn pc(
        name: &str,
        url: &str,
        models: &[&str],
        input_rate: f64,
        output_rate: f64,
        base_fee: f64,
    ) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            url: url.to_string(),
            api_key: None,
            models: models.iter().map(|s| s.to_string()).collect(),
            input_rate,
            output_rate,
            base_fee,
            model_rates: HashMap::new(),
            tier: Tier::default(),
            auto_discover: false,
            source: None,
            provider_id: None,
        }
    }

    fn test_providers() -> Vec<ProviderConfig> {
        // Rates in sats/1M (legacy 5/1k → 5000/1M, 15/1k → 15000/1M, etc.)
        vec![
            pc(
                "cheap",
                "https://cheap.example.com/v1",
                &["gpt-4o", "gpt-4o-mini"],
                5_000.0,
                15_000.0,
                0.0,
            ),
            pc(
                "expensive",
                "https://expensive.example.com/v1",
                &["gpt-4o", "claude-3.5-sonnet"],
                10_000.0,
                30_000.0,
                1.0,
            ),
        ]
    }

    #[test]
    fn test_select_cheapest() {
        let router = Router::new(test_providers(), vec![], "cheapest".to_string());
        let selected = router.select("gpt-4o", None, None, None).unwrap();
        assert_eq!(selected.name, "cheap");
    }

    #[test]
    fn test_no_providers_for_model() {
        let router = Router::new(test_providers(), vec![], "cheapest".to_string());
        let result = router.select("nonexistent-model", None, None, None);
        assert!(matches!(result, Err(Error::NoProviders { .. })));
    }

    #[test]
    fn test_base_fee_affects_cheapest_selection() {
        // blended r=3: (5000+3*10000)/4 + 8 = 8750+8 vs (8000+3*15000)/4 + 0 = 13250
        // wait - with high base_fee on low rate:
        // low-rate-high-fee: out=10000, base=8 → blend=(5000+30000)/4=8750 + 8 = 8758
        // high-rate-no-fee: out=15000, base=0 → blend=(8000+45000)/4=13250
        // low-rate-high-fee wins with blend ranking — different from old output+base sort.
        // Preserve old intent with providers where base_fee tips output-only sort:
        // A: out=10000 base=8 → old cost 10008; blend (5k+30k)/4+8=8758
        // B: out=15000 base=0 → old cost 15000; blend (8k+45k)/4=13250
        // Still A wins. Use case where B has lower blend:
        // A: in=5k out=10k base=50 → blend 8750+50=8800
        // B: in=8k out=12k base=0 → blend 11000
        // A still wins. For B to win need higher A cost:
        // A: out=10k base=8000 → 8750+8000=16750
        // B: out=15k base=0 → 13250 → B wins
        let providers = vec![
            pc(
                "low-rate-high-fee",
                "https://a.example.com/v1",
                &["gpt-4o"],
                5_000.0,
                10_000.0,
                8_000.0,
            ),
            pc(
                "high-rate-no-fee",
                "https://b.example.com/v1",
                &["gpt-4o"],
                8_000.0,
                15_000.0,
                0.0,
            ),
        ];
        let router = Router::new(providers, vec![], "cheapest".to_string());
        let selected = router.select("gpt-4o", None, None, None).unwrap();
        assert_eq!(selected.name, "high-rate-no-fee");
    }

    #[test]
    fn test_actual_cost_calculation() {
        // rates per 1M: 10_000 and 30_000 (= legacy 10 and 30 per 1k)
        // (100*10000 + 200*30000)/1e6 + 1 = 7 + 1 = 8
        let cost1 = actual_cost_sats(100, 200, 10_000.0, 30_000.0, 1.0);
        assert!((cost1 - 8.0).abs() < 1e-9, "Case 1: expected 8.0, got {cost1}");

        // (10*5000 + 5*15000)/1e6 = 0.125
        let cost2 = actual_cost_sats(10, 5, 5_000.0, 15_000.0, 0.0);
        assert!((cost2 - 0.125).abs() < 1e-9, "Case 2: expected 0.125, got {cost2}");

        let cost3 = actual_cost_sats(0, 0, 10_000.0, 30_000.0, 5.0);
        assert!((cost3 - 5.0).abs() < 1e-9, "Case 3: expected 5.0, got {cost3}");

        let cost4 = actual_cost_sats(1000, 1000, 10_000.0, 30_000.0, 0.0);
        assert!((cost4 - 40.0).abs() < 1e-9, "Case 4: expected 40.0, got {cost4}");
    }

    #[test]
    fn test_actual_cost_fractional_sats() {
        let cost = actual_cost_sats(10, 5, 5_000.0, 15_000.0, 0.0);
        assert!(cost > 0.0);
        assert!((cost - 0.125).abs() < 1e-9);
    }

    #[test]
    fn test_policy_keyword_matching() {
        let policies = vec![PolicyRule {
            name: "code".to_string(),
            allowed_models: vec!["gpt-4o".to_string()],
            strategy: "lowest_cost".to_string(),
            max_sats_per_1m_output: Some(20_000.0),
            keywords: vec!["function".to_string(), "code".to_string()],
        }];
        let router = Router::new(test_providers(), policies, "cheapest".to_string());
        let selected = router
            .select("gpt-4o", None, Some("Write a function to sort"), None)
            .unwrap();
        assert_eq!(selected.name, "cheap");
    }

    #[test]
    fn test_select_candidates_returns_ordered_list() {
        let providers = vec![
            pc(
                "medium",
                "https://medium.example.com/v1",
                &["gpt-4o"],
                8_000.0,
                20_000.0,
                5.0,
            ),
            pc(
                "cheapest",
                "https://cheapest.example.com/v1",
                &["gpt-4o"],
                3_000.0,
                10_000.0,
                0.0,
            ),
            pc(
                "pricey",
                "https://pricey.example.com/v1",
                &["gpt-4o"],
                15_000.0,
                40_000.0,
                10.0,
            ),
        ];
        let router = Router::new(providers, vec![], "cheapest".to_string());
        let candidates = router
            .select_candidates("gpt-4o", None, None, None)
            .unwrap();
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].name, "cheapest");
        assert_eq!(candidates[1].name, "medium");
        assert_eq!(candidates[2].name, "pricey");
    }

    #[test]
    fn test_select_candidates_deduplicates_by_name() {
        let providers = vec![
            pc(
                "alpha",
                "https://alpha-expensive.example.com/v1",
                &["gpt-4o"],
                10_000.0,
                30_000.0,
                5.0,
            ),
            pc(
                "alpha",
                "https://alpha-cheap.example.com/v1",
                &["gpt-4o"],
                3_000.0,
                10_000.0,
                0.0,
            ),
            pc(
                "beta",
                "https://beta.example.com/v1",
                &["gpt-4o"],
                5_000.0,
                15_000.0,
                2.0,
            ),
        ];
        let router = Router::new(providers, vec![], "cheapest".to_string());
        let candidates = router
            .select_candidates("gpt-4o", None, None, None)
            .unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].name, "alpha");
        assert_eq!(candidates[0].output_rate, 10_000.0);
        assert_eq!(candidates[1].name, "beta");
    }

    #[test]
    fn test_select_delegates_to_candidates() {
        let router = Router::new(test_providers(), vec![], "cheapest".to_string());
        let selected = router.select("gpt-4o", None, None, None).unwrap();
        let candidates = router
            .select_candidates("gpt-4o", None, None, None)
            .unwrap();
        assert_eq!(selected.name, candidates[0].name);
        assert_eq!(selected.url, candidates[0].url);
        assert_eq!(selected.output_rate, candidates[0].output_rate);
        assert_eq!(selected.base_fee, candidates[0].base_fee);
    }

    #[test]
    fn test_select_candidates_filters_by_model() {
        let providers = vec![
            pc(
                "has-model",
                "https://a.example.com/v1",
                &["gpt-4o"],
                5_000.0,
                15_000.0,
                0.0,
            ),
            pc(
                "no-model",
                "https://b.example.com/v1",
                &["claude-3.5-sonnet"],
                3_000.0,
                10_000.0,
                0.0,
            ),
        ];
        let router = Router::new(providers, vec![], "cheapest".to_string());
        let candidates = router
            .select_candidates("gpt-4o", None, None, None)
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "has-model");
    }

    fn tiered_providers() -> Vec<ProviderConfig> {
        vec![
            {
                let mut p = pc(
                    "local-cheap",
                    "https://local.example.com/v1",
                    &["gpt-4o"],
                    1_000.0,
                    5_000.0,
                    0.0,
                );
                p.tier = Tier::Local;
                p
            },
            {
                let mut p = pc(
                    "standard-mid",
                    "https://standard.example.com/v1",
                    &["gpt-4o"],
                    5_000.0,
                    15_000.0,
                    1.0,
                );
                p.tier = Tier::Standard;
                p
            },
            {
                let mut p = pc(
                    "frontier-expensive",
                    "https://frontier.example.com/v1",
                    &["gpt-4o"],
                    10_000.0,
                    30_000.0,
                    2.0,
                );
                p.tier = Tier::Frontier;
                p
            },
        ]
    }

    #[test]
    fn test_tier_filter_local_only() {
        let router = Router::new(tiered_providers(), vec![], "cheapest".to_string());
        let candidates = router
            .select_candidates("gpt-4o", None, None, Some(Tier::Local))
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "local-cheap");
    }

    #[test]
    fn test_tier_filter_standard_includes_local() {
        let router = Router::new(tiered_providers(), vec![], "cheapest".to_string());
        let candidates = router
            .select_candidates("gpt-4o", None, None, Some(Tier::Standard))
            .unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].name, "local-cheap");
        assert_eq!(candidates[1].name, "standard-mid");
    }

    #[test]
    fn test_tier_filter_frontier_includes_all() {
        let router = Router::new(tiered_providers(), vec![], "cheapest".to_string());
        let candidates = router
            .select_candidates("gpt-4o", None, None, Some(Tier::Frontier))
            .unwrap();
        assert_eq!(candidates.len(), 3);
    }

    #[test]
    fn test_tier_filter_none_includes_all() {
        let router = Router::new(tiered_providers(), vec![], "cheapest".to_string());
        let candidates = router
            .select_candidates("gpt-4o", None, None, None)
            .unwrap();
        assert_eq!(candidates.len(), 3);
    }

    #[test]
    fn test_tier_filter_no_match_returns_error() {
        let mut p = pc(
            "frontier-only",
            "https://frontier.example.com/v1",
            &["gpt-4o"],
            10_000.0,
            30_000.0,
            2.0,
        );
        p.tier = Tier::Frontier;
        let router = Router::new(vec![p], vec![], "cheapest".to_string());
        let result = router.select_candidates("gpt-4o", None, None, Some(Tier::Local));
        assert!(matches!(result, Err(Error::NoTierMatch { .. })));
    }

    #[test]
    fn test_select_candidates_empty_returns_error() {
        let router = Router::new(test_providers(), vec![], "cheapest".to_string());
        let result = router.select_candidates("nonexistent-model", None, None, None);
        assert!(matches!(result, Err(Error::NoProviders { .. })));
    }

    #[test]
    fn test_frontier_rates_returns_max_across_tiers() {
        let router = Router::new(tiered_providers(), vec![], "cheapest".to_string());
        let rates = router.frontier_rates("gpt-4o");
        assert_eq!(rates, Some((10_000.0, 30_000.0, 2.0)));
    }

    #[test]
    fn test_frontier_rates_single_tier_returns_that_tier() {
        let mut p = pc(
            "local-only",
            "https://local.example.com/v1",
            &["gpt-4o"],
            1_000.0,
            5_000.0,
            0.0,
        );
        p.tier = Tier::Local;
        let router = Router::new(vec![p], vec![], "cheapest".to_string());
        assert_eq!(router.frontier_rates("gpt-4o"), Some((1_000.0, 5_000.0, 0.0)));
    }

    #[test]
    fn test_frontier_rates_nonexistent_model_returns_none() {
        let router = Router::new(tiered_providers(), vec![], "cheapest".to_string());
        assert_eq!(router.frontier_rates("nonexistent-model"), None);
    }

    #[test]
    fn test_per_model_rates_override_defaults() {
        let mut p = pc(
            "multi",
            "https://m.example.com/v1",
            &["cheap-model", "dear-model"],
            50_000.0,
            100_000.0,
            0.0,
        );
        p.model_rates.insert(
            "cheap-model".to_string(),
            ModelRates::new(1_000.0, 2_000.0, 0.0),
        );
        let router = Router::new(vec![p], vec![], "cheapest".to_string());
        let cheap = router.select("cheap-model", None, None, None).unwrap();
        assert_eq!(cheap.output_rate, 2_000.0);
        let dear = router.select("dear-model", None, None, None).unwrap();
        assert_eq!(dear.output_rate, 100_000.0);
    }

    #[test]
    fn test_replace_providers_hot_swap() {
        let router = Router::new(test_providers(), vec![], "cheapest".to_string());
        assert!(router.select("gpt-4o", None, None, None).is_ok());
        router.replace_providers(vec![pc(
            "only",
            "https://only.example.com/v1",
            &["other"],
            1.0,
            1.0,
            0.0,
        )]);
        assert!(matches!(
            router.select("gpt-4o", None, None, None),
            Err(Error::NoProviders { .. })
        ));
        assert!(router.select("other", None, None, None).is_ok());
    }
}
