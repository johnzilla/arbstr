//! SSE stream observation module.
//!
//! Provides [`SseObserver`] for line-buffered extraction of usage data
//! and finish_reason from OpenAI-compatible SSE streaming responses.
//! Handles TCP chunk boundary reassembly correctly.
//!
//! The public API is [`wrap_sse_stream`], which wraps a byte stream and
//! returns a passthrough stream plus a [`StreamResultHandle`] that will
//! contain the extracted [`StreamResult`] once the stream is fully consumed
//! (or dropped).

use bytes::Bytes;
use futures::Stream;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};

/// Maximum buffer size (64 KB). If exceeded, the buffer is drained entirely
/// and a warning is logged. This prevents unbounded memory growth from a
/// misbehaving provider that sends no newlines.
const BUFFER_CAP: usize = 64 * 1024;

/// Handle for reading the [`StreamResult`] after a wrapped stream completes.
///
/// The result is written by the [`SseObserver`]'s `Drop` implementation
/// (or by [`SseObserver::into_result`] for direct unit-test use).
pub type StreamResultHandle = Arc<Mutex<Option<StreamResult>>>;

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
///
/// When created with a [`StreamResultHandle`] (via [`wrap_sse_stream`]),
/// the `Drop` impl writes the final [`StreamResult`] to the handle,
/// ensuring results are available even if the stream is dropped early.
pub(crate) struct SseObserver {
    /// Byte buffer for reassembling SSE lines across chunk boundaries.
    buffer: Vec<u8>,
    /// Extracted usage from the last chunk that had a non-null usage object.
    usage: Option<StreamUsage>,
    /// Extracted finish_reason from the last chunk with a non-null value.
    finish_reason: Option<String>,
    /// Whether `data: [DONE]` was received.
    done_received: bool,
    /// Optional handle for writing the result on Drop. Set to `None` when
    /// `into_result()` is called directly, to prevent double-write.
    result_handle: Option<StreamResultHandle>,
}

impl SseObserver {
    /// Create a new observer with empty state and no result handle.
    ///
    /// Used directly in unit tests where `into_result()` is called explicitly.
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            usage: None,
            finish_reason: None,
            done_received: false,
            result_handle: None,
        }
    }

    /// Create a new observer that will write its result to the given handle on Drop.
    pub fn with_handle(handle: StreamResultHandle) -> Self {
        Self {
            buffer: Vec::new(),
            usage: None,
            finish_reason: None,
            done_received: false,
            result_handle: Some(handle),
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
    ///
    /// Takes the `result_handle` (sets it to `None`) so that `Drop`
    /// does not double-write the result.
    #[allow(dead_code)]
    pub fn into_result(mut self) -> StreamResult {
        // Prevent Drop from also writing to the handle.
        self.result_handle.take();

        self.flush_buffer();

        if !self.done_received {
            return StreamResult::empty();
        }

        StreamResult {
            usage: self.usage.clone(),
            finish_reason: self.finish_reason.clone(),
            done_received: true,
        }
    }

    /// Build the [`StreamResult`] from current state.
    ///
    /// Returns `StreamResult::empty()` when `[DONE]` was not received.
    fn build_result(&self) -> StreamResult {
        if !self.done_received {
            return StreamResult::empty();
        }

        StreamResult {
            usage: self.usage.clone(),
            finish_reason: self.finish_reason.clone(),
            done_received: true,
        }
    }
}

impl Drop for SseObserver {
    fn drop(&mut self) {
        if let Some(handle) = self.result_handle.take() {
            // Flush any remaining buffer content (e.g. [DONE] without trailing newline).
            self.flush_buffer();

            let result = self.build_result();

            // Write result to handle, recovering from poisoned mutex.
            let mut guard = handle.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some(result);
        }
    }
}

/// Wrap an upstream byte stream for SSE observation.
///
/// Returns a passthrough stream that yields all bytes unmodified plus a
/// [`StreamResultHandle`] that will contain the extracted [`StreamResult`]
/// once the stream is fully consumed or dropped.
///
/// Panics inside the extraction logic are caught via [`std::panic::catch_unwind`]
/// and logged — they never affect byte passthrough to the client.
pub fn wrap_sse_stream<S>(
    stream: S,
) -> (
    impl Stream<Item = Result<Bytes, std::io::Error>>,
    StreamResultHandle,
)
where
    S: Stream<Item = Result<Bytes, reqwest::Error>>,
{
    use futures::StreamExt;

    let handle: StreamResultHandle = Arc::new(Mutex::new(None));
    let observer = SseObserver::with_handle(handle.clone());
    let observer = Arc::new(Mutex::new(observer));

    let wrapped = stream.map(move |chunk_result| match chunk_result {
        Ok(bytes) => {
            let obs = Arc::clone(&observer);
            // Panic isolation: extraction bugs must never break client stream.
            let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
                let mut guard = obs.lock().unwrap_or_else(|e| e.into_inner());
                guard.process_chunk(&bytes);
            }))
            .map_err(|_| {
                tracing::error!("Panic in SSE observer extraction logic — bytes still forwarded");
            });
            Ok(bytes)
        }
        Err(e) => {
            tracing::error!(error = %e, "Error streaming from provider");
            Err(std::io::Error::other(e))
        }
    });

    (wrapped, handle)
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

    // ---- wrap_sse_stream tests (Plan 2) ----

    /// Helper: build SSE byte chunks as a stream of `Result<Bytes, reqwest::Error>`.
    fn mock_sse_stream(
        chunks: Vec<&[u8]>,
    ) -> impl Stream<Item = Result<Bytes, reqwest::Error>> {
        futures::stream::iter(
            chunks
                .into_iter()
                .map(|b| Ok(Bytes::from(b.to_vec())))
                .collect::<Vec<_>>(),
        )
    }

    #[tokio::test]
    async fn test_wrap_sse_stream_basic() {
        use futures::StreamExt;

        let raw = b"data: {\"id\":\"abc\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}],\"usage\":null}\n\ndata: {\"id\":\"abc\",\"choices\":[],\"usage\":{\"prompt_tokens\":6,\"completion_tokens\":10,\"total_tokens\":16}}\n\ndata: [DONE]\n\n";

        let stream = mock_sse_stream(vec![raw.as_slice()]);
        let (wrapped, handle) = wrap_sse_stream(stream);

        // Collect all bytes, then drop the stream so Drop writes to handle.
        let collected: Vec<u8> = wrapped
            .map(|item| item.expect("stream item should be Ok").to_vec())
            .collect::<Vec<_>>()
            .await
            .concat();
        // wrapped is consumed by .collect() and dropped when the future completes.

        // Bytes pass through unmodified
        assert_eq!(collected, raw.as_slice());

        // StreamResult is available in the handle (written by Drop)
        let guard = handle.lock().unwrap();
        let result = guard.as_ref().expect("result should be Some after stream consumed");
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

    #[tokio::test]
    async fn test_wrap_sse_stream_no_done() {
        use futures::StreamExt;

        // Stream without [DONE]
        let raw = b"data: {\"id\":\"abc\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}],\"usage\":null}\n\n";

        let stream = mock_sse_stream(vec![raw.as_slice()]);
        let (wrapped, handle) = wrap_sse_stream(stream);

        // Consume and drop the stream
        let _: Vec<_> = wrapped
            .map(|item| item.expect("stream item should be Ok"))
            .collect()
            .await;

        let guard = handle.lock().unwrap();
        let result = guard.as_ref().expect("result should be Some after stream consumed");
        assert!(!result.done_received);
        assert!(result.usage.is_none());
        assert!(result.finish_reason.is_none());
    }

    #[tokio::test]
    async fn test_wrap_panic_isolation() {
        use futures::StreamExt;

        // Valid SSE data -- wrap_sse_stream should forward bytes even when
        // observer logic runs. This test verifies the catch_unwind code path
        // exists and bytes pass through under normal operation.
        let chunk1 = b"data: {\"id\":\"abc\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}],\"usage\":null}\n\n";
        let chunk2 = b"data: [DONE]\n\n";

        let stream = mock_sse_stream(vec![chunk1.as_slice(), chunk2.as_slice()]);
        let (wrapped, handle) = wrap_sse_stream(stream);

        // Consume and drop the stream
        let all_bytes: Vec<u8> = wrapped
            .map(|item| item.expect("stream item should be Ok").to_vec())
            .collect::<Vec<_>>()
            .await
            .concat();

        // All bytes forwarded
        let mut expected = Vec::new();
        expected.extend_from_slice(chunk1);
        expected.extend_from_slice(chunk2);
        assert_eq!(all_bytes, expected);

        // Result is also available
        let guard = handle.lock().unwrap();
        let result = guard.as_ref().expect("result should be Some");
        assert!(result.done_received);
    }

    #[tokio::test]
    async fn test_drop_writes_result() {
        use futures::StreamExt;

        let chunk1 = b"data: {\"id\":\"abc\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}],\"usage\":null}\n\n";
        let chunk2 = b"data: {\"id\":\"abc\",\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1,\"total_tokens\":4}}\n\n";
        let chunk3 = b"data: [DONE]\n\n";

        let stream = mock_sse_stream(vec![
            chunk1.as_slice(),
            chunk2.as_slice(),
            chunk3.as_slice(),
        ]);
        let (wrapped, handle) = wrap_sse_stream(stream);

        // Only consume first item, then drop the stream
        {
            futures::pin_mut!(wrapped);
            let first = wrapped.next().await;
            assert!(first.is_some());
            // wrapped is dropped here -- Drop on SseObserver should fire
        }

        // Despite not consuming all items, the handle should have a result
        // (written by Drop). It will reflect whatever the observer saw
        // before it was dropped (only chunk1 was processed).
        let guard = handle.lock().unwrap();
        let result = guard.as_ref().expect("Drop should have written a result");
        // Only chunk1 was processed, so done_received=false -> empty result
        assert!(!result.done_received);
    }

    #[tokio::test]
    async fn test_wrap_sse_stream_multi_chunk() {
        use futures::StreamExt;

        // Split SSE data across multiple chunks
        let chunk1 = b"data: {\"id\":\"abc\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"He";
        let chunk2 = b"llo\"},\"finish_reason\":\"stop\"}],\"usage\":null}\n\ndata: {\"id\":\"abc\",\"choices\":[]";
        let chunk3 = b",\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":7,\"total_tokens\":12}}\n\ndata: [DONE]\n\n";

        let stream = mock_sse_stream(vec![
            chunk1.as_slice(),
            chunk2.as_slice(),
            chunk3.as_slice(),
        ]);
        let (wrapped, handle) = wrap_sse_stream(stream);

        // Consume and drop the stream
        let all_bytes: Vec<u8> = wrapped
            .map(|item| item.expect("stream item should be Ok").to_vec())
            .collect::<Vec<_>>()
            .await
            .concat();

        // All bytes forwarded unmodified
        let mut expected = Vec::new();
        expected.extend_from_slice(chunk1);
        expected.extend_from_slice(chunk2);
        expected.extend_from_slice(chunk3);
        assert_eq!(all_bytes, expected);

        // Result extracted correctly across chunk boundaries
        let guard = handle.lock().unwrap();
        let result = guard.as_ref().expect("result should be Some");
        assert!(result.done_received);
        assert_eq!(
            result.usage,
            Some(StreamUsage {
                prompt_tokens: 5,
                completion_tokens: 7,
            })
        );
        assert_eq!(result.finish_reason, Some("stop".to_string()));
    }
}
