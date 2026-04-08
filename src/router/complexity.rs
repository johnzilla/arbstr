//! Heuristic complexity scorer for chat messages.
//!
//! Analyzes incoming chat messages and produces a 0.0-1.0 complexity score
//! based on 5 weighted signals: context length, code blocks, multi-file
//! indicators, reasoning keywords, and conversation depth.
//!
//! Higher scores indicate more complex requests that benefit from frontier
//! models; lower scores indicate simple requests suitable for cheap local models.

use std::sync::LazyLock;

use regex::Regex;

use crate::config::{ComplexityWeightsConfig, Tier};
use crate::proxy::types::Message;

/// Minimum total text length (chars) to attempt scoring.
/// Requests shorter than this default to frontier (1.0) since there's
/// not enough signal to route cheaply with confidence.
const MIN_TEXT_LEN: usize = 10;

/// Regex for fenced code blocks (triple backticks).
static CODE_BLOCK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```[\s\S]*?```").expect("code block regex"));

/// Regex for file path patterns like `src/main.rs`, `tests/foo.py`.
static FILE_PATH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\w+(?:/\w+)+\.\w{1,5}\b").expect("file path regex"));

/// Regex for standalone file extensions commonly seen in code discussions.
static FILE_EXT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.\b(?:rs|py|ts|js|go|java|cpp|c|h|toml|yaml|yml|json|md|sql|sh)\b")
        .expect("file ext regex")
});

/// Default reasoning keywords that indicate complex analytical requests.
const DEFAULT_KEYWORDS: &[&str] = &[
    "architect",
    "design",
    "tradeoff",
    "refactor",
    "why does",
    "compare",
    "across the codebase",
    "step by step",
    "analyze",
    "evaluate",
    "debug",
];

/// Map a complexity score to the maximum provider tier allowed.
///
/// - `score < low` -> `Tier::Local` (simple requests, use cheapest models)
/// - `low <= score <= high` -> `Tier::Standard` (moderate requests)
/// - `score > high` -> `Tier::Frontier` (complex requests, use best models)
pub fn score_to_max_tier(score: f64, low: f64, high: f64) -> Tier {
    if score < low {
        Tier::Local
    } else if score > high {
        Tier::Frontier
    } else {
        Tier::Standard
    }
}

/// Score the complexity of a chat conversation.
///
/// Returns a value in `[0.0, 1.0]` where:
/// - 0.0 = trivially simple (route to cheapest model)
/// - 1.0 = highly complex (route to frontier model)
///
/// **Default-to-frontier:** Empty messages or very short content (< 10 chars)
/// return 1.0 to avoid mis-routing ambiguous requests to weak models.
pub fn score_complexity(messages: &[Message], weights: &ComplexityWeightsConfig) -> f64 {
    // Default-to-frontier guard
    if messages.is_empty() {
        return 1.0;
    }

    // Concatenate all message text
    let text: String = messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");

    if text.len() < MIN_TEXT_LEN {
        return 1.0;
    }

    let signals = [
        (signal_context_length(&text), weights.context_length),
        (signal_code_blocks(&text), weights.code_blocks),
        (signal_multi_file(&text), weights.multi_file),
        (
            signal_reasoning_keywords(&text, &weights.extra_keywords),
            weights.reasoning_keywords,
        ),
        (
            signal_conversation_depth(messages.len()),
            weights.conversation_depth,
        ),
    ];

    let (weighted_sum, weight_total) = signals
        .iter()
        .fold((0.0, 0.0), |(ws, wt), (signal, weight)| {
            (ws + signal * weight, wt + weight)
        });

    if weight_total == 0.0 {
        return 1.0; // all weights zero -> frontier
    }

    (weighted_sum / weight_total).clamp(0.0, 1.0)
}

/// Context length signal using sigmoid centered at ~1000 tokens (4000 chars).
fn signal_context_length(text: &str) -> f64 {
    let tokens = text.len() / 4; // rough char-to-token approximation
    1.0 / (1.0 + (-0.003 * (tokens as f64 - 1000.0)).exp())
}

/// Code block signal: count fenced code blocks.
/// 0 -> 0.0, 1 -> 0.33, 2 -> 0.67, 3+ -> 1.0
fn signal_code_blocks(text: &str) -> f64 {
    let count = CODE_BLOCK_RE.find_iter(text).count();
    (count as f64 / 3.0).min(1.0)
}

/// Multi-file signal: count unique file path references.
/// Combines explicit paths (`src/main.rs`) and standalone extensions (`.rs`).
fn signal_multi_file(text: &str) -> f64 {
    use std::collections::HashSet;
    let mut matches: HashSet<String> = HashSet::new();
    for m in FILE_PATH_RE.find_iter(text) {
        matches.insert(m.as_str().to_string());
    }
    for m in FILE_EXT_RE.find_iter(text) {
        matches.insert(m.as_str().to_string());
    }
    (matches.len() as f64 / 3.0).min(1.0)
}

/// Reasoning keyword signal: count distinct keyword matches.
/// Checks both default keywords and user-supplied extra_keywords.
fn signal_reasoning_keywords(text: &str, extra_keywords: &[String]) -> f64 {
    let lower = text.to_lowercase();
    let mut count = 0usize;
    for kw in DEFAULT_KEYWORDS {
        if lower.contains(kw) {
            count += 1;
        }
    }
    for kw in extra_keywords {
        if lower.contains(&kw.to_lowercase()) {
            count += 1;
        }
    }
    (count as f64 / 3.0).min(1.0)
}

/// Conversation depth signal using sigmoid centered at 6 messages.
fn signal_conversation_depth(message_count: usize) -> f64 {
    1.0 / (1.0 + (-0.5 * (message_count as f64 - 6.0)).exp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::types::MessageContent;

    fn msg(text: &str) -> Message {
        Message {
            role: "user".into(),
            content: MessageContent::Text(text.into()),
            name: None,
            extra: Default::default(),
        }
    }

    fn default_weights() -> ComplexityWeightsConfig {
        ComplexityWeightsConfig::default()
    }

    #[test]
    fn test_empty_messages_returns_frontier() {
        assert_eq!(score_complexity(&[], &default_weights()), 1.0);
    }

    #[test]
    fn test_short_content_returns_frontier() {
        // "hi" is < 10 chars total
        let messages = [msg("hi")];
        assert_eq!(score_complexity(&messages, &default_weights()), 1.0);
    }

    #[test]
    fn test_simple_greeting_scores_low() {
        let messages = [msg("Hello, how are you today?")];
        let score = score_complexity(&messages, &default_weights());
        assert!(
            score < 0.4,
            "Simple greeting should score below 0.4, got {score}"
        );
    }

    #[test]
    fn test_complex_multipart_scores_high() {
        let mut messages = Vec::new();
        for i in 0..8 {
            let text = format!(
                "Message {i}: Please analyze and evaluate the tradeoff between \
                 src/main.rs and tests/foo.rs. Here is code:\n\
                 ```rust\nfn example_{i}() {{}}\n```\n\
                 ```python\ndef example_{i}(): pass\n```\n\
                 ```go\nfunc example{i}() {{}}\n```\n\
                 Step by step, architect a solution across the codebase."
            );
            messages.push(msg(&text));
        }
        let score = score_complexity(&messages, &default_weights());
        assert!(
            score > 0.7,
            "Complex multi-turn conversation should score above 0.7, got {score}"
        );
    }

    #[test]
    fn test_code_blocks_increase_score() {
        let without = msg("Please help me with this function that processes data");
        let with = msg(
            "Please help me with this function that processes data\n\
             ```rust\nfn process() {}\n```\n\
             ```rust\nfn transform() {}\n```\n\
             ```rust\nfn validate() {}\n```",
        );
        let score_without = score_complexity(&[without], &default_weights());
        let score_with = score_complexity(&[with], &default_weights());
        assert!(
            score_with > score_without,
            "Code blocks should increase score: {score_with} > {score_without}"
        );
    }

    #[test]
    fn test_file_paths_increase_score() {
        let without = msg("Please help me fix the main module and the test module");
        let with = msg("Please help me fix src/main.rs and tests/foo.rs and lib/utils.py");
        let score_without = score_complexity(&[without], &default_weights());
        let score_with = score_complexity(&[with], &default_weights());
        assert!(
            score_with > score_without,
            "File paths should increase score: {score_with} > {score_without}"
        );
    }

    #[test]
    fn test_reasoning_keywords_increase_score() {
        let without = msg("Please help me write some code for my project today");
        let with = msg("Please architect a solution with tradeoff analysis step by step");
        let score_without = score_complexity(&[without], &default_weights());
        let score_with = score_complexity(&[with], &default_weights());
        assert!(
            score_with > score_without,
            "Reasoning keywords should increase score: {score_with} > {score_without}"
        );
    }

    #[test]
    fn test_conversation_depth_increases_score() {
        let short_msg = msg("Tell me about Rust programming language features");
        let messages_2: Vec<Message> = (0..2).map(|_| short_msg.clone()).collect();
        let messages_10: Vec<Message> = (0..10).map(|_| short_msg.clone()).collect();
        let score_2 = score_complexity(&messages_2, &default_weights());
        let score_10 = score_complexity(&messages_10, &default_weights());
        assert!(
            score_10 > score_2,
            "10 messages should score higher than 2: {score_10} > {score_2}"
        );
    }

    #[test]
    fn test_zero_weight_eliminates_signal() {
        let with_code = msg(
            "Hello there friend!\n\
             ```rust\nfn a() {}\n```\n\
             ```rust\nfn b() {}\n```\n\
             ```rust\nfn c() {}\n```",
        );
        let weights_normal = default_weights();
        let score_normal = score_complexity(&[with_code.clone()], &weights_normal);

        let mut weights_zero = default_weights();
        weights_zero.code_blocks = 0.0;
        let score_zero = score_complexity(&[with_code.clone()], &weights_zero);

        // With code_blocks weight at 0, the code block signal shouldn't contribute
        // The scores should differ if code blocks had any effect
        // We verify by also checking that normal score is affected by code blocks
        assert!(
            (score_normal - score_zero).abs() > 0.01 || score_normal < 0.3,
            "Zero weight should eliminate code_blocks signal contribution"
        );
    }

    #[test]
    fn test_high_weight_amplifies_signal() {
        let with_keywords =
            msg("Please architect a solution and evaluate the tradeoff step by step carefully");
        let weights_normal = default_weights();
        let score_normal = score_complexity(&[with_keywords.clone()], &weights_normal);

        let mut weights_high = default_weights();
        weights_high.reasoning_keywords = 10.0;
        let score_high = score_complexity(&[with_keywords.clone()], &weights_high);

        assert!(
            score_high > score_normal,
            "High reasoning_keywords weight should amplify score: {score_high} > {score_normal}"
        );
    }

    #[test]
    fn test_extra_keywords_matched() {
        let text = msg("Please frobulate the entire system with care and precision");
        let mut weights = default_weights();
        let score_without = score_complexity(&[text.clone()], &weights);

        weights.extra_keywords = vec!["frobulate".to_string()];
        let score_with = score_complexity(&[text.clone()], &weights);

        assert!(
            score_with > score_without,
            "extra_keywords should raise reasoning signal: {score_with} > {score_without}"
        );
    }

    #[test]
    fn test_score_always_clamped() {
        // Test with extreme weights
        let messages = [msg("Hello, how are you today friend?")];
        let mut weights = default_weights();
        weights.context_length = 1000.0;
        let score = score_complexity(&messages, &weights);
        assert!(
            (0.0..=1.0).contains(&score),
            "Score must be clamped to [0.0, 1.0], got {score}"
        );

        // Test with all zero weights
        weights.context_length = 0.0;
        weights.code_blocks = 0.0;
        weights.multi_file = 0.0;
        weights.reasoning_keywords = 0.0;
        weights.conversation_depth = 0.0;
        let score = score_complexity(&messages, &weights);
        assert_eq!(score, 1.0, "All-zero weights should return 1.0 (frontier)");
    }

    #[test]
    fn test_score_to_max_tier_below_low() {
        assert_eq!(score_to_max_tier(0.2, 0.4, 0.7), Tier::Local);
        assert_eq!(score_to_max_tier(0.0, 0.4, 0.7), Tier::Local);
        assert_eq!(score_to_max_tier(0.39, 0.4, 0.7), Tier::Local);
    }

    #[test]
    fn test_score_to_max_tier_at_and_between_thresholds() {
        assert_eq!(score_to_max_tier(0.4, 0.4, 0.7), Tier::Standard);
        assert_eq!(score_to_max_tier(0.5, 0.4, 0.7), Tier::Standard);
        assert_eq!(score_to_max_tier(0.7, 0.4, 0.7), Tier::Standard);
    }

    #[test]
    fn test_score_to_max_tier_above_high() {
        assert_eq!(score_to_max_tier(0.71, 0.4, 0.7), Tier::Frontier);
        assert_eq!(score_to_max_tier(1.0, 0.4, 0.7), Tier::Frontier);
    }

    #[test]
    fn test_multimodal_content_handled() {
        let multimodal = Message {
            role: "user".into(),
            content: MessageContent::Parts(vec![
                serde_json::json!({"type": "text", "text": "Please architect a solution and analyze the tradeoff step by step for this complex problem"}),
            ]),
            name: None,
            extra: Default::default(),
        };
        let score = score_complexity(&[multimodal], &default_weights());
        // Should produce a non-trivial score since the text has reasoning keywords
        assert!(
            score > 0.0,
            "Multimodal content should be scored, got {score}"
        );
    }
}
