use crate::audio::constants::AUDIO_EXTENSIONS;
use crate::audio::ffmpeg::find_ffmpeg_path;
use crate::database::repositories::meeting::MeetingsRepository;
use crate::state::AppState;
use log::{info as log_info, warn as log_warn};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{AppHandle, Runtime};

const DEFAULT_SCENE_THRESHOLD: f64 = 0.4;
const DEFAULT_MAX_FRAMES: u32 = 40;
const DEFAULT_MIN_INTERVAL_SECS: f64 = 15.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyFrame {
    pub timestamp_secs: f64,
    pub image_path: String,
}

#[derive(Debug, Clone)]
struct FrameTime {
    timestamp_secs: f64,
    image_path: PathBuf,
}

#[tauri::command]
pub async fn extract_video_keyframes<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    scene_threshold: Option<f64>,
    max_frames: Option<u32>,
    min_interval_secs: Option<f64>,
) -> Result<Vec<KeyFrame>, String> {
    let scene_threshold = scene_threshold.unwrap_or(DEFAULT_SCENE_THRESHOLD);
    let max_frames = max_frames.unwrap_or(DEFAULT_MAX_FRAMES).max(1);
    let min_interval_secs = min_interval_secs.unwrap_or(DEFAULT_MIN_INTERVAL_SECS).max(1.0);

    let meeting = MeetingsRepository::get_meeting_metadata(state.db_manager.pool(), &meeting_id)
        .await
        .map_err(|e| format!("Failed to load meeting metadata: {}", e))?
        .ok_or_else(|| format!("Meeting not found: {}", meeting_id))?;

    let Some(folder_path) = meeting.folder_path.filter(|p| !p.trim().is_empty()) else {
        return Ok(vec![]);
    };

    let meeting_folder = PathBuf::from(folder_path);
    if !meeting_folder.exists() {
        return Ok(vec![]);
    }

    let Some(video_file) = find_video_file_in_meeting_folder(&meeting_folder) else {
        return Ok(vec![]);
    };

    let Some(ffmpeg_path) = find_ffmpeg_path() else {
        return Err("Failed to resolve ffmpeg binary".to_string());
    };

    let keyframes_dir = meeting_folder.join("keyframes");
    prepare_keyframes_dir(&keyframes_dir)?;

    let scene_output_pattern = keyframes_dir.join("scene_%04d.jpg");
    let scene_filter = format!("select='gt(scene,{})',showinfo", scene_threshold);

    let scene_output = Command::new(&ffmpeg_path)
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("info")
        .arg("-i")
        .arg(&video_file)
        .arg("-vf")
        .arg(scene_filter)
        .arg("-vsync")
        .arg("vfr")
        .arg("-q:v")
        .arg("4")
        .arg(&scene_output_pattern)
        .output()
        .map_err(|e| format!("Failed to run ffmpeg scene extraction: {}", e))?;

    if !scene_output.status.success() {
        let stderr = String::from_utf8_lossy(&scene_output.stderr);
        log_warn!("ffmpeg scene extraction failed: {}", stderr);
        return Ok(vec![]);
    }

    let scene_pts = parse_showinfo_pts_times(&String::from_utf8_lossy(&scene_output.stderr));
    let scene_images = sorted_images_by_prefix(&keyframes_dir, "scene_")?;
    let mut frames = pair_pts_with_images(&scene_pts, &scene_images);

    let duration_secs = probe_video_duration_secs(&ffmpeg_path, &video_file)
        .or_else(|| frames.iter().map(|f| f.timestamp_secs).reduce(f64::max))
        .unwrap_or(0.0);

    let min_needed = required_frame_count(duration_secs, min_interval_secs);

    if frames.len() < min_needed {
        let fallback_output_pattern = keyframes_dir.join("interval_%04d.jpg");
        let fps_filter = format!("fps=1/{}", min_interval_secs);

        let fallback_output = Command::new(&ffmpeg_path)
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-i")
            .arg(&video_file)
            .arg("-vf")
            .arg(fps_filter)
            .arg("-q:v")
            .arg("4")
            .arg(&fallback_output_pattern)
            .output()
            .map_err(|e| format!("Failed to run ffmpeg interval extraction: {}", e))?;

        if fallback_output.status.success() {
            let interval_images = sorted_images_by_prefix(&keyframes_dir, "interval_")?;
            let fallback_frames = estimate_interval_timestamps(&interval_images, min_interval_secs);
            frames = merge_and_dedup_frames(frames, fallback_frames, min_interval_secs * 0.3);
        }
    }

    frames.sort_by(|a, b| a.timestamp_secs.total_cmp(&b.timestamp_secs));
    let sampled = evenly_sample_frames(frames, max_frames as usize);

    let result = sampled
        .into_iter()
        .map(|f| KeyFrame {
            timestamp_secs: f.timestamp_secs,
            image_path: f.image_path.to_string_lossy().to_string(),
        })
        .collect::<Vec<_>>();

    log_info!("Extracted {} keyframes for meeting {}", result.len(), meeting_id);
    Ok(result)
}

fn find_video_file_in_meeting_folder(meeting_folder: &Path) -> Option<PathBuf> {
    let video_exts = ["mp4", "mkv", "webm"];
    let allowed: HashSet<String> = AUDIO_EXTENSIONS.iter().map(|ext| ext.to_string()).collect();

    let mut candidates = std::fs::read_dir(meeting_folder)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    let lower = e.to_lowercase();
                    allowed.contains(&lower) && video_exts.contains(&lower.as_str())
                })
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    candidates.sort();
    candidates.into_iter().next()
}

fn prepare_keyframes_dir(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|e| format!("Failed to clean keyframes dir: {}", e))?;
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("Failed to create keyframes dir: {}", e))
}

fn sorted_images_by_prefix(dir: &Path, prefix: &str) -> Result<Vec<PathBuf>, String> {
    let mut images = std::fs::read_dir(dir)
        .map_err(|e| format!("Failed to read keyframes dir: {}", e))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(prefix) && n.ends_with(".jpg"))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    images.sort();
    Ok(images)
}

fn parse_showinfo_pts_times(stderr: &str) -> Vec<f64> {
    let mut values = Vec::new();

    for line in stderr.lines() {
        if let Some(pos) = line.find("pts_time:") {
            let raw = &line[(pos + 9)..];
            let token = raw.split_whitespace().next().unwrap_or_default();
            if let Ok(value) = token.parse::<f64>() {
                if value.is_finite() && value >= 0.0 {
                    values.push(value);
                }
            }
        }
    }

    values
}

fn pair_pts_with_images(pts: &[f64], images: &[PathBuf]) -> Vec<FrameTime> {
    let len = pts.len().min(images.len());
    (0..len)
        .map(|i| FrameTime {
            timestamp_secs: pts[i],
            image_path: images[i].clone(),
        })
        .collect()
}

fn required_frame_count(duration_secs: f64, min_interval_secs: f64) -> usize {
    if duration_secs <= 0.0 {
        return 0;
    }

    ((duration_secs / min_interval_secs).ceil() as usize).max(1)
}

fn estimate_interval_timestamps(images: &[PathBuf], interval_secs: f64) -> Vec<FrameTime> {
    images
        .iter()
        .enumerate()
        .map(|(idx, path)| FrameTime {
            timestamp_secs: idx as f64 * interval_secs,
            image_path: path.clone(),
        })
        .collect()
}

fn merge_and_dedup_frames(
    mut primary: Vec<FrameTime>,
    secondary: Vec<FrameTime>,
    epsilon_secs: f64,
) -> Vec<FrameTime> {
    for candidate in secondary {
        let duplicate = primary
            .iter()
            .any(|f| (f.timestamp_secs - candidate.timestamp_secs).abs() <= epsilon_secs);

        if !duplicate {
            primary.push(candidate);
        }
    }

    primary.sort_by(|a, b| a.timestamp_secs.total_cmp(&b.timestamp_secs));
    primary
}

fn evenly_sample_frames(frames: Vec<FrameTime>, max_frames: usize) -> Vec<FrameTime> {
    if frames.len() <= max_frames {
        return frames;
    }

    if max_frames == 0 {
        return Vec::new();
    }

    if max_frames == 1 {
        return vec![frames[0].clone()];
    }

    let total = frames.len();
    let step = (total - 1) as f64 / (max_frames - 1) as f64;

    let mut result = Vec::with_capacity(max_frames);
    for i in 0..max_frames {
        let idx = (i as f64 * step).round() as usize;
        result.push(frames[idx.min(total - 1)].clone());
    }

    result
}

fn probe_video_duration_secs(ffmpeg_path: &Path, video_path: &Path) -> Option<f64> {
    let output = Command::new(ffmpeg_path)
        .arg("-hide_banner")
        .arg("-i")
        .arg(video_path)
        .arg("-f")
        .arg("null")
        .arg("-")
        .output()
        .ok()?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    parse_duration_from_ffmpeg_stderr(&stderr)
}

fn parse_duration_from_ffmpeg_stderr(stderr: &str) -> Option<f64> {
    for line in stderr.lines() {
        if let Some(pos) = line.find("Duration:") {
            let raw = line[(pos + 9)..].trim();
            let token = raw.split(',').next().unwrap_or_default().trim();
            if let Some(parsed) = parse_hms_duration(token) {
                return Some(parsed);
            }
        }
    }

    None
}

fn parse_hms_duration(value: &str) -> Option<f64> {
    let parts = value.split(':').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }

    let hours = parts[0].trim().parse::<f64>().ok()?;
    let minutes = parts[1].trim().parse::<f64>().ok()?;
    let seconds = parts[2].trim().parse::<f64>().ok()?;

    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_showinfo_extracts_pts_time_values() {
        let stderr = "[Parsed_showinfo_0 @ 0x1] n:0 pts:0 pts_time:0.000\n[Parsed_showinfo_0 @ 0x1] n:1 pts:1500 pts_time:1.500";
        let values = parse_showinfo_pts_times(stderr);
        assert_eq!(values, vec![0.0, 1.5]);
    }

    #[test]
    fn required_frame_count_respects_minimum_one_for_positive_duration() {
        assert_eq!(required_frame_count(0.0, 15.0), 0);
        assert_eq!(required_frame_count(1.0, 15.0), 1);
        assert_eq!(required_frame_count(31.0, 15.0), 3);
    }

    #[test]
    fn evenly_sample_frames_limits_to_max_count() {
        let frames = (0..10)
            .map(|idx| FrameTime {
                timestamp_secs: idx as f64,
                image_path: PathBuf::from(format!("f{}.jpg", idx)),
            })
            .collect::<Vec<_>>();

        let sampled = evenly_sample_frames(frames, 4);
        assert_eq!(sampled.len(), 4);
        assert_eq!(sampled.first().unwrap().timestamp_secs, 0.0);
        assert_eq!(sampled.last().unwrap().timestamp_secs, 9.0);
    }

    #[test]
    fn merge_and_dedup_frames_avoids_close_duplicates() {
        let primary = vec![FrameTime {
            timestamp_secs: 10.0,
            image_path: PathBuf::from("scene_0001.jpg"),
        }];
        let secondary = vec![
            FrameTime {
                timestamp_secs: 10.05,
                image_path: PathBuf::from("interval_0001.jpg"),
            },
            FrameTime {
                timestamp_secs: 25.0,
                image_path: PathBuf::from("interval_0002.jpg"),
            },
        ];

        let merged = merge_and_dedup_frames(primary, secondary, 0.1);
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|f| (f.timestamp_secs - 25.0).abs() < 0.0001));
    }

    #[test]
    fn parse_duration_from_stderr_reads_hms() {
        let stderr = "Input #0\n  Duration: 00:12:30.25, start: 0.000000, bitrate: 123 kb/s";
        let duration = parse_duration_from_ffmpeg_stderr(stderr);
        assert_eq!(duration, Some(750.25));
    }
}
