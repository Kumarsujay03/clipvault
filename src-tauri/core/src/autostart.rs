//! Launch-at-login via an XDG autostart entry (`~/.config/autostart/clipvault.desktop`).
//! Only the small capture daemon is started at login; the panel loads on demand.

use std::path::PathBuf;

use crate::desktop;
use crate::paths::Paths;

pub fn entry_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("autostart").join("clipvault.desktop"))
}

pub fn entry_contents() -> Option<String> {
    let exec = desktop::daemon_exec()?;
    Some(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=ClipVault\n\
         Comment=Clipboard history (background service)\n\
         Exec={exec}\n\
         Icon=clipvault\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n\
         X-GNOME-Autostart-Delay=2\n"
    ))
}

pub fn is_enabled() -> bool {
    entry_path().map(|p| p.exists()).unwrap_or(false)
}

/// Make the autostart entry match `enabled`. Never touches anything for isolated profiles
/// or for binaries run straight out of a cargo `target/` directory.
pub fn sync(paths: &Paths, enabled: bool) -> Result<(), String> {
    if paths.isolated || desktop::running_from_build_dir() {
        return Ok(());
    }
    let path = entry_path().ok_or("no XDG config directory")?;
    if enabled {
        let contents = entry_contents().ok_or("cannot determine the ClipVault executable")?;
        if std::fs::read_to_string(&path).ok().as_deref() != Some(contents.as_str()) {
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&path, contents).map_err(|e| e.to_string())?;
            log::info!("autostart entry written");
        }
    } else if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        log::info!("autostart entry removed");
    }
    Ok(())
}
