//! SSE stream observation module.
//!
//! Provides [`SseObserver`] for line-buffered extraction of usage data
//! and finish_reason from OpenAI-compatible SSE streaming responses.
//! Handles TCP chunk boundary reassembly correctly.

/// Maximum buffer size (64 KB). If exceeded, the buffer is drained entirely
/// and a warning is logged. This prevents unbounded memory growth from a
/// misbehaving provider that sends no newlines.
#[allow(dead_code)]
const BUFFER_CAP: usize = 64 * 1024;

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
#[allow(dead_code)]
pub(crate) struct SseObserver {
    /// Byte buffer for reassembling SSE lines across chunk boundaries.
    buffer: Vec<u8>,
    /// Extracted usage from the last chunk that had a non-null usage object.
    usage: Option<StreamUsage>,
    /// Extracted finish_reason from the last chunk with a non-null value.
    finish_reason: Option<String>,
    /// Whether `data: [DONE]` was received.
    done_received: bool,
}

#[allow(dead_code)]
impl SseObserver {
    /// Create a new observer with empty state.
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            usage: None,
            finish_reason: None,
            done_received: false,
        }
    }

    /// Process a chunk of bytes from the SSE stream.
    ///
    /// Appends bytes to the internal buffer, scans for complete lines
    /// (terminated by `\n`), handles `\r\n` by trimming `\r` before `\n`.
    /// Retains trailing incomplete bytes for the next chunk.
    /// Caps buffer at [`BUFFER_CAP`] -- if exceeded, drains entirely.
    pub fn process_chunk(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);

        // Safety valve: cap buffer at 64KB to prevent OOM
        if self.buffer.len() > BUFFER_CAP {
            tracing::warn!(
                buffer_len = self.buffer.len(),
                "SSE buffer exceeded {}KB cap, draining",
                BUFFER_CAP / 1024
            );
            self.buffer.clear();
            return;
        }

        // Process all complete lines in the buffer
        loop {
            let newline_pos = self.buffer.iter().position(|&b| b == b'\n');
            let Some(pos) = newline_pos else {
                break; // No complete line yet
            };

            // Handle \r\n by trimming \r before \n
            let line_end = if pos > 0 && self.buffer[pos - 1] == b'\r' {
                pos - 1
            } else {
                pos
            };

            let line_bytes = self.buffer[..line_end].to_vec();

            // Remove processed bytes including the \n
            self.buffer.drain(..=pos);

            // Process if valid UTF-8
            if let Ok(line) = std::str::from_utf8(&line_bytes) {
                self.process_line(line);
            } else {
                tracing::warn!("Non-UTF8 SSE line, skipping");
            }
        }
    }

    /// Flush any remaining content in the buffer as a final line.
    ///
    /// Handles the case where `data: [DONE]` is sent without a trailing
    /// newline (the last bytes before TCP FIN).
    fn flush_buffer(&mut self) {
        if self.buffer.is_empty() {
            return;
        }

        let remaining = std::mem::take(&mut self.buffer);

        // Trim trailing \r if present
        let line_bytes = if remaining.last() == Some(&b'\r') {
            &remaining[..remaining.len() - 1]
        } else {
            &remaining
        };

        if let Ok(line) = std::str::from_utf8(line_bytes) {
            self.process_line(line);
        } else {
            tracing::warn!("Non-UTF8 content in final buffer flush, skipping");
        }
    }

    /// Process a single complete SSE line.
    ///
    /// Skips empty lines (SSE event delimiters), comment lines (starting
    /// with `:`), and non-data SSE fields (`event:`, `id:`, `retry:`).
    /// For `data:` lines (with or without space after colon), delegates
    /// to [`process_data`](Self::process_data).
    fn process_line(&mut self, line: &str) {
        // Empty line = SSE event delimiter
        if line.is_empty() {
            return;
        }

        // Comment line
        if line.starts_with(':') {
            tracing::trace!(line = line, "SSE comment line");
            return;
        }

        // Non-data SSE fields
        if line.starts_with("event:")
            || line.starts_with("id:")
            || line.starts_with("retry:")
        {
            tracing::trace!(field = line, "SSE non-data field");
            return;
        }

        // Data lines: handle both "data: " (with space) and "data:" (without)
        if let Some(data) = line
            .strip_prefix("data: ")
            .or_else(|| line.strip_prefix("data:"))
        {
            self.process_data(data);
        }
        // Lines with unrecognized field names are silently ignored per SSE spec
    }

    /// Process the data payload of a `data:` SSE line.
    ///
    /// - `[DONE]` sets `done_received = true`
    /// - Otherwise parses as JSON and extracts `finish_reason` and `usage`
    /// - Malformed JSON is logged at warn level and skipped
    fn process_data(&mut self, data: &str) {
        let data = data.trim();

        if data == "[DONE]" {
            self.done_received = true;
            return;
        }

        // Parse as JSON
        let parsed: serde_json::Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to parse SSE data line as JSON");
                return;
            }
        };

        // Extract finish_reason from choices[0].finish_reason
        if let Some(reason) = parsed
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(|r| r.as_str())
        {
            self.finish_reason = Some(reason.to_string());
        }

        // Extract usage (only from chunks where usage is non-null)
        if let Some(usage) = parsed.get("usage").filter(|u| !u.is_null()) {
            if let (Some(prompt), Some(completion)) = (
                usage.get("prompt_tokens").and_then(|v| v.as_u64()),
                usage
                    .get("completion_tokens")
                    .and_then(|v| v.as_u64()),
            ) {
                self.usage = Some(StreamUsage {
                    prompt_tokens: prompt as u32,
                    completion_tokens: completion as u32,
                });
            } else {
                tracing::warn!("Usage object present but missing expected fields");
            }
        }
    }

    /// Consume the observer and produce the final result.
    ///
    /// Flushes any remaining buffer content (handles `[DONE]` without
    /// trailing newline), then returns `StreamResult::empty()` if
    /// `[DONE]` was not received.
    pub fn into_result(mut self) -> StreamResult {
        self.flush_buffer();

        if !self.done_received {
            return StreamResult::empty();
        }

        StreamResult {
            usage: self.usage,
            finish_reason: self.finish_reason,
            done_received: true,
        }
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
