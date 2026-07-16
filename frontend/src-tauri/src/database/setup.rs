use log::info;
use tauri::{AppHandle, Emitter, Manager};

use super::manager::DatabaseManager;
use crate::state::AppState;

/// Initialize database on app startup
/// Handles first launch detection and conditional initialization
pub async fn initialize_database_on_startup(app: &AppHandle) -> Result<(), String> {
    // Check if this is the first launch (no database exists yet)
    let is_first_launch = DatabaseManager::is_first_launch(app)
        .await
        .map_err(|e| format!("Failed to check first launch status: {}", e))?;

    if is_first_launch {
        info!("First launch detected - will notify window when ready");

        // Initialize a fresh database and register AppState immediately, even on
        // first launch. Onboarding may call DB-backed commands (e.g. Copilot
        // login persisting credentials, saving Azure transcript config) before
        // the user finishes setup; without AppState managed here those commands
        // fail with "state not managed". If the user later imports a legacy
        // database during onboarding, import_and_initialize_database re-manages
        // AppState, transparently replacing this fresh one.
        match DatabaseManager::new_from_app_handle(app).await {
            Ok(db_manager) => {
                app.manage(AppState { db_manager });
                info!("Fresh database initialized and AppState managed on first launch");
            }
            Err(e) => {
                info!("First-launch fresh DB init failed (will retry via onboarding): {}", e);
            }
        }

        // Delay event emission to ensure window is ready and React listeners are registered
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            app_handle
                .emit("first-launch-detected", ())
                .expect("Failed to emit first-launch-detected event");
            info!("Emitted first-launch-detected after delay");
        });
    } else {
        // Normal flow - initialize database immediately
        let db_manager = DatabaseManager::new_from_app_handle(app)
            .await
            .map_err(|e| format!("Failed to initialize database manager: {}", e))?;

        app.manage(AppState { db_manager });
        info!("Database initialized successfully");
    }

    Ok(())
}
