//! PhoinixDR desktop: a Tauri 2 shell that maps typed commands and events
//! onto the [`phoinix_session`] service layer. No recovery logic lives
//! here; see `docs/desktop/architecture.md`.

#![forbid(unsafe_code)]

mod commands;
mod elevate;
mod enginelog;
mod state;

use tauri::Manager;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Builds and runs the application.
pub fn run() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("phoinix=warn"));
    let (engine_layer, engine_switch, engine_rx) = enginelog::layer();
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(filter),
        )
        .with(engine_layer)
        .try_init();
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            enginelog::forward(app.handle().clone(), engine_rx);
            app.manage(engine_switch);
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
            commands::find_partitions,
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
            commands::verify_source,
            commands::app_info,
            commands::relaunch_elevated,
            commands::set_engine_log,
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        tracing::error!(error = %e, "the desktop application failed");
        std::process::exit(1);
    }
}
