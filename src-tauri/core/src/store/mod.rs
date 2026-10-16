//! SQLite persistence: history rows, FTS search, pruning. All access goes through a single
//! connection guarded by a mutex; callers run these methods off the UI thread.

pub mod migrations;

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row, ToSql, Transaction};

use crate::error::{AppError, AppResult};
use crate::model::*;
use crate::settings::{DedupeMode, Settings};

/// Largest text sent to the UI in one detail response.
const MAX_DETAIL_TEXT: usize = 1024 * 1024;

pub struct Store {
    conn: Mutex<Connection>,
    /// Set when a corrupt database was moved aside on startup.
    pub recovered: Option<String>,
}

/// Raw data needed to restore an item to the clipboard.
#[derive(Debug, Clone)]
pub struct RawItem {
    pub id: i64,
    pub item_type: ItemType,
    pub content_text: Option<String>,
    pub rich_html: Option<String>,
    pub mime_type: String,
    pub media_path: Option<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
pub struct ImportMeta {
    pub created_at: i64,
    pub copied_at: i64,
    pub last_used_at: Option<i64>,
    pub copy_count: i64,
    pub is_pinned: bool,
    pub is_favorite: bool,
    pub tags: Vec<String>,
}

/// One history row in a backup file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExportRow {
    pub item_type: String,
    pub content_text: Option<String>,
    pub rich_html: Option<String>,
    #[serde(default)]
    pub preview_text: String,
    pub mime_type: String,
    pub created_at: i64,
    pub copied_at: i64,
    pub last_used_at: Option<i64>,
    #[serde(default)]
    pub copy_count: i64,
    #[serde(default)]
    pub is_pinned: bool,
    #[serde(default)]
    pub is_favorite: bool,
    #[serde(skip_serializing, default)]
    pub media_path: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub search_extra: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub byte_size: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_base64: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub items: i64,
    pub media_bytes: i64,
}

const SUMMARY_COLS: &str =
    "id, item_type, preview_text, mime_type, created_at, copied_at, last_used_at, \
     copy_count, is_pinned, is_favorite, byte_size, width, height, thumbnail_path IS NOT NULL, \
     COALESCE(json_array_length(metadata_json, '$.uris'), 0), rich_html IS NOT NULL, \
     json_extract(metadata_json, '$.language'), tags_text";

/// Number of columns in SUMMARY_COLS; detail queries append after this.
const SUMMARY_N: usize = 18;

fn row_to_summary(r: &Row) -> rusqlite::Result<ItemSummary> {
    let t: String = r.get(1)?;
    Ok(ItemSummary {
        id: r.get(0)?,
        item_type: ItemType::parse(&t).unwrap_or(ItemType::Text),
        preview: r.get(2)?,
        mime_type: r.get(3)?,
        created_at: r.get(4)?,
        copied_at: r.get(5)?,
        last_used_at: r.get(6)?,
        copy_count: r.get(7)?,
        is_pinned: r.get(8)?,
        is_favorite: r.get(9)?,
        byte_size: r.get(10)?,
        width: r.get(11)?,
        height: r.get(12)?,
        has_thumbnail: r.get(13)?,
        file_count: r.get(14)?,
        has_html: r.get(15)?,
        language: r.get(16)?,
        tags: split_tags(&r.get::<_, String>(17)?),
    })
}

fn split_tags(s: &str) -> Vec<String> {
    s.split_whitespace()
        .map(|t| t.trim_start_matches('#').to_owned())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Lower-case, no spaces, at most 32 chars; empty tags dropped, max 20 per item.
pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t: String = t
            .trim()
            .trim_start_matches('#')
            .to_lowercase()
            .chars()
            .map(|c| if c.is_whitespace() { '-' } else { c })
            .filter(|c| c.is_alphanumeric() || "-_.".contains(*c))
            .take(32)
            .collect();
        if !t.is_empty() && !out.contains(&t) && out.len() < 20 {
            out.push(t);
        }
    }
    out
}

impl Store {
    /// Open (or create) the database. A database that fails its integrity check is renamed to
    /// `clipvault.db.corrupt-<timestamp>` and a fresh one is created; `recovered` explains this.
    pub fn open(path: &Path) -> AppResult<Store> {
        match Self::try_open(path) {
            Ok(conn) => Ok(Store {
                conn: Mutex::new(conn),
                recovered: None,
            }),
            Err(e) => {
                log::error!("database unusable, moving aside: {e}");
                let suffix = format!("corrupt-{}", now_ms());
                let backup = path.with_extension(format!("db.{suffix}"));
                std::fs::rename(path, &backup)?;
                for ext in ["db-wal", "db-shm"] {
                    let _ = std::fs::remove_file(path.with_extension(ext));
                }
                let conn = Self::try_open(path)?;
                Ok(Store {
                    conn: Mutex::new(conn),
                    recovered: Some(format!(
                        "The history database was damaged and has been reset. The old file was kept at {}",
                        backup.display()
                    )),
                })
            }
        }
    }

    fn try_open(path: &Path) -> AppResult<Connection> {
        let mut conn = Connection::open(path)?;
        Self::init(&mut conn)?;
        let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(AppError::Other("integrity check failed".into()));
        }
        Ok(conn)
    }

    pub fn open_in_memory() -> AppResult<Store> {
        let mut conn = Connection::open_in_memory()?;
        Self::init(&mut conn)?;
        Ok(Store {
            conn: Mutex::new(conn),
            recovered: None,
        })
    }

    fn init(conn: &mut Connection) -> AppResult<()> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrations::run(conn)?;
        Ok(())
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock cannot leave SQLite in a torn state, so recover.
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Returns the id of an existing item that a new copy with `hash` should merge into.
    pub fn find_dedupe_target(&self, hash: &str, mode: DedupeMode) -> AppResult<Option<i64>> {
        let conn = self.conn();
        Ok(match mode {
            DedupeMode::KeepAll => None,
            DedupeMode::MoveToTop => conn
                .query_row(
                    "SELECT id FROM items WHERE content_hash = ?1 ORDER BY copied_at DESC LIMIT 1",
                    [hash],
                    |r| r.get(0),
                )
                .optional()?,
            DedupeMode::Consecutive => conn
                .query_row(
                    "SELECT id, content_hash FROM items ORDER BY copied_at DESC, id DESC LIMIT 1",
                    [],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()?
                .filter(|(_, h)| h == hash)
                .map(|(id, _)| id),
        })
    }

    /// Record another copy of existing content: move it to the top.
    pub fn bump(&self, id: i64, now: i64) -> AppResult<()> {
        self.conn().execute(
            "UPDATE items SET copied_at = ?1, copy_count = copy_count + 1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    pub fn insert(&self, item: &NewItem, now: i64) -> AppResult<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO items (item_type, content_text, rich_html, preview_text, content_hash, mime_type,
                created_at, copied_at, source_application, media_path, thumbnail_path, byte_size,
                width, height, metadata_json, search_extra)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                item.item_type.as_str(),
                item.content_text,
                item.rich_html,
                item.preview_text,
                item.content_hash,
                item.mime_type,
                now,
                item.source_application,
                item.media_path,
                item.thumbnail_path,
                item.byte_size,
                item.width,
                item.height,
                item.metadata.to_string(),
                item.search_extra,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list(&self, q: &ListQuery) -> AppResult<Page> {
        let mut clauses: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();

        match q.filter {
            Filter::All => {}
            Filter::Pinned => clauses.push("is_pinned = 1".into()),
            Filter::Favorites => clauses.push("is_favorite = 1".into()),
            Filter::Text => clauses.push("item_type = 'text'".into()),
            Filter::Code => clauses.push("item_type = 'code'".into()),
            Filter::Image => clauses.push("item_type = 'image'".into()),
            Filter::Files => clauses.push("item_type = 'files'".into()),
        }

        if let Some(search) = q.search.as_deref() {
            let search: String = search.trim().chars().take(256).collect();
            for token in search.split_whitespace().take(8) {
                if token.chars().count() >= 3 {
                    clauses
                        .push("id IN (SELECT rowid FROM items_fts WHERE items_fts MATCH ?)".into());
                    args.push(Box::new(fts_phrase(token)));
                } else {
                    clauses.push(
                        "(content_text LIKE ? ESCAPE '\\' OR search_extra LIKE ? ESCAPE '\\' OR tags_text LIKE ? ESCAPE '\\')"
                            .into(),
                    );
                    let pat = format!("%{}%", like_escape(token));
                    args.push(Box::new(pat.clone()));
                    args.push(Box::new(pat.clone()));
                    args.push(Box::new(pat));
                }
            }
        }

        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        let order = match q.sort {
            Sort::Newest => "copied_at DESC, id DESC",
            Sort::Oldest => "copied_at ASC, id ASC",
            Sort::RecentlyUsed => "COALESCE(last_used_at, copied_at) DESC, id DESC",
        };
        let limit = if q.limit == 0 { 100 } else { q.limit.min(500) };

        let conn = self.conn();
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM items {where_sql}"),
            params_from_iter(args.iter()),
            |r| r.get(0),
        )?;
        let sql = format!(
            "SELECT {SUMMARY_COLS} FROM items {where_sql} ORDER BY {order} LIMIT {limit} OFFSET {}",
            q.offset
        );
        let mut stmt = conn.prepare(&sql)?;
        let items = stmt
            .query_map(params_from_iter(args.iter()), row_to_summary)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Page { items, total })
    }

    pub fn get(&self, id: i64) -> AppResult<ItemDetail> {
        let conn = self.conn();
        let sql = format!(
            "SELECT {SUMMARY_COLS}, content_text, source_application, metadata_json, rich_html FROM items WHERE id = ?1"
        );
        conn.query_row(&sql, [id], |r| {
            let summary = row_to_summary(r)?;
            let text: Option<String> = r.get(SUMMARY_N)?;
            let source: Option<String> = r.get(SUMMARY_N + 1)?;
            let meta_raw: String = r.get(SUMMARY_N + 2)?;
            let html: Option<String> = r.get(SUMMARY_N + 3)?;
            Ok((summary, text, source, meta_raw, html))
        })
        .optional()?
        .map(|(summary, text, source_application, meta_raw, rich_html)| {
            let metadata: serde_json::Value = serde_json::from_str(&meta_raw).unwrap_or_default();
            let file_paths = if summary.item_type == ItemType::Files {
                text.as_deref()
                    .map(|t| t.lines().map(str::to_owned).collect())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let (content_text, text_truncated) = match text {
                Some(t) if t.len() > MAX_DETAIL_TEXT => {
                    let mut cut = MAX_DETAIL_TEXT;
                    while !t.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    (Some(t[..cut].to_owned()), true)
                }
                other => (other, false),
            };
            ItemDetail {
                summary,
                content_text,
                text_truncated,
                file_paths,
                source_application,
                metadata,
                rich_html,
            }
        })
        .ok_or(AppError::NotFound)
    }

    pub fn get_raw(&self, id: i64) -> AppResult<RawItem> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, item_type, content_text, rich_html, mime_type, media_path, metadata_json FROM items WHERE id = ?1",
            [id],
            |r| {
                let t: String = r.get(1)?;
                let meta: String = r.get(6)?;
                Ok(RawItem {
                    id: r.get(0)?,
                    item_type: ItemType::parse(&t).unwrap_or(ItemType::Text),
                    content_text: r.get(2)?,
                    rich_html: r.get(3)?,
                    mime_type: r.get(4)?,
                    media_path: r.get(5)?,
                    metadata: serde_json::from_str(&meta).unwrap_or_default(),
                })
            },
        )
        .optional()?
        .ok_or(AppError::NotFound)
    }

    /// Paths (relative to the data dir) of an item's original media and thumbnail.
    pub fn media_paths(&self, id: i64) -> AppResult<(Option<String>, Option<String>, String)> {
        self.conn()
            .query_row(
                "SELECT media_path, thumbnail_path, mime_type FROM items WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or(AppError::NotFound)
    }

    pub fn set_pinned(&self, id: i64, pinned: bool) -> AppResult<()> {
        self.update_flag("is_pinned", id, pinned)
    }

    pub fn set_favorite(&self, id: i64, fav: bool) -> AppResult<()> {
        self.update_flag("is_favorite", id, fav)
    }

    fn update_flag(&self, col: &'static str, id: i64, v: bool) -> AppResult<()> {
        let n = self.conn().execute(
            &format!("UPDATE items SET {col} = ?1 WHERE id = ?2"),
            params![v, id],
        )?;
        if n == 0 {
            return Err(AppError::NotFound);
        }
        Ok(())
    }

    pub fn mark_used(&self, id: i64, now: i64) -> AppResult<()> {
        self.conn().execute(
            "UPDATE items SET last_used_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    /// Delete one item. Returns media paths that are no longer referenced by any row.
    pub fn delete(&self, id: i64) -> AppResult<Vec<String>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let paths = delete_where(&tx, "id = ?", &[&id])?;
        if paths.is_none() {
            return Err(AppError::NotFound);
        }
        let orphans = unreferenced(&tx, paths.unwrap_or_default())?;
        tx.commit()?;
        Ok(orphans)
    }

    /// Clear history. Pinned items are kept unless `include_pinned`.
    pub fn clear(&self, include_pinned: bool) -> AppResult<Vec<String>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let cond = if include_pinned {
            "1 = 1"
        } else {
            "is_pinned = 0"
        };
        let paths = delete_where(&tx, cond, &[])?.unwrap_or_default();
        let orphans = unreferenced(&tx, paths)?;
        tx.commit()?;
        Ok(orphans)
    }

    /// Apply retention, item-count and media-size limits. Pinned items are never pruned.
    /// Returns (rows deleted, orphaned media paths).
    pub fn prune(&self, s: &Settings, now: i64) -> AppResult<(usize, Vec<String>)> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let before: i64 = tx.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))?;
        let mut paths = Vec::new();

        if s.retention_days > 0 {
            let cutoff = now - s.retention_days as i64 * 86_400_000;
            paths.extend(
                delete_where(&tx, "is_pinned = 0 AND copied_at < ?", &[&cutoff])?
                    .unwrap_or_default(),
            );
        }

        let keep = s.max_items as i64;
        paths.extend(
            delete_where(
                &tx,
                "id IN (SELECT id FROM items WHERE is_pinned = 0 ORDER BY copied_at DESC, id DESC LIMIT -1 OFFSET ?)",
                &[&keep],
            )?
            .unwrap_or_default(),
        );

        // Media cap: keep the newest unpinned images whose cumulative size fits.
        let cap = s.max_media_mb as i64 * 1024 * 1024;
        let over: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM (SELECT id, SUM(byte_size) OVER (ORDER BY copied_at DESC, id DESC) AS running
                   FROM items WHERE is_pinned = 0 AND media_path IS NOT NULL) WHERE running > ?1",
            )?;
            let rows = stmt
                .query_map([cap], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        for id in over {
            paths.extend(delete_where(&tx, "id = ?", &[&id])?.unwrap_or_default());
        }

        let after: i64 = tx.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))?;
        let orphans = unreferenced(&tx, paths)?;
        tx.commit()?;
        Ok(((before - after) as usize, orphans))
    }

    pub fn referenced_media(&self) -> AppResult<HashSet<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT media_path FROM items WHERE media_path IS NOT NULL
             UNION SELECT thumbnail_path FROM items WHERE thumbnail_path IS NOT NULL",
        )?;
        let set = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<HashSet<_>, _>>()?;
        Ok(set)
    }

    /// Replace an item's tags. Tags are also indexed for search as `#tag`.
    pub fn set_tags(&self, id: i64, tags: &[String]) -> AppResult<Vec<String>> {
        let tags = normalize_tags(tags);
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM items WHERE id = ?1)",
            [id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(AppError::NotFound);
        }
        tx.execute("DELETE FROM item_tags WHERE item_id = ?1", [id])?;
        for t in &tags {
            tx.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [t])?;
            tx.execute(
                "INSERT OR IGNORE INTO item_tags (item_id, tag_id) SELECT ?1, id FROM tags WHERE name = ?2",
                params![id, t],
            )?;
        }
        let text = tags
            .iter()
            .map(|t| format!("#{t}"))
            .collect::<Vec<_>>()
            .join(" ");
        tx.execute(
            "UPDATE items SET tags_text = ?1 WHERE id = ?2",
            params![text, id],
        )?;
        tx.execute(
            "DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM item_tags)",
            [],
        )?;
        tx.commit()?;
        Ok(tags)
    }

    /// All tags with usage counts, most used first.
    pub fn all_tags(&self) -> AppResult<Vec<(String, i64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT t.name, COUNT(*) FROM tags t JOIN item_tags it ON it.tag_id = t.id GROUP BY t.id ORDER BY 2 DESC, 1",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Attach OCR text to an image so it becomes searchable.
    pub fn set_ocr_text(&self, id: i64, text: &str) -> AppResult<()> {
        self.conn().execute(
            "UPDATE items SET search_extra = search_extra || ' ' || ?1,
                    metadata_json = json_set(metadata_json, '$.ocr', json('true'))
             WHERE id = ?2",
            params![text, id],
        )?;
        Ok(())
    }

    pub fn hash_exists(&self, hash: &str) -> AppResult<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM items WHERE content_hash = ?1)",
            [hash],
            |r| r.get(0),
        )?)
    }

    /// Insert a row restored from a backup, keeping its timestamps and flags.
    pub fn import(&self, item: &NewItem, meta: &ImportMeta) -> AppResult<i64> {
        let id = self.insert(item, meta.copied_at)?;
        self.conn().execute(
            "UPDATE items SET created_at = ?1, last_used_at = ?2, copy_count = ?3, is_pinned = ?4, is_favorite = ?5
             WHERE id = ?6",
            params![meta.created_at, meta.last_used_at, meta.copy_count.max(1), meta.is_pinned, meta.is_favorite, id],
        )?;
        if !meta.tags.is_empty() {
            self.set_tags(id, &meta.tags)?;
        }
        Ok(id)
    }

    /// Every row, oldest first, for backups.
    pub fn export_rows(&self) -> AppResult<Vec<ExportRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT item_type, content_text, rich_html, preview_text, mime_type, created_at, copied_at, last_used_at,
                    copy_count, is_pinned, is_favorite, media_path, width, height, metadata_json, search_extra, tags_text,
                    byte_size
             FROM items ORDER BY copied_at ASC, id ASC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let meta: String = r.get(14)?;
                Ok(ExportRow {
                    item_type: r.get(0)?,
                    content_text: r.get(1)?,
                    rich_html: r.get(2)?,
                    preview_text: r.get(3)?,
                    mime_type: r.get(4)?,
                    created_at: r.get(5)?,
                    copied_at: r.get(6)?,
                    last_used_at: r.get(7)?,
                    copy_count: r.get(8)?,
                    is_pinned: r.get(9)?,
                    is_favorite: r.get(10)?,
                    media_path: r.get(11)?,
                    width: r.get(12)?,
                    height: r.get(13)?,
                    metadata: serde_json::from_str(&meta).unwrap_or_default(),
                    search_extra: r.get(15)?,
                    tags: split_tags(&r.get::<_, String>(16)?),
                    byte_size: r.get(17)?,
                    image_base64: None,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn counts(&self) -> AppResult<Counts> {
        Ok(self.conn().query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(is_pinned), 0), COALESCE(SUM(is_favorite), 0),
                    COALESCE(SUM(item_type = 'text'), 0), COALESCE(SUM(item_type = 'code'), 0),
                    COALESCE(SUM(item_type = 'image'), 0), COALESCE(SUM(item_type = 'files'), 0)
             FROM items",
            [],
            |r| {
                Ok(Counts {
                    all: r.get(0)?,
                    pinned: r.get(1)?,
                    favorites: r.get(2)?,
                    text: r.get(3)?,
                    code: r.get(4)?,
                    image: r.get(5)?,
                    files: r.get(6)?,
                })
            },
        )?)
    }

    pub fn stats(&self) -> AppResult<Stats> {
        Ok(self.conn().query_row(
            "SELECT COUNT(*), COALESCE(SUM(CASE WHEN media_path IS NOT NULL THEN byte_size END), 0) FROM items",
            [],
            |r| Ok(Stats { items: r.get(0)?, media_bytes: r.get(1)? }),
        )?)
    }
}

/// Delete rows matching `cond`, returning their media paths, or `None` if nothing matched.
fn delete_where(
    tx: &Transaction,
    cond: &str,
    args: &[&dyn ToSql],
) -> AppResult<Option<Vec<String>>> {
    let mut paths = Vec::new();
    {
        let mut stmt = tx.prepare(&format!(
            "SELECT media_path, thumbnail_path FROM items WHERE {cond}"
        ))?;
        let mut rows = stmt.query(args)?;
        while let Some(r) = rows.next()? {
            paths.extend(r.get::<_, Option<String>>(0)?);
            paths.extend(r.get::<_, Option<String>>(1)?);
        }
    }
    let n = tx.execute(&format!("DELETE FROM items WHERE {cond}"), args)?;
    Ok(if n == 0 { None } else { Some(paths) })
}

/// Of `paths`, keep those no longer referenced by any remaining row.
fn unreferenced(tx: &Transaction, mut paths: Vec<String>) -> AppResult<Vec<String>> {
    paths.sort();
    paths.dedup();
    let mut stmt = tx.prepare(
        "SELECT EXISTS(SELECT 1 FROM items WHERE media_path = ?1 OR thumbnail_path = ?1)",
    )?;
    let mut out = Vec::new();
    for p in paths {
        let used: bool = stmt.query_row([&p], |r| r.get(0))?;
        if !used {
            out.push(p);
        }
    }
    Ok(out)
}

/// Quote a user token as an FTS5 phrase so operators/punctuation are matched literally.
fn fts_phrase(token: &str) -> String {
    format!("\"{}\"", token.replace('"', "\"\""))
}

fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text_item(text: &str) -> NewItem {
        NewItem {
            item_type: ItemType::Text,
            content_text: Some(text.into()),
            rich_html: None,
            preview_text: text.chars().take(100).collect(),
            content_hash: format!("h-{text}"),
            mime_type: "text/plain".into(),
            source_application: None,
            media_path: None,
            thumbnail_path: None,
            byte_size: text.len() as i64,
            width: None,
            height: None,
            metadata: json!({}),
            search_extra: String::new(),
        }
    }

    fn image_item(hash: &str, bytes: i64) -> NewItem {
        NewItem {
            item_type: ItemType::Image,
            content_text: None,
            preview_text: String::new(),
            content_hash: hash.into(),
            mime_type: "image/png".into(),
            media_path: Some(format!("media/{hash}.png")),
            thumbnail_path: Some(format!("thumbs/{hash}.png")),
            byte_size: bytes,
            width: Some(10),
            height: Some(10),
            ..text_item("x")
        }
    }

    fn q(search: &str) -> ListQuery {
        ListQuery {
            search: Some(search.into()),
            ..Default::default()
        }
    }

    #[test]
    fn insert_list_and_search() {
        let s = Store::open_in_memory().unwrap();
        s.insert(&text_item("hello world"), 1).unwrap();
        s.insert(&text_item("cargo build --release"), 2).unwrap();
        s.insert(&text_item("Ünïcödé snowman ☃"), 3).unwrap();

        let all = s.list(&ListQuery::default()).unwrap();
        assert_eq!(all.total, 3);
        assert_eq!(all.items[0].preview, "Ünïcödé snowman ☃", "newest first");

        assert_eq!(s.list(&q("ello")).unwrap().total, 1, "trigram substring");
        assert_eq!(s.list(&q("WORLD")).unwrap().total, 1, "case-insensitive");
        assert_eq!(
            s.list(&q("--release")).unwrap().total,
            1,
            "punctuation literal"
        );
        assert_eq!(
            s.list(&q("he")).unwrap().total,
            1,
            "short token LIKE fallback"
        );
        assert_eq!(
            s.list(&q("cargo release")).unwrap().total,
            1,
            "multi-token AND"
        );
        assert_eq!(s.list(&q("snowman ☃")).unwrap().total, 1);
        assert_eq!(s.list(&q("nothing-here")).unwrap().total, 0);
        assert_eq!(
            s.list(&q("100%")).unwrap().total,
            0,
            "LIKE wildcards escaped"
        );
    }

    #[test]
    fn sorting_and_paging() {
        let s = Store::open_in_memory().unwrap();
        for i in 0..250 {
            s.insert(&text_item(&format!("item {i}")), i).unwrap();
        }
        let p = s
            .list(&ListQuery {
                limit: 100,
                offset: 200,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(p.total, 250);
        assert_eq!(p.items.len(), 50);
        let oldest = s
            .list(&ListQuery {
                sort: Sort::Oldest,
                limit: 1,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(oldest.items[0].preview, "item 0");
        let id = s.list(&q("item 7")).unwrap().items.last().unwrap().id;
        s.mark_used(id, 10_000).unwrap();
        let used = s
            .list(&ListQuery {
                sort: Sort::RecentlyUsed,
                limit: 1,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(used.items[0].id, id);
    }

    #[test]
    fn dedupe_modes() {
        let s = Store::open_in_memory().unwrap();
        let a = s.insert(&text_item("a"), 1).unwrap();
        s.insert(&text_item("b"), 2).unwrap();
        assert_eq!(
            s.find_dedupe_target("h-a", DedupeMode::MoveToTop).unwrap(),
            Some(a)
        );
        assert_eq!(
            s.find_dedupe_target("h-a", DedupeMode::Consecutive)
                .unwrap(),
            None
        );
        assert!(s
            .find_dedupe_target("h-b", DedupeMode::Consecutive)
            .unwrap()
            .is_some());
        assert_eq!(
            s.find_dedupe_target("h-b", DedupeMode::KeepAll).unwrap(),
            None
        );
        s.bump(a, 5).unwrap();
        let top = &s.list(&ListQuery::default()).unwrap().items[0];
        assert_eq!((top.id, top.copy_count), (a, 2));
    }

    #[test]
    fn pin_survives_prune_and_clear() {
        let s = Store::open_in_memory().unwrap();
        let old_pinned = s.insert(&text_item("pinned"), 0).unwrap();
        s.set_pinned(old_pinned, true).unwrap();
        s.insert(&text_item("old"), 0).unwrap();
        let now = 40 * 86_400_000;
        s.insert(&text_item("new"), now).unwrap();
        let settings = Settings {
            retention_days: 30,
            ..Settings::default()
        };
        let (deleted, _) = s.prune(&settings, now).unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(s.counts().unwrap().all, 2);

        s.clear(false).unwrap();
        let left = s.list(&ListQuery::default()).unwrap();
        assert_eq!(left.total, 1);
        assert!(left.items[0].is_pinned);
        s.clear(true).unwrap();
        assert_eq!(s.counts().unwrap().all, 0);
    }

    #[test]
    fn prune_max_items_and_media_cap() {
        let s = Store::open_in_memory().unwrap();
        for i in 0..20 {
            s.insert(&text_item(&format!("t{i}")), i).unwrap();
        }
        let settings = Settings {
            max_items: 10,
            retention_days: 0,
            ..Settings::default()
        };
        s.prune(&settings, 100).unwrap();
        assert_eq!(s.counts().unwrap().all, 10);

        s.clear(true).unwrap();
        let mb = 1024 * 1024;
        s.insert(&image_item("i1", 6 * mb), 1).unwrap();
        s.insert(&image_item("i2", 6 * mb), 2).unwrap();
        let pinned = s.insert(&image_item("i0", 50 * mb), 0).unwrap();
        s.set_pinned(pinned, true).unwrap();
        let settings = Settings {
            max_media_mb: 10,
            retention_days: 0,
            ..Settings::default()
        };
        let (deleted, orphans) = s.prune(&settings, 3).unwrap();
        assert_eq!(deleted, 1, "oldest unpinned image over the cap goes");
        assert_eq!(
            orphans,
            vec!["media/i1.png".to_string(), "thumbs/i1.png".to_string()]
        );
        assert_eq!(s.counts().unwrap().image, 2);
    }

    #[test]
    fn delete_reports_only_unreferenced_media() {
        let s = Store::open_in_memory().unwrap();
        let a = s.insert(&image_item("same", 10), 1).unwrap();
        let b = s.insert(&image_item("same", 10), 2).unwrap();
        assert!(s.delete(a).unwrap().is_empty(), "still referenced by b");
        assert_eq!(s.delete(b).unwrap().len(), 2);
        assert!(matches!(s.delete(b), Err(AppError::NotFound)));
    }

    #[test]
    fn corrupt_database_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipvault.db");
        std::fs::write(
            &path,
            b"this is not a sqlite database at all, just junk bytes......",
        )
        .unwrap();
        let s = Store::open(&path).unwrap();
        assert!(s.recovered.is_some());
        s.insert(&text_item("works"), 1).unwrap();
        let backups = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("corrupt")
            })
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn reopen_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipvault.db");
        {
            let s = Store::open(&path).unwrap();
            let id = s.insert(&text_item("persist me"), 1).unwrap();
            s.set_pinned(id, true).unwrap();
        }
        let s = Store::open(&path).unwrap();
        let p = s
            .list(&ListQuery {
                filter: Filter::Pinned,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(p.total, 1);
        assert_eq!(
            s.get(p.items[0].id).unwrap().content_text.as_deref(),
            Some("persist me")
        );
    }
}
