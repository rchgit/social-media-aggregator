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
            // Odd target heights leave the last row without a bottom pixel;
            // clamp instead of panicking.
            let bottom = rgb.get_pixel(x, (y + 1).min(new_h - 1)).0;
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

    #[test]
    fn url_extension_covers_all_supported_types() {
        assert_eq!(extension_from_url("https://x/a.PNG"), "png");
        assert_eq!(extension_from_url("https://x/b.JPG"), "jpg");
        assert_eq!(extension_from_url("https://x/c.jpeg"), "jpg");
        assert_eq!(extension_from_url("https://x/d.gif"), "gif");
        assert_eq!(extension_from_url("https://x/e.webp"), "webp");
        assert_eq!(extension_from_url(""), "img");
        assert_eq!(extension_from_url("https://x/a.png?q=1&v=2"), "png");
    }

    #[test]
    fn render_ansi_emits_half_blocks_and_reset() {
        let preview = Preview {
            rows: vec![vec![Cell { fg: [1, 2, 3], bg: [4, 5, 6] }]],
        };
        let s = render_ansi(&preview);
        assert!(s.contains("\x1b[38;2;1;2;3m"), "{s:?}");
        assert!(s.contains("\x1b[48;2;4;5;6m"), "{s:?}");
        assert!(s.contains('▀'), "{s:?}");
        assert!(s.ends_with("\x1b[0m\n"), "{s:?}");
    }

    #[test]
    fn empty_preview_renders_nothing() {
        let empty = Preview { rows: Vec::new() };
        assert!(render_ansi(&empty).is_empty());
    }

    #[test]
    fn preview_scales_down_to_fit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.png");
        let img = image::RgbImage::from_pixel(200, 100, image::Rgb([10, 20, 30]));
        img.save(&path).unwrap();
        let p = preview(&path, 50, 25).unwrap();
        assert!(p.rows.len() <= 25);
        assert!(p.rows[0].len() <= 50);
        // Tiny budget still yields at least one row/cell (min clamps).
        let p = preview(&path, 1, 1).unwrap();
        assert_eq!(p.rows.len(), 1);
        assert_eq!(p.rows[0].len(), 1);
    }

    #[test]
    fn preview_rejects_garbage_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.png");
        std::fs::write(&junk, b"not an image").unwrap();
        assert!(preview(&junk, 10, 10)
            .unwrap_err()
            .to_string()
            .contains("decode"));
        assert!(preview(&dir.path().join("nope.png"), 10, 10)
            .unwrap_err()
            .to_string()
            .contains("decode"));
    }

    // ---- download_image over a local HTTP server ----

    struct OneShot {
        status: u16,
        content_type: &'static str,
        body: &'static [u8],
    }

    fn serve_one(resp: OneShot) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let reason = if resp.status == 200 { "OK" } else { "Not Found" };
            let http = format!(
                "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                resp.status,
                reason,
                resp.content_type,
                resp.body.len()
            );
            let _ = stream.write_all(http.as_bytes());
            let _ = stream.write_all(resp.body);
            let _ = stream.flush();
        });
        (format!("http://127.0.0.1:{port}/img"), handle)
    }

    fn client() -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap()
    }

    #[test]
    fn download_image_writes_file_keyed_by_url_hash() {
        let (url, server) = serve_one(OneShot {
            status: 200,
            content_type: "image/png",
            body: b"\x89PNG fake bytes",
        });
        let dir = tempfile::tempdir().unwrap();
        let path = download_image(&client(), &url, dir.path()).unwrap().unwrap();
        server.join().unwrap();
        assert!(path.starts_with(dir.path()));
        assert_eq!(path.extension().unwrap(), "png");
        assert_eq!(std::fs::read(&path).unwrap(), b"\x89PNG fake bytes");

        // A different URL must map to a different cache file name.
        let (url2, server2) = serve_one(OneShot {
            status: 200,
            content_type: "image/png",
            body: b"x",
        });
        let path2 = download_image(&client(), &url2, dir.path()).unwrap().unwrap();
        server2.join().unwrap();
        assert_ne!(path.file_name().unwrap(), path2.file_name().unwrap());
    }

    #[test]
    fn download_image_prefers_content_type_over_url_extension() {
        let (url, server) = serve_one(OneShot {
            status: 200,
            content_type: "image/jpeg",
            body: b"jpegdata",
        });
        let dir = tempfile::tempdir().unwrap();
        let path = download_image(&client(), &url, dir.path()).unwrap().unwrap();
        server.join().unwrap();
        assert_eq!(path.extension().unwrap(), "jpg");
    }

    #[test]
    fn download_image_rejects_non_image_content_type() {
        let (url, server) = serve_one(OneShot {
            status: 200,
            content_type: "text/html",
            body: b"<html></html>",
        });
        let dir = tempfile::tempdir().unwrap();
        let got = download_image(&client(), &url, dir.path()).unwrap();
        server.join().unwrap();
        assert!(got.is_none());
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn download_image_returns_none_for_http_failure() {
        let (url, server) = serve_one(OneShot {
            status: 404,
            content_type: "image/png",
            body: b"nope",
        });
        let dir = tempfile::tempdir().unwrap();
        let got = download_image(&client(), &url, dir.path()).unwrap();
        server.join().unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn download_image_network_error_maps_to_image_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = download_image(&client(), "http://127.0.0.1:1/img.png", dir.path())
            .unwrap_err();
        assert!(err.to_string().contains("download failed"), "{err}");
    }
}
