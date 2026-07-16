// Meeting-app detection.
//
// Polls the running process list (via sysinfo, cross-platform) and detects when
// a known desktop conferencing client (Teams, Zoom, Tencent Meeting, Feishu/Lark,
// Webex, ...) starts running. On a rising edge (not-running -> running) it emits
// a Tauri event so the frontend can prompt the user to start recording.
//
// First version scope: desktop clients only (browser-based Meet/web Teams are
// not detected — they show up only as the browser process). Detection is by
// process-name substring match, covering both macOS and Windows executable names.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A conferencing app we know how to recognize.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeetingApp {
    /// Stable id, e.g. "teams".
    pub id: String,
    /// Human-friendly display name, e.g. "Microsoft Teams".
    pub display_name: String,
}

/// Known meeting apps and the lowercase process-name substrings that identify
/// them on macOS and Windows. Matching is case-insensitive substring on the
/// process name, so we list the distinctive tokens for each platform/version.
struct AppSignature {
    id: &'static str,
    display_name: &'static str,
    /// Lowercase substrings; if any matches a running process name, the app is present.
    patterns: &'static [&'static str],
}

const APP_SIGNATURES: &[AppSignature] = &[
    AppSignature {
        id: "teams",
        display_name: "Microsoft Teams",
        // macOS: "MSTeams", "Microsoft Teams", "Teams"; Windows: "ms-teams.exe", "Teams.exe"
        patterns: &["msteams", "ms-teams", "microsoft teams", "teams"],
    },
    AppSignature {
        id: "zoom",
        display_name: "Zoom",
        // macOS: "zoom.us"; Windows: "Zoom.exe"; helper: "CptHost.exe"
        patterns: &["zoom.us", "zoom"],
    },
    AppSignature {
        id: "tencent_meeting",
        display_name: "Tencent Meeting",
        // macOS: "TencentMeeting", "WeMeet"; Windows: "wemeetapp.exe"
        patterns: &["tencentmeeting", "wemeet"],
    },
    AppSignature {
        id: "feishu",
        display_name: "Feishu / Lark",
        // macOS: "Feishu"/"Lark"; Windows: "Feishu.exe"/"Lark.exe"
        patterns: &["feishu", "lark"],
    },
    AppSignature {
        id: "webex",
        display_name: "Webex",
        // macOS/Windows: "Webex", "CiscoWebexStart", "webexmta"
        patterns: &["webex", "ciscospark"],
    },
    AppSignature {
        id: "dingtalk",
        display_name: "DingTalk",
        patterns: &["dingtalk"],
    },
];

/// Given a list of running process names, return the set of meeting apps detected.
/// Pure function — testable without touching the real process table.
pub fn detect_from_process_names(process_names: &[String]) -> Vec<MeetingApp> {
    // Lowercase all process names once.
    let lowered: Vec<String> = process_names.iter().map(|n| n.to_lowercase()).collect();

    let mut seen = HashSet::new();
    let mut result = Vec::new();

    for sig in APP_SIGNATURES {
        let present = lowered
            .iter()
            .any(|proc_name| sig.patterns.iter().any(|pat| proc_name.contains(pat)));
        if present && seen.insert(sig.id) {
            result.push(MeetingApp {
                id: sig.id.to_string(),
                display_name: sig.display_name.to_string(),
            });
        }
    }

    result
}

/// Compute the rising edges: apps present now but not in the previous snapshot.
/// `previous` and `current` are sets of app ids. Returns apps newly appearing.
pub fn newly_started<'a>(
    previous: &HashSet<String>,
    current: &'a [MeetingApp],
) -> Vec<&'a MeetingApp> {
    current
        .iter()
        .filter(|app| !previous.contains(&app.id))
        .collect()
}

/// Two-signal decision: an app counts as "in a meeting" only if it is BOTH a
/// detected meeting app AND currently using the microphone.
///
/// `detected` = meeting apps found in the process list.
/// `mic_app_identifiers` = lowercased identifiers of apps using the mic
///   (bundle id / exe path / app name), from `mic_usage::microphone_usage`.
///
/// We match a detected app to a mic user if any of the app's process patterns
/// appears in any mic-user identifier (substring, case-insensitive). This keeps
/// one source of truth for app identity (the same pattern table as detection).
pub fn meetings_in_progress(
    detected: &[MeetingApp],
    mic_app_identifiers: &[String],
) -> Vec<MeetingApp> {
    let mic_lowered: Vec<String> = mic_app_identifiers.iter().map(|s| s.to_lowercase()).collect();

    detected
        .iter()
        .filter(|app| {
            // Find this app's signature patterns.
            let sig = APP_SIGNATURES.iter().find(|s| s.id == app.id);
            match sig {
                Some(sig) => mic_lowered
                    .iter()
                    .any(|mic| sig.patterns.iter().any(|pat| mic.contains(pat))),
                None => false,
            }
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(apps: &[MeetingApp]) -> Vec<String> {
        apps.iter().map(|a| a.id.clone()).collect()
    }

    #[test]
    fn detects_macos_new_teams() {
        let procs = vec!["MSTeams".to_string(), "Finder".to_string()];
        assert_eq!(ids(&detect_from_process_names(&procs)), vec!["teams"]);
    }

    #[test]
    fn detects_windows_new_teams() {
        let procs = vec!["ms-teams.exe".to_string(), "explorer.exe".to_string()];
        assert_eq!(ids(&detect_from_process_names(&procs)), vec!["teams"]);
    }

    #[test]
    fn detects_windows_classic_teams() {
        let procs = vec!["Teams.exe".to_string()];
        assert_eq!(ids(&detect_from_process_names(&procs)), vec!["teams"]);
    }

    #[test]
    fn detects_macos_teams_spaced_name() {
        let procs = vec!["Microsoft Teams".to_string()];
        assert_eq!(ids(&detect_from_process_names(&procs)), vec!["teams"]);
    }

    #[test]
    fn detects_zoom_macos_and_windows() {
        assert_eq!(
            ids(&detect_from_process_names(&["zoom.us".to_string()])),
            vec!["zoom"]
        );
        assert_eq!(
            ids(&detect_from_process_names(&["Zoom.exe".to_string()])),
            vec!["zoom"]
        );
    }

    #[test]
    fn detects_tencent_and_feishu() {
        let procs = vec![
            "wemeetapp.exe".to_string(),
            "Feishu".to_string(),
        ];
        let got = ids(&detect_from_process_names(&procs));
        assert!(got.contains(&"tencent_meeting".to_string()));
        assert!(got.contains(&"feishu".to_string()));
    }

    #[test]
    fn no_false_positive_on_unrelated_processes() {
        let procs = vec![
            "Finder".to_string(),
            "chrome".to_string(),
            "rust-analyzer".to_string(),
        ];
        assert!(detect_from_process_names(&procs).is_empty());
    }

    #[test]
    fn each_app_reported_once_even_with_multiple_procs() {
        // Teams spawns several helper processes; should still report "teams" once.
        let procs = vec![
            "MSTeams".to_string(),
            "MSTeams Helper".to_string(),
            "msteams".to_string(),
        ];
        assert_eq!(ids(&detect_from_process_names(&procs)), vec!["teams"]);
    }

    #[test]
    fn newly_started_reports_only_rising_edges() {
        let mut previous = HashSet::new();
        previous.insert("zoom".to_string());

        let current = detect_from_process_names(&[
            "zoom.us".to_string(),
            "MSTeams".to_string(),
        ]);
        // zoom was already running; only teams is a rising edge.
        let newly = newly_started(&previous, &current);
        assert_eq!(newly.len(), 1);
        assert_eq!(newly[0].id, "teams");
    }

    #[test]
    fn newly_started_empty_when_nothing_new() {
        let mut previous = HashSet::new();
        previous.insert("teams".to_string());
        let current = detect_from_process_names(&["MSTeams".to_string()]);
        assert!(newly_started(&previous, &current).is_empty());
    }

    #[test]
    fn in_progress_requires_mic_usage() {
        // Teams process is running...
        let detected = detect_from_process_names(&["MSTeams".to_string()]);
        // ...but nothing is using the mic -> NOT in a meeting.
        assert!(meetings_in_progress(&detected, &[]).is_empty());

        // Now Teams is using the mic (macOS bundle id) -> in a meeting.
        let mic = vec!["com.microsoft.teams2".to_string()];
        let in_prog = meetings_in_progress(&detected, &mic);
        assert_eq!(in_prog.len(), 1);
        assert_eq!(in_prog[0].id, "teams");
    }

    #[test]
    fn in_progress_matches_windows_exe_path() {
        let detected = detect_from_process_names(&["ms-teams.exe".to_string()]);
        // Windows consent-store style identifier (normalized exe path).
        let mic = vec![r"c:\program files\windowsapps\msteams\ms-teams.exe".to_string()];
        let in_prog = meetings_in_progress(&detected, &mic);
        assert_eq!(in_prog.len(), 1);
        assert_eq!(in_prog[0].id, "teams");
    }

    #[test]
    fn in_progress_ignores_mic_user_that_is_not_a_meeting_app() {
        // Teams running + mic used by some OTHER app (e.g. Voice Memos) -> not a Teams meeting.
        let detected = detect_from_process_names(&["MSTeams".to_string()]);
        let mic = vec!["com.apple.voicememos".to_string()];
        assert!(meetings_in_progress(&detected, &mic).is_empty());
    }

    #[test]
    fn in_progress_picks_the_right_app_when_multiple_running() {
        // Both Teams and Zoom running, but only Zoom is using the mic.
        let detected = detect_from_process_names(&["MSTeams".to_string(), "zoom.us".to_string()]);
        let mic = vec!["us.zoom.xos".to_string()];
        let in_prog = meetings_in_progress(&detected, &mic);
        assert_eq!(in_prog.len(), 1);
        assert_eq!(in_prog[0].id, "zoom");
    }
}
