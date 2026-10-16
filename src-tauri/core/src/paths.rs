use std::path::{Component, Path, PathBuf};

use crate::error::{AppError, AppResult};

/// XDG locations used by ClipVault.
///
/// `CLIPVAULT_DATA_DIR` / `CLIPVAULT_CONFIG_DIR` override the defaults (used by tests and
/// for running an isolated development profile).
#[derive(Debug, Clone)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub config_dir: PathBuf,
    /// A test/development profile set via environment overrides. Desktop integration
    /// (autostart, keyboard shortcuts, extension files) is never touched for these.
    pub isolated: bool,
}

impl Paths {
    pub fn from_env() -> AppResult<Self> {
        let data_dir = match std::env::var_os("CLIPVAULT_DATA_DIR") {
            Some(p) => PathBuf::from(p),
            None => dirs::data_dir()
                .ok_or_else(|| AppError::Other("cannot resolve XDG data dir".into()))?
                .join("clipvault"),
        };
        let config_dir = match std::env::var_os("CLIPVAULT_CONFIG_DIR") {
            Some(p) => PathBuf::from(p),
            None => dirs::config_dir()
                .ok_or_else(|| AppError::Other("cannot resolve XDG config dir".into()))?
                .join("clipvault"),
        };
        let isolated = std::env::var_os("CLIPVAULT_DATA_DIR").is_some()
            || std::env::var_os("CLIPVAULT_CONFIG_DIR").is_some();
        Ok(Self {
            isolated,
            ..Self::new(data_dir, config_dir)
        })
    }

    pub fn new(data_dir: PathBuf, config_dir: PathBuf) -> Self {
        Self {
            data_dir,
            config_dir,
            isolated: true,
        }
    }

    pub fn ensure(&self) -> AppResult<()> {
        for dir in [
            &self.data_dir,
            &self.config_dir,
            &self.media_dir(),
            &self.thumbs_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
            set_private(dir);
        }
        Ok(())
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("clipvault.db")
    }
    pub fn media_dir(&self) -> PathBuf {
        self.data_dir.join("media")
    }
    pub fn thumbs_dir(&self) -> PathBuf {
        self.data_dir.join("thumbs")
    }
    /// Where the GNOME extension finds the launcher command.
    pub fn launcher_path(&self) -> PathBuf {
        self.config_dir.join("launcher.json")
    }
    pub fn settings_path(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    /// Resolve a media path stored in the database (relative to the data dir).
    /// Only paths inside `media/` or `thumbs/` without `..` components are accepted.
    pub fn resolve_media(&self, rel: &str) -> AppResult<PathBuf> {
        let p = Path::new(rel);
        let mut comps = p.components();
        let first_ok =
            matches!(comps.next(), Some(Component::Normal(c)) if c == "media" || c == "thumbs");
        let rest_ok = comps.all(|c| matches!(c, Component::Normal(_)));
        if !first_ok || !rest_ok {
            return Err(AppError::Invalid(
                "media path outside data directory".into(),
            ));
        }
        Ok(self.data_dir.join(p))
    }
}

/// Clipboard history is private: restrict directories to the owner (0700).
fn set_private(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_media_rejects_traversal() {
        let p = Paths::new("/d".into(), "/c".into());
        assert!(p.resolve_media("media/ab/x.png").is_ok());
        assert!(p.resolve_media("thumbs/ab/x.png").is_ok());
        assert!(p.resolve_media("../etc/passwd").is_err());
        assert!(p.resolve_media("media/../../etc/passwd").is_err());
        assert!(p.resolve_media("/etc/passwd").is_err());
        assert!(p.resolve_media("clipvault.db").is_err());
    }
}
