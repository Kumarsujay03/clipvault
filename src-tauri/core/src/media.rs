//! Content-addressed media store: original images in `media/`, PNG thumbnails in `thumbs/`.

use std::collections::HashSet;
use std::io::Cursor;
use std::path::Path;

use image::{ImageFormat, ImageReader, Limits};

use crate::error::{AppError, AppResult};
use crate::paths::Paths;

/// Hard limits applied to every decode, independent of user settings.
const MAX_DIMENSION: u32 = 16_384;
const MAX_DECODE_ALLOC: u64 = 768 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SavedImage {
    pub media_rel: String,
    pub thumb_rel: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone)]
pub struct MediaStore {
    paths: Paths,
}

pub fn ext_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" | "image/x-bmp" => "bmp",
        "image/tiff" => "tiff",
        _ => "bin",
    }
}

pub fn mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tiff" => "image/tiff",
        _ => "application/octet-stream",
    }
}

fn limits() -> Limits {
    let mut l = Limits::default();
    l.max_image_width = Some(MAX_DIMENSION);
    l.max_image_height = Some(MAX_DIMENSION);
    l.max_alloc = Some(MAX_DECODE_ALLOC);
    l
}

/// Decode with format sniffing and safety limits.
pub fn decode(bytes: &[u8]) -> AppResult<image::DynamicImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits());
    Ok(reader.decode()?)
}

/// Re-encode arbitrary supported image bytes as PNG (used when restoring non-PNG images).
pub fn to_png(bytes: &[u8]) -> AppResult<Vec<u8>> {
    let img = decode(bytes)?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)?;
    Ok(out.into_inner())
}

impl MediaStore {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    /// Validate, store and thumbnail an image. `hash` names the files.
    pub fn save_image(
        &self,
        hash: &str,
        mime: &str,
        bytes: &[u8],
        thumb_size: u32,
    ) -> AppResult<SavedImage> {
        let img = decode(bytes)?;
        let (width, height) = (img.width(), img.height());
        let shard = &hash[..2];
        let media_rel = format!("media/{shard}/{hash}.{}", ext_for_mime(mime));
        let thumb_rel = format!("thumbs/{shard}/{hash}-{thumb_size}.png");

        let media_abs = self.paths.resolve_media(&media_rel)?;
        if !media_abs.exists() {
            write_atomic(&media_abs, bytes)?;
        }
        let thumb_abs = self.paths.resolve_media(&thumb_rel)?;
        if !thumb_abs.exists() {
            let thumb = if width > thumb_size || height > thumb_size {
                img.thumbnail(thumb_size, thumb_size)
            } else {
                img
            };
            let mut out = Cursor::new(Vec::new());
            thumb.write_to(&mut out, ImageFormat::Png)?;
            write_atomic(&thumb_abs, out.get_ref())?;
        }
        Ok(SavedImage {
            media_rel,
            thumb_rel,
            width,
            height,
        })
    }

    pub fn read(&self, rel: &str) -> AppResult<Vec<u8>> {
        Ok(std::fs::read(self.paths.resolve_media(rel)?)?)
    }

    /// Remove files that the database no longer references. Never touches anything outside
    /// the media/thumbs directories.
    pub fn remove(&self, rels: &[String]) {
        for rel in rels {
            match self.paths.resolve_media(rel) {
                Ok(p) => {
                    if let Err(e) = std::fs::remove_file(&p) {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            log::warn!("could not remove media file: {e}");
                        }
                    }
                }
                Err(_) => log::warn!("refusing to remove media path outside the store"),
            }
        }
    }

    /// Delete orphaned files (and stale temp files). Returns the number removed.
    pub fn cleanup_orphans(&self, referenced: &HashSet<String>) -> AppResult<usize> {
        let mut removed = 0;
        for top in ["media", "thumbs"] {
            let root = self.paths.data_dir.join(top);
            let Ok(shards) = std::fs::read_dir(&root) else {
                continue;
            };
            for shard in shards.flatten() {
                if !shard.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                for f in std::fs::read_dir(shard.path())?.flatten() {
                    if !f.file_type().map(|t| t.is_file()).unwrap_or(false) {
                        continue;
                    }
                    let rel = format!(
                        "{top}/{}/{}",
                        shard.file_name().to_string_lossy(),
                        f.file_name().to_string_lossy()
                    );
                    if !referenced.contains(&rel) && std::fs::remove_file(f.path()).is_ok() {
                        removed += 1;
                    }
                }
            }
        }
        Ok(removed)
    }

    pub fn disk_usage(&self) -> u64 {
        fn walk(p: &Path) -> u64 {
            let Ok(rd) = std::fs::read_dir(p) else {
                return 0;
            };
            rd.flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => walk(&e.path()),
                    Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                    _ => 0,
                })
                .sum()
        }
        walk(&self.paths.media_dir()) + walk(&self.paths.thumbs_dir())
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let dir = path
        .parent()
        .ok_or_else(|| AppError::Other("media path has no parent".into()))?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([x as u8, y as u8, 128, 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn saves_thumbnails_and_cleans_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().join("d"), dir.path().join("c"));
        paths.ensure().unwrap();
        let m = MediaStore::new(paths.clone());
        let bytes = png_bytes(800, 400);
        let saved = m.save_image("abcdef", "image/png", &bytes, 200).unwrap();
        assert_eq!((saved.width, saved.height), (800, 400));
        let thumb = decode(&m.read(&saved.thumb_rel).unwrap()).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (200, 100));

        let mut keep = HashSet::new();
        keep.insert(saved.media_rel.clone());
        assert_eq!(
            m.cleanup_orphans(&keep).unwrap(),
            1,
            "thumbnail was unreferenced"
        );
        assert!(paths.resolve_media(&saved.media_rel).unwrap().exists());
    }

    #[test]
    fn rejects_malformed_images() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().join("d"), dir.path().join("c"));
        paths.ensure().unwrap();
        let m = MediaStore::new(paths);
        let mut bad = png_bytes(10, 10);
        bad.truncate(40);
        assert!(m.save_image("ffffff", "image/png", &bad, 100).is_err());
        assert!(m
            .save_image("eeeeee", "image/png", b"not an image", 100)
            .is_err());
    }
}
