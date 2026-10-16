//! `clipvaultd`: the always-running part of ClipVault. It captures the clipboard, owns
//! restored content, enforces retention and shows the status icon. It loads no GUI toolkit,
//! so it stays at a few megabytes; the panel UI is a separate process started on demand.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{mpsc, Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use crate::capture::{session::SessionInfo, CaptureController, CaptureStatus, Shared};
use crate::ingest::{Captured, Ingestor, Outcome};
use crate::ipc::{self, DaemonStatus, Event, Request, Response};
use crate::media::MediaStore;
use crate::model::{now_ms, ItemType};
use crate::paths::Paths;
use crate::settings::Settings;
use crate::store::Store;
use crate::tray::{self, Tray, TrayAction};
use crate::{autostart, desktop, ocr, restore};

pub struct Daemon {
    pub paths: Paths,
    pub store: Arc<Store>,
    pub media: MediaStore,
    settings: RwLock<Settings>,
    capture: OnceLock<Arc<CaptureController>>,
    subscribers: Mutex<Vec<UnixStream>>,
    tray: Mutex<Option<ksni::blocking::Handle<Tray>>>,
    ocr_available: bool,
}

impl Daemon {
    fn capture(&self) -> &Arc<CaptureController> {
        self.capture.get().expect("capture started")
    }

    fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    fn broadcast(&self, ev: &Event) {
        let Ok(mut line) = serde_json::to_string(ev) else {
            return;
        };
        line.push('\n');
        // Drop subscribers that went away or stopped reading.
        self.subscribers
            .lock()
            .unwrap()
            .retain_mut(|s| s.write_all(line.as_bytes()).is_ok());
    }

    fn status(&self) -> DaemonStatus {
        DaemonStatus {
            capture: self.capture().status(),
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            ocr_available: self.ocr_available,
            tray_visible: self.tray.lock().unwrap().is_some(),
        }
    }

    fn apply_capture_settings(&self, s: &Settings) {
        use std::sync::atomic::Ordering::Relaxed;
        let sh = &self.capture().shared;
        sh.want_images.store(s.capture_images, Relaxed);
        sh.want_files.store(s.capture_files, Relaxed);
        sh.max_bytes.store(s.max_item_bytes(), Relaxed);
    }

    fn prune(&self) -> usize {
        match self.store.prune(&self.settings(), now_ms()) {
            Ok((n, orphans)) => {
                self.media.remove(&orphans);
                n
            }
            Err(e) => {
                log::error!("pruning failed: {e}");
                0
            }
        }
    }

    fn set_tray_visible(self: &Arc<Self>, visible: bool) {
        let mut tray = self.tray.lock().unwrap();
        match (visible, tray.is_some()) {
            (true, false) => {
                let weak = Arc::downgrade(self);
                let on_action: Arc<dyn Fn(TrayAction) + Send + Sync> = Arc::new(move |a| {
                    if let Some(d) = weak.upgrade() {
                        // Run outside the tray's D-Bus callback.
                        std::thread::spawn(move || d.on_tray(a));
                    }
                });
                *tray = tray::spawn(self.capture().status().paused, on_action);
            }
            (false, true) => {
                if let Some(h) = tray.take() {
                    h.shutdown().wait();
                }
            }
            _ => {}
        }
    }

    fn on_tray(self: Arc<Self>, action: TrayAction) {
        match action {
            TrayAction::Open => open_ui(),
            TrayAction::TogglePause => {
                let paused = self.capture().status().paused;
                self.capture().set_paused(!paused);
            }
            TrayAction::Quit => self.quit(),
        }
    }

    fn reload_settings(self: &Arc<Self>) -> Result<(), String> {
        let (s, warn) = Settings::load(&self.paths);
        if let Some(w) = warn {
            log::warn!("{w}");
        }
        *self.settings.write().unwrap() = s.clone();
        self.apply_capture_settings(&s);
        if let Err(e) = autostart::sync(&self.paths, s.start_at_login) {
            log::warn!("autostart: {e}");
        }
        self.set_tray_visible(s.show_tray_icon);
        if self.prune() > 0 {
            self.broadcast(&Event::HistoryChanged);
        }
        Ok(())
    }

    fn restore(&self, id: i64) -> Result<(), String> {
        let raw = self.store.get_raw(id).map_err(|e| e.to_string())?;
        let formats = restore::formats_for(&raw, &self.media).map_err(|e| e.to_string())?;
        let image = raw.item_type == ItemType::Image;
        self.capture().set_clipboard(formats)?;
        self.store
            .mark_used(id, now_ms())
            .map_err(|e| e.to_string())?;
        if image {
            crate::trim_memory();
        }
        self.broadcast(&Event::HistoryChanged);
        Ok(())
    }

    fn handle(self: &Arc<Self>, req: Request) -> Response {
        match req {
            Request::Ping => {
                Response::ok(serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }))
            }
            Request::Status => Response::ok(self.status()),
            Request::Restore { id } => match self.restore(id) {
                Ok(()) => Response::ok(()),
                Err(e) => Response::err(e),
            },
            Request::SetPaused { paused } => {
                self.capture().set_paused(paused);
                Response::ok(self.capture().status())
            }
            Request::ReloadSettings => match self.reload_settings() {
                Ok(()) => Response::ok(()),
                Err(e) => Response::err(e),
            },
            Request::Subscribe | Request::Quit => Response::err("unexpected request"),
        }
    }

    pub fn quit(&self) -> ! {
        log::info!("shutting down");
        ipc::send_ui("quit");
        if let Some(c) = self.capture.get() {
            c.shutdown();
        }
        if let Some(h) = self.tray.lock().unwrap().take() {
            h.shutdown().wait();
        }
        let _ = std::fs::remove_file(ipc::daemon_socket());
        std::process::exit(0);
    }
}

fn open_ui() {
    match desktop::ui_argv() {
        Some(mut argv) => {
            argv.push("--show".into());
            if let Err(e) = desktop::spawn_detached(&argv) {
                log::warn!("could not open the panel: {e}");
            }
        }
        None => log::warn!("cannot find the clipvault executable"),
    }
}

fn handle_conn(d: Arc<Daemon>, stream: UnixStream) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut writer = stream;
    writer.set_write_timeout(Some(Duration::from_secs(2))).ok();
    for line in BufReader::new(read_half).lines() {
        let Ok(line) = line else { return };
        if line.len() > 64 * 1024 {
            return;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(Request::Subscribe) => {
                let ev = Event::Status {
                    status: d.capture().status(),
                };
                let ok = serde_json::to_string(&ev)
                    .map(|l| writeln!(writer, "{l}").is_ok())
                    .unwrap_or(false);
                if ok {
                    d.subscribers.lock().unwrap().push(writer);
                }
                return;
            }
            Ok(Request::Quit) => {
                let _ = writeln!(
                    writer,
                    "{}",
                    serde_json::to_string(&Response::ok(())).unwrap()
                );
                d.quit();
            }
            Ok(req) => d.handle(req),
            Err(e) => Response::err(format!("bad request: {e}")),
        };
        if writeln!(
            writer,
            "{}",
            serde_json::to_string(&resp).unwrap_or_default()
        )
        .is_err()
        {
            return;
        }
    }
}

fn spawn_ingest_worker(d: Arc<Daemon>, rx: mpsc::Receiver<Captured>) {
    std::thread::Builder::new()
        .name("ingest".into())
        .spawn(move || {
            let ingestor = Ingestor::new(d.store.clone(), d.media.clone());
            for clip in rx {
                let settings = d.settings();
                let had_image = clip.formats.iter().any(|(m, _)| m.starts_with("image/"));
                match ingestor.ingest(&clip, &settings, now_ms()) {
                    Ok(Outcome::Inserted(id)) => {
                        d.prune();
                        d.broadcast(&Event::HistoryChanged);
                        if had_image && settings.ocr_images && d.ocr_available {
                            run_ocr(&d, id);
                        }
                    }
                    Ok(Outcome::Bumped(_)) => {
                        d.broadcast(&Event::HistoryChanged);
                    }
                    Ok(Outcome::Duplicate) => {}
                    Ok(Outcome::Skipped(reason)) => {
                        log::info!("clipboard change not recorded: {reason}");
                        d.broadcast(&Event::Skipped { reason });
                    }
                    Err(e) => {
                        log::error!("failed to record clipboard item: {e}");
                        d.broadcast(&Event::Error {
                            message: e.to_string(),
                        });
                    }
                }
                drop(clip);
                if had_image {
                    // Decoding a large screenshot briefly needs tens of MB; give it back.
                    crate::trim_memory();
                }
            }
        })
        .expect("spawn ingest thread");
}

fn run_ocr(d: &Daemon, id: i64) {
    let Ok((Some(rel), _, _)) = d.store.media_paths(id) else {
        return;
    };
    let Ok(path) = d.paths.resolve_media(&rel) else {
        return;
    };
    if let Some(text) = ocr::recognize(&path) {
        if d.store.set_ocr_text(id, &text).is_ok() {
            log::info!(
                "OCR added {} characters of searchable text",
                text.chars().count()
            );
            d.broadcast(&Event::HistoryChanged);
        }
    }
}

/// After a package upgrade replaces `clipvaultd` on disk, restart onto the new binary so
/// users never keep running an old version until their next login.
fn spawn_upgrade_watcher(d: Arc<Daemon>) {
    std::thread::Builder::new()
        .name("upgrade-watch".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(20));
            let Ok(link) = std::fs::read_link("/proc/self/exe") else {
                continue;
            };
            let s = link.to_string_lossy();
            let Some(path) = s.strip_suffix(" (deleted)") else {
                continue;
            };
            let path = std::path::PathBuf::from(path);
            if !path.is_file() {
                continue;
            }
            // Let the package manager finish writing files.
            std::thread::sleep(Duration::from_secs(3));
            log::info!("ClipVault was upgraded; restarting the capture service");
            if let Some(c) = d.capture.get() {
                c.shutdown();
            }
            if let Some(h) = d.tray.lock().unwrap().take() {
                h.shutdown().wait();
            }
            let _ = std::fs::remove_file(ipc::daemon_socket());
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new(&path).exec();
            log::error!("could not restart after upgrade: {err}");
            std::process::exit(1);
        })
        .expect("spawn upgrade watcher");
}

fn spawn_maintenance(d: Arc<Daemon>) {
    std::thread::Builder::new()
        .name("maintenance".into())
        .spawn(move || {
            match d
                .store
                .referenced_media()
                .and_then(|r| d.media.cleanup_orphans(&r))
            {
                Ok(0) => {}
                Ok(n) => log::info!("removed {n} orphaned media files"),
                Err(e) => log::warn!("orphan cleanup failed: {e}"),
            }
            loop {
                if d.prune() > 0 {
                    d.broadcast(&Event::HistoryChanged);
                }
                std::thread::sleep(Duration::from_secs(3600));
            }
        })
        .expect("spawn maintenance thread");
}

/// On the very first start of an installed copy: start at login, Super+V and the GNOME
/// extension. Runs once (marker file); undo with `clipvault --remove-integration`.
fn first_run_setup(paths: &Paths, settings: &Settings) {
    let marker = paths.config_dir.join(".integration-done");
    if paths.isolated || desktop::running_from_build_dir() || marker.exists() {
        return;
    }
    for line in desktop::setup_all(paths, &settings.shortcut, settings.start_at_login) {
        log::info!("first-run setup: {line}");
    }
    let _ = std::fs::write(marker, "");
}

pub fn run() -> Result<(), String> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("clipvault_core=info,warn"),
    )
    .init();
    let paths = Paths::from_env().map_err(|e| e.to_string())?;
    paths.ensure().map_err(|e| e.to_string())?;

    let sock = ipc::daemon_socket();
    if UnixStream::connect(&sock).is_ok() {
        // A service left over from an earlier login can be stuck on that session's display.
        // Keep a healthy one; replace one that cannot capture.
        let healthy = ipc::request(&Request::Status, Duration::from_secs(2))
            .ok()
            .and_then(|v| serde_json::from_value::<DaemonStatus>(v).ok())
            .map(|s| s.capture.state == crate::capture::BackendState::Running)
            .unwrap_or(false);
        if healthy {
            log::info!("already running");
            return Ok(());
        }
        log::warn!("replacing a capture service that is not working");
        let _ = ipc::request(&Request::Quit, Duration::from_secs(2));
        for _ in 0..30 {
            if UnixStream::connect(&sock).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock)
        .map_err(|e| format!("cannot listen on {}: {e}", sock.display()))?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600));
    }

    let (settings, warn) = Settings::load(&paths);
    if let Some(w) = warn {
        log::warn!("{w}");
    }
    let store = Arc::new(Store::open(&paths.db_path()).map_err(|e| e.to_string())?);
    if let Some(r) = &store.recovered {
        log::warn!("{r}");
    }

    let d = Arc::new(Daemon {
        media: MediaStore::new(paths.clone()),
        paths,
        store,
        settings: RwLock::new(settings.clone()),
        capture: OnceLock::new(),
        subscribers: Mutex::new(Vec::new()),
        tray: Mutex::new(None),
        ocr_available: ocr::available(),
    });

    let shared = Shared {
        paused: Arc::new(AtomicBool::new(
            settings.privacy_mode_on_startup || !settings.start_monitoring,
        )),
        want_images: Arc::new(AtomicBool::new(settings.capture_images)),
        want_files: Arc::new(AtomicBool::new(settings.capture_files)),
        max_bytes: Arc::new(AtomicUsize::new(settings.max_item_bytes())),
    };
    let (cap_tx, cap_rx) = mpsc::channel::<Captured>();
    let weak = Arc::downgrade(&d);
    let capture = CaptureController::start(
        SessionInfo::detect(),
        shared,
        move |clip| {
            let _ = cap_tx.send(clip);
        },
        move |status: CaptureStatus| {
            if let Some(d) = weak.upgrade() {
                if let Some(h) = d.tray.lock().unwrap().as_ref() {
                    h.update(|t| t.paused = status.paused);
                }
                d.broadcast(&Event::Status { status });
            }
        },
    );
    let _ = d.capture.set(capture);

    spawn_ingest_worker(d.clone(), cap_rx);
    spawn_maintenance(d.clone());
    spawn_upgrade_watcher(d.clone());
    desktop::refresh_extension(&d.paths);
    d.set_tray_visible(settings.show_tray_icon);
    if let Err(e) = autostart::sync(&d.paths, settings.start_at_login) {
        log::warn!("autostart: {e}");
    }
    desktop::write_launcher_info(&d.paths);
    first_run_setup(&d.paths, &settings);
    log::info!("ready (pid {})", std::process::id());

    for stream in listener.incoming().flatten() {
        let d = d.clone();
        std::thread::spawn(move || handle_conn(d, stream));
    }
    Ok(())
}
