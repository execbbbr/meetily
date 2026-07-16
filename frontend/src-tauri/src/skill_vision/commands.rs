use crate::database::repositories::meeting::MeetingsRepository;
use crate::state::AppState;
use crate::video_frames::commands::KeyFrame;
use log::{error as log_error, info as log_info};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use tauri::{AppHandle, Runtime};
use base64::Engine;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum TimelineKind {
    Audio,
    Image,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimelineEntry {
    timestamp_secs: f64,
    kind: TimelineKind,
    text: Option<String>,
    image_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIChatMessage {
    role: String,
    content: Vec<OpenAIContentPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum OpenAIContentPart {
    #[serde(rename = "text")]
    Text {
        text: String,
    },
    #[serde(rename = "image_url")]
    ImageUrl {
        image_url: OpenAIImageUrl,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAIImageUrl {
    url: String,
}

#[derive(Debug, Clone, Serialize)]
struct OpenAIChatRequest {
    model: String,
    messages: Vec<OpenAIChatMessage>,
    temperature: f32,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAIChatResponse {
    choices: Vec<OpenAIChoice>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAIChoice {
    message: OpenAIChoiceMessage,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAIChoiceMessage {
    content: Option<String>,
}

#[tauri::command]
pub async fn generate_skill_with_vision<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    timeline_text: String,
    keyframes: Vec<KeyFrame>,
    endpoint_base_url: String,
    api_key: String,
    model: String,
) -> Result<String, String> {
    if endpoint_base_url.trim().is_empty() {
        return Err("Vision endpoint base URL is required".to_string());
    }
    if model.trim().is_empty() {
        return Err("Vision model is required".to_string());
    }

    let meeting = MeetingsRepository::get_meeting_metadata(state.db_manager.pool(), &meeting_id)
        .await
        .map_err(|e| format!("Failed to load meeting metadata: {}", e))?
        .ok_or_else(|| format!("Meeting not found: {}", meeting_id))?;

    let entries = build_timeline_entries(&timeline_text, &keyframes);
    let content = build_openai_content_parts(&entries)?;

    let prompt_header = format!(
        "You are generating a reusable SKILL.md from a timeline-interleaved meeting/tutorial transcript and screen keyframes. Meeting title: {}.\n\nReturn valid markdown only.",
        meeting.title
    );

    let mut final_content = Vec::with_capacity(content.len() + 1);
    final_content.push(OpenAIContentPart::Text {
        text: format!(
            "{}\n\nRequired output format:\n---\nname: <lowercase-hyphenated-skill-name>\ndescription: <one sentence>\n---\n\n## When To Use\n- ...\n\n## Prerequisites\n- ...\n\n## Steps\n1. ...\n\n## Pitfalls & Gotchas\n- ...\n\n## Verification\n- ...\n\nUse explicit commands/config/code details seen in text or screenshots. If missing, write [Missing from timeline].",
            prompt_header
        ),
    });
    final_content.extend(content);

    let request_body = OpenAIChatRequest {
        model: model.trim().to_string(),
        messages: vec![OpenAIChatMessage {
            role: "user".to_string(),
            content: final_content,
        }],
        temperature: 0.2,
    };

    let url = format!("{}/chat/completions", endpoint_base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request_body)
        .send()
        .await
        .map_err(|e| format!("Vision endpoint request failed: {}", e))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("Failed reading vision endpoint response: {}", e))?;

    if !status.is_success() {
        log_error!("Vision endpoint returned {}: {}", status, body);
        return Err(format!(
            "Vision endpoint returned {}. Falling back to audio-only is recommended.",
            status
        ));
    }

    let parsed: OpenAIChatResponse = serde_json::from_str(&body)
        .map_err(|e| format!("Invalid vision endpoint response payload: {}", e))?;

    let markdown = parsed
        .choices
        .first()
        .and_then(|c| c.message.content.clone())
        .filter(|c| !c.trim().is_empty())
        .ok_or_else(|| "Vision endpoint returned no markdown content".to_string())?;

    log_info!(
        "Generated visual skill markdown for meeting {} with {} keyframes",
        meeting_id,
        keyframes.len()
    );

    Ok(markdown)
}

fn build_timeline_entries(timeline_text: &str, keyframes: &[KeyFrame]) -> Vec<TimelineEntry> {
    let mut entries = Vec::new();

    for line in timeline_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some((timestamp, content)) = parse_timeline_line(trimmed) {
            entries.push(TimelineEntry {
                timestamp_secs: timestamp,
                kind: TimelineKind::Audio,
                text: Some(content.to_string()),
                image_path: None,
            });
        }
    }

    for frame in keyframes {
        entries.push(TimelineEntry {
            timestamp_secs: frame.timestamp_secs,
            kind: TimelineKind::Image,
            text: None,
            image_path: Some(frame.image_path.clone()),
        });
    }

    entries.sort_by(|a, b| a.timestamp_secs.total_cmp(&b.timestamp_secs));
    entries
}

fn parse_timeline_line(line: &str) -> Option<(f64, &str)> {
    if !line.starts_with('[') {
        return None;
    }

    let close = line.find(']')?;
    let ts = &line[1..close];
    let rest = line[(close + 1)..].trim();
    if rest.is_empty() {
        return None;
    }

    let parts = ts.split(':').collect::<Vec<_>>();
    if parts.len() != 2 {
        return None;
    }

    let minutes = parts[0].trim().parse::<f64>().ok()?;
    let seconds = parts[1].trim().parse::<f64>().ok()?;
    Some((minutes * 60.0 + seconds, rest))
}

fn build_openai_content_parts(entries: &[TimelineEntry]) -> Result<Vec<OpenAIContentPart>, String> {
    let mut parts = Vec::new();

    for entry in entries {
        match entry.kind {
            TimelineKind::Audio => {
                let text = entry.text.clone().unwrap_or_default();
                parts.push(OpenAIContentPart::Text {
                    text: format!("🔊 [{:.2}s] {}", entry.timestamp_secs, text),
                });
            }
            TimelineKind::Image => {
                let path = entry
                    .image_path
                    .as_ref()
                    .ok_or_else(|| "Missing keyframe image path".to_string())?;
                let data_url = image_file_to_data_url(path)?;
                parts.push(OpenAIContentPart::Text {
                    text: format!("🖼️ [{:.2}s] Keyframe", entry.timestamp_secs),
                });
                parts.push(OpenAIContentPart::ImageUrl {
                    image_url: OpenAIImageUrl { url: data_url },
                });
            }
        }
    }

    Ok(parts)
}

fn image_file_to_data_url(path: &str) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("Failed to read keyframe image {}: {}", path, e))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);

    let mime = match Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "png" => "image/png",
        _ => "image/jpeg",
    };

    Ok(format!("data:{};base64,{}", mime, encoded))
}

// ---------------------------------------------------------------------------
// Skill artifact persistence
//
// Skills are stored in their own table (skill_artifacts), separate from the
// meeting summary, so a meeting can have BOTH a summary and one or more skills
// visible side by side from the meeting details page.
// ---------------------------------------------------------------------------

use crate::database::repositories::skill_artifact::{SkillArtifact, SkillArtifactsRepository};

/// Persist a generated skill for a meeting. Returns the new artifact id.
#[tauri::command]
pub async fn save_skill_artifact(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    skill_name: String,
    markdown: String,
) -> Result<String, String> {
    SkillArtifactsRepository::save(state.db_manager.pool(), &meeting_id, &skill_name, &markdown)
        .await
        .map_err(|e| format!("Failed to save skill artifact: {}", e))
}

/// List all skill artifacts for a meeting (newest first).
#[tauri::command]
pub async fn list_skill_artifacts(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<SkillArtifact>, String> {
    SkillArtifactsRepository::list_for_meeting(state.db_manager.pool(), &meeting_id)
        .await
        .map_err(|e| format!("Failed to list skill artifacts: {}", e))
}

/// Delete a skill artifact by id. Returns true if a row was removed.
#[tauri::command]
pub async fn delete_skill_artifact(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<bool, String> {
    SkillArtifactsRepository::delete(state.db_manager.pool(), &id)
        .await
        .map_err(|e| format!("Failed to delete skill artifact: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn timeline_entries_interleave_audio_and_images_by_time() {
        let timeline_text = "[00:01] hello\n[00:05] run command";
        let keyframes = vec![
            KeyFrame {
                timestamp_secs: 2.0,
                image_path: "/tmp/f1.jpg".to_string(),
            },
            KeyFrame {
                timestamp_secs: 6.0,
                image_path: "/tmp/f2.jpg".to_string(),
            },
        ];

        let entries = build_timeline_entries(timeline_text, &keyframes);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].kind, TimelineKind::Audio);
        assert_eq!(entries[1].kind, TimelineKind::Image);
        assert_eq!(entries[2].kind, TimelineKind::Audio);
        assert_eq!(entries[3].kind, TimelineKind::Image);
    }

    #[test]
    fn parse_timeline_line_parses_timestamp_and_text() {
        let parsed = parse_timeline_line("[03:15] do something").unwrap();
        assert!((parsed.0 - 195.0).abs() < 0.0001);
        assert_eq!(parsed.1, "do something");
    }

    #[test]
    fn image_file_to_data_url_builds_jpeg_data_url() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("frame.jpg");

        let mut file = std::fs::File::create(&file_path).unwrap();
        file.write_all(&[0xFF, 0xD8, 0xFF, 0xD9]).unwrap();

        let data_url = image_file_to_data_url(file_path.to_string_lossy().as_ref()).unwrap();
        assert!(data_url.starts_with("data:image/jpeg;base64,"));
    }
}
