//! Iris manager. This process is the tray UI and the IPC client.
//! Wayland and the GPU stay in `iris-daemon`.

mod client;
mod commands;
mod supervisor;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use client::DaemonClient;
use iris_core::MonitorTarget;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, RunEvent, WindowEvent};

/// Hide the window on close, and leave the process up until Quit Iris.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let client = DaemonClient::local();
    let quitting = Arc::new(AtomicBool::new(false));
    let quitting_for_run = Arc::clone(&quitting);

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(client.clone())
        .invoke_handler(tauri::generate_handler![
            commands::get_system_monitors,
            commands::get_wallpaper_status,
            commands::apply_wallpaper_command,
            commands::clear_wallpaper_command,
            commands::set_paused_command,
            commands::inspect_wallpaper_package,
            commands::list_library,
            commands::ensure_daemon,
            commands::set_pointer_command,
            commands::get_telemetry_command,
        ])
        .setup(move |app| {
            install_tray(app, client, quitting)?;
            Ok(())
        })
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(err) => {
            eprintln!("iris: {err}");
            std::process::exit(1);
        }
    };

    app.run(move |app, event| match event {
        RunEvent::ExitRequested { api, .. } => {
            if !quitting_for_run.load(Ordering::SeqCst) {
                api.prevent_exit();
            }
        }
        RunEvent::WindowEvent { label, event, .. } => {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if quitting_for_run.load(Ordering::SeqCst) {
                    return;
                }
                api.prevent_close();
                if let Some(window) = app.get_webview_window(&label) {
                    let _ = window.hide();
                }
            }
        }
        _ => {}
    });
}

fn install_tray(
    app: &mut tauri::App,
    client: DaemonClient,
    quitting: Arc<AtomicBool>,
) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "Iris: Starting", false, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause-all", "Pause All", true, None::<&str>)?;
    let resume = MenuItem::with_id(app, "resume-all", "Resume All", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Manager", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Iris", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&status, &separator, &pause, &resume, &open, &quit])?;

    let menu_client = client.clone();
    let menu_quitting = Arc::clone(&quitting);
    let mut builder = TrayIconBuilder::with_id("iris")
        .menu(&menu)
        .tooltip("Iris")
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "pause-all" => {
                let client = menu_client.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = client.pause(MonitorTarget::All).await {
                        tracing::warn!("pause all: {err}");
                    }
                });
            }
            "resume-all" => {
                let client = menu_client.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = client.resume(MonitorTarget::All).await {
                        tracing::warn!("resume all: {err}");
                    }
                });
            }
            "open" => show_main(app),
            "quit" => {
                menu_quitting.store(true, Ordering::SeqCst);
                let client = menu_client.clone();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = client.shutdown_daemon().await {
                        tracing::warn!("daemon shutdown: {err}");
                    }
                    app.exit(0);
                });
            }
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let tray = builder.build(app)?;
    app.manage(tray);

    let status_boot = status.clone();
    let client_boot = client.clone();
    tauri::async_runtime::spawn(async move {
        match supervisor::ensure_running(&client_boot).await {
            Ok(()) => {
                if let Ok(monitors) = client_boot.list_monitors().await {
                    let _ = status_boot.set_text(running_label(monitors.len()));
                }
            }
            Err(err) => {
                tracing::warn!("{err}");
                let _ = status_boot.set_text(menu_label(&err.to_string()));
            }
        }
    });

    let status_loop = status.clone();
    let client_loop = client.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let text = match client_loop.list_monitors().await {
                Ok(monitors) => running_label(monitors.len()),
                Err(err) => {
                    tracing::debug!("tray status: {err}");
                    "Iris: Daemon offline".to_owned()
                }
            };
            if status_loop.set_text(text).is_err() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
    Ok(())
}

fn show_main<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn running_label(count: usize) -> String {
    if count == 1 {
        "Iris: Running (1 screen)".to_owned()
    } else {
        format!("Iris: Running ({count} screens)")
    }
}

fn menu_label(text: &str) -> String {
    let escaped = text.replace('&', "&&");
    let mut chars = escaped.chars();
    let clipped: String = chars.by_ref().take(80).collect();
    if chars.next().is_some() {
        format!("{clipped}…")
    } else {
        clipped
    }
}
