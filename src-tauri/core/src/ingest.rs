//! Turns a raw clipboard snapshot into a history row: classification, filtering,
//! deduplication and media storage. Never logs clipboard content.

use std::sync::{Arc, Mutex};

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::classify;
use crate::error::AppResult;
use crate::media::MediaStore;
use crate::model::{ItemType, NewItem};
use crate::settings::{DedupeMode, Settings};
use crate::store::Store;

/// One clipboard change as read from the backend: every fetched (mime, bytes) pair.
#[derive(Debug, Clone, Default)]
pub struct Captured {
    pub formats: Vec<(String, Vec<u8>)>,
    /// The source flagged the content as secret (e.g. `x-kde-passwordManagerHint`).
    pub password_hint: bool,
    pub source_application: Option<String>,
    /// Snapshot taken when monitoring started, not a fresh copy by the user.
    pub initial: bool,
}

impl Captured {
    pub fn get(&self, mime: &str) -> Option<&[u8]> {
        self.formats
            .iter()
            .find(|(m, _)| m.eq_ignore_ascii_case(mime))
            .map(|(_, b)| b.as_slice())
    }
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Inserted(i64),
    Bumped(i64),
    /// Same content again: a startup snapshot or a repeated owner-change notification.
    Duplicate,
    Skipped(String),
}

/// Identical content within this window is treated as one copy.
const ECHO_WINDOW_MS: i64 = 1500;

pub const IMAGE_MIMES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "image/bmp",
    "image/tiff",
];
pub const TEXT_TARGETS: &[&str] = &[
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
    "TEXT",
];
pub const FILE_TARGETS: &[&str] = &["x-special/gnome-copied-files", "text/uri-list"];
pub const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const PREVIEW_CHARS: usize = 500;

pub struct Ingestor {
    store: Arc<Store>,
    media: MediaStore,
    recent: Mutex<Option<(String, i64)>>,
}

/// Content hash used for deduplication (also used when importing backups).
pub fn sha256_hex(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Decode clipboard text: UTF-8 (lossy), UTF-16 with BOM, Latin-1 for X `STRING`.
pub fn decode_text(mime: &str, bytes: &[u8]) -> String {
    let s = if bytes.len() >= 2 && (bytes[..2] == [0xFF, 0xFE] || bytes[..2] == [0xFE, 0xFF]) {
        let le = bytes[0] == 0xFF;
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    } else if mime == "STRING" && std::str::from_utf8(bytes).is_err() {
        bytes.iter().map(|&b| b as char).collect()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    s.trim_end_matches('\0').to_owned()
}

/// Very small tag stripper for the html-only fallback (no rendering happens here).
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

fn preview(text: &str) -> String {
    text.chars().take(PREVIEW_CHARS).collect()
}

impl Ingestor {
    pub fn new(store: Arc<Store>, media: MediaStore) -> Self {
        Self {
            store,
            media,
            recent: Mutex::new(None),
        }
    }

    /// Decide whether `hash` should merge into existing history instead of being inserted.
    fn dedupe(
        &self,
        hash: &str,
        clip: &Captured,
        s: &Settings,
        now: i64,
    ) -> AppResult<Option<Outcome>> {
        {
            let mut recent = self.recent.lock().unwrap_or_else(|p| p.into_inner());
            let echo = matches!(&*recent, Some((h, t)) if h == hash && now - t < ECHO_WINDOW_MS);
            *recent = Some((hash.to_owned(), now));
            if echo {
                return Ok(Some(Outcome::Duplicate));
            }
        }
        if clip.initial
            && self
                .store
                .find_dedupe_target(hash, DedupeMode::MoveToTop)?
                .is_some()
        {
            return Ok(Some(Outcome::Duplicate));
        }
        if let Some(id) = self.store.find_dedupe_target(hash, s.dedupe)? {
            self.store.bump(id, now)?;
            return Ok(Some(Outcome::Bumped(id)));
        }
        Ok(None)
    }

    pub fn ingest(&self, clip: &Captured, s: &Settings, now: i64) -> AppResult<Outcome> {
        let skip = |r: &str| Ok(Outcome::Skipped(r.to_owned()));
        if clip.password_hint {
            return skip("content marked secret by its source (password manager hint)");
        }
        if let Some(src) = &clip.source_application {
            if s.excluded_apps.iter().any(|a| a.eq_ignore_ascii_case(src)) {
                return skip("source application is excluded");
            }
        }
        let max = s.max_item_bytes();

        // Images take priority: browsers offer image/png alongside text/html for copied images.
        if s.capture_images {
            if let Some((mime, bytes)) = IMAGE_MIMES
                .iter()
                .find_map(|m| clip.get(m).filter(|b| !b.is_empty()).map(|b| (*m, b)))
            {
                if bytes.len() > max {
                    return skip("image exceeds the maximum item size");
                }
                let hash = sha256_hex(&[b"image\0", bytes]);
                if let Some(o) = self.dedupe(&hash, clip, s, now)? {
                    return Ok(o);
                }
                let saved = match self.media.save_image(&hash, mime, bytes, s.thumbnail_size) {
                    Ok(v) => v,
                    Err(e) => {
                        log::warn!(
                            "could not decode copied image ({mime}, {} bytes): {e}",
                            bytes.len()
                        );
                        return skip("image data could not be decoded");
                    }
                };
                let item = NewItem {
                    item_type: ItemType::Image,
                    content_text: None,
                    rich_html: None,
                    preview_text: format!("Image {}×{}", saved.width, saved.height),
                    content_hash: hash,
                    mime_type: mime.to_owned(),
                    source_application: clip.source_application.clone(),
                    media_path: Some(saved.media_rel),
                    thumbnail_path: Some(saved.thumb_rel),
                    byte_size: bytes.len() as i64,
                    width: Some(saved.width as i64),
                    height: Some(saved.height as i64),
                    metadata: json!({}),
                    search_extra: format!("image {}x{} {}", saved.width, saved.height, mime),
                };
                return Ok(Outcome::Inserted(self.store.insert(&item, now)?));
            }
        }

        if s.capture_files {
            if let Some(raw) = FILE_TARGETS.iter().find_map(|m| clip.get(m)) {
                let (op, uris) = classify::parse_uri_list(&decode_text("text/uri-list", raw));
                if !uris.is_empty() {
                    let paths: Vec<String> =
                        uris.iter().map(|u| classify::uri_to_path(u)).collect();
                    let names: Vec<&str> = paths.iter().map(|p| classify::file_name(p)).collect();
                    let text = paths.join("\n");
                    let hash = sha256_hex(&[b"files\0", uris.join("\n").as_bytes()]);
                    if let Some(o) = self.dedupe(&hash, clip, s, now)? {
                        return Ok(o);
                    }
                    let item = NewItem {
                        item_type: ItemType::Files,
                        preview_text: preview(&names.join(", ")),
                        content_hash: hash,
                        mime_type: "text/uri-list".into(),
                        source_application: clip.source_application.clone(),
                        media_path: None,
                        thumbnail_path: None,
                        byte_size: text.len() as i64,
                        width: None,
                        height: None,
                        metadata: json!({ "uris": uris, "operation": op }),
                        search_extra: names.join(" "),
                        content_text: Some(text),
                        rich_html: None,
                    };
                    return Ok(Outcome::Inserted(self.store.insert(&item, now)?));
                }
            }
        }

        let html = clip
            .get("text/html")
            .filter(|b| b.len() <= MAX_HTML_BYTES)
            .map(|b| decode_text("text/html", b))
            .filter(|h| !h.trim().is_empty());
        let (text, mime) = match TEXT_TARGETS
            .iter()
            .find_map(|m| clip.get(m).map(|b| (decode_text(m, b), *m)))
        {
            Some(v) => v,
            None => match &html {
                Some(h) => (strip_tags(h), "text/html"),
                None => return skip("no supported clipboard format"),
            },
        };
        if text.trim().is_empty() {
            return skip("empty text");
        }
        if text.len() > max {
            return skip("text exceeds the maximum item size");
        }
        if s.exclude_sensitive {
            if let Some(reason) = crate::sensitive::detect(&text) {
                return Ok(Outcome::Skipped(format!("likely sensitive: {reason}")));
            }
        }
        let hash = sha256_hex(&[b"text\0", text.as_bytes()]);
        if let Some(o) = self.dedupe(&hash, clip, s, now)? {
            return Ok(o);
        }
        let is_code = classify::looks_like_code(&text);
        let language = if is_code {
            classify::detect_language(&text)
        } else {
            None
        };
        let item = NewItem {
            item_type: if is_code {
                ItemType::Code
            } else {
                ItemType::Text
            },
            preview_text: preview(&text),
            content_hash: hash,
            mime_type: if mime == "UTF8_STRING" || mime == "STRING" || mime == "TEXT" {
                "text/plain".into()
            } else {
                mime.to_owned()
            },
            source_application: clip.source_application.clone(),
            media_path: None,
            thumbnail_path: None,
            byte_size: text.len() as i64,
            width: None,
            height: None,
            metadata: match language {
                Some(l) => {
                    json!({ "lines": text.lines().count(), "chars": text.chars().count(), "language": l })
                }
                None => json!({ "lines": text.lines().count(), "chars": text.chars().count() }),
            },
            search_extra: language.unwrap_or_default().to_string(),
            content_text: Some(text),
            rich_html: html,
        };
        Ok(Outcome::Inserted(self.store.insert(&item, now)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ListQuery;
    use crate::paths::Paths;
    use crate::settings::DedupeMode;

    fn setup() -> (tempfile::TempDir, Ingestor) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().join("d"), dir.path().join("c"));
        paths.ensure().unwrap();
        let ing = Ingestor::new(
            Arc::new(Store::open_in_memory().unwrap()),
            MediaStore::new(paths),
        );
        (dir, ing)
    }

    fn clip(pairs: &[(&str, &[u8])]) -> Captured {
        Captured {
            formats: pairs
                .iter()
                .map(|(m, b)| (m.to_string(), b.to_vec()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn text_dedupe_and_code() {
        let (_d, ing) = setup();
        let s = Settings::default();
        let a = ing
            .ingest(&clip(&[("UTF8_STRING", b"hello there")]), &s, 1)
            .unwrap();
        let Outcome::Inserted(id) = a else {
            panic!("{a:?}")
        };
        assert_eq!(
            ing.ingest(
                &clip(&[("text/plain;charset=utf-8", b"hello there")]),
                &s,
                2
            )
            .unwrap(),
            Outcome::Duplicate,
            "echo"
        );
        assert_eq!(
            ing.ingest(
                &clip(&[("text/plain;charset=utf-8", b"hello there")]),
                &s,
                5_000
            )
            .unwrap(),
            Outcome::Bumped(id)
        );
        let c = ing
            .ingest(
                &clip(&[("UTF8_STRING", b"fn main() {\n    let x = 1;\n}")]),
                &s,
                6_000,
            )
            .unwrap();
        let Outcome::Inserted(cid) = c else { panic!() };
        assert_eq!(
            ing.store.get(cid).unwrap().summary.item_type,
            ItemType::Code
        );

        let keep = Settings {
            dedupe: DedupeMode::KeepAll,
            ..Settings::default()
        };
        assert!(matches!(
            ing.ingest(&clip(&[("UTF8_STRING", b"hello there")]), &keep, 9_000)
                .unwrap(),
            Outcome::Inserted(_)
        ));
    }

    #[test]
    fn startup_snapshot_never_bumps_or_duplicates() {
        let (_d, ing) = setup();
        let keep = Settings {
            dedupe: DedupeMode::KeepAll,
            ..Settings::default()
        };
        let Outcome::Inserted(id) = ing
            .ingest(&clip(&[("UTF8_STRING", b"already here")]), &keep, 1)
            .unwrap()
        else {
            panic!()
        };
        let mut snap = clip(&[("UTF8_STRING", b"already here")]);
        snap.initial = true;
        assert_eq!(
            ing.ingest(&snap, &keep, 60_000).unwrap(),
            Outcome::Duplicate
        );
        assert_eq!(ing.store.get(id).unwrap().summary.copy_count, 1);
        assert_eq!(ing.store.counts().unwrap().all, 1);
        let mut fresh = clip(&[("UTF8_STRING", b"new at startup")]);
        fresh.initial = true;
        assert!(matches!(
            ing.ingest(&fresh, &keep, 70_000).unwrap(),
            Outcome::Inserted(_)
        ));
    }

    #[test]
    fn skips_sensitive_hint_empty_and_oversize() {
        let (_d, ing) = setup();
        let s = Settings::default();
        let mut c = clip(&[("UTF8_STRING", b"hunter2")]);
        c.password_hint = true;
        assert!(matches!(
            ing.ingest(&c, &s, 1).unwrap(),
            Outcome::Skipped(_)
        ));
        assert!(matches!(
            ing.ingest(&clip(&[("UTF8_STRING", b"AKIAIOSFODNN7EXAMPLE")]), &s, 1)
                .unwrap(),
            Outcome::Skipped(_)
        ));
        let off = Settings {
            exclude_sensitive: false,
            ..Settings::default()
        };
        assert!(matches!(
            ing.ingest(&clip(&[("UTF8_STRING", b"AKIAIOSFODNN7EXAMPLE")]), &off, 1)
                .unwrap(),
            Outcome::Inserted(_)
        ));
        assert!(matches!(
            ing.ingest(&clip(&[("UTF8_STRING", b"   \n")]), &s, 1)
                .unwrap(),
            Outcome::Skipped(_)
        ));
        let big = vec![b'a'; 2 * 1024 * 1024];
        let small = Settings {
            max_item_mb: 1,
            ..Settings::default()
        };
        assert!(matches!(
            ing.ingest(&clip(&[("UTF8_STRING", &big)]), &small, 1)
                .unwrap(),
            Outcome::Skipped(_)
        ));
        assert_eq!(ing.store.counts().unwrap().all, 1);
    }

    #[test]
    fn images_files_and_html() {
        let (_d, ing) = setup();
        let s = Settings::default();
        let png = crate::media::tests::png_bytes(64, 32);
        let r = ing
            .ingest(
                &clip(&[("text/html", b"<img src=x>"), ("image/png", &png)]),
                &s,
                1,
            )
            .unwrap();
        let Outcome::Inserted(id) = r else {
            panic!("{r:?}")
        };
        let d = ing.store.get(id).unwrap();
        assert_eq!(
            (
                d.summary.item_type,
                d.summary.width,
                d.summary.has_thumbnail
            ),
            (ItemType::Image, Some(64), true)
        );

        let r = ing
            .ingest(
                &clip(&[
                    (
                        "x-special/gnome-copied-files",
                        b"copy\nfile:///home/u/My%20Doc.pdf",
                    ),
                    ("UTF8_STRING", b"/home/u/My Doc.pdf"),
                ]),
                &s,
                2,
            )
            .unwrap();
        let Outcome::Inserted(fid) = r else { panic!() };
        let d = ing.store.get(fid).unwrap();
        assert_eq!(d.file_paths, vec!["/home/u/My Doc.pdf"]);
        assert_eq!(
            ing.store
                .list(&ListQuery {
                    search: Some("Doc.pdf".into()),
                    ..Default::default()
                })
                .unwrap()
                .total,
            1
        );

        let r = ing
            .ingest(&clip(&[("text/html", b"<b>bold</b> &amp; plain")]), &s, 3)
            .unwrap();
        let Outcome::Inserted(hid) = r else { panic!() };
        assert_eq!(
            ing.store.get(hid).unwrap().content_text.as_deref(),
            Some("bold & plain")
        );
        assert!(ing.store.get(hid).unwrap().summary.has_html);
    }

    #[test]
    fn decodes_utf16_and_latin1() {
        assert_eq!(
            decode_text("text/html", &[0xFF, 0xFE, b'h', 0, b'i', 0]),
            "hi"
        );
        assert_eq!(decode_text("STRING", &[0x63, 0x61, 0x66, 0xE9]), "café");
        assert_eq!(decode_text("UTF8_STRING", b"abc\0"), "abc");
    }
}
