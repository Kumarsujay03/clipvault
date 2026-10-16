//! Live clipboard tests against the real desktop session. They overwrite the clipboard, so
//! they are ignored by default. Run with:
//!
//!     cargo test --test live_clipboard -- --ignored --test-threads=1
//!
//! Requires wl-clipboard (`wl-copy` / `wl-paste`) and a Wayland or X11 session.

use std::io::{Cursor, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use clipvault_core::capture::{session::SessionInfo, BackendState, CaptureController, Shared};
use clipvault_core::ingest::Captured;

fn start() -> (Arc<CaptureController>, mpsc::Receiver<Captured>) {
    let (tx, rx) = mpsc::channel();
    let shared = Shared {
        paused: Arc::new(AtomicBool::new(false)),
        want_images: Arc::new(AtomicBool::new(true)),
        want_files: Arc::new(AtomicBool::new(true)),
        max_bytes: Arc::new(AtomicUsize::new(64 * 1024 * 1024)),
    };
    let ctl = CaptureController::start(
        SessionInfo::detect(),
        shared,
        move |c| {
            let _ = tx.send(c);
        },
        |_| {},
    );
    for _ in 0..50 {
        if ctl.status().state == BackendState::Running {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        ctl.status().state,
        BackendState::Running,
        "{:?}",
        ctl.status().error
    );
    // Drain the startup snapshot of the current clipboard.
    std::thread::sleep(Duration::from_millis(500));
    while rx.try_recv().is_ok() {}
    (ctl, rx)
}

fn wl_copy(mime: &str, data: &[u8]) {
    let mut child = Command::new("wl-copy")
        .args(["--type", mime])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("wl-copy");
    child.stdin.take().unwrap().write_all(data).unwrap();
    child.wait().unwrap();
}

fn wl_paste(mime: &str) -> Vec<u8> {
    let out = Command::new("wl-paste")
        .args(["--no-newline", "--type", mime])
        .output()
        .expect("wl-paste");
    out.stdout
}

fn wait_capture(rx: &mpsc::Receiver<Captured>, mime_prefix: &str) -> Captured {
    loop {
        let c = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("no clipboard change captured within 5s");
        if c.formats.iter().any(|(m, _)| {
            m.starts_with(mime_prefix) || (mime_prefix == "text" && m == "UTF8_STRING")
        }) {
            return c;
        }
    }
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([(x * 7) as u8, (y * 3) as u8, (x ^ y) as u8, 255])
    });
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

#[test]
#[ignore]
fn captures_and_restores_text() {
    let (ctl, rx) = start();
    let token = format!("clipvault-live-{}", std::process::id());
    wl_copy("text/plain;charset=utf-8", token.as_bytes());
    let c = wait_capture(&rx, "text");
    assert!(
        c.formats.iter().any(|(_, b)| b == token.as_bytes()),
        "captured text matches"
    );

    let restored = format!("restored-ünïcode-{}", std::process::id());
    ctl.set_clipboard(vec![
        ("UTF8_STRING".into(), restored.as_bytes().to_vec()),
        (
            "text/plain;charset=utf-8".into(),
            restored.as_bytes().to_vec(),
        ),
    ])
    .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(wl_paste("text/plain;charset=utf-8"), restored.as_bytes());
    assert!(
        rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "own restore is not re-captured"
    );
    ctl.shutdown();
}

#[test]
#[ignore]
fn captures_and_restores_large_image() {
    let (ctl, rx) = start();
    let bytes = png(1600, 1200); // several MB: exercises INCR in both directions
    wl_copy("image/png", &bytes);
    let c = wait_capture(&rx, "image/png");
    let got = &c.formats.iter().find(|(m, _)| m == "image/png").unwrap().1;
    assert_eq!(got.len(), bytes.len());
    assert!(got == &bytes);

    let other = png(900, 700);
    ctl.set_clipboard(vec![("image/png".into(), other.clone())])
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(wl_paste("image/png") == other, "restored image round-trips");
    ctl.shutdown();
}

#[test]
#[ignore]
fn paused_capture_records_nothing() {
    let (ctl, rx) = start();
    ctl.set_paused(true);
    wl_copy("text/plain;charset=utf-8", b"should not be captured");
    assert!(rx.recv_timeout(Duration::from_secs(2)).is_err());
    ctl.set_paused(false);
    wl_copy("text/plain;charset=utf-8", b"captured again");
    wait_capture(&rx, "text");
    ctl.shutdown();
}
