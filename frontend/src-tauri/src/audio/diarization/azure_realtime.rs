use futures_util::{SinkExt, StreamExt};
use log::{debug, warn};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::http::HeaderValue, tungstenite::Message};
use uuid::Uuid;

const TICKS_PER_SECOND: f64 = 10_000_000.0;
const MAX_EVENT_HISTORY: usize = 512;

#[derive(Clone, Debug)]
struct SpeakerEvent {
    speaker: String,
    start_sec: f64,
    end_sec: f64,
}

#[derive(Clone, Debug)]
struct AudioPacket {
    request_id: String,
    pcm16le: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct AzureRealtimeDiarizationClient {
    speaker_events: Arc<RwLock<Vec<SpeakerEvent>>>,
    outbound_tx: mpsc::UnboundedSender<AudioPacket>,
}

impl AzureRealtimeDiarizationClient {
    pub fn new(key: String, region: String) -> Option<Self> {
        if key.trim().is_empty() || region.trim().is_empty() {
            return None;
        }

        let speaker_events = Arc::new(RwLock::new(Vec::new()));
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();

        let events_for_task = speaker_events.clone();
        tokio::spawn(async move {
            if let Err(err) = run_ws_session(key, region, outbound_rx, events_for_task).await {
                warn!("Azure realtime diarization session stopped: {}", err);
            }
        });

        Some(Self {
            speaker_events,
            outbound_tx,
        })
    }

    pub async fn push_audio_chunk(&self, sample_rate: u32, audio: &[f32]) {
        if audio.is_empty() {
            return;
        }

        let pcm16le = float32_to_pcm16le(audio);
        if pcm16le.is_empty() {
            return;
        }

        let _ = self.outbound_tx.send(AudioPacket {
            request_id: Uuid::new_v4().to_string().replace('-', ""),
            pcm16le,
        });

        let _ = sample_rate;
    }

    pub async fn speaker_for_window(
        &self,
        start_sec: f64,
        end_sec: f64,
        _text: &str,
    ) -> Option<String> {
        let events = self.speaker_events.read().await;
        choose_speaker_for_window(&events, start_sec, end_sec)
    }
}

async fn run_ws_session(
    key: String,
    region: String,
    mut outbound_rx: mpsc::UnboundedReceiver<AudioPacket>,
    speaker_events: Arc<RwLock<Vec<SpeakerEvent>>>,
) -> Result<(), String> {
    let url = format!(
        "wss://{}.stt.speech.microsoft.com/speech/recognition/conversation/cognitiveservices/v1?format=detailed&language=en-US",
        region.trim()
    );

    let mut request = url
        .into_client_request()
        .map_err(|e| format!("failed to create ws request: {e}"))?;
    request.headers_mut().insert(
        "Ocp-Apim-Subscription-Key",
        HeaderValue::from_str(key.trim())
            .map_err(|e| format!("invalid azure speech key header: {e}"))?,
    );
    request.headers_mut().insert(
        "X-ConnectionId",
        HeaderValue::from_str(&Uuid::new_v4().to_string().replace('-', ""))
            .map_err(|e| format!("invalid connection id header: {e}"))?,
    );

    let (ws_stream, _) = connect_async(request)
        .await
        .map_err(|e| format!("failed to connect azure ws: {e}"))?;

    let (mut ws_write, mut ws_read) = ws_stream.split();

    let config_message = format!(
        "Path: speech.config\r\nX-RequestId: {}\r\nX-Timestamp: {}\r\nContent-Type: application/json\r\n\r\n{}",
        Uuid::new_v4().to_string().replace('-', ""),
        now_rfc3339_like(),
        r#"{"context":{"system":{"version":"1.0.00000"},"os":{"platform":"desktop"},"audio":{"source":{"bitspersample":16,"channelcount":1,"samplerate":16000,"type":"raw"}}}}"#
    );

    ws_write
        .send(Message::Text(config_message))
        .await
        .map_err(|e| format!("failed to send speech config: {e}"))?;

    loop {
        tokio::select! {
            maybe_packet = outbound_rx.recv() => {
                let Some(packet) = maybe_packet else {
                    let _ = ws_write.send(Message::Close(None)).await;
                    break;
                };

                let audio_message = format!(
                    "Path: audio\r\nX-RequestId: {}\r\nX-Timestamp: {}\r\nContent-Type: audio/x-wav\r\n\r\n",
                    packet.request_id,
                    now_rfc3339_like(),
                );

                let mut framed = audio_message.into_bytes();
                framed.extend_from_slice(&packet.pcm16le);

                if let Err(e) = ws_write.send(Message::Binary(framed)).await {
                    return Err(format!("failed to send audio frame: {e}"));
                }
            }
            maybe_message = ws_read.next() => {
                match maybe_message {
                    Some(Ok(Message::Text(frame))) => {
                        // Diagnostic: dump raw Azure frames to confirm whether the
                        // speaker-recognition stream also carries transcript text
                        // (DisplayText / NBest[].Display). Enable with
                        // MEETILY_AZURE_FRAME_DEBUG=1; silent otherwise.
                        if std::env::var("MEETILY_AZURE_FRAME_DEBUG").is_ok() {
                            log::info!("[azure-frame] {}", frame);
                        }
                        if let Some(event) = parse_speaker_event_from_frame(&frame) {
                            let mut events = speaker_events.write().await;
                            events.push(event);
                            if events.len() > MAX_EVENT_HISTORY {
                                let drain_count = events.len() - MAX_EVENT_HISTORY;
                                events.drain(0..drain_count);
                            }
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {}
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = ws_write.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Close(_))) => break,
                    Some(Err(e)) => return Err(format!("azure ws read error: {e}")),
                    None => break,
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

fn parse_speaker_event_from_frame(frame: &str) -> Option<SpeakerEvent> {
    let payload = extract_json_payload(frame)?;
    let speaker = extract_speaker_label(&payload)?;

    let offset_ticks = extract_tick_value(&payload, &["Offset", "OffsetInTicks", "AudioOffset"]).unwrap_or(0.0);
    let duration_ticks = extract_tick_value(&payload, &["Duration", "DurationInTicks"]).unwrap_or(2.0 * TICKS_PER_SECOND);

    let start_sec = offset_ticks / TICKS_PER_SECOND;
    let end_sec = (offset_ticks + duration_ticks) / TICKS_PER_SECOND;

    if end_sec <= start_sec {
        return None;
    }

    Some(SpeakerEvent {
        speaker,
        start_sec,
        end_sec,
    })
}

fn choose_speaker_for_window(events: &[SpeakerEvent], start_sec: f64, end_sec: f64) -> Option<String> {
    if events.is_empty() {
        return None;
    }

    let mut best_overlap = 0.0;
    let mut best_speaker: Option<String> = None;

    for event in events.iter() {
        let overlap_start = start_sec.max(event.start_sec);
        let overlap_end = end_sec.min(event.end_sec);
        let overlap = (overlap_end - overlap_start).max(0.0);

        if overlap > best_overlap {
            best_overlap = overlap;
            best_speaker = Some(event.speaker.clone());
        }
    }

    if best_speaker.is_some() {
        return best_speaker;
    }

    let center = (start_sec + end_sec) / 2.0;
    events
        .iter()
        .min_by(|a, b| {
            let da = distance_to_interval(center, a.start_sec, a.end_sec);
            let db = distance_to_interval(center, b.start_sec, b.end_sec);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .and_then(|event| {
            let distance = distance_to_interval(center, event.start_sec, event.end_sec);
            if distance <= 1.5 {
                Some(event.speaker.clone())
            } else {
                None
            }
        })
}

fn distance_to_interval(point: f64, start: f64, end: f64) -> f64 {
    if point < start {
        start - point
    } else if point > end {
        point - end
    } else {
        0.0
    }
}

fn extract_json_payload(frame: &str) -> Option<Value> {
    let payload = if let Some((_, body)) = frame.split_once("\r\n\r\n") {
        body
    } else {
        frame
    };

    serde_json::from_str(payload).ok()
}

fn extract_speaker_label(payload: &Value) -> Option<String> {
    let direct = payload
        .get("SpeakerId")
        .or_else(|| payload.get("speakerId"))
        .or_else(|| payload.get("speaker"))
        .and_then(Value::as_str)
        .map(str::to_string);

    if direct.is_some() {
        return direct;
    }

    payload
        .get("NBest")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|first| {
            first
                .get("SpeakerId")
                .or_else(|| first.get("speakerId"))
                .or_else(|| first.get("speaker"))
                .and_then(Value::as_str)
        })
        .map(str::to_string)
}

/// Extract transcript text from an Azure Speech recognition payload.
///
/// Azure emits recognized text in a few shapes depending on the result type:
/// - Top-level `DisplayText` (Speech-to-Text `RecognizedSpeech` frames)
/// - `NBest[0].Display` (rich results with an N-best list) — preferred, best formatted
/// - `NBest[0].Lexical` (fallback when Display is absent)
/// - Top-level `Text` (some streaming/partial frames)
///
/// Returns None for frames that carry no transcript (e.g. pure speaker events,
/// session/turn control frames), or when the text is empty/whitespace.
#[allow(dead_code)] // Wired into the streaming path in the follow-up commit; unit-tested now.
fn extract_transcript_text(payload: &Value) -> Option<String> {
    // Prefer NBest[0].Display / .Lexical (richest, punctuated).
    if let Some(first) = payload
        .get("NBest")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
    {
        if let Some(display) = first
            .get("Display")
            .or_else(|| first.get("display"))
            .and_then(Value::as_str)
        {
            if !display.trim().is_empty() {
                return Some(display.trim().to_string());
            }
        }
        if let Some(lexical) = first
            .get("Lexical")
            .or_else(|| first.get("lexical"))
            .and_then(Value::as_str)
        {
            if !lexical.trim().is_empty() {
                return Some(lexical.trim().to_string());
            }
        }
    }

    // Fall back to top-level DisplayText / Text.
    for key in ["DisplayText", "displayText", "Text", "text"] {
        if let Some(s) = payload.get(key).and_then(Value::as_str) {
            if !s.trim().is_empty() {
                return Some(s.trim().to_string());
            }
        }
    }

    None
}

fn extract_tick_value(payload: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(value) = payload.get(*key) {
            if let Some(number) = value.as_f64() {
                return Some(number);
            }
            if let Some(as_str) = value.as_str() {
                if let Ok(parsed) = as_str.parse::<f64>() {
                    return Some(parsed);
                }
            }
        }
    }

    payload
        .get("NBest")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|first| {
            keys.iter().find_map(|key| {
                first.get(*key).and_then(|value| {
                    value
                        .as_f64()
                        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()))
                })
            })
        })
}

fn float32_to_pcm16le(audio: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(audio.len() * 2);

    for sample in audio {
        let clamped = sample.clamp(-1.0, 1.0);
        let scaled = (clamped * i16::MAX as f32) as i16;
        out.extend_from_slice(&scaled.to_le_bytes());
    }

    out
}

fn now_rfc3339_like() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    format!("{}", now.as_secs())
}

use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_speaker_event_from_headered_frame() {
        let frame = "Path: speech.phrase\r\nContent-Type: application/json\r\n\r\n{\"SpeakerId\":\"Guest-1\",\"Offset\":10000000,\"Duration\":30000000}";
        let event = parse_speaker_event_from_frame(frame).expect("event");

        assert_eq!(event.speaker, "Guest-1");
        assert!((event.start_sec - 1.0).abs() < 0.0001);
        assert!((event.end_sec - 4.0).abs() < 0.0001);
    }

    #[test]
    fn extracts_text_from_display_text() {
        let payload = extract_json_payload(
            "Path: speech.phrase\r\nContent-Type: application/json\r\n\r\n{\"RecognitionStatus\":\"Success\",\"DisplayText\":\"Hello world.\",\"Offset\":0,\"Duration\":20000000}",
        )
        .expect("payload");
        assert_eq!(extract_transcript_text(&payload).as_deref(), Some("Hello world."));
    }

    #[test]
    fn extracts_text_prefers_nbest_display() {
        // When both NBest[0].Display and top-level DisplayText exist, prefer NBest Display.
        let payload: Value = serde_json::from_str(
            r#"{"DisplayText":"lower quality","NBest":[{"Display":"Best formatted text.","Lexical":"best formatted text"}]}"#,
        )
        .unwrap();
        assert_eq!(
            extract_transcript_text(&payload).as_deref(),
            Some("Best formatted text.")
        );
    }

    #[test]
    fn extracts_text_falls_back_to_lexical_then_text() {
        let lexical_only: Value =
            serde_json::from_str(r#"{"NBest":[{"Lexical":"raw words here"}]}"#).unwrap();
        assert_eq!(
            extract_transcript_text(&lexical_only).as_deref(),
            Some("raw words here")
        );

        let text_only: Value = serde_json::from_str(r#"{"Text":"partial hypothesis"}"#).unwrap();
        assert_eq!(
            extract_transcript_text(&text_only).as_deref(),
            Some("partial hypothesis")
        );
    }

    #[test]
    fn extracts_no_text_from_speaker_only_or_control_frames() {
        // Pure speaker event — no transcript fields.
        let speaker_only: Value =
            serde_json::from_str(r#"{"SpeakerId":"Guest-1","Offset":10000000}"#).unwrap();
        assert_eq!(extract_transcript_text(&speaker_only), None);

        // Empty / whitespace text must not be treated as a transcript.
        let empty: Value = serde_json::from_str(r#"{"DisplayText":"   "}"#).unwrap();
        assert_eq!(extract_transcript_text(&empty), None);

        // Turn/session control frame.
        let control: Value = serde_json::from_str(r#"{"RecognitionStatus":"EndOfDictation"}"#).unwrap();
        assert_eq!(extract_transcript_text(&control), None);
    }

    #[test]
    fn picks_best_overlap_speaker() {
        let events = vec![
            SpeakerEvent {
                speaker: "Speaker A".to_string(),
                start_sec: 0.0,
                end_sec: 2.0,
            },
            SpeakerEvent {
                speaker: "Speaker B".to_string(),
                start_sec: 2.0,
                end_sec: 6.0,
            },
        ];

        let speaker = choose_speaker_for_window(&events, 3.0, 4.0);
        assert_eq!(speaker.as_deref(), Some("Speaker B"));
    }

    #[test]
    fn falls_back_to_nearest_speaker_for_close_segment() {
        let events = vec![SpeakerEvent {
            speaker: "Speaker B".to_string(),
            start_sec: 10.0,
            end_sec: 12.0,
        }];

        let speaker = choose_speaker_for_window(&events, 12.2, 12.4);
        assert_eq!(speaker.as_deref(), Some("Speaker B"));
    }

    #[test]
    fn returns_none_for_far_away_segment() {
        let events = vec![SpeakerEvent {
            speaker: "Speaker B".to_string(),
            start_sec: 10.0,
            end_sec: 12.0,
        }];

        let speaker = choose_speaker_for_window(&events, 20.0, 21.0);
        assert_eq!(speaker, None);
    }

    #[test]
    fn rejects_empty_credentials() {
        assert!(AzureRealtimeDiarizationClient::new("".to_string(), "eastus".to_string()).is_none());
        assert!(AzureRealtimeDiarizationClient::new("key".to_string(), "".to_string()).is_none());
    }
}
