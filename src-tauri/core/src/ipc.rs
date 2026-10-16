//! Local IPC between the panel UI and the capture daemon: newline-delimited JSON over Unix
//! sockets in `$XDG_RUNTIME_DIR/clipvault/` (directory mode 0700, so only this user can
//! connect).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capture::CaptureStatus;

pub fn runtime_dir() -> PathBuf {
    let base = dirs::runtime_dir().unwrap_or_else(|| {
        // SAFETY: getuid has no preconditions.
        std::env::temp_dir().join(format!("clipvault-{}", unsafe { libc::getuid() }))
    });
    let dir = match std::env::var_os("CLIPVAULT_DATA_DIR") {
        // Isolated profiles get their own sockets so they never talk to the real daemon.
        Some(d) => PathBuf::from(d).join("run"),
        None => base.join("clipvault"),
    };
    let _ = std::fs::create_dir_all(&dir);
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    dir
}

pub fn daemon_socket() -> PathBuf {
    runtime_dir().join("daemon.sock")
}

pub fn ui_socket() -> PathBuf {
    runtime_dir().join("ui.sock")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Status,
    /// Put a history item on the system clipboard.
    Restore {
        id: i64,
    },
    SetPaused {
        paused: bool,
    },
    /// Settings file changed; re-read it and apply.
    ReloadSettings,
    /// Keep the connection open and stream [`Event`]s.
    Subscribe,
    Quit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(data: impl Serialize) -> Self {
        Self {
            ok: true,
            data: serde_json::to_value(data).unwrap_or(Value::Null),
            error: None,
        }
    }
    pub fn err(e: impl ToString) -> Self {
        Self {
            ok: false,
            data: Value::Null,
            error: Some(e.to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    HistoryChanged,
    Status { status: CaptureStatus },
    Skipped { reason: String },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    pub capture: CaptureStatus,
    pub version: String,
    pub pid: u32,
    pub ocr_available: bool,
    pub tray_visible: bool,
}

pub const NOT_RUNNING: &str = "the ClipVault capture service is not running";

/// One request/response round trip to the daemon.
pub fn request(req: &Request, timeout: Duration) -> Result<Value, String> {
    let mut s = UnixStream::connect(daemon_socket()).map_err(|_| NOT_RUNNING.to_string())?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(timeout)).ok();
    let mut line = serde_json::to_string(req).map_err(|e| e.to_string())?;
    line.push('\n');
    s.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(&s)
        .read_line(&mut reply)
        .map_err(|e| format!("no reply from capture service: {e}"))?;
    let resp: Response =
        serde_json::from_str(&reply).map_err(|e| format!("bad reply from capture service: {e}"))?;
    if resp.ok {
        Ok(resp.data)
    } else {
        Err(resp.error.unwrap_or_else(|| "capture service error".into()))
    }
}

pub fn daemon_running() -> bool {
    request(&Request::Ping, Duration::from_millis(500)).is_ok()
}

/// Open an event stream. The iterator ends when the daemon goes away.
pub fn subscribe() -> std::io::Result<impl Iterator<Item = Event>> {
    let mut s = UnixStream::connect(daemon_socket())?;
    s.write_all(b"{\"cmd\":\"subscribe\"}\n")?;
    Ok(BufReader::new(s)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str::<Event>(&l).ok()))
}

/// Send a one-word command (`show`, `hide`, `toggle`, `quit`) to a running panel.
/// Returns false if no panel is running.
pub fn send_ui(cmd: &str) -> bool {
    match UnixStream::connect(ui_socket()) {
        Ok(mut s) => s.write_all(format!("{cmd}\n").as_bytes()).is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_is_stable() {
        assert_eq!(
            serde_json::to_string(&Request::Restore { id: 7 }).unwrap(),
            r#"{"cmd":"restore","id":7}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::HistoryChanged).unwrap(),
            r#"{"event":"history_changed"}"#
        );
        let r: Request = serde_json::from_str(r#"{"cmd":"set_paused","paused":true}"#).unwrap();
        assert!(matches!(r, Request::SetPaused { paused: true }));
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"rm -rf"}"#).is_err());
    }
}
