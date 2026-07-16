// Meeting-app detection: recognize running desktop conferencing clients
// (Microsoft Teams, Zoom, Tencent Meeting, Feishu/Lark, Webex, DingTalk) and
// notify the frontend so the user can be prompted to start recording.
//
// Cross-platform (macOS + Windows) via sysinfo process enumeration. Kept
// cohesive so it can evolve independently (e.g. add mic-in-use signals later).

pub mod commands;
pub mod detector;
pub mod mic_usage;

pub use detector::{detect_from_process_names, MeetingApp};
