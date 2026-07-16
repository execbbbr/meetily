// Background poller + Tauri commands for meeting-app detection.
//
// A background task polls the process list on an interval. When a known meeting
// app newly appears (rising edge), it emits a `meeting-app-detected` Tauri event
// carrying the app info; the frontend decides whether to prompt the user.
//
// The poller can be started/stopped via commands and is a no-op if already running.

use crate::meeting_detection::detector::{
    detect_from_process_names, meetings_in_progress, MeetingApp,
};
use crate::meeting_detection::mic_usage::microphone_usage;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use sysinfo::{ProcessesToUpdate, System};
use tauri::{AppHandle, Emitter, Runtime};

/// Poll interval. A few seconds is responsive enough for "a meeting started"
/// without meaningfully loading the CPU.
const POLL_INTERVAL_SECS: u64 = 5;

/// Global running flag so start is idempotent and stop is cooperative.
static POLLER_RUNNING: AtomicBool = AtomicBool::new(false);

/// Payload emitted to the frontend when a meeting app newly starts.
#[derive(Debug, Clone, Serialize)]
pub struct MeetingDetectedEvent {
    pub app_id: String,
    pub display_name: String,
}

/// Snapshot the current set of running process names via sysinfo.
fn current_process_names(sys: &mut System) -> Vec<String> {
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.processes()
        .values()
        .map(|p| p.name().to_string_lossy().to_string())
        .collect()
}

/// Compute the set of apps currently *in a meeting* (process running AND mic in use).
fn meetings_now(sys: &mut System) -> Vec<MeetingApp> {
    let names = current_process_names(sys);
    let detected = detect_from_process_names(&names);
    if detected.is_empty() {
        return Vec::new();
    }
    let mic = microphone_usage();
    if !mic.in_use {
        return Vec::new();
    }
    meetings_in_progress(&detected, &mic.app_identifiers)
}

/// Start the background detection poller. Idempotent: if already running, does nothing.
#[tauri::command]
pub fn start_meeting_detection<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    // Compare-and-set: only one poller at a time.
    if POLLER_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // Already running.
        return Ok(());
    }

    tauri::async_runtime::spawn(async move {
        let mut sys = System::new();
        // Seed with meetings already in progress, so we only fire for meetings
        // that START after detection is enabled (no prompt for an ongoing one).
        let mut previous: HashSet<String> = meetings_now(&mut sys)
            .into_iter()
            .map(|a| a.id)
            .collect();

        while POLLER_RUNNING.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;

            if !POLLER_RUNNING.load(Ordering::SeqCst) {
                break;
            }

            let current: Vec<MeetingApp> = meetings_now(&mut sys);

            // Rising edge: a meeting app now in-meeting that wasn't before.
            for app_info in current.iter().filter(|a| !previous.contains(&a.id)) {
                let _ = app.emit(
                    "meeting-app-detected",
                    MeetingDetectedEvent {
                        app_id: app_info.id.clone(),
                        display_name: app_info.display_name.clone(),
                    },
                );
            }

            previous = current.into_iter().map(|a| a.id).collect();
        }
    });

    Ok(())
}

/// Stop the background detection poller.
#[tauri::command]
pub fn stop_meeting_detection() -> Result<(), String> {
    POLLER_RUNNING.store(false, Ordering::SeqCst);
    Ok(())
}

/// Whether the poller is currently running.
#[tauri::command]
pub fn is_meeting_detection_running() -> bool {
    POLLER_RUNNING.load(Ordering::SeqCst)
}

/// One-shot: return meeting apps currently *in a meeting* (process + mic in use).
#[tauri::command]
pub fn detect_meeting_apps_now() -> Vec<MeetingApp> {
    let mut sys = System::new();
    meetings_now(&mut sys)
}
