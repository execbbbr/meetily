use crate::api::TranscriptSegment;

pub mod azure_realtime;

pub use azure_realtime::AzureRealtimeDiarizationClient;

/// Local (on-device) speaker diarization is not yet implemented.
///
/// The previous placeholder that labelled speakers purely by time buckets has
/// been removed because it did not reflect who was actually speaking. Real
/// local diarization (speaker embedding + clustering via ONNX) is planned as a
/// follow-up. Until then this is a no-op and segments keep `speaker = None`,
/// so the transcript UI simply shows no speaker label for the local route.
///
/// The Azure realtime route ([`AzureRealtimeDiarizationClient`]) provides real
/// speaker identification today.
pub fn maybe_apply_local_diarization(
    _enabled: bool,
    _provider: &str,
    _segments: &mut [TranscriptSegment],
) {
    // Intentionally a no-op: local diarization is "coming soon".
    // Segments retain speaker = None (graceful fallback).
}
