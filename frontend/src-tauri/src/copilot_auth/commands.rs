// Tauri commands for GitHub Copilot OAuth login and token access.
//
// Login is a device flow: the frontend calls `copilot_start_login` to get a
// user code + verification URL to show the user, then polls `copilot_poll_login`
// until the user authorizes in the browser. On success the credential is saved.
//
// `copilot_get_valid_token` returns a currently-valid short-lived copilot token
// (auto-refreshing from the stored github token when expired) plus the base URL,
// which the LLM layer uses as the bearer + endpoint for /chat/completions.

use crate::copilot_auth::oauth::{
    self, CopilotCredential, DeviceCodeInfo, PollOutcome,
};
use crate::database::repositories::setting::SettingsRepository;
use crate::state::AppState;
use serde::Serialize;
use tauri::{AppHandle, Runtime};
use tauri_plugin_opener::OpenerExt;

#[derive(Debug, Serialize)]
pub struct CopilotLoginStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CopilotPollResult {
    Complete,
    Pending,
    SlowDown { interval: Option<u64> },
}

#[derive(Debug, Serialize)]
pub struct CopilotStatus {
    pub logged_in: bool,
    pub base_url: Option<String>,
}

/// Step 1: begin device flow. Frontend shows user_code + verification_uri.
#[tauri::command]
pub async fn copilot_start_login<R: Runtime>(app: AppHandle<R>) -> Result<CopilotLoginStart, String> {
    let info: DeviceCodeInfo = oauth::start_device_flow().await?;

    // Open the GitHub verification page in the system browser automatically.
    // The Tauri webview does not follow `<a target="_blank">` to an external
    // browser, so without this the user sees a code but nothing opens. Uses the
    // official tauri-plugin-opener. Best-effort: a failure is logged but never
    // blocks login (the UI still shows the code + a manual "Open GitHub" link).
    if let Err(e) = app.opener().open_url(info.verification_uri.clone(), None::<&str>) {
        log::warn!("Failed to open browser for Copilot login: {e}");
    }

    Ok(CopilotLoginStart {
        device_code: info.device_code,
        user_code: info.user_code,
        verification_uri: info.verification_uri,
        interval: info.interval.unwrap_or(5),
        expires_in: info.expires_in,
    })
}

/// Step 2 (called on interval): poll for authorization. On completion, exchanges
/// for a copilot token and persists the credential.
#[tauri::command]
pub async fn copilot_poll_login<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    device_code: String,
) -> Result<CopilotPollResult, String> {
    match oauth::poll_access_token(&device_code).await? {
        PollOutcome::Pending => Ok(CopilotPollResult::Pending),
        PollOutcome::SlowDown(interval) => Ok(CopilotPollResult::SlowDown { interval }),
        PollOutcome::Complete(github_token) => {
            // Exchange github token for the short-lived copilot token + base URL.
            let credential = oauth::fetch_copilot_token(&github_token).await?;
            SettingsRepository::save_copilot_config(state.db_manager.pool(), &credential)
                .await
                .map_err(|e| format!("Failed to save copilot credential: {}", e))?;
            Ok(CopilotPollResult::Complete)
        }
    }
}

/// Return a valid copilot token + base URL, refreshing if the current one expired.
/// Used by the LLM layer. Errors if the user has not logged in.
#[tauri::command]
pub async fn copilot_get_valid_token<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<CopilotCredential, String> {
    get_valid_credential(state.db_manager.pool()).await
}

/// Shared helper: load credential, refresh copilot token if expired, persist, return.
pub async fn get_valid_credential(
    pool: &sqlx::SqlitePool,
) -> Result<CopilotCredential, String> {
    let stored = SettingsRepository::get_copilot_config(pool)
        .await
        .map_err(|e| format!("Failed to load copilot credential: {}", e))?
        .ok_or_else(|| "Not logged in to GitHub Copilot".to_string())?;

    if stored.is_copilot_token_valid() {
        return Ok(stored);
    }

    if stored.github_token.is_empty() {
        return Err("Copilot session expired; please log in again".to_string());
    }

    // Refresh: use the long-lived github token to mint a fresh copilot token.
    let refreshed = oauth::fetch_copilot_token(&stored.github_token).await?;
    SettingsRepository::save_copilot_config(pool, &refreshed)
        .await
        .map_err(|e| format!("Failed to save refreshed copilot credential: {}", e))?;
    Ok(refreshed)
}

/// Whether the user is logged in (has a stored github token).
#[tauri::command]
pub async fn copilot_status<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<CopilotStatus, String> {
    let stored = SettingsRepository::get_copilot_config(state.db_manager.pool())
        .await
        .map_err(|e| format!("Failed to load copilot credential: {}", e))?;

    match stored {
        Some(cred) if !cred.github_token.is_empty() => Ok(CopilotStatus {
            logged_in: true,
            base_url: Some(cred.base_url),
        }),
        _ => Ok(CopilotStatus {
            logged_in: false,
            base_url: None,
        }),
    }
}

/// Log out: clear the stored credential.
#[tauri::command]
pub async fn copilot_logout<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let empty = CopilotCredential::default();
    SettingsRepository::save_copilot_config(state.db_manager.pool(), &empty)
        .await
        .map_err(|e| format!("Failed to clear copilot credential: {}", e))
}

/// List the chat models available to the user's Copilot subscription.
/// Auto-refreshes the token if needed. Frontend uses this to populate the
/// model picker after login.
#[tauri::command]
pub async fn copilot_list_models<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<oauth::CopilotModel>, String> {
    let cred = get_valid_credential(state.db_manager.pool()).await?;
    oauth::list_available_models(&cred.copilot_token, &cred.base_url).await
}
