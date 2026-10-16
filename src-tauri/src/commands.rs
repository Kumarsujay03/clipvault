//! Typed commands exposed to the panel. Inputs are validated here; the frontend never gets
//! filesystem, shell or network access, and never sees media paths.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use clipvault_core::autostart;
use clipvault_core::capture::CaptureStatus;
use clipvault_core::desktop::{self, ShortcutStatus};
use clipvault_core::error::{AppError, AppResult};
use clipvault_core::export::{self, BackupInfo, ImportReport};
use clipvault_core::ipc::{self, DaemonStatus, Request};
use clipvault_core::model::{Counts, ItemDetail, ListQuery, Page};
use clipvault_core::settings::Settings;

use crate::app::{self, AppState};
use crate::update;

type St<'a> = State<'a, Arc<AppState>>;

/// Run blocking work (SQLite, file I/O, subprocesses) off the async runtime's core threads.
async fn blocking<T: Send + 'static>(
    state: &St<'_>,
    f: impl FnOnce(&AppState) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    let s = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || f(&s))
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
}

fn valid_id(id: i64) -> AppResult<i64> {
    if id <= 0 {
        return Err(AppError::Invalid("invalid item id".into()));
    }
    Ok(id)
}

fn daemon(req: Request) -> AppResult<serde_json::Value> {
    ipc::request(&req, Duration::from_secs(6)).map_err(AppError::Clipboard)
}

fn changed(app: &AppHandle) {
    let _ = app.emit("history-changed", ());
}

#[tauri::command]
pub async fn panel_ready(app: AppHandle) -> AppResult<()> {
    let a = app.clone();
    app.run_on_main_thread(move || app::show_panel(&a))
        .map_err(|e| AppError::Other(e.to_string()))
}

/// Begin moving the panel with the mouse. Marked first so the focus loss GNOME causes
/// while the compositor moves the window does not hide the panel. The frontend reports the
/// end of the move with `end_move`.
#[tauri::command]
pub fn start_move(app: AppHandle, state: St<'_>) -> AppResult<()> {
    use std::sync::atomic::Ordering;
    state
        .moving_since
        .store(clipvault_core::model::now_ms() as u64, Ordering::SeqCst);
    state.moving.store(true, Ordering::SeqCst);
    let w = app
        .get_webview_window("main")
        .ok_or_else(|| AppError::Other("no window".into()))?;
    w.start_dragging().map_err(|e| {
        state.moving.store(false, Ordering::SeqCst);
        AppError::Other(e.to_string())
    })
}

/// The move is over (the pointer came back to the panel, or the button was released). If
/// focus really went elsewhere meanwhile, hide now as hide-on-blur would have.
#[tauri::command]
pub fn end_move(app: AppHandle, state: St<'_>) -> AppResult<()> {
    use std::sync::atomic::Ordering;
    if !state.moving.swap(false, Ordering::SeqCst) {
        return Ok(());
    }
    let st = state.inner().clone();
    std::thread::spawn(move || {
        // Give the compositor a moment to hand focus back after the move.
        std::thread::sleep(Duration::from_millis(400));
        let a = app.clone();
        let _ = app.run_on_main_thread(move || {
            let Some(w) = a.get_webview_window("main") else {
                return;
            };
            let unfocused = w.is_visible().unwrap_or(false) && !w.is_focused().unwrap_or(true);
            if unfocused && st.settings().hide_on_blur && !st.is_moving() {
                app::hide_panel(&a, "focus did not return after moving");
            }
        });
    });
    Ok(())
}

#[tauri::command]
pub async fn list_items(state: St<'_>, query: ListQuery) -> AppResult<Page> {
    if query.search.as_deref().map(str::len).unwrap_or(0) > 1024 {
        return Err(AppError::Invalid("search query too long".into()));
    }
    blocking(&state, move |s| s.store.list(&query)).await
}

#[tauri::command]
pub async fn get_item(state: St<'_>, id: i64) -> AppResult<ItemDetail> {
    let id = valid_id(id)?;
    blocking(&state, move |s| s.store.get(id)).await
}

#[tauri::command]
pub async fn get_counts(state: St<'_>) -> AppResult<Counts> {
    blocking(&state, |s| s.store.counts()).await
}

#[derive(Serialize)]
pub struct TagCount {
    name: String,
    count: i64,
}

#[tauri::command]
pub async fn all_tags(state: St<'_>) -> AppResult<Vec<TagCount>> {
    blocking(&state, |s| {
        Ok(s.store
            .all_tags()?
            .into_iter()
            .map(|(name, count)| TagCount { name, count })
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn set_pinned(app: AppHandle, state: St<'_>, id: i64, pinned: bool) -> AppResult<()> {
    let id = valid_id(id)?;
    blocking(&state, move |s| s.store.set_pinned(id, pinned)).await?;
    changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn set_favorite(app: AppHandle, state: St<'_>, id: i64, favorite: bool) -> AppResult<()> {
    let id = valid_id(id)?;
    blocking(&state, move |s| s.store.set_favorite(id, favorite)).await?;
    changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn set_tags(
    app: AppHandle,
    state: St<'_>,
    id: i64,
    tags: Vec<String>,
) -> AppResult<Vec<String>> {
    let id = valid_id(id)?;
    if tags.len() > 50 || tags.iter().any(|t| t.len() > 100) {
        return Err(AppError::Invalid("too many or too long tags".into()));
    }
    let saved = blocking(&state, move |s| s.store.set_tags(id, &tags)).await?;
    changed(&app);
    Ok(saved)
}

/// Removes the history entry and its cached media. Original files referenced by file
/// entries are never touched.
#[tauri::command]
pub async fn delete_item(app: AppHandle, state: St<'_>, id: i64) -> AppResult<()> {
    let id = valid_id(id)?;
    blocking(&state, move |s| {
        let orphans = s.store.delete(id)?;
        s.media.remove(&orphans);
        Ok(())
    })
    .await?;
    changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn clear_history(app: AppHandle, state: St<'_>, include_pinned: bool) -> AppResult<()> {
    blocking(&state, move |s| {
        let orphans = s.store.clear(include_pinned)?;
        s.media.remove(&orphans);
        Ok(())
    })
    .await?;
    changed(&app);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyResult {
    /// The item was pasted into the previous window (GNOME extension present).
    pasted: bool,
    hidden: bool,
}

/// Put an item on the clipboard (the daemon owns it, so it stays pasteable after the panel
/// unloads). With `paste` and the GNOME extension active, also paste it into the window that
/// was focused before the panel opened.
#[tauri::command]
pub async fn copy_item(
    app: AppHandle,
    state: St<'_>,
    id: i64,
    paste: bool,
) -> AppResult<CopyResult> {
    let id = valid_id(id)?;
    let settings = state.settings();
    blocking(&state, move |_| daemon(Request::Restore { id }).map(|_| ())).await?;
    let want_paste = paste && settings.paste_on_select;
    let hide = want_paste || settings.close_on_copy;
    if hide {
        let a = app.clone();
        let _ = app.run_on_main_thread(move || app::hide_panel(&a, "item chosen"));
    }
    let pasted = if want_paste {
        blocking(&state, |_| Ok(desktop::request_paste().is_ok())).await?
    } else {
        false
    };
    Ok(CopyResult {
        pasted,
        hidden: hide,
    })
}

#[tauri::command]
pub async fn get_settings(state: St<'_>) -> AppResult<Settings> {
    Ok(state.settings())
}

#[tauri::command]
pub async fn update_settings(
    app: AppHandle,
    state: St<'_>,
    settings: Settings,
) -> AppResult<Settings> {
    settings.validate()?;
    let saved = blocking(&state, move |s| {
        let old = s.settings();
        settings.save(&s.paths)?;
        *s.settings.write().unwrap() = settings.clone();
        // The daemon applies capture, retention, tray and autostart changes.
        if ipc::request(&Request::ReloadSettings, Duration::from_secs(5)).is_err() {
            let _ = autostart::sync(&s.paths, settings.start_at_login);
        }
        if old.shortcut != settings.shortcut && desktop::shortcut_status(&s.paths).method == "gnome"
        {
            let _ = desktop::setup_shortcut(&s.paths, &settings.shortcut);
        }
        Ok(settings)
    })
    .await?;
    changed(&app);
    Ok(saved)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbStatus {
    pub ok: bool,
    pub error: Option<String>,
    pub path: String,
    pub items: i64,
    pub media_bytes: i64,
    pub disk_bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    pub capture: Option<CaptureStatus>,
    pub daemon: Option<DaemonStatus>,
    pub db: DbStatus,
    pub data_dir: String,
    pub config_dir: String,
    pub backup_dir: String,
    pub version: String,
    pub warnings: Vec<String>,
    pub autostart: bool,
    pub isolated: bool,
}

#[tauri::command]
pub async fn get_status(state: St<'_>) -> AppResult<AppStatus> {
    blocking(&state, |s| {
        let (ok, error, stats) = match s.store.stats() {
            Ok(st) => (true, None, st),
            Err(e) => (false, Some(e.to_string()), Default::default()),
        };
        let daemon: Option<DaemonStatus> = ipc::request(&Request::Status, Duration::from_secs(2))
            .ok()
            .and_then(|v| serde_json::from_value(v).ok());
        Ok(AppStatus {
            capture: daemon
                .as_ref()
                .map(|d| d.capture.clone())
                .or_else(|| s.capture.read().unwrap().clone()),
            daemon,
            db: DbStatus {
                ok,
                error,
                path: s.paths.db_path().display().to_string(),
                items: stats.items,
                media_bytes: stats.media_bytes,
                disk_bytes: s.media.disk_usage(),
            },
            data_dir: s.paths.data_dir.display().to_string(),
            config_dir: s.paths.config_dir.display().to_string(),
            backup_dir: export::backup_dir().display().to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            warnings: s.warnings.clone(),
            autostart: autostart::is_enabled(),
            isolated: s.paths.isolated,
        })
    })
    .await
}

#[tauri::command]
pub async fn get_shortcut_status(state: St<'_>) -> AppResult<ShortcutStatus> {
    blocking(&state, |s| Ok(desktop::shortcut_status(&s.paths))).await
}

#[tauri::command]
pub async fn set_paused(state: St<'_>, paused: bool) -> AppResult<CaptureStatus> {
    let v = blocking(&state, move |_| daemon(Request::SetPaused { paused })).await?;
    serde_json::from_value(v).map_err(|e| AppError::Other(e.to_string()))
}

#[tauri::command]
pub async fn start_service(state: St<'_>) -> AppResult<bool> {
    blocking(&state, |_| Ok(app::ensure_daemon())).await
}

#[tauri::command]
pub async fn hide_window(app: AppHandle) -> AppResult<()> {
    let a = app.clone();
    app.run_on_main_thread(move || app::hide_panel(&a, "close button or Esc"))
        .map_err(|e| AppError::Other(e.to_string()))
}

/// Stop ClipVault entirely (capture service and panel).
#[tauri::command]
pub async fn quit_app(app: AppHandle) -> AppResult<()> {
    let _ = ipc::request(&Request::Quit, Duration::from_secs(2));
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub async fn setup_shortcut(state: St<'_>) -> AppResult<ShortcutStatus> {
    blocking(&state, |s| {
        desktop::setup_shortcut(&s.paths, &s.settings().shortcut).map_err(AppError::Other)
    })
    .await
}

#[tauri::command]
pub async fn remove_shortcut(state: St<'_>) -> AppResult<ShortcutStatus> {
    blocking(&state, |s| {
        desktop::remove_shortcut(&s.paths).map_err(AppError::Other)?;
        Ok(desktop::shortcut_status(&s.paths))
    })
    .await
}

#[tauri::command]
pub async fn install_extension(state: St<'_>) -> AppResult<String> {
    blocking(&state, |s| {
        let r = desktop::install_extension(&s.paths).map_err(AppError::Other);
        desktop::write_launcher_info(&s.paths);
        r
    })
    .await
}

#[tauri::command]
pub async fn export_history(state: St<'_>, include_images: bool) -> AppResult<String> {
    blocking(&state, move |s| {
        Ok(export::export(&s.store, &s.media, include_images)?
            .display()
            .to_string())
    })
    .await
}

#[tauri::command]
pub async fn list_backups(state: St<'_>) -> AppResult<Vec<BackupInfo>> {
    blocking(&state, |_| export::list()).await
}

#[tauri::command]
pub async fn import_backup(app: AppHandle, state: St<'_>, name: String) -> AppResult<ImportReport> {
    let r = blocking(&state, move |s| {
        export::import(&s.store, &s.media, &s.settings(), &name)
    })
    .await?;
    changed(&app);
    Ok(r)
}

/// Check GitHub for a newer release. With `only_if_due`, skip (returning None) unless the
/// daily automatic check is enabled and a day has passed since the last check.
#[tauri::command]
pub async fn check_update(
    state: St<'_>,
    only_if_due: bool,
) -> AppResult<Option<update::UpdateInfo>> {
    blocking(&state, move |s| {
        if only_if_due {
            let due = s.settings().check_updates
                && update::since_last_check(&s.paths)
                    .map(|ms| ms > 86_400_000)
                    .unwrap_or(true);
            if !due {
                return Ok(None);
            }
        }
        update::check(&s.paths).map(Some).map_err(AppError::Other)
    })
    .await
}

/// Download, verify and install the latest release, then restart ClipVault on it.
/// Progress is reported through `update-progress` events.
#[tauri::command]
pub async fn install_update(app: AppHandle, state: St<'_>) -> AppResult<String> {
    use std::sync::atomic::Ordering;
    state.busy.store(true, Ordering::SeqCst);
    let progress_app = app.clone();
    let result = blocking(&state, move |s| {
        update::install(&s.paths, &|msg: String| {
            let _ = progress_app.emit("update-progress", msg);
        })
        .map_err(AppError::Other)
    })
    .await;
    state.busy.store(false, Ordering::SeqCst);
    let msg = result?;
    if msg.starts_with("Updated") {
        let _ = app.emit("update-progress", "Restarting ClipVault…".to_string());
        update::restart_service();
        // Relaunch the panel from the new binary once this process has exited.
        if let Some(mut argv) = clipvault_core::desktop::ui_argv() {
            argv.push(format!("--wait-pid={}", std::process::id()));
            argv.push("--show".into());
            let _ = clipvault_core::desktop::spawn_detached(&argv);
        }
        let exit_app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(800));
            exit_app.exit(0);
        });
    }
    Ok(msg)
}
