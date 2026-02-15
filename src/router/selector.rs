//! Provider selection logic.

use std::collections::HashSet;

use crate::config::{ApiKey, PolicyRule, ProviderConfig};
use crate::error::{Error, Result};

/// A provider selected for routing.
#[derive(Debug, Clone)]
pub struct SelectedProvider {
    pub name: String,
    pub url: String,
    pub api_key: Option<ApiKey>,
    pub input_rate: u64,
    pub output_rate: u64,
    pub base_fee: u64,
}

impl From<&ProviderConfig> for SelectedProvider {
    fn from(config: &ProviderConfig) -> Self {
        Self {
            name: config.name.clone(),
            url: config.url.clone(),
            api_key: config.api_key.clone(),
            input_rate: config.input_rate,
            output_rate: config.output_rate,
            base_fee: config.base_fee,
        }
    }
}

/// Router for selecting providers.
#[derive(Debug, Clone)]
pub struct Router {
    providers: Vec<ProviderConfig>,
    policy_rules: Vec<PolicyRule>,
    #[allow(dead_code)]
    // Preserved for future strategy-based dispatch (lowest_latency, round_robin)
    default_strategy: String,
}

impl Router {
    /// Create a new router with the given providers and policies.
    pub fn new(
        providers: Vec<ProviderConfig>,
        policy_rules: Vec<PolicyRule>,
        default_strategy: String,
    ) -> Self {
        Self {
            providers,
            policy_rules,
            default_strategy,
        }
    }

    /// Select the best provider for a request.
    ///
    /// Returns the cheapest single provider that matches the model and policy
    /// constraints. Delegates to [`select_candidates`] and returns the first
    /// (cheapest) candidate.
    ///
    /// # Arguments
    /// * `model` - The requested model name
    /// * `policy_name` - Optional policy name from X-Arbstr-Policy header
    /// * `prompt` - The user's prompt (for heuristic matching)
    pub fn select(
        &self,
        model: &str,
        policy_name: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<SelectedProvider> {
        self.select_candidates(model, policy_name, prompt)
            .map(|mut v| v.remove(0))
    }

    /// Select all candidate providers for a request, sorted cheapest-first.
    ///
    /// Returns a `Vec<SelectedProvider>` filtered by model and policy
    /// constraints, sorted by routing cost (`output_rate + base_fee`
    /// ascending), and deduplicated by provider name (keeping the cheapest
    /// entry for each name).
    ///
    /// # Arguments
    /// * `model` - The requested model name
    /// * `policy_name` - Optional policy name from X-Arbstr-Policy header
    /// * `prompt` - The user's prompt (for heuristic matching)
    pub fn select_candidates(
        &self,
        model: &str,
        policy_name: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<Vec<SelectedProvider>> {
        // Find matching policy
        let policy = self.find_policy(policy_name, prompt);

        // Filter providers by model support
        let mut candidates: Vec<&ProviderConfig> = self
            .providers
            .iter()
            .filter(|p| p.models.is_empty() || p.models.iter().any(|m| m == model))
            .collect();

        if candidates.is_empty() {
            return Err(Error::NoProviders {
                model: model.to_string(),
            });
        }

        // Apply policy constraints if present
        if let Some(policy) = &policy {
            candidates = self.apply_policy_constraints(candidates, policy, model)?;
        }

        // Sort by routing cost (output_rate + base_fee), cheapest first
        candidates.sort_by_key(|p| p.output_rate + p.base_fee);

        // Deduplicate by provider name (keep first occurrence = cheapest)
        let mut seen = HashSet::new();
        let unique: Vec<SelectedProvider> = candidates
            .into_iter()
            .filter(|p| seen.insert(p.name.clone()))
            .map(SelectedProvider::from)
            .collect();

        if unique.is_empty() {
            return Err(Error::NoPolicyMatch);
        }

        Ok(unique)
    }

    /// Find a matching policy by name or heuristics.
    fn find_policy(&self, policy_name: Option<&str>, prompt: Option<&str>) -> Option<&PolicyRule> {
        // First try explicit policy name
        if let Some(name) = policy_name {
            if let Some(policy) = self.policy_rules.iter().find(|p| p.name == name) {
                tracing::debug!(policy = %name, "Matched policy by header");
                return Some(policy);
            }
        }

        // Fall back to keyword heuristics
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

    /// Apply policy constraints to filter providers.
    fn apply_policy_constraints<'a>(
        &self,
        candidates: Vec<&'a ProviderConfig>,
        policy: &PolicyRule,
        model: &str,
    ) -> Result<Vec<&'a ProviderConfig>> {
        let mut filtered = candidates;

        // Filter by allowed models
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

        // Filter by max cost
        if let Some(max_sats) = policy.max_sats_per_1k_output {
            filtered.retain(|p| p.output_rate <= max_sats);
        }

        if filtered.is_empty() {
            return Err(Error::NoPolicyMatch);
        }

        Ok(filtered)
    }

    /// Get all configured providers.
    pub fn providers(&self) -> &[ProviderConfig] {
        &self.providers
    }
}

/// Calculate the actual cost in satoshis for a completed request.
///
/// # Formula
/// `(input_tokens * input_rate + output_tokens * output_rate) / 1000.0 + base_fee`
///
/// Rates are in sats per 1000 tokens. The result is an `f64` to preserve
/// sub-satoshi precision (important for cheap models with small token counts).
pub fn actual_cost_sats(
    input_tokens: u32,
    output_tokens: u32,
    input_rate: u64,
    output_rate: u64,
    base_fee: u64,
) -> f64 {
    let input_cost = input_tokens as f64 * input_rate as f64;
    let output_cost = output_tokens as f64 * output_rate as f64;
    (input_cost + output_cost) / 1000.0 + base_fee as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_providers() -> Vec<ProviderConfig> {
        vec![
            ProviderConfig {
                name: "cheap".to_string(),
                url: "https://cheap.example.com/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()],
                input_rate: 5,
                output_rate: 15,
                base_fee: 0,
            },
            ProviderConfig {
                name: "expensive".to_string(),
                url: "https://expensive.example.com/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o".to_string(), "claude-3.5-sonnet".to_string()],
                input_rate: 10,
                output_rate: 30,
                base_fee: 1,
            },
        ]
    }

    #[test]
    fn test_select_cheapest() {
        let router = Router::new(test_providers(), vec![], "cheapest".to_string());

        let selected = router.select("gpt-4o", None, None).unwrap();
        assert_eq!(selected.name, "cheap");
    }

    #[test]
    fn test_no_providers_for_model() {
        let router = Router::new(test_providers(), vec![], "cheapest".to_string());

        let result = router.select("nonexistent-model", None, None);
        assert!(matches!(result, Err(Error::NoProviders { .. })));
    }

    #[test]
    fn test_base_fee_affects_cheapest_selection() {
        // Case 2 from behavior spec:
        // low-rate-high-fee(output_rate=10, base_fee=8) vs high-rate-no-fee(output_rate=15, base_fee=0)
        // Routing cost: 10+8=18 vs 15+0=15 -> "high-rate-no-fee" wins
        let providers = vec![
            ProviderConfig {
                name: "low-rate-high-fee".to_string(),
                url: "https://a.example.com/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o".to_string()],
                input_rate: 5,
                output_rate: 10,
                base_fee: 8,
            },
            ProviderConfig {
                name: "high-rate-no-fee".to_string(),
                url: "https://b.example.com/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o".to_string()],
                input_rate: 8,
                output_rate: 15,
                base_fee: 0,
            },
        ];

        let router = Router::new(providers, vec![], "cheapest".to_string());
        let selected = router.select("gpt-4o", None, None).unwrap();
        assert_eq!(
            selected.name, "high-rate-no-fee",
            "Provider with lower output_rate+base_fee (15+0=15) should beat (10+8=18)"
        );
    }

    #[test]
    fn test_actual_cost_calculation() {
        // Case 1: (100*10 + 200*30)/1000.0 + 1 = 8.0
        let cost1 = actual_cost_sats(100, 200, 10, 30, 1);
        assert!(
            (cost1 - 8.0).abs() < f64::EPSILON,
            "Case 1: expected 8.0, got {cost1}"
        );

        // Case 2: (10*5 + 5*15)/1000.0 + 0 = 0.125
        let cost2 = actual_cost_sats(10, 5, 5, 15, 0);
        assert!(
            (cost2 - 0.125).abs() < f64::EPSILON,
            "Case 2: expected 0.125, got {cost2}"
        );

        // Case 3: (0*10 + 0*30)/1000.0 + 5 = 5.0 (base_fee only)
        let cost3 = actual_cost_sats(0, 0, 10, 30, 5);
        assert!(
            (cost3 - 5.0).abs() < f64::EPSILON,
            "Case 3: expected 5.0, got {cost3}"
        );

        // Case 4: (1000*10 + 1000*30)/1000.0 + 0 = 40.0
        let cost4 = actual_cost_sats(1000, 1000, 10, 30, 0);
        assert!(
            (cost4 - 40.0).abs() < f64::EPSILON,
            "Case 4: expected 40.0, got {cost4}"
        );
    }

    #[test]
    fn test_actual_cost_fractional_sats() {
        // Verify sub-sat precision: (10*5 + 5*15)/1000.0 = 0.125, not 0
        let cost = actual_cost_sats(10, 5, 5, 15, 0);
        assert!(cost > 0.0, "Fractional sats must be preserved, got {cost}");
        assert!(
            (cost - 0.125).abs() < f64::EPSILON,
            "Expected 0.125, got {cost}"
        );
    }

    #[test]
    fn test_policy_keyword_matching() {
        let policies = vec![PolicyRule {
            name: "code".to_string(),
            allowed_models: vec!["gpt-4o".to_string()],
            strategy: "lowest_cost".to_string(),
            max_sats_per_1k_output: Some(20),
            keywords: vec!["function".to_string(), "code".to_string()],
        }];

        let router = Router::new(test_providers(), policies, "cheapest".to_string());

        // Should match "code" policy and select cheap provider (under 20 sats)
        let selected = router
            .select("gpt-4o", None, Some("Write a function to sort"))
            .unwrap();
        assert_eq!(selected.name, "cheap");
    }

    #[test]
    fn test_select_candidates_returns_ordered_list() {
        // Three providers at different costs for gpt-4o
        let providers = vec![
            ProviderConfig {
                name: "medium".to_string(),
                url: "https://medium.example.com/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o".to_string()],
                input_rate: 8,
                output_rate: 20,
                base_fee: 5, // routing cost: 25
            },
            ProviderConfig {
                name: "cheapest".to_string(),
                url: "https://cheapest.example.com/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o".to_string()],
                input_rate: 3,
                output_rate: 10,
                base_fee: 0, // routing cost: 10
            },
            ProviderConfig {
                name: "pricey".to_string(),
                url: "https://pricey.example.com/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o".to_string()],
                input_rate: 15,
                output_rate: 40,
                base_fee: 10, // routing cost: 50
            },
        ];

        let router = Router::new(providers, vec![], "cheapest".to_string());
        let candidates = router.select_candidates("gpt-4o", None, None).unwrap();

        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].name, "cheapest");
        assert_eq!(candidates[1].name, "medium");
        assert_eq!(candidates[2].name, "pricey");
    }

    #[test]
    fn test_select_candidates_deduplicates_by_name() {
        // Two providers with the same name but different rates
        let providers = vec![
            ProviderConfig {
                name: "alpha".to_string(),
                url: "https://alpha-expensive.example.com/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o".to_string()],
                input_rate: 10,
                output_rate: 30,
                base_fee: 5, // routing cost: 35
            },
            ProviderConfig {
                name: "alpha".to_string(),
                url: "https://alpha-cheap.example.com/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o".to_string()],
                input_rate: 3,
                output_rate: 10,
                base_fee: 0, // routing cost: 10
            },
            ProviderConfig {
                name: "beta".to_string(),
                url: "https://beta.example.com/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o".to_string()],
                input_rate: 5,
                output_rate: 15,
                base_fee: 2, // routing cost: 17
            },
        ];

        let router = Router::new(providers, vec![], "cheapest".to_string());
        let candidates = router.select_candidates("gpt-4o", None, None).unwrap();

        // alpha appears twice in config but should only appear once in results
        assert_eq!(candidates.len(), 2);
        // cheapest alpha (cost 10) kept, expensive alpha (cost 35) removed
        assert_eq!(candidates[0].name, "alpha");
        assert_eq!(candidates[0].output_rate, 10);
        assert_eq!(candidates[1].name, "beta");
    }

    #[test]
    fn test_select_delegates_to_candidates() {
        let router = Router::new(test_providers(), vec![], "cheapest".to_string());

        let selected = router.select("gpt-4o", None, None).unwrap();
        let candidates = router.select_candidates("gpt-4o", None, None).unwrap();

        assert_eq!(selected.name, candidates[0].name);
        assert_eq!(selected.url, candidates[0].url);
        assert_eq!(selected.output_rate, candidates[0].output_rate);
        assert_eq!(selected.base_fee, candidates[0].base_fee);
    }

    #[test]
    fn test_select_candidates_filters_by_model() {
        let providers = vec![
            ProviderConfig {
                name: "has-model".to_string(),
                url: "https://a.example.com/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o".to_string()],
                input_rate: 5,
                output_rate: 15,
                base_fee: 0,
            },
            ProviderConfig {
                name: "no-model".to_string(),
                url: "https://b.example.com/v1".to_string(),
                api_key: None,
                models: vec!["claude-3.5-sonnet".to_string()],
                input_rate: 3,
                output_rate: 10,
                base_fee: 0,
            },
        ];

        let router = Router::new(providers, vec![], "cheapest".to_string());
        let candidates = router.select_candidates("gpt-4o", None, None).unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "has-model");
    }

    #[test]
    fn test_select_candidates_empty_returns_error() {
        let router = Router::new(test_providers(), vec![], "cheapest".to_string());

        let result = router.select_candidates("nonexistent-model", None, None);
        assert!(matches!(result, Err(Error::NoProviders { .. })));
    }
}
