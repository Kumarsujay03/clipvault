//! Builds the clipboard formats offered when an item is restored.

use crate::classify;
use crate::error::{AppError, AppResult};
use crate::media::{self, MediaStore};
use crate::model::ItemType;
use crate::store::RawItem;

pub fn formats_for(item: &RawItem, media: &MediaStore) -> AppResult<Vec<(String, Vec<u8>)>> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let text = |out: &mut Vec<(String, Vec<u8>)>, t: &str| {
        for m in [
            "text/plain;charset=utf-8",
            "UTF8_STRING",
            "text/plain",
            "TEXT",
        ] {
            out.push((m.into(), t.as_bytes().to_vec()));
        }
        if t.is_ascii() {
            out.push(("STRING".into(), t.as_bytes().to_vec()));
        }
    };
    match item.item_type {
        ItemType::Text | ItemType::Code => {
            let t = item.content_text.as_deref().ok_or(AppError::NotFound)?;
            text(&mut out, t);
            if let Some(html) = &item.rich_html {
                out.push(("text/html".into(), html.as_bytes().to_vec()));
            }
        }
        ItemType::Image => {
            let rel = item.media_path.as_deref().ok_or(AppError::NotFound)?;
            let bytes = media.read(rel)?;
            if item.mime_type != "image/png" {
                // Most Linux apps paste image/png; offer it alongside the original format.
                out.push(("image/png".into(), media::to_png(&bytes)?));
            }
            out.push((item.mime_type.clone(), bytes));
        }
        ItemType::Files => {
            let uris: Vec<String> = match item.metadata.get("uris").and_then(|u| u.as_array()) {
                Some(a) => a
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect(),
                None => item
                    .content_text
                    .as_deref()
                    .unwrap_or("")
                    .lines()
                    .map(classify::path_to_uri)
                    .collect(),
            };
            if uris.is_empty() {
                return Err(AppError::NotFound);
            }
            let paths: Vec<String> = uris.iter().map(|u| classify::uri_to_path(u)).collect();
            out.push((
                "text/uri-list".into(),
                format!("{}\r\n", uris.join("\r\n")).into_bytes(),
            ));
            out.push((
                "x-special/gnome-copied-files".into(),
                format!("copy\n{}", uris.join("\n")).into_bytes(),
            ));
            text(&mut out, &paths.join("\n"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use serde_json::json;

    fn raw(item_type: ItemType) -> RawItem {
        RawItem {
            id: 1,
            item_type,
            content_text: None,
            rich_html: None,
            mime_type: "text/plain".into(),
            media_path: None,
            metadata: json!({}),
        }
    }

    fn get<'a>(f: &'a [(String, Vec<u8>)], m: &str) -> Option<&'a [u8]> {
        f.iter().find(|(k, _)| k == m).map(|(_, v)| v.as_slice())
    }

    fn media() -> (tempfile::TempDir, Paths, MediaStore) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().join("d"), dir.path().join("c"));
        paths.ensure().unwrap();
        (dir, paths.clone(), MediaStore::new(paths))
    }

    #[test]
    fn text_offers_utf8_targets_and_html() {
        let (_d, _p, m) = media();
        let mut r = raw(ItemType::Text);
        r.content_text = Some("héllo".into());
        r.rich_html = Some("<b>héllo</b>".into());
        let f = formats_for(&r, &m).unwrap();
        assert_eq!(get(&f, "UTF8_STRING"), Some("héllo".as_bytes()));
        assert_eq!(get(&f, "text/html"), Some("<b>héllo</b>".as_bytes()));
        assert!(
            get(&f, "STRING").is_none(),
            "non-ASCII text is not offered as Latin-1 STRING"
        );
    }

    #[test]
    fn non_png_images_also_offer_png() {
        let (_d, _p, m) = media();
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3]));
        let mut jpg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut jpg, image::ImageFormat::Jpeg)
            .unwrap();
        let saved = m
            .save_image("aabbcc", "image/jpeg", jpg.get_ref(), 64)
            .unwrap();
        let mut r = raw(ItemType::Image);
        r.mime_type = "image/jpeg".into();
        r.media_path = Some(saved.media_rel);
        let f = formats_for(&r, &m).unwrap();
        assert_eq!(get(&f, "image/jpeg"), Some(jpg.get_ref().as_slice()));
        let png = get(&f, "image/png").expect("png offered");
        assert_eq!(&png[..4], b"\x89PNG");
    }

    #[test]
    fn files_offer_uri_list_and_gnome_format() {
        let (_d, _p, m) = media();
        let mut r = raw(ItemType::Files);
        r.metadata = json!({ "uris": ["file:///home/u/a%20b.txt"] });
        let f = formats_for(&r, &m).unwrap();
        assert_eq!(
            get(&f, "text/uri-list"),
            Some(&b"file:///home/u/a%20b.txt\r\n"[..])
        );
        assert_eq!(
            get(&f, "x-special/gnome-copied-files"),
            Some(&b"copy\nfile:///home/u/a%20b.txt"[..])
        );
        assert_eq!(get(&f, "UTF8_STRING"), Some(&b"/home/u/a b.txt"[..]));
    }

    #[test]
    fn missing_media_is_an_error_not_a_panic() {
        let (_d, _p, m) = media();
        let mut r = raw(ItemType::Image);
        r.media_path = Some("media/zz/missing.png".into());
        assert!(formats_for(&r, &m).is_err());
        r.media_path = Some("../../etc/passwd".into());
        assert!(formats_for(&r, &m).is_err());
    }
}
