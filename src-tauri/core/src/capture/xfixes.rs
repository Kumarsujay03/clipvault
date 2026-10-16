//! XFixes clipboard backend for X11 and Xwayland.
//!
//! * Monitoring: `XFixesSelectSelectionInput` delivers an event whenever the CLIPBOARD owner
//!   changes. No polling; the thread sleeps in `poll(2)` between events.
//! * Reading: `ConvertSelection` into a property on a private InputOnly window, including the
//!   INCR protocol for large payloads. Reading never changes clipboard ownership.
//! * Restoring: ClipVault takes ownership of CLIPBOARD and answers `SelectionRequest`s
//!   (with INCR for large data). On GNOME Wayland, Mutter bridges this to Wayland clients.

use std::collections::HashMap;
use std::fmt;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::errors::{ConnectError, ConnectionError, ReplyError, ReplyOrIdError};
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    PropMode, Property, SelectionNotifyEvent, SelectionRequestEvent, Timestamp, Window,
    WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE};

use super::{Command, Shared};
use crate::ingest::{Captured, FILE_TARGETS, IMAGE_MIMES, MAX_HTML_BYTES, TEXT_TARGETS};

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        CLIPBOARD,
        TARGETS,
        TIMESTAMP,
        INCR,
        CLIPVAULT_DATA,
        CLIPVAULT_TIME,
    }
}

const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";
const READ_TIMEOUT: Duration = Duration::from_secs(3);
const INCR_CHUNK_TIMEOUT: Duration = Duration::from_secs(5);
const OUTGOING_IDLE_LIMIT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub enum XErr {
    /// The X connection is gone; the backend must be restarted.
    Conn(String),
    /// A recoverable protocol error.
    Other(String),
}

impl fmt::Display for XErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            XErr::Conn(e) => write!(f, "X connection lost: {e}"),
            XErr::Other(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for XErr {}

impl From<ConnectionError> for XErr {
    fn from(e: ConnectionError) -> Self {
        XErr::Conn(e.to_string())
    }
}
impl From<ConnectError> for XErr {
    fn from(e: ConnectError) -> Self {
        XErr::Conn(e.to_string())
    }
}
impl From<ReplyError> for XErr {
    fn from(e: ReplyError) -> Self {
        match e {
            ReplyError::ConnectionError(c) => c.into(),
            ReplyError::X11Error(x) => XErr::Other(format!("X11 error {:?}", x.error_kind)),
        }
    }
}
impl From<ReplyOrIdError> for XErr {
    fn from(e: ReplyOrIdError) -> Self {
        match e {
            ReplyOrIdError::ConnectionError(c) => c.into(),
            ReplyOrIdError::X11Error(x) => XErr::Other(format!("X11 error {:?}", x.error_kind)),
            ReplyOrIdError::IdsExhausted => XErr::Conn("X resource ids exhausted".into()),
        }
    }
}
impl From<std::io::Error> for XErr {
    fn from(e: std::io::Error) -> Self {
        XErr::Conn(e.to_string())
    }
}

struct Owned {
    formats: HashMap<Atom, Arc<Vec<u8>>>,
    targets: Vec<Atom>,
    time: Timestamp,
}

struct IncrSend {
    requestor: Window,
    property: Atom,
    target: Atom,
    data: Arc<Vec<u8>>,
    offset: usize,
    last_activity: Instant,
}

pub struct XfixesBackend {
    conn: RustConnection,
    win: Window,
    atoms: Atoms,
    names: HashMap<Atom, String>,
    ids: HashMap<String, Atom>,
    owned: Option<Owned>,
    outgoing: Vec<IncrSend>,
    pending_change: bool,
    /// The next capture is the snapshot taken at startup.
    initial: bool,
    chunk: usize,
    shared: Shared,
}

impl XfixesBackend {
    pub fn connect(shared: Shared) -> Result<Self, XErr> {
        let (conn, screen_num) = connect_current_session()?;
        let root = conn.setup().roots[screen_num].root;
        let atoms = Atoms::new(&conn)?.reply()?;
        conn.xfixes_query_version(5, 0)?.reply()?;
        let win = conn.generate_id()?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            win,
            root,
            -10,
            -10,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            COPY_FROM_PARENT,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        conn.xfixes_select_selection_input(
            win,
            atoms.CLIPBOARD,
            SelectionEventMask::SET_SELECTION_OWNER
                | SelectionEventMask::SELECTION_WINDOW_DESTROY
                | SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )?;
        conn.flush()?;
        let chunk = conn
            .maximum_request_bytes()
            .saturating_sub(1024)
            .clamp(4096, 1 << 20);
        Ok(Self {
            conn,
            win,
            atoms,
            names: HashMap::new(),
            ids: HashMap::new(),
            owned: None,
            outgoing: Vec::new(),
            pending_change: false,
            initial: true,
            chunk,
            shared,
        })
    }

    /// Event loop. Returns when told to shut down, or with an error if the X connection dies.
    pub fn run(
        mut self,
        rx: Receiver<Command>,
        mut wake: UnixStream,
        on_capture: &dyn Fn(Captured),
    ) -> Result<(), XErr> {
        // Record whatever is on the clipboard at startup (deduplication avoids repeats).
        self.pending_change = self
            .conn
            .get_selection_owner(self.atoms.CLIPBOARD)?
            .reply()?
            .owner
            != NONE;
        let wake_fd = wake.as_raw_fd();
        loop {
            while let Some(ev) = self.conn.poll_for_event()? {
                self.handle_event(ev)?;
            }
            loop {
                match rx.try_recv() {
                    Ok(Command::SetClipboard(formats, reply)) => {
                        let r = self.take_ownership(formats);
                        if let Err(XErr::Conn(e)) = &r {
                            let _ = reply.send(Err(e.clone()));
                            return Err(XErr::Conn(e.clone()));
                        }
                        let _ = reply.send(r.map_err(|e| e.to_string()));
                    }
                    Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => return Ok(()),
                    Err(TryRecvError::Empty) => break,
                }
            }
            let mut buf = [0u8; 64];
            loop {
                match wake.read(&mut buf) {
                    Ok(0) => return Ok(()), // controller dropped
                    Ok(_) => continue,
                    Err(_) => break, // WouldBlock
                }
            }
            if std::mem::take(&mut self.pending_change)
                && !self.shared.paused.load(Ordering::Relaxed)
            {
                let initial = std::mem::take(&mut self.initial);
                match self.capture() {
                    Ok(Some(c)) => on_capture(Captured { initial, ..c }),
                    Ok(None) => {}
                    Err(XErr::Conn(e)) => return Err(XErr::Conn(e)),
                    Err(e) => log::warn!("clipboard read failed: {e}"),
                }
            }
            let now = Instant::now();
            self.outgoing
                .retain(|t| now.duration_since(t.last_activity) < OUTGOING_IDLE_LIMIT);
            self.conn.flush()?;
            if let Some(ev) = self.conn.poll_for_event()? {
                self.handle_event(ev)?;
                continue;
            }
            if self.pending_change {
                continue;
            }
            let timeout = if self.outgoing.is_empty() { -1 } else { 1000 };
            poll_fds(&[self.conn.stream().as_raw_fd(), wake_fd], timeout)?;
        }
    }

    fn handle_event(&mut self, ev: Event) -> Result<(), XErr> {
        match ev {
            Event::XfixesSelectionNotify(e) if e.selection == self.atoms.CLIPBOARD => {
                // Our own ownership changes (restores) are not new clipboard content.
                if e.owner != self.win && e.owner != NONE {
                    self.pending_change = true;
                }
            }
            Event::SelectionRequest(e) => self.serve(e)?,
            Event::SelectionClear(e) if e.selection == self.atoms.CLIPBOARD => {
                self.owned = None;
            }
            Event::PropertyNotify(e) if e.state == Property::DELETE => {
                self.continue_incr(e.window, e.atom)?
            }
            Event::Error(e) => log::debug!("X11 error event: {:?}", e.error_kind),
            _ => {}
        }
        Ok(())
    }

    // ---------------------------------------------------------------- reading

    fn capture(&mut self) -> Result<Option<Captured>, XErr> {
        let Some(raw) = self.fetch(self.atoms.TARGETS, 64 * 1024)? else {
            return Ok(None);
        };
        let targets: Vec<Atom> = raw
            .chunks_exact(4)
            .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let mut available: Vec<(String, Atom)> = Vec::with_capacity(targets.len());
        for a in targets {
            if a != NONE {
                available.push((self.atom_name(a)?, a));
            }
        }
        let find = |m: &str| {
            available
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(m))
                .map(|(n, a)| (n.clone(), *a))
        };

        if find(PASSWORD_HINT).is_some() {
            return Ok(Some(Captured {
                password_hint: true,
                ..Default::default()
            }));
        }

        let max = self.shared.max_bytes.load(Ordering::Relaxed);
        let mut wanted: Vec<(String, Atom, usize)> = Vec::new();
        if self.shared.want_images.load(Ordering::Relaxed) {
            if let Some((n, a)) = IMAGE_MIMES.iter().find_map(|m| find(m)) {
                wanted.push((n, a, max));
            }
        }
        if wanted.is_empty() {
            if self.shared.want_files.load(Ordering::Relaxed) {
                for m in FILE_TARGETS {
                    if let Some((n, a)) = find(m) {
                        wanted.push((n, a, 4 * 1024 * 1024));
                    }
                }
            }
            if let Some((n, a)) = TEXT_TARGETS.iter().find_map(|m| find(m)) {
                wanted.push((n, a, max));
            }
            if let Some((n, a)) = find("text/html") {
                wanted.push((n, a, MAX_HTML_BYTES));
            }
        }

        let mut formats = Vec::new();
        for (name, atom, cap) in wanted {
            match self.fetch(atom, cap)? {
                Some(bytes) => formats.push((name, bytes)),
                None => log::debug!("clipboard target {name} unavailable or over size limit"),
            }
        }
        if formats.is_empty() {
            return Ok(None);
        }
        Ok(Some(Captured {
            formats,
            ..Default::default()
        }))
    }

    /// Convert CLIPBOARD to `target` and read the result. `None` = refused, timed out or too big.
    fn fetch(&mut self, target: Atom, cap: usize) -> Result<Option<Vec<u8>>, XErr> {
        let (win, clip, prop) = (self.win, self.atoms.CLIPBOARD, self.atoms.CLIPVAULT_DATA);
        self.conn.delete_property(win, prop)?;
        self.conn
            .convert_selection(win, clip, target, prop, CURRENT_TIME)?;
        let ev = self.wait_for(READ_TIMEOUT, |ev| {
            matches!(ev, Event::SelectionNotify(e) if e.requestor == win && e.selection == clip)
        })?;
        let Some(Event::SelectionNotify(e)) = ev else {
            log::debug!("timed out waiting for clipboard owner");
            return Ok(None);
        };
        if e.property == NONE {
            return Ok(None);
        }
        let (ty, data) = self.read_property(true)?;
        if ty == self.atoms.INCR {
            return self.fetch_incr(cap);
        }
        if data.len() > cap {
            log::info!(
                "clipboard payload of {} bytes exceeds limit of {cap}",
                data.len()
            );
            return Ok(None);
        }
        Ok(Some(data))
    }

    fn fetch_incr(&mut self, cap: usize) -> Result<Option<Vec<u8>>, XErr> {
        let (win, prop) = (self.win, self.atoms.CLIPVAULT_DATA);
        let mut out = Vec::new();
        let mut too_big = false;
        loop {
            let ev = self.wait_for(INCR_CHUNK_TIMEOUT, |ev| {
                matches!(ev, Event::PropertyNotify(e) if e.window == win && e.atom == prop && e.state == Property::NEW_VALUE)
            })?;
            if ev.is_none() {
                log::debug!("incremental clipboard transfer timed out");
                return Ok(None);
            }
            let (_, chunk) = self.read_property(true)?;
            if chunk.is_empty() {
                break;
            }
            // Keep draining when over the limit so the owner is not left hanging.
            if !too_big {
                out.extend_from_slice(&chunk);
                if out.len() > cap {
                    too_big = true;
                    out = Vec::new();
                }
            }
        }
        if too_big {
            log::info!("incremental clipboard payload exceeds limit of {cap}");
            return Ok(None);
        }
        Ok(Some(out))
    }

    fn read_property(&mut self, delete: bool) -> Result<(Atom, Vec<u8>), XErr> {
        let mut data = Vec::new();
        let mut offset = 0u32;
        loop {
            let r = self
                .conn
                .get_property(
                    delete,
                    self.win,
                    self.atoms.CLIPVAULT_DATA,
                    AtomEnum::ANY,
                    offset,
                    1 << 20,
                )?
                .reply()?;
            data.extend_from_slice(&r.value);
            if r.bytes_after == 0 {
                return Ok((r.type_, data));
            }
            offset += (r.value.len() / 4) as u32;
        }
    }

    /// Process events until `pred` matches one (returned) or the timeout elapses.
    /// Non-matching events are handled normally, so restores keep working during reads.
    fn wait_for(
        &mut self,
        timeout: Duration,
        pred: impl Fn(&Event) -> bool,
    ) -> Result<Option<Event>, XErr> {
        let deadline = Instant::now() + timeout;
        loop {
            self.conn.flush()?;
            while let Some(ev) = self.conn.poll_for_event()? {
                if pred(&ev) {
                    return Ok(Some(ev));
                }
                self.handle_event(ev)?;
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let ms = (deadline - now).as_millis().min(i32::MAX as u128) as i32 + 1;
            poll_fds(&[self.conn.stream().as_raw_fd()], ms)?;
        }
    }

    fn atom_name(&mut self, a: Atom) -> Result<String, XErr> {
        if let Some(n) = self.names.get(&a) {
            return Ok(n.clone());
        }
        let name = String::from_utf8_lossy(&self.conn.get_atom_name(a)?.reply()?.name).into_owned();
        self.names.insert(a, name.clone());
        self.ids.insert(name.clone(), a);
        Ok(name)
    }

    fn intern(&mut self, name: &str) -> Result<Atom, XErr> {
        if let Some(a) = self.ids.get(name) {
            return Ok(*a);
        }
        let a = self.conn.intern_atom(false, name.as_bytes())?.reply()?.atom;
        self.ids.insert(name.to_owned(), a);
        self.names.insert(a, name.to_owned());
        Ok(a)
    }

    // ---------------------------------------------------------------- restoring

    fn take_ownership(&mut self, formats: Vec<(String, Vec<u8>)>) -> Result<(), XErr> {
        let mut map = HashMap::new();
        let mut targets = vec![self.atoms.TARGETS, self.atoms.TIMESTAMP];
        for (mime, bytes) in formats {
            let a = self.intern(&mime)?;
            if let std::collections::hash_map::Entry::Vacant(v) = map.entry(a) {
                v.insert(Arc::new(bytes));
                targets.push(a);
            }
        }
        let time = self.server_time()?;
        self.conn
            .set_selection_owner(self.win, self.atoms.CLIPBOARD, time)?;
        let owner = self
            .conn
            .get_selection_owner(self.atoms.CLIPBOARD)?
            .reply()?
            .owner;
        if owner != self.win {
            return Err(XErr::Other(
                "another application holds the clipboard; ownership was refused".into(),
            ));
        }
        self.owned = Some(Owned {
            formats: map,
            targets,
            time,
        });
        Ok(())
    }

    /// Obtain a real server timestamp (ICCCM discourages CurrentTime for ownership).
    fn server_time(&mut self) -> Result<Timestamp, XErr> {
        let (win, prop) = (self.win, self.atoms.CLIPVAULT_TIME);
        self.conn
            .change_property8(PropMode::APPEND, win, prop, AtomEnum::STRING, &[])?;
        let ev = self.wait_for(
            READ_TIMEOUT,
            |ev| matches!(ev, Event::PropertyNotify(e) if e.window == win && e.atom == prop),
        )?;
        match ev {
            Some(Event::PropertyNotify(e)) => Ok(e.time),
            _ => Ok(CURRENT_TIME),
        }
    }

    fn serve(&mut self, e: SelectionRequestEvent) -> Result<(), XErr> {
        let property = if e.property == NONE {
            e.target
        } else {
            e.property
        };
        let mut ok = false;
        if e.selection == self.atoms.CLIPBOARD {
            if let Some(owned) = &self.owned {
                if e.target == self.atoms.TARGETS {
                    self.conn.change_property32(
                        PropMode::REPLACE,
                        e.requestor,
                        property,
                        AtomEnum::ATOM,
                        &owned.targets,
                    )?;
                    ok = true;
                } else if e.target == self.atoms.TIMESTAMP {
                    self.conn.change_property32(
                        PropMode::REPLACE,
                        e.requestor,
                        property,
                        AtomEnum::INTEGER,
                        &[owned.time],
                    )?;
                    ok = true;
                } else if let Some(data) = owned.formats.get(&e.target) {
                    if data.len() > self.chunk {
                        self.conn.change_window_attributes(
                            e.requestor,
                            &ChangeWindowAttributesAux::new()
                                .event_mask(EventMask::PROPERTY_CHANGE),
                        )?;
                        self.conn.change_property32(
                            PropMode::REPLACE,
                            e.requestor,
                            property,
                            self.atoms.INCR,
                            &[data.len() as u32],
                        )?;
                        self.outgoing.push(IncrSend {
                            requestor: e.requestor,
                            property,
                            target: e.target,
                            data: data.clone(),
                            offset: 0,
                            last_activity: Instant::now(),
                        });
                    } else {
                        self.conn.change_property8(
                            PropMode::REPLACE,
                            e.requestor,
                            property,
                            e.target,
                            data,
                        )?;
                    }
                    ok = true;
                }
            }
        }
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: e.time,
            requestor: e.requestor,
            selection: e.selection,
            target: e.target,
            property: if ok { property } else { NONE },
        };
        self.conn
            .send_event(false, e.requestor, EventMask::NO_EVENT, notify)?;
        self.conn.flush()?;
        Ok(())
    }

    fn continue_incr(&mut self, window: Window, atom: Atom) -> Result<(), XErr> {
        let Some(i) = self
            .outgoing
            .iter()
            .position(|t| t.requestor == window && t.property == atom)
        else {
            return Ok(());
        };
        let t = &mut self.outgoing[i];
        let start = t.offset;
        let end = (start + self.chunk).min(t.data.len());
        self.conn.change_property8(
            PropMode::REPLACE,
            window,
            atom,
            t.target,
            &t.data[start..end],
        )?;
        if start == end {
            // Zero-length write terminates the transfer.
            self.outgoing.remove(i);
        } else {
            t.offset = end;
            t.last_activity = Instant::now();
        }
        self.conn.flush()?;
        Ok(())
    }
}

/// Connect to the display of the *current* session with its current cookie (see `xauth`).
fn connect_current_session() -> Result<(RustConnection, usize), XErr> {
    let target = super::xauth::resolve().ok_or_else(|| XErr::Conn("no X display".into()))?;
    let number = super::xauth::display_number(&target.display);
    let cookie = match (&target.authority, number) {
        (Some(path), Some(n)) => super::xauth::read_cookie(path, n),
        _ => None,
    };
    let socket = number.map(|n| format!("/tmp/.X11-unix/X{n}"));
    if let (Some((name, data)), Some(socket)) = (cookie, socket) {
        if let Ok(unix) = std::os::unix::net::UnixStream::connect(&socket) {
            let (stream, _) = x11rb::rust_connection::DefaultStream::from_unix_stream(unix)?;
            let conn = RustConnection::connect_to_stream_with_auth_info(stream, 0, name, data)?;
            return Ok((conn, 0));
        }
    }
    // Fall back to the standard lookup (abstract sockets, TCP, ~/.Xauthority).
    Ok(x11rb::connect(Some(&target.display))?)
}

fn poll_fds(fds: &[i32], timeout_ms: i32) -> std::io::Result<()> {
    let mut pfds: Vec<libc::pollfd> = fds
        .iter()
        .map(|&fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    // SAFETY: pfds is a valid, correctly sized array of pollfd for the duration of the call.
    let r = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout_ms) };
    if r < 0 {
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
    Ok(())
}
