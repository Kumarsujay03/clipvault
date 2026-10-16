//! The clipboard panel: a small frameless window that is created on demand, hides when it
//! loses focus (like Win+V) and unloads entirely after `panel_keep_alive_secs`, leaving only
//! the lightweight capture daemon running.

use std::io::{BufRead, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, WindowEvent};

use clipvault_core::capture::CaptureStatus;
use clipvault_core::desktop;
use clipvault_core::ipc::{self, Event};
use clipvault_core::media::MediaStore;
use clipvault_core::paths::Paths;
use clipvault_core::settings::Settings;
use clipvault_core::store::Store;

use crate::{commands, media_protocol};

pub struct AppState {
    pub paths: Paths,
    pub store: Arc<Store>,
    pub media: MediaStore,
    pub settings: RwLock<Settings>,
    pub warnings: Vec<String>,
    pub capture: RwLock<Option<CaptureStatus>>,
    hide_generation: AtomicU64,
    shown_once: AtomicBool,
}

impl AppState {
    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }
}

fn state(app: &AppHandle) -> Arc<AppState> {
    app.state::<Arc<AppState>>().inner().clone()
}

static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

pub fn show_panel(app: &AppHandle) {
    let st = state(app);
    st.hide_generation.fetch_add(1, Ordering::SeqCst);
    if !st.shown_once.swap(true, Ordering::SeqCst) {
        if let Some(t) = STARTED.get() {
            log::info!("panel ready {} ms after launch", t.elapsed().as_millis());
        }
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.center();
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    let _ = app.emit("panel-shown", ());
}

pub fn hide_panel(app: &AppHandle) {
    let st = state(app);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    let _ = app.emit("panel-hidden", ());
    // Unload the whole UI (WebKit and all) once it has been hidden long enough.
    let generation = st.hide_generation.fetch_add(1, Ordering::SeqCst) + 1;
    let keep = Duration::from_secs(st.settings().panel_keep_alive_secs as u64)
        .max(Duration::from_millis(400));
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(keep);
        let st = state(&app);
        let visible = app
            .get_webview_window("main")
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(false);
        if st.hide_generation.load(Ordering::SeqCst) == generation && !visible {
            log::info!("panel idle; unloading UI");
            app.exit(0);
        }
    });
}

fn toggle_panel(app: &AppHandle) {
    let visible_focused = app
        .get_webview_window("main")
        .map(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false))
        .unwrap_or(false);
    if visible_focused {
        hide_panel(app)
    } else {
        show_panel(app)
    }
}

/// Make sure the capture daemon runs; start it if needed.
pub fn ensure_daemon() -> bool {
    if ipc::daemon_running() {
        return true;
    }
    let Some(argv) = desktop::daemon_argv() else {
        log::warn!("capture service executable not found");
        return false;
    };
    if desktop::spawn_detached(&argv).is_err() {
        return false;
    }
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        if ipc::daemon_running() {
            return true;
        }
    }
    false
}

/// Relay daemon events to the UI; reconnects if the daemon restarts.
fn spawn_event_relay(app: AppHandle) {
    std::thread::Builder::new()
        .name("daemon-events".into())
        .spawn(move || loop {
            if let Ok(events) = ipc::subscribe() {
                for ev in events {
                    match ev {
                        Event::HistoryChanged => {
                            let _ = app.emit("history-changed", ());
                        }
                        Event::Status { status } => {
                            *state(&app).capture.write().unwrap() = Some(status.clone());
                            let _ = app.emit("status-changed", Some(status));
                        }
                        Event::Skipped { reason } => {
                            let _ = app
                                .emit("capture-skipped", serde_json::json!({ "reason": reason }));
                        }
                        Event::Error { message } => {
                            let _ = app.emit("backend-error", message);
                        }
                    }
                }
            }
            *state(&app).capture.write().unwrap() = None;
            let _ = app.emit("status-changed", None::<CaptureStatus>);
            std::thread::sleep(Duration::from_secs(2));
        })
        .expect("spawn event relay");
}

/// Accept `show` / `hide` / `toggle` / `quit` from `clipvault --…` invocations.
fn spawn_ui_socket(app: AppHandle) -> Result<(), String> {
    let path = ipc::ui_socket();
    if UnixStream::connect(&path).is_ok() {
        return Err("another ClipVault panel is already running".into());
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    std::thread::Builder::new()
        .name("ui-socket".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                stream.set_read_timeout(Some(Duration::from_secs(1))).ok();
                if BufReader::new(stream).read_line(&mut line).is_err() {
                    continue;
                }
                let app2 = app.clone();
                let cmd = line.trim().to_owned();
                let _ = app.run_on_main_thread(move || match cmd.as_str() {
                    "show" => show_panel(&app2),
                    "hide" => hide_panel(&app2),
                    "toggle" => toggle_panel(&app2),
                    "quit" => app2.exit(0),
                    _ => {}
                });
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn run() {
    let _ = STARTED.set(std::time::Instant::now());
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("clipvault_lib=info,clipvault_core=info,warn"),
    )
    .init();

    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("cvmedia", media_protocol::handler)
        .setup(|app| {
            let mut warnings = Vec::new();
            let paths = Paths::from_env()?;
            paths.ensure()?;
            let (settings, warn) = Settings::load(&paths);
            warnings.extend(warn);
            let store = Arc::new(Store::open(&paths.db_path())?);
            warnings.extend(store.recovered.clone());
            let state = Arc::new(AppState {
                media: MediaStore::new(paths.clone()),
                paths,
                store,
                settings: RwLock::new(settings),
                warnings,
                capture: RwLock::new(None),
                hide_generation: AtomicU64::new(0),
                shown_once: AtomicBool::new(false),
            });
            app.manage(state.clone());

            if let Err(e) = spawn_ui_socket(app.handle().clone()) {
                log::warn!("{e}");
            }
            // Daemon start and subscription happen off the main thread so the panel opens fast.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                ensure_daemon();
                spawn_event_relay(handle);
            });
            // Fallback in case the frontend never reports ready.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1500));
                if !state.shown_once.load(Ordering::SeqCst) {
                    let h = handle.clone();
                    let _ = handle.run_on_main_thread(move || show_panel(&h));
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                hide_panel(window.app_handle());
            }
            WindowEvent::Focused(false) => {
                let app = window.app_handle().clone();
                if !state(&app).settings().hide_on_blur {
                    return;
                }
                // Ignore momentary focus flicker while the window maps.
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(180));
                    let a = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        if let Some(w) = a.get_webview_window("main") {
                            if w.is_visible().unwrap_or(false) && !w.is_focused().unwrap_or(true) {
                                hide_panel(&a);
                            }
                        }
                    });
                });
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::panel_ready,
            commands::list_items,
            commands::get_item,
            commands::get_counts,
            commands::all_tags,
            commands::set_pinned,
            commands::set_favorite,
            commands::set_tags,
            commands::delete_item,
            commands::clear_history,
            commands::copy_item,
            commands::get_settings,
            commands::update_settings,
            commands::get_status,
            commands::get_shortcut_status,
            commands::set_paused,
            commands::start_service,
            commands::hide_window,
            commands::quit_app,
            commands::setup_shortcut,
            commands::remove_shortcut,
            commands::install_extension,
            commands::export_history,
            commands::list_backups,
            commands::import_backup,
        ])
        .build(tauri::generate_context!())
        .expect("error while building ClipVault")
        .run(|_app, event| {
            if let tauri::RunEvent::Exit = event {
                let _ = std::fs::remove_file(ipc::ui_socket());
            }
        });
}
