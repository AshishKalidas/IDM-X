//! The desktop shell. It owns the engine and the local API server, shows the UI in a window, and
//! adds what only a native app can: a tray icon with live speed, OS notifications, and a clean
//! shutdown that pauses downloads so they resume next launch.

use idmx_core::types::{Event, GlobalStats};
use idmx_core::{Engine, EngineOptions};
use idmx_server::{ServeConfig, ServerHandle};
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::{watch, Mutex};

#[derive(Clone, Serialize)]
struct Bootstrap {
    port: u16,
    token: String,
}

/// Shared with commands: filled in once the engine and server are up.
struct Shell {
    ready: watch::Receiver<Option<Bootstrap>>,
    engine: Mutex<Option<Engine>>,
    quitting: AtomicBool,
}

/// The UI asks where the API is. It may ask before the engine has finished starting, so wait.
#[tauri::command]
async fn bootstrap(shell: tauri::State<'_, Arc<Shell>>) -> Result<Bootstrap, String> {
    let mut rx = shell.ready.clone();
    loop {
        if let Some(b) = rx.borrow().clone() {
            return Ok(b);
        }
        rx.changed().await.map_err(|_| "the engine failed to start".to_string())?;
    }
}

/// Open a release page (or any web link) in the user's browser. Web links only.
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    if idmx_core::os::open_url(&url) { Ok(()) } else { Err("not a web link".into()) }
}

pub fn run() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into())).init();

    let (ready_tx, ready_rx) = watch::channel(None);
    let shell = Arc::new(Shell { ready: ready_rx, engine: Mutex::new(None), quitting: AtomicBool::new(false) });

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            open_inputs(app, args.into_iter().skip(1).collect());
            show_window(app)
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(shell.clone())
        .invoke_handler(tauri::generate_handler![bootstrap, open_external])
        .setup(move |app| {
            let handle = app.handle().clone();
            let shell = shell.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = start_engine(handle, shell, ready_tx).await {
                    tracing::error!("engine failed to start: {e:#}");
                }
            });
            build_tray(app.handle())?;
            // Launched by double-clicking a .torrent (Windows / Linux pass it as an argument).
            open_inputs(app.handle(), std::env::args().skip(1).collect());
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window leaves the app (and its downloads) running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build the IDMX app");

    app.run(|app, event| match event {
        // ⌘Q / tray → Quit: stop downloads cleanly first so they resume where they left off.
        RunEvent::ExitRequested { api, .. } => {
            let shell = app.state::<Arc<Shell>>().inner().clone();
            if shell.quitting.swap(true, Ordering::SeqCst) {
                return;
            }
            api.prevent_exit();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Some(e) = shell.engine.lock().await.take() {
                    e.shutdown().await;
                }
                app.exit(0);
            });
        }
        // Clicking the Dock icon should bring the window back.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_window(app),
        // macOS hands magnet: links and opened .torrent files over this way.
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        RunEvent::Opened { urls } => {
            open_inputs(app, urls.iter().map(|u| if u.scheme() == "file" { u.to_file_path().map(|p| p.display().to_string()).unwrap_or_default() } else { u.to_string() }).collect());
            show_window(app)
        }
        _ => {}
    });
}

/// Add magnet links and `.torrent` files the OS handed to us, once the engine is up.
fn open_inputs(app: &AppHandle, inputs: Vec<String>) {
    let items: Vec<String> = inputs.into_iter().filter(|a| a.to_ascii_lowercase().starts_with("magnet:?") || a.to_ascii_lowercase().ends_with(".torrent")).collect();
    if items.is_empty() {
        return;
    }
    let shell = app.state::<Arc<Shell>>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let mut ready = shell.ready.clone();
        if ready.wait_for(|b| b.is_some()).await.is_err() {
            return;
        }
        if let Some(e) = shell.engine.lock().await.as_ref() {
            // Let the user choose files and a folder in the Add sheet rather than starting blind.
            for url in items {
                e.request_add(url);
            }
        }
    });
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

async fn start_engine(app: AppHandle, shell: Arc<Shell>, ready: watch::Sender<Option<Bootstrap>>) -> anyhow::Result<()> {
    let engine = Engine::start_with(idmx_core::default_data_dir(), EngineOptions { update_url: std::env::var("IDMX_UPDATE_URL").ok().filter(|u| !u.is_empty()), ..Default::default() }).await?;
    *shell.engine.lock().await = Some(engine.clone());

    let settings = engine.settings();
    let mut handle = serve_for(&engine).await?;
    let _ = ready.send(Some(Bootstrap { port: handle.addr.port(), token: handle.token.clone() }));

    // React to the engine: OS notifications, tray readout, and re-binding the server when the
    // remote-access settings change.
    let mut events = engine.subscribe();
    let mut bound = (settings.remote.lan, settings.remote.port);
    let notifications_app = app.clone();
    loop {
        match events.recv().await {
            Ok(Event::Notice { level, message }) => {
                let (title, body): (String, Option<String>) = match level.as_str() {
                    "complete" => ("Download complete".into(), Some(message)),
                    "error" => ("Download failed".into(), Some(message)),
                    "queue-done" => (message, None),
                    _ => continue,
                };
                if engine.settings().notifications {
                    let mut n = notifications_app.notification().builder().title(title);
                    if let Some(b) = body {
                        n = n.body(b);
                    }
                    let _ = n.show();
                }
            }
            Ok(Event::Tick { stats, .. }) => update_tray(&app, &stats),
            Ok(Event::Action { action, path, .. }) => match action.as_str() {
                "reveal" => {
                    if let Some(p) = path {
                        idmx_core::os::reveal(std::path::Path::new(&p));
                    }
                }
                "quit" => app.exit(0),
                _ => {}
            },
            Ok(Event::Settings(s)) => {
                let want = (s.remote.lan, s.remote.port);
                if want != bound {
                    bound = want;
                    drop(handle);
                    handle = serve_for(&engine).await?;
                    let _ = ready.send(Some(Bootstrap { port: handle.addr.port(), token: handle.token.clone() }));
                }
            }
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(_) => break,
        }
    }
    Ok(())
}

/// Listen where the settings say: loopback by default, every interface if the user opened the
/// dashboard to their network. Falls back to a free port if the preferred one is taken.
async fn serve_for(engine: &Engine) -> anyhow::Result<ServerHandle> {
    let s = engine.settings();
    let bind = if s.remote.lan { IpAddr::from([0, 0, 0, 0]) } else { IpAddr::V4(Ipv4Addr::LOCALHOST) };
    let cfg = ServeConfig { bind, port: s.remote.port, token: String::new() };
    match idmx_server::serve(engine.clone(), cfg.clone()).await {
        Ok(h) => Ok(h),
        Err(_) => idmx_server::serve(engine.clone(), ServeConfig { port: 0, ..cfg }).await,
    }
}

// ───────────────────────────── tray ─────────────────────────────

const TRAY_ID: &str = "idmx-tray";

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show IDMX", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "Pause all", true, None::<&str>)?;
    let resume = MenuItem::with_id(app, "resume", "Resume all", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit IDMX", true, Some("CmdOrCtrl+Q"))?;
    let menu = Menu::with_items(app, &[&show, &PredefinedMenuItem::separator(app)?, &pause, &resume, &PredefinedMenuItem::separator(app)?, &quit])?;

    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("IDMX")
        .on_menu_event(|app, event| {
            let shell = app.state::<Arc<Shell>>().inner().clone();
            match event.id.as_ref() {
                "show" => show_window(app),
                "quit" => app.exit(0),
                id @ ("pause" | "resume") => {
                    let pause = id == "pause";
                    tauri::async_runtime::spawn(async move {
                        if let Some(e) = shell.engine.lock().await.as_ref() {
                            if pause { e.pause_all() } else { e.resume_all() }
                        }
                    });
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Live speed next to the menu-bar icon (macOS shows the title; other platforms the tooltip).
fn update_tray(app: &AppHandle, stats: &GlobalStats) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let text = if stats.speed > 1.0 { format!("↓ {}/s", idmx_core::util::human_bytes(stats.speed as u64)) } else { String::new() };
    #[cfg(target_os = "macos")]
    let _ = tray.set_title(if text.is_empty() { None } else { Some(text.as_str()) });
    let tip = if text.is_empty() { "IDMX".to_string() } else { format!("IDMX — {text}") };
    let _ = tray.set_tooltip(Some(tip));
}
