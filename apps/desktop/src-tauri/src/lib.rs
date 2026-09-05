//! PhoinixDR desktop: a Tauri 2 shell that maps typed commands and events
//! onto the [`phoinix_session`] service layer. No recovery logic lives
//! here; see `docs/desktop/architecture.md`.

#![forbid(unsafe_code)]

mod commands;
mod state;

use tauri::Manager;

/// Builds and runs the application.
pub fn run() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("phoinix=warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let sessions = app
                .path()
                .app_data_dir()
                .map(|d| d.join("sessions"))
                .unwrap_or_else(|_| std::env::temp_dir().join("phoinixdr-sessions"));
            app.manage(state::AppState::new(sessions));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_devices,
            commands::inspect_source,
            commands::start_scan,
            commands::cancel_scan,
            commands::scan_running,
            commands::list_sessions,
            commands::load_session,
            commands::current_session,
            commands::candidates,
            commands::candidate_detail,
            commands::preview_candidate,
            commands::check_destination,
            commands::recover,
            commands::app_info,
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        tracing::error!(error = %e, "the desktop application failed");
        std::process::exit(1);
    }
}
