// audio/transcription/azure_provider.rs
//
// Azure realtime transcription provider (Plan A: one WebSocket carries both
// recognized text and speaker events).
//
// This provider does NOT open its own connection. It reuses the shared
// AzureRealtimeDiarizationClient (a persistent Azure Speech WebSocket already
// used for diarization). On each transcribe() call it:
//   1. pushes the VAD-segmented audio chunk into the WS
//   2. waits briefly for Azure to emit final phrase(s)
//   3. drains whatever recognized text has arrived and returns it
//
// TIMING CAVEAT (verify with a live key): Azure streaming returns finals
// asynchronously, so the text for chunk N may only arrive during chunk N+1.
// The worker calls transcribe() sequentially and Azure returns finals in order,
// so draining after each push roughly tracks the audio, but exact per-chunk
// alignment is best-effort. If a chunk gets no text in time, its text surfaces
// on the next call. This is the part that needs end-to-end validation.

use super::provider::{TranscriptionError, TranscriptionProvider, TranscriptResult};
use crate::audio::diarization::AzureRealtimeDiarizationClient;
use async_trait::async_trait;
use std::time::Duration;

/// How long to wait for Azure to return a final phrase after pushing audio,
/// before giving up and letting the text surface on a later call.
const RECOGNITION_WAIT: Duration = Duration::from_millis(1200);
/// Poll interval while waiting for text.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Sample rate the worker feeds us (16kHz mono f32).
const SAMPLE_RATE: u32 = 16000;

/// Transcription provider backed by the shared Azure realtime WebSocket.
pub struct AzureRealtimeTranscriptionProvider {
    client: AzureRealtimeDiarizationClient,
}

impl AzureRealtimeTranscriptionProvider {
    pub fn new(client: AzureRealtimeDiarizationClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl TranscriptionProvider for AzureRealtimeTranscriptionProvider {
    async fn transcribe(
        &self,
        audio: Vec<f32>,
        _language: Option<String>,
    ) -> std::result::Result<TranscriptResult, TranscriptionError> {
        if audio.is_empty() {
            return Ok(empty_result());
        }

        // Push this chunk into the persistent Azure WebSocket.
        self.client.push_audio_chunk(SAMPLE_RATE, &audio).await;

        // Wait (bounded) for Azure to emit final phrase(s), polling the buffer.
        let deadline = std::time::Instant::now() + RECOGNITION_WAIT;
        loop {
            if let Some(text) = self.client.take_all_pending_text().await {
                return Ok(TranscriptResult {
                    text,
                    confidence: None,
                    is_partial: false,
                    speaker: None,
                });
            }
            if std::time::Instant::now() >= deadline {
                // No text yet — it will surface on a later call. Return empty so
                // the worker doesn't block; not an error.
                return Ok(empty_result());
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    async fn is_model_loaded(&self) -> bool {
        // Cloud service: always "ready" once configured.
        true
    }

    async fn get_current_model(&self) -> Option<String> {
        Some("azure-realtime".to_string())
    }

    fn provider_name(&self) -> &'static str {
        "Azure Speech (realtime)"
    }
}

fn empty_result() -> TranscriptResult {
    TranscriptResult {
        text: String::new(),
        confidence: None,
        is_partial: false,
        speaker: None,
    }
}
