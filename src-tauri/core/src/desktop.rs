//! GNOME desktop integration: executables, keyboard shortcut, Shell extension, auto-paste.
//!
//! Everything here runs fixed `gsettings` / `gdbus` / `gnome-extensions` invocations with
//! validated arguments; nothing is passed through a shell.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::paths::Paths;

pub const EXTENSION_UUID: &str = "clipvault@clipvault.dev";
const EXTENSION_JS: &str = include_str!("../../../gnome-extension/extension.js");
const EXTENSION_META: &str = include_str!("../../../gnome-extension/metadata.json");

const MEDIA_KEYS: &str = "org.gnome.settings-daemon.plugins.media-keys";
const CUSTOM_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
const CUSTOM_PATH: &str =
    "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/clipvault/";
const SHELL_KEYS: &str = "org.gnome.shell.keybindings";
const MESSAGE_TRAY_KEY: &str = "toggle-message-tray";

// ------------------------------------------------------------------ executables

fn appimage() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|p| p.exists())
}

fn sibling(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = exe.with_file_name(name);
    p.exists().then_some(p)
}

pub fn running_from_build_dir() -> bool {
    appimage().is_none()
        && std::env::current_exe()
            .map(|e| {
                let s = e.to_string_lossy();
                s.contains("/target/debug/") || s.contains("/target/release/")
            })
            .unwrap_or(false)
}

/// argv that launches the panel UI.
pub fn ui_argv() -> Option<Vec<String>> {
    if let Some(a) = appimage() {
        return Some(vec![a.to_string_lossy().into_owned()]);
    }
    sibling("clipvault").map(|p| vec![p.to_string_lossy().into_owned()])
}

/// argv that launches the capture daemon.
pub fn daemon_argv() -> Option<Vec<String>> {
    if let Some(a) = appimage() {
        return Some(vec![a.to_string_lossy().into_owned(), "--daemon".into()]);
    }
    sibling("clipvaultd").map(|p| vec![p.to_string_lossy().into_owned()])
}

fn quote(arg: &str) -> String {
    if arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/-_.+=:".contains(c))
    {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Exec line for desktop entries.
pub fn daemon_exec() -> Option<String> {
    daemon_argv().map(|a| {
        a.iter()
            .map(|s| {
                if s.contains(' ') {
                    format!("\"{s}\"")
                } else {
                    s.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// Start a detached process (used to launch the daemon from the UI and the UI from the tray).
/// The child gets no terminal: no inherited output and its own process group, so a
/// service started from `clipvault --update` in a terminal neither writes into it nor dies
/// with it.
pub fn spawn_detached(argv: &[String]) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    // Reap the child when it exits so it never lingers as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Record how to launch the UI so the GNOME extension can do it.
pub fn write_launcher_info(paths: &Paths) {
    if paths.isolated || running_from_build_dir() {
        return;
    }
    if let Some(ui) = ui_argv() {
        let info = serde_json::json!({ "ui": ui, "version": env!("CARGO_PKG_VERSION") });
        let _ = std::fs::write(paths.launcher_path(), info.to_string());
    }
}

// ------------------------------------------------------------------ accelerators

/// "Super+V" → "<Super>v". Returns None for invalid or modifier-less shortcuts.
pub fn accel_to_gtk(s: &str) -> Option<String> {
    let parts: Vec<&str> = s
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let (key, mods) = parts.split_last()?;
    if mods.is_empty() {
        return None;
    }
    let mut out = String::new();
    for m in mods {
        out.push_str(match m.to_ascii_lowercase().as_str() {
            "super" | "win" | "windows" | "meta" => "<Super>",
            "ctrl" | "control" => "<Primary>",
            "shift" => "<Shift>",
            "alt" => "<Alt>",
            _ => return None,
        });
    }
    let k = key.to_ascii_lowercase();
    let valid = (k.chars().count() == 1
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || "`-=[];',./\\".contains(c)))
        || matches!(k.as_str(), "space" | "insert" | "tab" | "return")
        || (k.starts_with('f')
            && k[1..]
                .parse::<u8>()
                .map(|n| (1..=24).contains(&n))
                .unwrap_or(false));
    if !valid {
        return None;
    }
    out.push_str(&match k.as_str() {
        "space" => "space".to_string(),
        "insert" => "Insert".to_string(),
        "tab" => "Tab".to_string(),
        "return" => "Return".to_string(),
        f if f.starts_with('f') && f.len() > 1 => f.to_uppercase(),
        other => other.to_string(),
    });
    Some(out)
}

// ------------------------------------------------------------------ gsettings

fn gsettings(args: &[&str]) -> Result<String, String> {
    let out = Command::new("gsettings")
        .args(args)
        .output()
        .map_err(|e| format!("gsettings unavailable: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Parse a GVariant string array as printed by gsettings: `['a', 'b']` or `@as []`.
pub fn parse_strv(s: &str) -> Vec<String> {
    let s = s.trim().trim_start_matches("@as").trim();
    let inner = s.trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|p| p.trim().trim_matches('\'').trim_matches('"').to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

pub fn format_strv(v: &[String]) -> String {
    if v.is_empty() {
        return "@as []".into();
    }
    format!(
        "[{}]",
        v.iter()
            .map(|s| format!("'{}'", s.replace('\'', "\\'")))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn custom_key(key: &str) -> Result<String, String> {
    gsettings(&["get", &format!("{CUSTOM_SCHEMA}:{CUSTOM_PATH}"), key])
        .map(|v| v.trim_matches('\'').to_string())
}

fn set_custom_key(key: &str, value: &str) -> Result<(), String> {
    gsettings(&["set", &format!("{CUSTOM_SCHEMA}:{CUSTOM_PATH}"), key, value]).map(|_| ())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutStatus {
    /// How the shortcut is currently provided: "extension", "gnome" or "none".
    pub method: String,
    pub binding: Option<String>,
    pub extension: String,
    pub message: String,
}

fn freed_marker(paths: &Paths) -> PathBuf {
    paths.config_dir.join("freed-super-v")
}

/// Create/refresh the GNOME custom shortcut that runs `clipvault --toggle`.
/// For Super+V, GNOME's own use of that key (show notification list; Super+M still works)
/// is removed and remembered so it can be restored.
pub fn setup_shortcut(paths: &Paths, shortcut: &str) -> Result<ShortcutStatus, String> {
    if paths.isolated {
        return Err("desktop integration is disabled for isolated test profiles".into());
    }
    let accel = accel_to_gtk(shortcut).ok_or("invalid shortcut")?;
    // The extension grabs the key itself; a second GNOME shortcut would fight over it.
    if extension_state() == "active" {
        return Ok(shortcut_status(paths));
    }
    let ui = ui_argv().ok_or("cannot find the clipvault executable")?;
    let command = format!(
        "{} --toggle",
        ui.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
    );

    let mut list = parse_strv(&gsettings(&["get", MEDIA_KEYS, "custom-keybindings"])?);
    if !list.iter().any(|p| p == CUSTOM_PATH) {
        list.push(CUSTOM_PATH.to_string());
        gsettings(&["set", MEDIA_KEYS, "custom-keybindings", &format_strv(&list)])?;
    }
    set_custom_key("name", "ClipVault")?;
    set_custom_key("command", &command)?;
    set_custom_key("binding", &accel)?;

    if accel == "<Super>v" {
        free_super_v(paths)?;
    }
    Ok(shortcut_status(paths))
}

fn free_super_v(paths: &Paths) -> Result<(), String> {
    let tray = parse_strv(&gsettings(&["get", SHELL_KEYS, MESSAGE_TRAY_KEY])?);
    if tray.iter().any(|a| a.eq_ignore_ascii_case("<Super>v")) {
        let kept: Vec<String> = tray
            .into_iter()
            .filter(|a| !a.eq_ignore_ascii_case("<Super>v"))
            .collect();
        gsettings(&["set", SHELL_KEYS, MESSAGE_TRAY_KEY, &format_strv(&kept)])?;
        let _ = std::fs::write(freed_marker(paths), "");
        log::info!("freed <Super>v from GNOME's notification list shortcut");
    }
    Ok(())
}

/// Remove ClipVault's custom shortcut and give Super+V back to GNOME if we took it.
pub fn remove_shortcut(paths: &Paths) -> Result<(), String> {
    if paths.isolated {
        return Ok(());
    }
    let list = parse_strv(&gsettings(&["get", MEDIA_KEYS, "custom-keybindings"])?);
    if list.iter().any(|p| p == CUSTOM_PATH) {
        let kept: Vec<String> = list.into_iter().filter(|p| p != CUSTOM_PATH).collect();
        gsettings(&["set", MEDIA_KEYS, "custom-keybindings", &format_strv(&kept)])?;
    }
    for key in ["name", "command", "binding"] {
        let _ = gsettings(&["reset", &format!("{CUSTOM_SCHEMA}:{CUSTOM_PATH}"), key]);
    }
    if freed_marker(paths).exists() {
        let mut tray = parse_strv(&gsettings(&["get", SHELL_KEYS, MESSAGE_TRAY_KEY])?);
        if !tray.iter().any(|a| a.eq_ignore_ascii_case("<Super>v")) {
            tray.push("<Super>v".into());
            gsettings(&["set", SHELL_KEYS, MESSAGE_TRAY_KEY, &format_strv(&tray)])?;
        }
        let _ = std::fs::remove_file(freed_marker(paths));
    }
    Ok(())
}

pub fn shortcut_status(paths: &Paths) -> ShortcutStatus {
    let extension = extension_state();
    let listed = gsettings(&["get", MEDIA_KEYS, "custom-keybindings"])
        .map(|v| parse_strv(&v).iter().any(|p| p == CUSTOM_PATH))
        .unwrap_or(false);
    let binding = if listed {
        custom_key("binding").ok().filter(|b| !b.is_empty())
    } else {
        None
    };
    let command_ok = listed
        && custom_key("command")
            .map(|c| c.contains("--toggle"))
            .unwrap_or(false);
    let _ = paths;

    let (method, message) = if extension == "active" {
        ("extension", "Handled by the ClipVault GNOME extension (panel opens at the pointer and can paste for you).".to_string())
    } else if command_ok && binding.is_some() {
        let b = binding.clone().unwrap_or_default();
        let conflict = b == "<Super>v"
            && gsettings(&["get", SHELL_KEYS, MESSAGE_TRAY_KEY])
                .map(|v| {
                    parse_strv(&v)
                        .iter()
                        .any(|a| a.eq_ignore_ascii_case("<Super>v"))
                })
                .unwrap_or(false);
        if conflict {
            (
                "none",
                "Super+V is still assigned to GNOME's notification list.".to_string(),
            )
        } else {
            (
                "gnome",
                format!(
                    "Registered as a GNOME keyboard shortcut ({}).",
                    pretty_accel(&b)
                ),
            )
        }
    } else {
        (
            "none",
            "No keyboard shortcut is registered yet.".to_string(),
        )
    };
    ShortcutStatus {
        method: method.into(),
        binding: binding.map(|b| pretty_accel(&b)),
        extension,
        message,
    }
}

pub fn pretty_accel(a: &str) -> String {
    a.replace("<Super>", "Super+")
        .replace("<Primary>", "Ctrl+")
        .replace("<Control>", "Ctrl+")
        .replace("<Shift>", "Shift+")
        .replace("<Alt>", "Alt+")
        .split('+')
        .map(|p| {
            if p.len() == 1 {
                p.to_uppercase()
            } else {
                p.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

// ------------------------------------------------------------------ Shell extension

fn extension_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("gnome-shell/extensions").join(EXTENSION_UUID))
}

fn gdbus_shell(method: &str) -> Result<String, String> {
    let out = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--timeout",
            "2",
            "--dest",
            "org.gnome.Shell",
            "--object-path",
            "/dev/clipvault/Shell",
            "--method",
            &format!("dev.clipvault.Shell.{method}"),
        ])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err("extension not reachable".into())
    }
}

/// "active", "pending_login" (installed and enabled, loads at next login), "disabled" or
/// "not_installed".
pub fn extension_state() -> String {
    if gdbus_shell("Ping").is_ok() {
        return "active".into();
    }
    let installed = extension_dir()
        .map(|d| d.join("metadata.json").exists())
        .unwrap_or(false)
        || Path::new("/usr/share/gnome-shell/extensions")
            .join(EXTENSION_UUID)
            .exists();
    if !installed {
        return "not_installed".into();
    }
    let enabled = gsettings(&["get", "org.gnome.shell", "enabled-extensions"])
        .map(|v| parse_strv(&v).iter().any(|u| u == EXTENSION_UUID))
        .unwrap_or(false);
    if enabled { "pending_login" } else { "disabled" }.into()
}

/// Install the bundled extension for this user and enable it. GNOME on Wayland loads newly
/// installed extensions at the next login.
/// Keep an installed extension in step with the one bundled in this version. Only rewrites
/// files that differ; GNOME on Wayland picks the new code up at the next login.
pub fn refresh_extension(paths: &Paths) {
    if paths.isolated || running_from_build_dir() {
        return;
    }
    let Some(dir) = extension_dir() else { return };
    if !dir.join("metadata.json").exists() {
        return;
    }
    for (name, contents) in [
        ("extension.js", EXTENSION_JS),
        ("metadata.json", EXTENSION_META),
    ] {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(contents)
            && std::fs::write(&path, contents).is_ok()
        {
            log::info!("updated GNOME extension file {name}; active after the next login");
        }
    }
}

pub fn install_extension(paths: &Paths) -> Result<String, String> {
    if paths.isolated {
        return Err("desktop integration is disabled for isolated test profiles".into());
    }
    let dir = extension_dir().ok_or("no XDG data directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let changed = std::fs::read_to_string(dir.join("extension.js"))
        .ok()
        .as_deref()
        != Some(EXTENSION_JS);
    std::fs::write(dir.join("extension.js"), EXTENSION_JS).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("metadata.json"), EXTENSION_META).map_err(|e| e.to_string())?;

    let mut enabled = parse_strv(&gsettings(&[
        "get",
        "org.gnome.shell",
        "enabled-extensions",
    ])?);
    if !enabled.iter().any(|u| u == EXTENSION_UUID) {
        enabled.push(EXTENSION_UUID.into());
        gsettings(&[
            "set",
            "org.gnome.shell",
            "enabled-extensions",
            &format_strv(&enabled),
        ])?;
    }
    let disabled = parse_strv(
        &gsettings(&["get", "org.gnome.shell", "disabled-extensions"]).unwrap_or_default(),
    );
    if disabled.iter().any(|u| u == EXTENSION_UUID) {
        let kept: Vec<String> = disabled
            .into_iter()
            .filter(|u| u != EXTENSION_UUID)
            .collect();
        let _ = gsettings(&[
            "set",
            "org.gnome.shell",
            "disabled-extensions",
            &format_strv(&kept),
        ]);
    }
    // Takes effect immediately if the Shell already knows the extension (e.g. reinstall).
    let _ = Command::new("gnome-extensions")
        .args(["enable", EXTENSION_UUID])
        .stderr(Stdio::null())
        .status();
    if changed && extension_state() == "active" {
        log::info!("extension files updated; changes apply after the next login");
    }
    Ok(extension_state())
}

pub fn uninstall_extension() -> Result<(), String> {
    let _ = Command::new("gnome-extensions")
        .args(["disable", EXTENSION_UUID])
        .stderr(Stdio::null())
        .status();
    if let Ok(v) = gsettings(&["get", "org.gnome.shell", "enabled-extensions"]) {
        let kept: Vec<String> = parse_strv(&v)
            .into_iter()
            .filter(|u| u != EXTENSION_UUID)
            .collect();
        let _ = gsettings(&[
            "set",
            "org.gnome.shell",
            "enabled-extensions",
            &format_strv(&kept),
        ]);
    }
    if let Some(dir) = extension_dir() {
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Ask the extension to paste into the window that was focused before the panel opened.
pub fn request_paste() -> Result<(), String> {
    gdbus_shell("Paste").map(|_| ())
}

/// Everything a fresh install needs: autostart, shortcut, extension.
pub fn setup_all(paths: &Paths, shortcut: &str, autostart: bool) -> Vec<String> {
    let mut report = Vec::new();
    match crate::autostart::sync(paths, autostart) {
        Ok(()) => report.push(format!(
            "Start at login: {}",
            if autostart { "on" } else { "off" }
        )),
        Err(e) => report.push(format!("Start at login: failed ({e})")),
    }
    match setup_shortcut(paths, shortcut) {
        Ok(s) => report.push(format!("Shortcut: {}", s.message)),
        Err(e) => report.push(format!("Shortcut: failed ({e})")),
    }
    match install_extension(paths) {
        Ok(state) => report.push(format!(
            "GNOME extension: {}",
            match state.as_str() {
                "active" => "active".to_string(),
                "pending_login" => "installed — log out and back in to activate it".to_string(),
                other => other.to_string(),
            }
        )),
        Err(e) => report.push(format!("GNOME extension: failed ({e})")),
    }
    write_launcher_info(paths);
    let _ = std::fs::write(paths.config_dir.join(".integration-done"), "");
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_accelerators() {
        assert_eq!(accel_to_gtk("Super+V").as_deref(), Some("<Super>v"));
        assert_eq!(
            accel_to_gtk("Ctrl+Shift+V").as_deref(),
            Some("<Primary><Shift>v")
        );
        assert_eq!(accel_to_gtk("Alt+F10").as_deref(), Some("<Alt>F10"));
        assert_eq!(accel_to_gtk("Super+Space").as_deref(), Some("<Super>space"));
        assert_eq!(accel_to_gtk("V"), None, "needs a modifier");
        assert_eq!(accel_to_gtk("Hyper+V"), None);
        assert_eq!(accel_to_gtk("Super+Banana"), None);
        assert_eq!(pretty_accel("<Primary><Shift>v"), "Ctrl+Shift+V");
    }

    #[test]
    fn gvariant_string_arrays() {
        assert_eq!(parse_strv("@as []"), Vec::<String>::new());
        assert_eq!(
            parse_strv("['<Super>v', '<Super>m']"),
            vec!["<Super>v", "<Super>m"]
        );
        assert_eq!(format_strv(&["/a/".into(), "/b/".into()]), "['/a/', '/b/']");
        assert_eq!(format_strv(&[]), "@as []");
    }

    #[test]
    fn quoting() {
        assert_eq!(quote("/usr/bin/clipvault"), "/usr/bin/clipvault");
        assert_eq!(
            quote("/home/me/My Apps/ClipVault.AppImage"),
            "'/home/me/My Apps/ClipVault.AppImage'"
        );
    }
}
