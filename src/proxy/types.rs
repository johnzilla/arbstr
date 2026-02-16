//! OpenAI-compatible request and response types.

use serde::{Deserialize, Serialize};

/// Chat completion request (OpenAI-compatible).
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
}

/// A chat message.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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

impl ChatCompletionRequest {
    /// Extract the user's prompt from messages (last user message).
    pub fn user_prompt(&self) -> Option<&str> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
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

    /// Helper to build a minimal ChatCompletionRequest for testing.
    fn minimal_request() -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "gpt-4o".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "hello".to_string(),
                name: None,
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
