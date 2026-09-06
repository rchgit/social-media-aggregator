use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// One half-block terminal cell: foreground paints the top pixel, background
/// paints the bottom pixel of the `▀` glyph.
#[derive(Debug, Clone, Copy)]
pub struct Cell {
    pub fg: [u8; 3],
    pub bg: [u8; 3],
}

/// A downscaled image, one cell per terminal column and two rows of pixels
/// per terminal row.
#[derive(Debug, Clone)]
pub struct Preview {
    pub rows: Vec<Vec<Cell>>,
}

/// Downloads an image to the cache directory, keyed by a stable hash of the
/// URL, and returns its local path. Returns `Ok(None)` for non-image content
/// types so a bad link degrades to a text-only post.
pub fn download_image(
    client: &reqwest::blocking::Client,
    url: &str,
    cache_dir: &Path,
) -> Result<Option<PathBuf>> {
    let resp = client
        .get(url)
        .send()
        .map_err(|e| Error::Image(format!("download failed: {e}")))?;
    if !resp.status().is_success() {
        return Ok(None);
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let ext = extension_from_content_type(&content_type).unwrap_or_else(|| extension_from_url(url));
    if !matches!(ext, "png" | "jpg" | "jpeg" | "gif" | "webp") {
        return Ok(None);
    }

    let bytes = resp
        .bytes()
        .map_err(|e| Error::Image(format!("read body failed: {e}")))?;
    std::fs::create_dir_all(cache_dir)?;

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    let path = cache_dir.join(format!("{:016x}.{ext}", hasher.finish()));
    std::fs::write(&path, &bytes)?;
    Ok(Some(path))
}

/// Decodes and downscales an image into a `Preview` of half-block cells.
pub fn preview(path: &Path, max_width: usize, max_height: usize) -> Result<Preview> {
    let img =
        image::open(path).map_err(|e| Error::Image(format!("decode {}: {e}", path.display())))?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Ok(Preview { rows: Vec::new() });
    }

    // Half-block glyphs pack two vertical pixels per row of cells.
    let avail_h_px = (max_height.max(1) as u32).saturating_mul(2);
    let scale = (max_width.max(1) as f64 / w as f64)
        .min(avail_h_px as f64 / h as f64)
        .min(1.0);
    let new_w = ((w as f64 * scale).round() as u32).max(1);
    let new_h = ((h as f64 * scale).round() as u32).max(2);

    let img = img.resize_exact(new_w, new_h, image::imageops::FilterType::Triangle);
    let rgb = img.to_rgb8();

    let mut rows = Vec::with_capacity((new_h as usize).div_ceil(2));
    for y in (0..new_h).step_by(2) {
        let mut row = Vec::with_capacity(new_w as usize);
        for x in 0..new_w {
            let top = rgb.get_pixel(x, y).0;
            let bottom = rgb.get_pixel(x, y + 1).0;
            row.push(Cell {
                fg: top,
                bg: bottom,
            });
        }
        rows.push(row);
    }
    Ok(Preview { rows })
}

/// Renders a `Preview` as raw ANSI truecolor half-blocks, for use outside a
/// TUI frame (plain terminal output and tests).
pub fn render_ansi(preview: &Preview) -> String {
    let mut out = String::new();
    for row in &preview.rows {
        for cell in row {
            out.push_str(&format!(
                "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m▀",
                cell.fg[0], cell.fg[1], cell.fg[2], cell.bg[0], cell.bg[1], cell.bg[2]
            ));
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

fn extension_from_content_type(ct: &str) -> Option<&'static str> {
    let ct = ct.split(';').next().unwrap_or("").trim();
    match ct {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

fn extension_from_url(url: &str) -> &'static str {
    let path = url.split('?').next().unwrap_or(url);
    match Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "png",
        Some("jpg") | Some("jpeg") => "jpg",
        Some("gif") => "gif",
        Some("webp") => "webp",
        _ => "img",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_maps_to_extension() {
        assert_eq!(
            extension_from_content_type("image/png; charset=binary"),
            Some("png")
        );
        assert_eq!(extension_from_content_type("image/jpeg"), Some("jpg"));
        assert_eq!(extension_from_content_type("text/html"), None);
    }

    #[test]
    fn url_extension_falls_back() {
        assert_eq!(extension_from_url("https://x/a.png?w=1"), "png");
        assert_eq!(extension_from_url("https://x/a"), "img");
    }

    #[test]
    fn preview_preserves_dimensions() {
        // 4x4 red square -> 2 rows of 2 cells.
        let dir = std::env::temp_dir().join("sma-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("red.png");
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([255, 0, 0]));
        img.save(&path).unwrap();
        let p = preview(&path, 10, 10).unwrap();
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].len(), 4);
        assert_eq!(p.rows[0][0].fg, [255, 0, 0]);
        std::fs::remove_file(&path).ok();
    }
}
