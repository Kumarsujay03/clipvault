//! Optional local OCR using the `tesseract` command, so text inside copied images and
//! screenshots becomes searchable. Off by default; runs at the lowest CPU priority.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CHARS: usize = 20_000;

pub fn available() -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("tesseract").is_file()))
        .unwrap_or(false)
}

/// Recognize text in an image file. Returns None on failure, timeout or empty output.
pub fn recognize(image: &Path) -> Option<String> {
    let mut child = Command::new("nice")
        .args(["-n", "19", "tesseract"])
        .arg(image)
        .args(["stdout", "--psm", "3"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Read concurrently so a large output cannot block the child on a full pipe.
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.by_ref().take(1024 * 1024).read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                log::warn!("OCR timed out");
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return None,
        }
    }
    let out = reader.join().ok()?;
    let text: String = String::from_utf8_lossy(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let text: String = text.chars().take(MAX_CHARS).collect();
    (!text.is_empty()).then_some(text)
}
