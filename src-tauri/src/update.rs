//! Updates from GitHub Releases. Network access happens only here, only when the user asks
//! (or has turned on the daily check), and every download is verified against the release's
//! SHA256SUMS before anything is replaced.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use clipvault_core::desktop;
use clipvault_core::ipc::{self, Request};
use clipvault_core::paths::Paths;

pub const REPO: &str = "Kumarsujay03/clipvault";
const MAX_DOWNLOAD: u64 = 400 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallKind {
    /// Installed from the .deb package (`/usr/bin/clipvault`).
    Deb,
    /// Running from an AppImage file the user can replace without root.
    AppImage,
    /// Built from source or run from a build directory: update with git + rebuild.
    Manual,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub available: bool,
    pub notes: String,
    pub url: String,
    pub published_at: String,
    pub install_kind: InstallKind,
    pub asset_size: Option<u64>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: String,
    html_url: String,
    #[serde(default)]
    published_at: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize, Clone)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// The running version. `CLIPVAULT_FAKE_VERSION` overrides it for testing the update flow.
pub fn current_version() -> String {
    std::env::var("CLIPVAULT_FAKE_VERSION")
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}

/// "v1.2.3" / "1.2.3" → (1, 2, 3). Pre-release suffixes are ignored.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((
        it.next()??,
        it.next().flatten().unwrap_or(0),
        it.next().flatten().unwrap_or(0),
    ))
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

pub fn install_kind() -> InstallKind {
    if std::env::var_os("APPIMAGE")
        .map(|p| Path::new(&p).is_file())
        .unwrap_or(false)
    {
        return InstallKind::AppImage;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    if exe.starts_with("/usr/bin") {
        let ok = Command::new("dpkg-query")
            .args(["-W", "-f=${Status}", "clip-vault"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("install ok installed"))
            .unwrap_or(false);
        if ok {
            return InstallKind::Deb;
        }
    }
    InstallKind::Manual
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .user_agent(format!(
            "ClipVault/{} (+https://github.com/{REPO})",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .into()
}

fn fetch_release() -> Result<Release, String> {
    let url = std::env::var("CLIPVAULT_UPDATE_URL")
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases/latest"));
    let mut resp = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("could not reach GitHub: {e}"))?;
    let text = resp
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| format!("could not read the release information: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("unexpected reply from GitHub: {e}"))
}

fn pick_asset(assets: &[Asset], kind: InstallKind) -> Option<Asset> {
    let suffix = match kind {
        InstallKind::Deb => "_amd64.deb",
        InstallKind::AppImage => "_amd64.AppImage",
        InstallKind::Manual => return None,
    };
    assets.iter().find(|a| a.name.ends_with(suffix)).cloned()
}

fn record_check(paths: &Paths) {
    let _ = std::fs::write(
        paths.config_dir.join("update-check.json"),
        serde_json::json!({ "checked_at": clipvault_core::model::now_ms() }).to_string(),
    );
}

/// Milliseconds since the last update check, if any.
pub fn since_last_check(paths: &Paths) -> Option<i64> {
    let raw = std::fs::read_to_string(paths.config_dir.join("update-check.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(clipvault_core::model::now_ms() - v.get("checked_at")?.as_i64()?)
}

pub fn check(paths: &Paths) -> Result<UpdateInfo, String> {
    let rel = fetch_release()?;
    record_check(paths);
    let current = current_version();
    let latest = rel.tag_name.trim_start_matches('v').to_string();
    let kind = install_kind();
    Ok(UpdateInfo {
        available: is_newer(&latest, &current),
        asset_size: pick_asset(&rel.assets, kind).map(|a| a.size),
        current,
        latest,
        notes: rel.body.chars().take(4000).collect(),
        url: rel.html_url,
        published_at: rel.published_at,
        install_kind: kind,
    })
}

/// "hash  filename" lines → hash for `name`.
pub fn hash_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        let hash = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

fn download(
    url: &str,
    dest: &Path,
    limit: u64,
    progress: &dyn Fn(String),
) -> Result<String, String> {
    let mut resp = agent()
        .get(url)
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    let mut reader = resp.body_mut().with_config().limit(limit).reader();
    let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut total: u64 = 0;
    let mut last_report = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        total += n as u64;
        if total - last_report > 2 * 1024 * 1024 {
            last_report = total;
            progress(format!("Downloading… {} MB", total / (1024 * 1024)));
        }
    }
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("clipvault")
        .join("updates")
}

/// Download, verify and install the latest release. Returns a message for the user.
pub fn install(paths: &Paths, progress: &dyn Fn(String)) -> Result<String, String> {
    progress("Checking for the latest version…".into());
    let rel = fetch_release()?;
    let latest = rel.tag_name.trim_start_matches('v').to_string();
    if !is_newer(&latest, &current_version()) {
        return Ok(format!(
            "ClipVault {} is already the latest version.",
            current_version()
        ));
    }
    let kind = install_kind();
    if kind == InstallKind::Manual {
        return Err(format!(
            "This copy was built from source. Update with: git pull && npm ci && npm run tauri build && ./scripts/install.sh (version {latest} is available)."
        ));
    }
    let asset =
        pick_asset(&rel.assets, kind).ok_or("the release has no package for this install type")?;
    let sums_asset = rel
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS")
        .ok_or("the release has no SHA256SUMS file, so the download cannot be verified")?;

    let dir = cache_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let sums_path = dir.join("SHA256SUMS");
    download(
        &sums_asset.browser_download_url,
        &sums_path,
        64 * 1024,
        &|_| {},
    )?;
    let sums = std::fs::read_to_string(&sums_path).map_err(|e| e.to_string())?;
    let expected = hash_for(&sums, &asset.name).ok_or("SHA256SUMS does not list the package")?;

    let target_dir = match kind {
        // Write next to the AppImage so the final rename is atomic.
        InstallKind::AppImage => PathBuf::from(std::env::var_os("APPIMAGE").unwrap())
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or(dir.clone()),
        _ => dir.clone(),
    };
    let part = target_dir.join(format!(".{}.part", asset.name));
    progress(format!("Downloading ClipVault {latest}…"));
    let actual = download(&asset.browser_download_url, &part, MAX_DOWNLOAD, progress)?;
    if actual != expected {
        let _ = std::fs::remove_file(&part);
        return Err("the download did not match its published checksum and was discarded".into());
    }

    match kind {
        InstallKind::Deb => {
            let deb = dir.join(&asset.name);
            std::fs::rename(&part, &deb).map_err(|e| e.to_string())?;
            progress("Installing… (enter your password in the system dialog)".into());
            let status = Command::new("pkexec")
                .args([
                    "/usr/bin/apt-get",
                    "install",
                    "-y",
                    "--no-install-recommends",
                ])
                .arg(&deb)
                .status()
                .map_err(|e| format!("could not start the installer: {e}"))?;
            match status.code() {
                Some(0) => {}
                Some(126) | Some(127) => return Err("installation was cancelled".into()),
                _ => return Err(format!("the package installer failed ({status})")),
            }
            let _ = std::fs::remove_file(&deb);
        }
        InstallKind::AppImage => {
            use std::os::unix::fs::PermissionsExt;
            let appimage = PathBuf::from(std::env::var_os("APPIMAGE").unwrap());
            std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| e.to_string())?;
            std::fs::rename(&part, &appimage)
                .map_err(|e| format!("could not replace the AppImage: {e}"))?;
        }
        InstallKind::Manual => unreachable!(),
    }
    let _ = paths;
    Ok(format!("Updated to ClipVault {latest}."))
}

/// Restart the capture service so it runs the newly installed binary.
pub fn restart_service() -> bool {
    let _ = ipc::request(&Request::Quit, Duration::from_secs(3));
    for _ in 0..30 {
        if !ipc::daemon_running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    match desktop::daemon_argv() {
        Some(argv) => desktop::spawn_detached(&argv).is_ok(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert_eq!(parse_version("v0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("1.10"), Some((1, 10, 0)));
        assert_eq!(parse_version("2.0.1-beta.1"), Some((2, 0, 1)));
        assert_eq!(parse_version("banana"), None);
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn reads_checksums() {
        let h = "a".repeat(64);
        let sums = format!(
            "{h}  ClipVault_0.2.0_amd64.deb\n{}  *ClipVault_0.2.0_amd64.AppImage\n",
            "b".repeat(64)
        );
        assert_eq!(
            hash_for(&sums, "ClipVault_0.2.0_amd64.deb").as_deref(),
            Some(h.as_str())
        );
        assert_eq!(
            hash_for(&sums, "ClipVault_0.2.0_amd64.AppImage"),
            Some("b".repeat(64))
        );
        assert_eq!(hash_for(&sums, "other.deb"), None);
        assert_eq!(
            hash_for(
                "xyz  ClipVault_0.2.0_amd64.deb",
                "ClipVault_0.2.0_amd64.deb"
            ),
            None,
            "bad hash rejected"
        );
    }

    #[test]
    fn picks_assets_by_install_kind() {
        let a = |n: &str| Asset {
            name: n.into(),
            browser_download_url: format!("https://x/{n}"),
            size: 1,
        };
        let assets = vec![
            a("ClipVault_0.2.0_amd64.deb"),
            a("ClipVault_0.2.0_amd64.AppImage"),
            a("SHA256SUMS"),
        ];
        assert_eq!(
            pick_asset(&assets, InstallKind::Deb).unwrap().name,
            "ClipVault_0.2.0_amd64.deb"
        );
        assert_eq!(
            pick_asset(&assets, InstallKind::AppImage).unwrap().name,
            "ClipVault_0.2.0_amd64.AppImage"
        );
        assert!(pick_asset(&assets, InstallKind::Manual).is_none());
    }
}
