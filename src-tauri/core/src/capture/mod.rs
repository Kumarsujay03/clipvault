//! Clipboard capture and restore.
//!
//! Backends are isolated behind a small command interface so other desktops can be added.
//! Current backend: XFixes selection monitoring over X11 / Xwayland (GNOME Wayland mirrors
//! the Wayland clipboard into Xwayland). A GNOME Shell extension bridge is planned for
//! sessions without Xwayland.

pub mod session;
pub mod xauth;
pub mod xfixes;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ingest::Captured;
use session::SessionInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendState {
    Starting,
    Running,
    Error,
    Unavailable,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStatus {
    pub backend: Option<String>,
    pub state: BackendState,
    pub error: Option<String>,
    /// Capture paused by the user (the backend may still be running to serve restores).
    pub paused: bool,
    pub session: SessionInfo,
}

pub enum Command {
    SetClipboard(Vec<(String, Vec<u8>)>, Sender<Result<(), String>>),
    Shutdown,
}

/// Flags the backend reads on every clipboard change.
#[derive(Clone)]
pub struct Shared {
    pub paused: Arc<AtomicBool>,
    pub want_images: Arc<AtomicBool>,
    pub want_files: Arc<AtomicBool>,
    pub max_bytes: Arc<AtomicUsize>,
}

struct BackendHandle {
    tx: Sender<Command>,
    waker: UnixStream,
}

impl BackendHandle {
    fn send(&self, cmd: Command) -> Result<(), String> {
        self.tx
            .send(cmd)
            .map_err(|_| "clipboard backend stopped".to_string())?;
        let _ = (&self.waker).write(&[1]);
        Ok(())
    }
}

pub struct CaptureController {
    pub shared: Shared,
    status: RwLock<CaptureStatus>,
    handle: Mutex<Option<BackendHandle>>,
    shutdown: AtomicBool,
    on_status: Box<dyn Fn(CaptureStatus) + Send + Sync>,
}

impl CaptureController {
    /// Start the supervisor thread. `on_capture` runs on the capture thread and must not
    /// block (hand the snapshot to a worker).
    pub fn start(
        session: SessionInfo,
        shared: Shared,
        on_capture: impl Fn(Captured) + Send + 'static,
        on_status: impl Fn(CaptureStatus) + Send + Sync + 'static,
    ) -> Arc<Self> {
        let ctl = Arc::new(Self {
            status: RwLock::new(CaptureStatus {
                backend: None,
                state: BackendState::Starting,
                error: None,
                paused: shared.paused.load(Ordering::Relaxed),
                session: session.clone(),
            }),
            shared,
            handle: Mutex::new(None),
            shutdown: AtomicBool::new(false),
            on_status: Box::new(on_status),
        });
        let sup = ctl.clone();
        std::thread::Builder::new()
            .name("clipvault-capture".into())
            .spawn(move || sup.supervise(session, on_capture))
            .expect("spawn capture thread");
        ctl
    }

    fn supervise(self: Arc<Self>, session: SessionInfo, on_capture: impl Fn(Captured)) {
        let mut backoff = Duration::from_secs(1);
        while !self.shutdown.load(Ordering::Relaxed) {
            // Re-resolved on every attempt: a new login brings a new Xwayland cookie.
            if xauth::resolve().is_none() {
                self.set_status(
                    None,
                    BackendState::Unavailable,
                    Some(
                        "No X11/Xwayland display is available. ClipVault reads the clipboard through \
                         Xwayland; see TROUBLESHOOTING.md."
                            .into(),
                    ),
                );
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
            self.set_status(Some("xfixes"), BackendState::Starting, None);
            match xfixes::XfixesBackend::connect(self.shared.clone()) {
                Ok(backend) => {
                    let (tx, rx) = mpsc::channel();
                    let (wake_tx, wake_rx) = match UnixStream::pair() {
                        Ok(p) => p,
                        Err(e) => {
                            self.set_status(
                                Some("xfixes"),
                                BackendState::Error,
                                Some(format!("socketpair failed: {e}")),
                            );
                            return;
                        }
                    };
                    let _ = wake_tx.set_nonblocking(true);
                    let _ = wake_rx.set_nonblocking(true);
                    *self.handle.lock().unwrap() = Some(BackendHandle { tx, waker: wake_tx });
                    self.set_status(Some("xfixes"), BackendState::Running, None);
                    log::info!(
                        "clipboard backend running: xfixes on {}",
                        session.x_display.as_deref().unwrap_or("?")
                    );
                    backoff = Duration::from_secs(1);
                    let result = backend.run(rx, wake_rx, &on_capture);
                    *self.handle.lock().unwrap() = None;
                    if self.shutdown.load(Ordering::Relaxed) {
                        break;
                    }
                    let msg = match result {
                        Ok(()) => "clipboard backend exited".to_string(),
                        Err(e) => format!("clipboard backend failed: {e}"),
                    };
                    log::warn!("{msg}; restarting");
                    self.set_status(Some("xfixes"), BackendState::Error, Some(msg));
                }
                Err(e) => {
                    log::warn!("cannot connect to X display: {e}");
                    self.set_status(
                        Some("xfixes"),
                        BackendState::Error,
                        Some(format!("cannot connect to the X display: {e}")),
                    );
                }
            }
            let mut slept = Duration::ZERO;
            while slept < backoff && !self.shutdown.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(200));
                slept += Duration::from_millis(200);
            }
            backoff = (backoff * 2).min(Duration::from_secs(5));
        }
        self.set_status(None, BackendState::Stopped, None);
    }

    fn set_status(&self, backend: Option<&str>, state: BackendState, error: Option<String>) {
        let snapshot = {
            let mut s = self.status.write().unwrap();
            s.backend = backend.map(str::to_owned);
            s.state = state;
            s.error = error;
            s.paused = self.shared.paused.load(Ordering::Relaxed);
            s.clone()
        };
        (self.on_status)(snapshot);
    }

    pub fn status(&self) -> CaptureStatus {
        let mut s = self.status.read().unwrap().clone();
        s.paused = self.shared.paused.load(Ordering::Relaxed);
        s
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
        (self.on_status)(self.status());
    }

    /// Put content on the system clipboard. ClipVault keeps serving it while running.
    pub fn set_clipboard(&self, formats: Vec<(String, Vec<u8>)>) -> Result<(), String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        {
            let guard = self.handle.lock().unwrap();
            let h = guard
                .as_ref()
                .ok_or("the clipboard backend is not running")?;
            h.send(Command::SetClipboard(formats, reply_tx))?;
        }
        reply_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "timed out waiting for the clipboard backend".to_string())?
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.lock().unwrap().as_ref() {
            let _ = h.send(Command::Shutdown);
        }
    }
}
