//! OpenAI-compatible request and response types.

use serde::{Deserialize, Serialize};

/// Chat completion request (OpenAI-compatible).
///
/// Known fields are explicitly typed for arbstr's routing and cost logic.
/// Unknown fields (e.g., `tools`, `tool_choice`, `response_format`, `seed`)
/// are captured by `extra` and forwarded to the upstream provider unchanged.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<StopSequence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// A chat message.
///
/// Unknown fields (e.g., `tool_calls`, `tool_call_id`) are captured by
/// `extra` and forwarded to the upstream provider unchanged.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Message {
    pub role: String,
    #[serde(default)]
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Message content can be a plain string or an array of content parts
/// (for multimodal requests with images/audio).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<serde_json::Value>),
}

impl Default for MessageContent {
    fn default() -> Self {
        MessageContent::Text(String::new())
    }
}

impl MessageContent {
    /// Extract the text content as a string slice.
    /// For multimodal content, returns the first text part if present.
    pub fn as_str(&self) -> &str {
        match self {
            MessageContent::Text(s) => s,
            MessageContent::Parts(parts) => parts
                .iter()
                .find_map(|p| {
                    if p.get("type")?.as_str()? == "text" {
                        p.get("text")?.as_str()
                    } else {
                        None
                    }
                })
                .unwrap_or(""),
        }
    }
}

/// Stop sequence can be a string or array of strings.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum StopSequence {
    Single(String),
    Multiple(Vec<String>),
}

/// Options controlling streaming response behavior (OpenAI-compatible).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StreamOptions {
    /// When true, the final streaming chunk includes a usage object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_usage: Option<bool>,
}

/// Chat completion response (OpenAI-compatible).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// arbstr extension: which provider handled this request
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arbstr_provider: Option<String>,
}

/// A completion choice.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    pub finish_reason: Option<String>,
}

/// Token usage statistics.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Streaming chunk response.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
}

/// A streaming choice delta.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: Delta,
    pub finish_reason: Option<String>,
}

/// Delta content in streaming response.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Delta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Tokens a provider adds to every request beyond message content
/// (chat template, assistant priming).
pub const REQUEST_OVERHEAD_TOKENS: u32 = 8;

/// Tokens a provider adds per message (role markers and separators).
pub const PER_MESSAGE_OVERHEAD_TOKENS: u32 = 4;

/// Reserve estimate for one `image_url` part. Providers bill images by pixel
/// size, not by the length of their base64 encoding; this covers the largest
/// image Anthropic processes without downscaling (about 1.15 megapixels,
/// ~1,600 tokens), which is also above OpenAI's high-detail cost for a
/// typical image.
pub const IMAGE_PART_TOKENS: u32 = 1_600;

impl ChatCompletionRequest {
    /// Extract the user's prompt from messages (last user message).
    pub fn user_prompt(&self) -> Option<&str> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
    }

    /// Estimate input and output token counts for cost estimation.
    ///
    /// Input tokens: message content character lengths / 4 (rough heuristic, rounded
    /// up), plus the framing tokens providers bill on top of the content: a fixed
    /// per-request overhead and a per-message overhead for role markers. Without
    /// these, short prompts are underestimated by roughly half (a 52-char prompt
    /// estimated at 13 tokens was billed by Anthropic as 25), the vault reservation
    /// comes in below the real cost, and vault caps the settle at the reservation.
    /// For multimodal `Parts` content, text parts count by length and each image
    /// part counts as [`IMAGE_PART_TOKENS`]; other part types (audio, files) fall
    /// back to their serialized JSON length.
    /// Output tokens: `max_tokens` if set, otherwise `default_output` parameter.
    ///
    /// Returns `(estimated_input_tokens, estimated_output_tokens)`.
    pub fn estimate_tokens(&self, default_output: u32) -> (u32, u32) {
        let total_chars: usize = self.messages.iter().map(|m| m.content.char_len()).sum();
        let images: u32 = self.messages.iter().map(|m| m.content.image_parts()).sum();
        let content = total_chars.div_ceil(4) as u32 + images * IMAGE_PART_TOKENS;
        let framing =
            REQUEST_OVERHEAD_TOKENS + PER_MESSAGE_OVERHEAD_TOKENS * self.messages.len() as u32;
        let input = (content + framing).max(1);
        let output = self.max_tokens.unwrap_or(default_output);
        (input, output)
    }
}

impl MessageContent {
    /// Character length of the content for token estimation.
    ///
    /// For text content, returns the string length. For multimodal Parts, text
    /// parts contribute their text length and image parts nothing (they are
    /// counted by [`Self::image_parts`]); any other part type contributes its
    /// serialized JSON length as a conservative approximation.
    pub fn char_len(&self) -> usize {
        match self {
            MessageContent::Text(s) => s.len(),
            MessageContent::Parts(parts) => parts
                .iter()
                .map(|p| match p.get("type").and_then(|t| t.as_str()) {
                    Some("text") => p.get("text").and_then(|t| t.as_str()).map_or(0, str::len),
                    Some("image_url") => 0,
                    _ => serde_json::to_string(p).map_or(0, |s| s.len()),
                })
                .sum(),
        }
    }

    /// Number of image parts in multimodal content.
    pub fn image_parts(&self) -> u32 {
        match self {
            MessageContent::Text(_) => 0,
            MessageContent::Parts(parts) => parts
                .iter()
                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("image_url"))
                .count() as u32,
        }
    }
}

/// Ensure stream_options includes `include_usage: true` for streaming requests.
///
/// Merges with any existing client-provided stream_options rather than overwriting.
/// Only adds `include_usage: true` if the field is not already set.
pub fn ensure_stream_options(request: &mut ChatCompletionRequest) {
    match &mut request.stream_options {
        Some(opts) => {
            if opts.include_usage.is_none() {
                opts.include_usage = Some(true);
            }
        }
        None => {
            request.stream_options = Some(StreamOptions {
                include_usage: Some(true),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens_includes_framing_overhead() {
        let mut req = minimal_request();
        // Prompt from a live Anthropic request that billed 25 input tokens.
        req.messages[0].content =
            MessageContent::Text("In one short sentence, what is the Lightning Network?".into());
        let (input, _) = req.estimate_tokens(256);
        // 53 chars -> 14 content tokens (rounded up) + 8 request + 4 message
        assert_eq!(input, 26);
        assert!(input >= 25, "estimate must cover the billed 25 tokens");
    }

    #[test]
    fn test_estimate_tokens_scales_overhead_per_message() {
        let mut req = minimal_request();
        // 8 chars: divides evenly by 4, so each copy adds exactly 2 content tokens
        req.messages[0].content = MessageContent::Text("abcdefgh".into());
        let one = req.estimate_tokens(256).0;
        req.messages.push(req.messages[0].clone());
        let two = req.estimate_tokens(256).0;
        assert_eq!(
            one,
            2 + REQUEST_OVERHEAD_TOKENS + PER_MESSAGE_OVERHEAD_TOKENS
        );
        assert_eq!(two - one, 2 + PER_MESSAGE_OVERHEAD_TOKENS);
    }

    #[test]
    fn test_estimate_tokens_counts_images_by_part_not_encoding() {
        let mut req = minimal_request();
        // A ~1 MB base64 image: by encoded length it would estimate ~250k tokens.
        let image = format!("data:image/png;base64,{}", "A".repeat(1_000_000));
        req.messages[0].content = MessageContent::Parts(vec![
            serde_json::json!({"type": "text", "text": "abcdefgh"}),
            serde_json::json!({"type": "image_url", "image_url": {"url": image}}),
        ]);
        let (input, _) = req.estimate_tokens(256);
        assert_eq!(
            input,
            2 + IMAGE_PART_TOKENS + REQUEST_OVERHEAD_TOKENS + PER_MESSAGE_OVERHEAD_TOKENS
        );
    }

    #[test]
    fn test_estimate_tokens_unknown_parts_use_json_length() {
        let mut req = minimal_request();
        let part = serde_json::json!({"type": "input_audio", "input_audio": {"data": "x".repeat(400), "format": "wav"}});
        let len = serde_json::to_string(&part).unwrap().len();
        req.messages[0].content = MessageContent::Parts(vec![part]);
        let (input, _) = req.estimate_tokens(256);
        assert_eq!(
            input,
            len.div_ceil(4) as u32 + REQUEST_OVERHEAD_TOKENS + PER_MESSAGE_OVERHEAD_TOKENS
        );
    }

    /// Helper to build a minimal ChatCompletionRequest for testing.
    fn minimal_request() -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "gpt-4o".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
                name: None,
                extra: Default::default(),
            }],
            temperature: None,
            max_tokens: None,
            stream: Some(true),
            stream_options: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop: None,
            user: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn ensure_stream_options_sets_when_none() {
        let mut req = minimal_request();
        assert!(req.stream_options.is_none());

        ensure_stream_options(&mut req);

        let opts = req.stream_options.as_ref().unwrap();
        assert_eq!(opts.include_usage, Some(true));
    }

    #[test]
    fn ensure_stream_options_sets_when_include_usage_is_none() {
        let mut req = minimal_request();
        req.stream_options = Some(StreamOptions {
            include_usage: None,
        });

        ensure_stream_options(&mut req);

        let opts = req.stream_options.as_ref().unwrap();
        assert_eq!(opts.include_usage, Some(true));
    }

    #[test]
    fn ensure_stream_options_preserves_existing_false() {
        let mut req = minimal_request();
        req.stream_options = Some(StreamOptions {
            include_usage: Some(false),
        });

        ensure_stream_options(&mut req);

        // Should NOT override -- merge strategy only sets when is_none
        let opts = req.stream_options.as_ref().unwrap();
        assert_eq!(opts.include_usage, Some(false));
    }

    #[test]
    fn ensure_stream_options_preserves_existing_true() {
        let mut req = minimal_request();
        req.stream_options = Some(StreamOptions {
            include_usage: Some(true),
        });

        ensure_stream_options(&mut req);

        let opts = req.stream_options.as_ref().unwrap();
        assert_eq!(opts.include_usage, Some(true));
    }

    #[test]
    fn stream_options_not_serialized_when_none() {
        let req = minimal_request();
        assert!(req.stream_options.is_none());

        let json = serde_json::to_string(&req).unwrap();
        assert!(
            !json.contains("stream_options"),
            "stream_options should be absent in JSON when None: {}",
            json
        );
    }

    #[test]
    fn stream_options_serialized_after_ensure() {
        let mut req = minimal_request();
        ensure_stream_options(&mut req);

        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains(r#""stream_options":{"include_usage":true}"#),
            "JSON should contain stream_options with include_usage:true: {}",
            json
        );
    }
}
