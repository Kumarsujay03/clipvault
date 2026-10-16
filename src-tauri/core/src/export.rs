//! Backup and restore of history as a single JSON file in `~/Documents/ClipVault Backups/`.
//! Restoring merges: items whose content already exists are skipped.

use std::path::PathBuf;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{AppError, AppResult};
use crate::ingest::sha256_hex;
use crate::media::MediaStore;
use crate::model::{now_ms, ItemType, NewItem};
use crate::settings::Settings;
use crate::store::{ExportRow, ImportMeta, Store};

const FORMAT: &str = "clipvault-backup";
const MAX_BACKUP_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct BackupFile {
    format: String,
    version: u32,
    exported_at: i64,
    items: Vec<ExportRow>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub name: String,
    pub bytes: u64,
    pub modified: i64,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub imported: usize,
    pub skipped: usize,
}

pub fn backup_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CLIPVAULT_BACKUP_DIR") {
        return PathBuf::from(d);
    }
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ClipVault Backups")
}

/// Write a backup. Returns the file path.
pub fn export(store: &Store, media: &MediaStore, include_images: bool) -> AppResult<PathBuf> {
    let mut items = store.export_rows()?;
    if include_images {
        for row in items.iter_mut().filter(|r| r.item_type == "image") {
            if let Some(rel) = &row.media_path {
                if let Ok(bytes) = media.read(rel) {
                    row.image_base64 =
                        Some(base64::engine::general_purpose::STANDARD.encode(bytes));
                }
            }
        }
    } else {
        items.retain(|r| r.item_type != "image");
    }
    let file = BackupFile {
        format: FORMAT.into(),
        version: 1,
        exported_at: now_ms(),
        items,
    };
    let dir = backup_dir();
    std::fs::create_dir_all(&dir)?;
    let stamp = chrono_like_stamp(file.exported_at);
    let path = dir.join(format!("clipvault-{stamp}.json"));
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&file)?)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// `YYYYMMDD-HHMMSS` in UTC without pulling in a date library.
fn chrono_like_stamp(ms: i64) -> String {
    let secs = ms / 1000;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub fn list() -> AppResult<Vec<BackupInfo>> {
    let dir = backup_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut out: Vec<BackupInfo> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            let modified = m
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_millis() as i64;
            Some(BackupInfo {
                name: e.file_name().to_string_lossy().into_owned(),
                bytes: m.len(),
                modified,
            })
        })
        .collect();
    out.sort_by_key(|b| std::cmp::Reverse(b.modified));
    Ok(out)
}

fn validate_name(name: &str) -> AppResult<PathBuf> {
    if name.is_empty()
        || name.contains('/')
        || name.contains("..")
        || !name.ends_with(".json")
        || name.len() > 255
    {
        return Err(AppError::Invalid("invalid backup file name".into()));
    }
    Ok(backup_dir().join(name))
}

/// Merge a backup from the backup folder into history.
pub fn import(
    store: &Store,
    media: &MediaStore,
    settings: &Settings,
    name: &str,
) -> AppResult<ImportReport> {
    let path = validate_name(name)?;
    if std::fs::metadata(&path)?.len() > MAX_BACKUP_BYTES {
        return Err(AppError::Invalid("backup file is too large".into()));
    }
    let file: BackupFile = serde_json::from_slice(&std::fs::read(&path)?)
        .map_err(|e| AppError::Invalid(format!("not a ClipVault backup: {e}")))?;
    if file.format != FORMAT || file.version != 1 {
        return Err(AppError::Invalid("unsupported backup format".into()));
    }
    let mut report = ImportReport {
        imported: 0,
        skipped: 0,
    };
    for row in file.items {
        match import_row(store, media, settings, row) {
            Ok(true) => report.imported += 1,
            Ok(false) => report.skipped += 1,
            Err(e) => {
                log::warn!("skipping unreadable backup entry: {e}");
                report.skipped += 1;
            }
        }
    }
    crate::trim_memory();
    Ok(report)
}

fn import_row(
    store: &Store,
    media: &MediaStore,
    settings: &Settings,
    row: ExportRow,
) -> AppResult<bool> {
    let item_type = ItemType::parse(&row.item_type)
        .ok_or_else(|| AppError::Invalid("unknown item type".into()))?;
    let meta = ImportMeta {
        created_at: row.created_at,
        copied_at: row.copied_at,
        last_used_at: row.last_used_at,
        copy_count: row.copy_count,
        is_pinned: row.is_pinned,
        is_favorite: row.is_favorite,
        tags: row.tags.clone(),
    };
    let base = NewItem {
        item_type,
        content_text: row.content_text.clone(),
        rich_html: row.rich_html.clone(),
        preview_text: row.preview_text.clone(),
        content_hash: String::new(),
        mime_type: row.mime_type.clone(),
        source_application: None,
        media_path: None,
        thumbnail_path: None,
        byte_size: row.byte_size,
        width: row.width,
        height: row.height,
        metadata: if row.metadata.is_object() {
            row.metadata.clone()
        } else {
            json!({})
        },
        search_extra: row.search_extra.clone(),
    };
    let item = match item_type {
        ItemType::Image => {
            let b64 = row
                .image_base64
                .as_deref()
                .ok_or_else(|| AppError::Invalid("image without data".into()))?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|_| AppError::Invalid("bad image data".into()))?;
            let hash = sha256_hex(&[b"image\0", &bytes]);
            if store.hash_exists(&hash)? {
                return Ok(false);
            }
            let saved = media.save_image(&hash, &row.mime_type, &bytes, settings.thumbnail_size)?;
            NewItem {
                content_hash: hash,
                media_path: Some(saved.media_rel),
                thumbnail_path: Some(saved.thumb_rel),
                byte_size: bytes.len() as i64,
                width: Some(saved.width as i64),
                height: Some(saved.height as i64),
                content_text: None,
                ..base
            }
        }
        ItemType::Files => {
            let uris: Vec<String> = row
                .metadata
                .get("uris")
                .and_then(|u| u.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            if uris.is_empty() {
                return Err(AppError::Invalid("file entry without URIs".into()));
            }
            NewItem {
                content_hash: sha256_hex(&[b"files\0", uris.join("\n").as_bytes()]),
                ..base
            }
        }
        ItemType::Text | ItemType::Code => {
            let text = row
                .content_text
                .as_deref()
                .ok_or_else(|| AppError::Invalid("text entry without text".into()))?;
            NewItem {
                content_hash: sha256_hex(&[b"text\0", text.as_bytes()]),
                ..base
            }
        }
    };
    if store.hash_exists(&item.content_hash)? {
        return Ok(false);
    }
    store.import(&item, &meta)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::{Captured, Ingestor};
    use crate::model::ListQuery;
    use crate::paths::Paths;
    use std::sync::Arc;

    #[test]
    fn stamp_formatting() {
        assert_eq!(chrono_like_stamp(0), "19700101-000000");
        assert_eq!(chrono_like_stamp(1_792_141_733_572), "20261016-090853");
    }

    #[test]
    fn rejects_unsafe_names() {
        for n in ["../x.json", "a/b.json", "x.txt", ""] {
            assert!(validate_name(n).is_err(), "{n}");
        }
        assert!(validate_name("clipvault-1.json").is_ok());
    }

    #[test]
    fn round_trip_merges_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("CLIPVAULT_BACKUP_DIR", dir.path().join("backups"));
        let paths = Paths::new(dir.path().join("d"), dir.path().join("c"));
        paths.ensure().unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let media = MediaStore::new(paths.clone());
        let ing = Ingestor::new(store.clone(), media.clone());
        let s = Settings::default();
        let clip = |m: &str, b: &[u8]| Captured {
            formats: vec![(m.into(), b.to_vec())],
            ..Default::default()
        };
        ing.ingest(&clip("UTF8_STRING", b"backup me"), &s, 1)
            .unwrap();
        ing.ingest(
            &clip("image/png", &crate::media::tests::png_bytes(20, 10)),
            &s,
            10_000,
        )
        .unwrap();
        let id = store.list(&ListQuery::default()).unwrap().items[1].id;
        store.set_pinned(id, true).unwrap();
        store.set_tags(id, &["Work Notes".into()]).unwrap();

        let path = export(&store, &media, true).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(list().unwrap()[0].name, name);

        // Into an empty store: everything comes back, flags and tags intact.
        let fresh = Store::open_in_memory().unwrap();
        let r = import(&fresh, &media, &s, &name).unwrap();
        assert_eq!(
            r,
            ImportReport {
                imported: 2,
                skipped: 0
            }
        );
        let page = fresh
            .list(&ListQuery {
                search: Some("#work-notes".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total, 1);
        assert!(page.items[0].is_pinned);
        assert_eq!(fresh.counts().unwrap().image, 1);

        // Again: nothing new.
        assert_eq!(
            import(&fresh, &media, &s, &name).unwrap(),
            ImportReport {
                imported: 0,
                skipped: 2
            }
        );
        std::env::remove_var("CLIPVAULT_BACKUP_DIR");
    }
}
