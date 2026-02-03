//! Provider selection logic.

use crate::config::{PolicyRule, ProviderConfig};
use crate::error::{Error, Result};

/// A provider selected for routing.
#[derive(Debug, Clone)]
pub struct SelectedProvider {
    pub name: String,
    pub url: String,
    pub api_key: Option<String>,
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

        // Select based on strategy
        let strategy = policy
            .map(|p| p.strategy.as_str())
            .unwrap_or(&self.default_strategy);

        let selected = match strategy {
            "lowest_cost" | "cheapest" => self.select_cheapest(&candidates),
            "lowest_latency" => self.select_first(&candidates), // TODO: track latency
            "round_robin" => self.select_first(&candidates),    // TODO: implement
            _ => self.select_cheapest(&candidates),
        };

        selected
            .map(SelectedProvider::from)
            .ok_or(Error::NoPolicyMatch)
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
        if !policy.allowed_models.is_empty() {
            if !policy.allowed_models.iter().any(|m| m == model) {
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
        }

        // Filter by max cost
        if let Some(max_sats) = policy.max_sats_per_1k_output {
            filtered = filtered
                .into_iter()
                .filter(|p| p.output_rate <= max_sats)
                .collect();
        }

        if filtered.is_empty() {
            return Err(Error::NoPolicyMatch);
        }

        Ok(filtered)
    }

    /// Select the cheapest provider (by output rate, since that dominates cost).
    fn select_cheapest<'a>(&self, candidates: &[&'a ProviderConfig]) -> Option<&'a ProviderConfig> {
        candidates.iter().min_by_key(|p| p.output_rate).copied()
    }

    /// Select the first provider (placeholder for latency-based selection).
    fn select_first<'a>(&self, candidates: &[&'a ProviderConfig]) -> Option<&'a ProviderConfig> {
        candidates.first().copied()
    }

    /// Get all configured providers.
    pub fn providers(&self) -> &[ProviderConfig] {
        &self.providers
    }
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
        assert!((cost1 - 8.0).abs() < f64::EPSILON, "Case 1: expected 8.0, got {cost1}");

        // Case 2: (10*5 + 5*15)/1000.0 + 0 = 0.125
        let cost2 = actual_cost_sats(10, 5, 5, 15, 0);
        assert!((cost2 - 0.125).abs() < f64::EPSILON, "Case 2: expected 0.125, got {cost2}");

        // Case 3: (0*10 + 0*30)/1000.0 + 5 = 5.0 (base_fee only)
        let cost3 = actual_cost_sats(0, 0, 10, 30, 5);
        assert!((cost3 - 5.0).abs() < f64::EPSILON, "Case 3: expected 5.0, got {cost3}");

        // Case 4: (1000*10 + 1000*30)/1000.0 + 0 = 40.0
        let cost4 = actual_cost_sats(1000, 1000, 10, 30, 0);
        assert!((cost4 - 40.0).abs() < f64::EPSILON, "Case 4: expected 40.0, got {cost4}");
    }

    #[test]
    fn test_actual_cost_fractional_sats() {
        // Verify sub-sat precision: (10*5 + 5*15)/1000.0 = 0.125, not 0
        let cost = actual_cost_sats(10, 5, 5, 15, 0);
        assert!(cost > 0.0, "Fractional sats must be preserved, got {cost}");
        assert!((cost - 0.125).abs() < f64::EPSILON, "Expected 0.125, got {cost}");
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
}
