// GitHub Copilot OAuth authentication + LLM backend integration.
//
// Self-contained: reimplements the public VS Code Copilot OAuth device flow in
// Rust (no Node/pi-ai dependency) so a Copilot subscription can be used as the
// LLM backend for summaries, skill generation, and visual skill generation.
// Kept cohesive so it could be extracted into a standalone crate later.

pub mod commands;
pub mod oauth;

pub use commands::get_valid_credential;
pub use oauth::{copilot_headers, CopilotCredential};
