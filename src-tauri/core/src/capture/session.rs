use serde::{Deserialize, Serialize};

/// What the running desktop session offers. Read from the environment at startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_type: String,
    pub desktop: String,
    pub wayland_display: Option<String>,
    pub x_display: Option<String>,
}

impl SessionInfo {
    pub fn detect() -> Self {
        let var = |k| std::env::var(k).ok().filter(|v: &String| !v.is_empty());
        let wayland_display = var("WAYLAND_DISPLAY");
        let session_type = var("XDG_SESSION_TYPE").unwrap_or_else(|| {
            if wayland_display.is_some() {
                "wayland".into()
            } else {
                "x11".into()
            }
        });
        Self {
            session_type,
            desktop: var("XDG_CURRENT_DESKTOP").unwrap_or_else(|| "unknown".into()),
            wayland_display,
            x_display: var("DISPLAY"),
        }
    }

    pub fn is_gnome(&self) -> bool {
        self.desktop.to_ascii_lowercase().contains("gnome")
    }
}
