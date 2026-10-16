use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::paths::Paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    System,
    Light,
    Dark,
}

/// How repeated copies of identical content are recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DedupeMode {
    /// Identical content anywhere in history is moved to the top instead of duplicated.
    MoveToTop,
    /// Only a copy identical to the most recent entry is merged.
    Consecutive,
    /// Every copy creates a new entry.
    KeepAll,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Start capturing as soon as ClipVault launches.
    pub start_monitoring: bool,
    /// Privacy mode: capture stays off after launch until enabled explicitly.
    pub privacy_mode_on_startup: bool,
    pub theme: Theme,
    pub shortcut: String,
    pub start_at_login: bool,
    /// Unpinned items older than this are pruned. 0 = keep forever.
    pub retention_days: u32,
    pub max_items: u32,
    pub max_media_mb: u32,
    /// Clipboard payloads larger than this are not recorded.
    pub max_item_mb: u32,
    /// Longest edge of generated thumbnails, in pixels.
    pub thumbnail_size: u32,
    pub dedupe: DedupeMode,
    /// Hide the window after an item is copied (Win+V style).
    pub close_on_copy: bool,
    pub capture_images: bool,
    pub capture_files: bool,
    pub exclude_sensitive: bool,
    /// Application identifiers to ignore (only effective when the backend can identify sources).
    pub excluded_apps: Vec<String>,
    /// Show the status icon in the GNOME top bar.
    pub show_tray_icon: bool,
    /// Seconds the panel stays in memory after it is hidden (0 = unload immediately).
    /// Reopening within this window is instant; afterwards the UI process exits and only
    /// the small capture daemon remains.
    pub panel_keep_alive_secs: u32,
    /// Hide the panel when it loses focus (like Windows' Win+V).
    pub hide_on_blur: bool,
    /// After choosing an item, paste it into the previous window (needs the GNOME extension).
    pub paste_on_select: bool,
    /// Extract text from copied images with Tesseract (if installed) so they are searchable.
    pub ocr_images: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            start_monitoring: true,
            privacy_mode_on_startup: false,
            theme: Theme::System,
            shortcut: "Super+V".into(),
            start_at_login: true,
            retention_days: 30,
            max_items: 1000,
            max_media_mb: 500,
            max_item_mb: 25,
            thumbnail_size: 320,
            dedupe: DedupeMode::MoveToTop,
            close_on_copy: true,
            capture_images: true,
            capture_files: true,
            exclude_sensitive: true,
            excluded_apps: Vec::new(),
            show_tray_icon: true,
            panel_keep_alive_secs: 120,
            hide_on_blur: true,
            paste_on_select: true,
            ocr_images: false,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> AppResult<()> {
        fn range(name: &str, v: u32, lo: u32, hi: u32) -> AppResult<()> {
            if v < lo || v > hi {
                return Err(AppError::Invalid(format!(
                    "{name} must be between {lo} and {hi}"
                )));
            }
            Ok(())
        }
        range("retention_days", self.retention_days, 0, 3650)?;
        range("max_items", self.max_items, 10, 100_000)?;
        range("max_media_mb", self.max_media_mb, 10, 100_000)?;
        range("max_item_mb", self.max_item_mb, 1, 512)?;
        range("thumbnail_size", self.thumbnail_size, 96, 1024)?;
        range(
            "panel_keep_alive_secs",
            self.panel_keep_alive_secs,
            0,
            86_400,
        )?;
        if crate::desktop::accel_to_gtk(&self.shortcut).is_none() {
            return Err(AppError::Invalid(
                "shortcut must look like Super+V or Ctrl+Shift+V".into(),
            ));
        }
        if self.shortcut.trim().is_empty() || self.shortcut.len() > 64 {
            return Err(AppError::Invalid("shortcut must be 1-64 characters".into()));
        }
        if self.excluded_apps.len() > 200 || self.excluded_apps.iter().any(|a| a.len() > 256) {
            return Err(AppError::Invalid(
                "too many or too long excluded application entries".into(),
            ));
        }
        Ok(())
    }

    pub fn max_item_bytes(&self) -> usize {
        self.max_item_mb as usize * 1024 * 1024
    }

    /// Load settings; a missing file yields defaults, an unreadable/invalid one is backed up
    /// and replaced by defaults so the app still starts.
    pub fn load(paths: &Paths) -> (Settings, Option<String>) {
        let path = paths.settings_path();
        let raw = match std::fs::read_to_string(&path) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (Settings::default(), None)
            }
            Err(e) => {
                return (
                    Settings::default(),
                    Some(format!("could not read settings: {e}")),
                )
            }
        };
        match serde_json::from_str::<Settings>(&raw) {
            Ok(s) if s.validate().is_ok() => (s, None),
            Ok(_) | Err(_) => {
                let backup = path.with_extension("json.invalid");
                let _ = std::fs::rename(&path, &backup);
                (
                    Settings::default(),
                    Some(format!(
                        "settings were invalid and were reset (backup: {})",
                        backup.display()
                    )),
                )
            }
        }
    }

    pub fn save(&self, paths: &Paths) -> AppResult<()> {
        self.validate()?;
        let path = paths.settings_path();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        Settings::default().validate().unwrap();
    }

    #[test]
    fn rejects_out_of_range() {
        let s = Settings {
            max_items: 1,
            ..Settings::default()
        };
        assert!(s.validate().is_err());
        let s = Settings {
            thumbnail_size: 5000,
            ..Settings::default()
        };
        assert!(s.validate().is_err());
        let s = Settings {
            shortcut: " ".into(),
            ..Settings::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn partial_json_fills_defaults() {
        let s: Settings = serde_json::from_str(r#"{"max_items": 50}"#).unwrap();
        assert_eq!(s.max_items, 50);
        assert_eq!(s.retention_days, 30);
    }

    #[test]
    fn load_resets_invalid_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().join("d"), dir.path().join("c"));
        paths.ensure().unwrap();
        std::fs::write(paths.settings_path(), "{not json").unwrap();
        let (s, warn) = Settings::load(&paths);
        assert_eq!(s, Settings::default());
        assert!(warn.is_some());
        assert!(paths
            .settings_path()
            .with_extension("json.invalid")
            .exists());
    }
}
