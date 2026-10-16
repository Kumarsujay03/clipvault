use rusqlite::Connection;

use crate::error::AppResult;

/// Ordered schema migrations. `PRAGMA user_version` records how many have been applied.
/// Never edit an existing entry; append a new one.
pub const MIGRATIONS: &[&str] = &[
    // 1 — initial schema
    r#"
    CREATE TABLE items (
        id                 INTEGER PRIMARY KEY AUTOINCREMENT,
        item_type          TEXT    NOT NULL CHECK (item_type IN ('text','code','image','files')),
        content_text       TEXT,
        rich_html          TEXT,
        preview_text       TEXT    NOT NULL DEFAULT '',
        content_hash       TEXT    NOT NULL,
        mime_type          TEXT    NOT NULL,
        created_at         INTEGER NOT NULL,
        copied_at          INTEGER NOT NULL,
        last_used_at       INTEGER,
        copy_count         INTEGER NOT NULL DEFAULT 1,
        is_pinned          INTEGER NOT NULL DEFAULT 0,
        is_favorite        INTEGER NOT NULL DEFAULT 0,
        source_application TEXT,
        media_path         TEXT,
        thumbnail_path     TEXT,
        byte_size          INTEGER NOT NULL DEFAULT 0,
        width              INTEGER,
        height             INTEGER,
        metadata_json      TEXT    NOT NULL DEFAULT '{}',
        search_extra       TEXT    NOT NULL DEFAULT ''
    );
    CREATE INDEX idx_items_copied ON items(copied_at DESC, id DESC);
    CREATE INDEX idx_items_hash   ON items(content_hash);
    CREATE INDEX idx_items_type   ON items(item_type, copied_at DESC);
    CREATE INDEX idx_items_pinned ON items(is_pinned, copied_at DESC);
    CREATE INDEX idx_items_used   ON items(last_used_at DESC);

    -- Trigram FTS gives substring search ("ello" finds "hello") for queries of 3+ chars.
    CREATE VIRTUAL TABLE items_fts USING fts5(
        content_text, search_extra,
        content='items', content_rowid='id',
        tokenize='trigram'
    );
    CREATE TRIGGER items_ai AFTER INSERT ON items BEGIN
        INSERT INTO items_fts(rowid, content_text, search_extra)
        VALUES (new.id, new.content_text, new.search_extra);
    END;
    CREATE TRIGGER items_ad AFTER DELETE ON items BEGIN
        INSERT INTO items_fts(items_fts, rowid, content_text, search_extra)
        VALUES ('delete', old.id, old.content_text, old.search_extra);
    END;
    CREATE TRIGGER items_au AFTER UPDATE OF content_text, search_extra ON items BEGIN
        INSERT INTO items_fts(items_fts, rowid, content_text, search_extra)
        VALUES ('delete', old.id, old.content_text, old.search_extra);
        INSERT INTO items_fts(rowid, content_text, search_extra)
        VALUES (new.id, new.content_text, new.search_extra);
    END;

    CREATE TABLE tags (
        id   INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL UNIQUE COLLATE NOCASE
    );
    CREATE TABLE item_tags (
        item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
        tag_id  INTEGER NOT NULL REFERENCES tags(id)  ON DELETE CASCADE,
        PRIMARY KEY (item_id, tag_id)
    );
    "#,
    // 2 — tags become searchable; rebuild the FTS index with a third column.
    r#"
    ALTER TABLE items ADD COLUMN tags_text TEXT NOT NULL DEFAULT '';
    DROP TRIGGER items_ai;
    DROP TRIGGER items_ad;
    DROP TRIGGER items_au;
    DROP TABLE items_fts;
    CREATE VIRTUAL TABLE items_fts USING fts5(
        content_text, search_extra, tags_text,
        content='items', content_rowid='id',
        tokenize='trigram'
    );
    INSERT INTO items_fts(items_fts) VALUES ('rebuild');
    CREATE TRIGGER items_ai AFTER INSERT ON items BEGIN
        INSERT INTO items_fts(rowid, content_text, search_extra, tags_text)
        VALUES (new.id, new.content_text, new.search_extra, new.tags_text);
    END;
    CREATE TRIGGER items_ad AFTER DELETE ON items BEGIN
        INSERT INTO items_fts(items_fts, rowid, content_text, search_extra, tags_text)
        VALUES ('delete', old.id, old.content_text, old.search_extra, old.tags_text);
    END;
    CREATE TRIGGER items_au AFTER UPDATE OF content_text, search_extra, tags_text ON items BEGIN
        INSERT INTO items_fts(items_fts, rowid, content_text, search_extra, tags_text)
        VALUES ('delete', old.id, old.content_text, old.search_extra, old.tags_text);
        INSERT INTO items_fts(rowid, content_text, search_extra, tags_text)
        VALUES (new.id, new.content_text, new.search_extra, new.tags_text);
    END;
    "#,
];

pub fn run(conn: &mut Connection) -> AppResult<u32> {
    let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (idx, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = idx as u32 + 1;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
        log::info!("applied database migration {version}");
    }
    Ok(MIGRATIONS.len() as u32)
}
