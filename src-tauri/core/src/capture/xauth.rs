//! Find the *current* X display and its credentials.
//!
//! The daemon can outlive a GNOME session (Ubuntu keeps user processes after logout), and
//! each login starts Xwayland with a fresh cookie file. So instead of trusting the
//! environment the process was started with, this re-resolves on every (re)connect:
//! systemd's user environment (updated by gnome-session at login) → our own environment →
//! the newest Mutter cookie file in the runtime dir.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct XTarget {
    pub display: String,
    pub authority: Option<PathBuf>,
}

fn systemd_user_env() -> Vec<(String, String)> {
    let Ok(out) = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn newest_mutter_cookie() -> Option<PathBuf> {
    let dir = dirs::runtime_dir()?;
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".mutter-Xwaylandauth.")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

pub fn resolve() -> Option<XTarget> {
    let sys = systemd_user_env();
    let get = |k: &str| sys.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let exists = |p: &String| Path::new(p).is_file();

    let display = get("DISPLAY")
        .filter(|d| !d.is_empty())
        .or_else(|| std::env::var("DISPLAY").ok().filter(|d| !d.is_empty()))?;
    let authority = get("XAUTHORITY")
        .filter(exists)
        .or_else(|| std::env::var("XAUTHORITY").ok().filter(exists))
        .map(PathBuf::from)
        .or_else(newest_mutter_cookie)
        .or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".Xauthority"))
                .filter(|p| p.is_file())
        });
    Some(XTarget { display, authority })
}

/// Display number from ":0", ":1.0", "unix/:0" …
pub fn display_number(display: &str) -> Option<u16> {
    let after = display.rsplit_once(':')?.1;
    after.split('.').next()?.parse().ok()
}

/// Pick the MIT-MAGIC-COOKIE-1 entry for `display` from an Xauthority file.
pub fn read_cookie(path: &Path, display: u16) -> Option<(Vec<u8>, Vec<u8>)> {
    let data = std::fs::read(path).ok()?;
    let mut pos = 0usize;
    let mut fallback = None;
    let want = display.to_string();
    let take = |pos: &mut usize| -> Option<Vec<u8>> {
        let len = u16::from_be_bytes([*data.get(*pos)?, *data.get(*pos + 1)?]) as usize;
        let v = data.get(*pos + 2..*pos + 2 + len)?.to_vec();
        *pos += 2 + len;
        Some(v)
    };
    while pos + 2 <= data.len() {
        pos += 2; // family
        let _address = take(&mut pos)?;
        let number = take(&mut pos)?;
        let name = take(&mut pos)?;
        let cookie = take(&mut pos)?;
        if name != b"MIT-MAGIC-COOKIE-1" {
            continue;
        }
        if number.is_empty() || number == want.as_bytes() {
            return Some((name, cookie));
        }
        fallback.get_or_insert((name, cookie));
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(number: &str, name: &str, data: &[u8]) -> Vec<u8> {
        let mut v = vec![0x01, 0x00];
        for field in [b"host".as_slice(), number.as_bytes(), name.as_bytes(), data] {
            v.extend_from_slice(&(field.len() as u16).to_be_bytes());
            v.extend_from_slice(field);
        }
        v
    }

    #[test]
    fn parses_display_numbers() {
        assert_eq!(display_number(":0"), Some(0));
        assert_eq!(display_number(":1.0"), Some(1));
        assert_eq!(display_number("unix/:12"), Some(12));
        assert_eq!(display_number("bogus"), None);
    }

    #[test]
    fn picks_matching_cookie() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("auth");
        let mut f = entry("1", "MIT-MAGIC-COOKIE-1", b"one");
        f.extend(entry("0", "XDM-AUTHORIZATION-1", b"nope"));
        f.extend(entry("0", "MIT-MAGIC-COOKIE-1", b"zero"));
        std::fs::write(&p, f).unwrap();
        assert_eq!(read_cookie(&p, 0).unwrap().1, b"zero");
        assert_eq!(read_cookie(&p, 1).unwrap().1, b"one");
        assert_eq!(
            read_cookie(&p, 7).unwrap().1,
            b"one",
            "falls back to any cookie"
        );
        std::fs::write(&p, [0u8, 1, 0]).unwrap();
        assert!(read_cookie(&p, 0).is_none(), "truncated file");
    }

    #[test]
    fn resolves_live_session() {
        // On a desktop session this must find a display; in CI it may legitimately be None.
        if std::env::var_os("DISPLAY").is_some() {
            let t = resolve().expect("display");
            assert!(display_number(&t.display).is_some());
        }
    }
}
