//! SSE stream observation module.
//!
//! Provides [`SseObserver`] for line-buffered extraction of usage data
//! and finish_reason from OpenAI-compatible SSE streaming responses.
//! Handles TCP chunk boundary reassembly correctly.

/// Token usage extracted from the final SSE chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// Result of observing an SSE stream to completion.
#[derive(Debug, Clone)]
pub struct StreamResult {
    /// Token usage from the final chunk's usage object, if present.
    pub usage: Option<StreamUsage>,
    /// The finish_reason from the last chunk with a non-null finish_reason.
    pub finish_reason: Option<String>,
    /// Whether `data: [DONE]` was received.
    pub done_received: bool,
}

impl StreamResult {
    /// An empty result for streams that ended without `[DONE]`.
    pub fn empty() -> Self {
        Self {
            usage: None,
            finish_reason: None,
            done_received: false,
        }
    }
}

/// Internal state for SSE line buffering and usage extraction.
///
/// Buffers raw bytes across chunk boundaries, reassembles complete SSE lines,
/// and extracts usage + finish_reason from `data:` lines.
pub(crate) struct SseObserver {
    buffer: Vec<u8>,
    usage: Option<StreamUsage>,
    finish_reason: Option<String>,
    done_received: bool,
}

impl SseObserver {
    /// Create a new observer with empty state.
    pub fn new() -> Self {
        todo!("not yet implemented")
    }

    /// Process a chunk of bytes from the SSE stream.
    pub fn process_chunk(&mut self, _bytes: &[u8]) {
        todo!("not yet implemented")
    }

    /// Flush any remaining content in the buffer as a final line.
    fn flush_buffer(&mut self) {
        todo!("not yet implemented")
    }

    /// Process a single complete SSE line.
    fn process_line(&mut self, _line: &str) {
        todo!("not yet implemented")
    }

    /// Process the data payload of a `data:` SSE line.
    fn process_data(&mut self, _data: &str) {
        todo!("not yet implemented")
    }

    /// Consume the observer and produce the final result.
    ///
    /// Flushes any remaining buffer content, then returns
    /// `StreamResult::empty()` if `[DONE]` was not received.
    pub fn into_result(mut self) -> StreamResult {
        todo!("not yet implemented")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build SSE data from event lines, then split at the given byte positions.
    ///
    /// Each event string is appended with `\n\n` (SSE event delimiter).
    /// The resulting byte buffer is split at the specified positions to
    /// simulate TCP chunk boundaries.
    fn split_sse_at_positions(events: &[&str], split_positions: &[usize]) -> Vec<Vec<u8>> {
        let full: Vec<u8> = events
            .iter()
            .flat_map(|e| format!("{}\n\n", e).into_bytes())
            .collect();

        let mut chunks = Vec::new();
        let mut prev = 0;
        for &pos in split_positions {
            if pos > prev && pos < full.len() {
                chunks.push(full[prev..pos].to_vec());
                prev = pos;
            }
        }
        chunks.push(full[prev..].to_vec());
        chunks
    }

    #[test]
    fn test_single_chunk_full_stream() {
        let events = [
            r#"data: {"id":"abc","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}],"usage":null}"#,
            r#"data: {"id":"abc","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}],"usage":null}"#,
            r#"data: {"id":"abc","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":"stop"}],"usage":null}"#,
            r#"data: {"id":"abc","choices":[],"usage":{"prompt_tokens":6,"completion_tokens":10,"total_tokens":16}}"#,
            "data: [DONE]",
        ];

        let chunks = split_sse_at_positions(&events, &[]);
        assert_eq!(chunks.len(), 1, "Should be a single chunk");

        let mut observer = SseObserver::new();
        observer.process_chunk(&chunks[0]);
        let result = observer.into_result();

        assert!(result.done_received);
        assert_eq!(
            result.usage,
            Some(StreamUsage {
                prompt_tokens: 6,
                completion_tokens: 10,
            })
        );
        assert_eq!(result.finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_usage_split_across_chunks() {
        let events = [
            r#"data: {"id":"abc","choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":"stop"}],"usage":null}"#,
            r#"data: {"id":"abc","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#,
            "data: [DONE]",
        ];

        // Split at multiple positions inside the usage JSON line
        let chunks = split_sse_at_positions(&events, &[50, 120, 180]);
        assert!(chunks.len() > 1, "Should be split into multiple chunks");

        let mut observer = SseObserver::new();
        for chunk in &chunks {
            observer.process_chunk(chunk);
        }
        let result = observer.into_result();

        assert!(result.done_received);
        assert_eq!(
            result.usage,
            Some(StreamUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
            })
        );
        assert_eq!(result.finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_no_usage_with_done() {
        let events = [
            r#"data: {"id":"abc","choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":"stop"}],"usage":null}"#,
            "data: [DONE]",
        ];

        let chunks = split_sse_at_positions(&events, &[]);

        let mut observer = SseObserver::new();
        observer.process_chunk(&chunks[0]);
        let result = observer.into_result();

        assert!(result.done_received);
        assert!(result.usage.is_none());
        assert_eq!(result.finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_no_done_returns_empty() {
        // Stream ends without [DONE] -- should return empty result
        let events = [
            r#"data: {"id":"abc","choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":"stop"}],"usage":null}"#,
        ];

        let chunks = split_sse_at_positions(&events, &[]);

        let mut observer = SseObserver::new();
        observer.process_chunk(&chunks[0]);
        let result = observer.into_result();

        assert!(!result.done_received);
        assert!(result.usage.is_none());
        assert!(result.finish_reason.is_none());
    }

    #[test]
    fn test_malformed_json_skipped() {
        let events = [
            "data: {this is not valid json}",
            r#"data: {"id":"abc","choices":[],"usage":{"prompt_tokens":8,"completion_tokens":3,"total_tokens":11}}"#,
            "data: [DONE]",
        ];

        let chunks = split_sse_at_positions(&events, &[]);

        let mut observer = SseObserver::new();
        observer.process_chunk(&chunks[0]);
        let result = observer.into_result();

        assert!(result.done_received);
        assert_eq!(
            result.usage,
            Some(StreamUsage {
                prompt_tokens: 8,
                completion_tokens: 3,
            })
        );
    }

    #[test]
    fn test_non_data_sse_fields_skipped() {
        // Mix in event:, id:, retry:, and comment lines
        let raw = b"event: message\nid: 123\nretry: 5000\n: this is a comment\ndata: {\"id\":\"abc\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}],\"usage\":null}\n\ndata: [DONE]\n\n";

        let mut observer = SseObserver::new();
        observer.process_chunk(raw);
        let result = observer.into_result();

        assert!(result.done_received);
        assert_eq!(result.finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_crlf_line_endings() {
        let raw = b"data: {\"id\":\"abc\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}],\"usage\":null}\r\n\r\ndata: {\"id\":\"abc\",\"choices\":[],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2,\"total_tokens\":6}}\r\n\r\ndata: [DONE]\r\n\r\n";

        let mut observer = SseObserver::new();
        observer.process_chunk(raw);
        let result = observer.into_result();

        assert!(result.done_received);
        assert_eq!(
            result.usage,
            Some(StreamUsage {
                prompt_tokens: 4,
                completion_tokens: 2,
            })
        );
        assert_eq!(result.finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_data_without_space() {
        // data:{...} without space after colon
        let raw = b"data:{\"id\":\"abc\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}],\"usage\":null}\n\ndata:[DONE]\n\n";

        let mut observer = SseObserver::new();
        observer.process_chunk(raw);
        let result = observer.into_result();

        assert!(result.done_received);
        assert_eq!(result.finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_done_without_trailing_newline() {
        // [DONE] is the last bytes without a trailing newline
        let raw = b"data: {\"id\":\"abc\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}],\"usage\":null}\n\ndata: [DONE]";

        let mut observer = SseObserver::new();
        observer.process_chunk(raw);
        let result = observer.into_result();

        // flush_buffer in into_result should handle this
        assert!(result.done_received);
        assert_eq!(result.finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_empty_stream() {
        let observer = SseObserver::new();
        let result = observer.into_result();

        assert!(!result.done_received);
        assert!(result.usage.is_none());
        assert!(result.finish_reason.is_none());
    }

    #[test]
    fn test_finish_reason_extracted() {
        // Verify finish_reason is captured from a content chunk
        let events = [
            r#"data: {"id":"abc","choices":[{"index":0,"delta":{"content":"done"},"finish_reason":"stop"}],"usage":null}"#,
            "data: [DONE]",
        ];

        let chunks = split_sse_at_positions(&events, &[]);

        let mut observer = SseObserver::new();
        observer.process_chunk(&chunks[0]);
        let result = observer.into_result();

        assert!(result.done_received);
        assert_eq!(result.finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_buffer_cap() {
        // Create a chunk exceeding 64KB without any newlines
        let huge_chunk = vec![b'x'; 65 * 1024];

        let mut observer = SseObserver::new();
        observer.process_chunk(&huge_chunk);

        // After exceeding 64KB, the buffer should be drained.
        // Then we can still process normal data.
        let normal = b"data: {\"id\":\"abc\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}],\"usage\":null}\n\ndata: [DONE]\n\n";
        observer.process_chunk(normal);
        let result = observer.into_result();

        assert!(result.done_received);
        assert_eq!(result.finish_reason, Some("stop".to_string()));
    }
}
