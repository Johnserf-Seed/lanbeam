//! The clipboard as something to SEND (paste-to-send).
//!
//! A screenshot never touches the disk: Win+Shift+S and ⌃⇧⌘4 leave nothing but
//! pixels on the clipboard, and the transfer stack sends files by path. So the
//! one thing this module adds to "read the clipboard" is turning those pixels
//! into a PNG the ordinary send flow can pick up — everything else on the
//! clipboard is already sendable as it is:
//!
//! | on the clipboard | what the UI gets | how it is sent |
//! |---|---|---|
//! | files / folders copied in the file manager | their paths | `send_files` |
//! | an image (a screenshot, "copy image") | the path of a PNG written here | `send_files` |
//! | text | the text | `send_text` (quick text) |
//!
//! Files win outright: a file copied in Finder/Explorer also carries its name as
//! text and often its icon as an image, and neither is what the user copied.
//! Text and image are BOTH reported when both are there (a spreadsheet's cells
//! are text and a picture of themselves) — which one was meant is the user's
//! call, so the UI offers the other.
//!
//! The PNGs live in the app cache, one folder per image named by its content
//! hash: pasting the same screenshot twice reuses the file instead of writing a
//! second copy. They cannot be deleted when the send ends the way the EXIF
//! scratch copies are — the transfer history's 再次发送 needs the path to still
//! be there — so they are swept instead: [`KEEP`] newest, none older than
//! [`MAX_AGE`].

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::LanBeamError;

/// How many pasted images are kept at most.
pub const KEEP: usize = 24;
/// How long a pasted image is kept.
pub const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// The preview's longest side, in pixels.
const THUMB_MAX: u32 = 160;
/// Clipboard text handed to the UI is cut here. Far above the quick-text cap
/// (`MAX_TEXT_BYTES`, 60 KiB), so a text that was cut can never be SENT cut:
/// the quick-text panel already refuses it as too long. The cut only keeps a
/// multi-megabyte clipboard from being marshalled into the webview to be told so.
const MAX_TEXT: usize = 256 * 1024;

/// What the clipboard holds that LanBeam can send.
#[derive(Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardPayload {
    /// Files and folders copied in the file manager. When this is non-empty
    /// the other two are not looked at.
    pub files: Vec<String>,
    pub image: Option<ClipboardImage>,
    pub text: Option<String>,
}

/// A clipboard image, written out as a PNG.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardImage {
    pub path: String,
    /// The file name the receiver will see.
    pub name: String,
    /// Bytes of the PNG.
    pub size: u64,
    pub width: u32,
    pub height: u32,
    /// A small preview as a `data:` URL — the file name says nothing about
    /// which screenshot this is.
    pub thumb: String,
}

/// Read the system clipboard. An image is written under `root`.
#[cfg(desktop)]
pub fn read(root: &Path) -> Result<ClipboardPayload, LanBeamError> {
    let mut cb = arboard::Clipboard::new()
        .map_err(|e| LanBeamError::Io(format!("clipboard unavailable: {e}")))?;

    let files: Vec<String> = cb
        .get()
        .file_list()
        .unwrap_or_default()
        .into_iter()
        // A path copied a while ago may be gone; one that is not UTF-8 cannot
        // cross the IPC boundary as the string `send_files` takes.
        .filter(|p| p.exists())
        .filter_map(|p| p.to_str().map(str::to_owned))
        .collect();
    if !files.is_empty() {
        return Ok(ClipboardPayload {
            files,
            ..Default::default()
        });
    }

    let text = cb.get_text().ok().and_then(usable_text);
    // "No image there" is an answer; failing to WRITE one is an error — it must
    // not read as an empty clipboard.
    let image = match cb.get_image() {
        Ok(img) => save_image(root, &img.bytes, img.width, img.height, &local_stamp())?,
        Err(_) => None,
    };
    Ok(ClipboardPayload { files, image, text })
}

/// Mobile has no arboard backend (and no paste-to-send surface).
#[cfg(not(desktop))]
pub fn read(_root: &Path) -> Result<ClipboardPayload, LanBeamError> {
    Ok(ClipboardPayload::default())
}

/// `20261116-143025`, local time — it goes into the file name the receiver sees.
#[cfg(desktop)]
fn local_stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// Clipboard text worth offering: not blank, and cut to [`MAX_TEXT`] on a
/// character boundary.
fn usable_text(mut text: String) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    if text.len() > MAX_TEXT {
        let mut end = MAX_TEXT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    Some(text)
}

/// Write `rgba` (8-bit RGBA, row-major) as a PNG under `root` and describe it.
/// `None` for something that is not a picture — zero-sized, or a buffer that
/// does not match its dimensions. The same pixels always land in the same
/// folder, so a second paste of one screenshot returns the file already there.
pub fn save_image(
    root: &Path,
    rgba: &[u8],
    width: usize,
    height: usize,
    stamp: &str,
) -> Result<Option<ClipboardImage>, LanBeamError> {
    let (Ok(w), Ok(h)) = (u32::try_from(width), u32::try_from(height)) else {
        return Ok(None);
    };
    let expected = width.checked_mul(height).and_then(|n| n.checked_mul(4));
    if w == 0 || h == 0 || expected != Some(rgba.len()) {
        return Ok(None);
    }

    let dir = root.join(content_key(rgba, w, h));
    let path = match existing_png(&dir) {
        Some(p) => {
            // Used again: count it as fresh, so the sweep keeps what is in use.
            if let Ok(f) = std::fs::File::options().write(true).open(&p) {
                let _ = f.set_modified(SystemTime::now());
            }
            p
        }
        None => {
            create_private_dir(&dir)?;
            let path = dir.join(format!("clipboard-{stamp}.png"));
            // Written whole or not at all: a half-written PNG under the content
            // key would be "the file already there" for every later paste.
            let part = dir.join("image.part");
            write_owner_only(&part, &encode_png(rgba, w, h)?)?;
            std::fs::rename(&part, &path)?;
            path
        }
    };

    let size = std::fs::metadata(&path)?.len();
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("clipboard.png")
        .to_owned();
    let (t, tw, th) = thumbnail(rgba, w, h, THUMB_MAX);
    Ok(Some(ClipboardImage {
        path: path.to_string_lossy().into_owned(),
        name,
        size,
        width: w,
        height: h,
        thumb: data_url(&encode_png(&t, tw, th)?),
    }))
}

/// Remove pasted images older than `max_age`, and all but the `keep` newest.
pub fn sweep(root: &Path, now: SystemTime, max_age: Duration, keep: usize) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut dirs: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let dir = e.path();
            // The PNG's own time (a reuse bumps it); a folder without one is a
            // write that never finished, as old as the folder itself.
            let at = existing_png(&dir)
                .or_else(|| Some(dir.clone()))
                .and_then(|p| std::fs::metadata(p).ok())
                .and_then(|m| m.modified().ok())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            (at, dir)
        })
        .collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.0));
    for (i, (at, dir)) in dirs.iter().enumerate() {
        let expired = now.duration_since(*at).is_ok_and(|age| age > max_age);
        if i >= keep || expired {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// The folder name for these pixels: the first 16 hex digits of their SHA-256
/// (dimensions included — the same bytes at another width are another picture).
fn content_key(rgba: &[u8], w: u32, h: u32) -> String {
    let mut hasher = Sha256::new();
    hasher.update(w.to_le_bytes());
    hasher.update(h.to_le_bytes());
    hasher.update(rgba);
    hasher.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn existing_png(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "png"))
}

fn encode_png(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, LanBeamError> {
    let failed = |e: png::EncodingError| LanBeamError::Io(format!("png encode: {e}"));
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    // A 5K screenshot is ~60 MB of RGBA and the user is waiting on the paste:
    // quick beats small. Lossless either way.
    enc.set_compression(png::Compression::Fast);
    let mut writer = enc.write_header().map_err(failed)?;
    writer.write_image_data(rgba).map_err(failed)?;
    writer.finish().map_err(failed)?;
    Ok(out)
}

/// Shrink to at most `max` on the longest side by averaging each block of
/// source pixels (a screenshot is mostly text; picking single pixels shreds it).
/// An image already that small is returned as it is.
fn thumbnail(rgba: &[u8], w: u32, h: u32, max: u32) -> (Vec<u8>, u32, u32) {
    let longest = w.max(h);
    if longest <= max {
        return (rgba.to_vec(), w, h);
    }
    let scale = |v: u32| ((u64::from(v) * u64::from(max)) / u64::from(longest)).max(1) as u32;
    let (tw, th) = (scale(w), scale(h));
    let (w, h, tw64, th64) = (w as usize, h as usize, tw as usize, th as usize);
    let mut out = Vec::with_capacity(tw64 * th64 * 4);
    for ty in 0..th64 {
        let (y0, y1) = (ty * h / th64, ((ty + 1) * h / th64).max(ty * h / th64 + 1));
        for tx in 0..tw64 {
            let (x0, x1) = (tx * w / tw64, ((tx + 1) * w / tw64).max(tx * w / tw64 + 1));
            let mut sum = [0u64; 4];
            for y in y0..y1 {
                let row = &rgba[(y * w + x0) * 4..(y * w + x1) * 4];
                for px in row.as_chunks::<4>().0 {
                    for (s, c) in sum.iter_mut().zip(px) {
                        *s += u64::from(*c);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            out.extend(sum.map(|s| (s / n) as u8));
        }
    }
    (out, tw, th)
}

fn data_url(png: &[u8]) -> String {
    use base64::Engine;
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    )
}

/// The image folder, owner-only on Unix (0700): what is in it is whatever the
/// user last took a screenshot of, and a cache dir that fell back to /tmp is
/// world-traversable. Windows' per-user profile ACLs already cover it.
fn create_private_dir(dir: &Path) -> Result<(), LanBeamError> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Write `bytes`, owner-read/write only on Unix (0600).
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), LanBeamError> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lanbeam-clip-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// `w`×`h` of one colour.
    fn solid(w: usize, h: usize, px: [u8; 4]) -> Vec<u8> {
        px.iter().copied().cycle().take(w * h * 4).collect()
    }

    fn decode(png_bytes: &[u8]) -> (Vec<u8>, u32, u32) {
        let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes))
            .read_info()
            .unwrap();
        let (w, h) = (reader.info().width, reader.info().height);
        let mut buf = vec![0; (w * h * 4) as usize];
        reader.next_frame(&mut buf).unwrap();
        (buf, w, h)
    }

    #[test]
    fn a_pasted_image_becomes_a_png_with_the_same_pixels() {
        let root = scratch("save");
        let mut rgba = solid(3, 2, [10, 20, 30, 255]);
        rgba[4..8].copy_from_slice(&[200, 100, 50, 128]);

        let img = save_image(&root, &rgba, 3, 2, "20261116-143025")
            .unwrap()
            .expect("a 3×2 image");

        assert_eq!(img.name, "clipboard-20261116-143025.png");
        assert_eq!((img.width, img.height), (3, 2));
        let bytes = std::fs::read(&img.path).unwrap();
        assert_eq!(img.size, bytes.len() as u64);
        assert_eq!(decode(&bytes), (rgba, 3, 2), "lossless, alpha included");
        assert!(img.thumb.starts_with("data:image/png;base64,"));
        assert!(
            !Path::new(&img.path).with_file_name("image.part").exists(),
            "the part file was renamed into place"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Ctrl+V twice on one screenshot is one file, under its first name.
    #[test]
    fn the_same_pixels_reuse_the_file_already_written() {
        let root = scratch("reuse");
        let rgba = solid(4, 4, [1, 2, 3, 255]);

        let first = save_image(&root, &rgba, 4, 4, "20261116-100000")
            .unwrap()
            .unwrap();
        let again = save_image(&root, &rgba, 4, 4, "20261116-100500")
            .unwrap()
            .unwrap();
        assert_eq!(again.path, first.path);
        assert_eq!(again.name, "clipboard-20261116-100000.png");

        // Same bytes, another shape: another picture.
        let reshaped = save_image(&root, &rgba, 8, 2, "20261116-100500")
            .unwrap()
            .unwrap();
        assert_ne!(reshaped.path, first.path);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn what_is_not_a_picture_is_not_saved() {
        let root = scratch("bad");
        assert_eq!(save_image(&root, &[], 0, 0, "s").unwrap(), None);
        assert_eq!(save_image(&root, &[0; 16], 0, 4, "s").unwrap(), None);
        assert_eq!(
            save_image(&root, &[0; 15], 2, 2, "s").unwrap(),
            None,
            "a buffer that does not match its dimensions"
        );
        assert!(!root.exists(), "and nothing was created for it");
    }

    #[test]
    fn the_preview_is_scaled_to_fit_and_keeps_the_shape() {
        // 400×100 → 160×40, each output pixel the average of its block.
        let mut rgba = solid(400, 100, [0, 0, 0, 255]);
        for row in rgba.as_chunks_mut::<{ 400 * 4 }>().0 {
            // The right half white.
            row[200 * 4..].fill(255);
        }
        let (t, tw, th) = thumbnail(&rgba, 400, 100, 160);
        assert_eq!((tw, th), (160, 40));
        assert_eq!(t.len(), 160 * 40 * 4);
        assert_eq!(&t[..4], &[0, 0, 0, 255], "left edge");
        assert_eq!(&t[159 * 4..160 * 4], &[255, 255, 255, 255], "right edge");

        // Tall and thin never collapses to zero width.
        let (_, tw, th) = thumbnail(&solid(1, 1000, [9, 9, 9, 255]), 1, 1000, 160);
        assert_eq!((tw, th), (1, 160));

        // Already small: untouched.
        let small = solid(5, 7, [4, 5, 6, 7]);
        assert_eq!(thumbnail(&small, 5, 7, 160), (small.clone(), 5, 7));
    }

    #[test]
    fn the_sweep_keeps_the_newest_and_drops_the_expired() {
        let root = scratch("sweep");
        let now = SystemTime::now();
        let age = |secs: u64| now - Duration::from_secs(secs);
        // (folder, age in seconds)
        for (name, secs) in [
            ("fresh", 10),
            ("older", 100),
            ("oldest", 200),
            ("stale", 5000),
        ] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let png = dir.join("clipboard-x.png");
            std::fs::write(&png, b"x").unwrap();
            std::fs::File::options()
                .write(true)
                .open(&png)
                .unwrap()
                .set_modified(age(secs))
                .unwrap();
        }
        // Not ours to judge: a stray file next to the folders stays.
        std::fs::write(root.join("note.txt"), b"x").unwrap();

        sweep(&root, now, Duration::from_secs(1000), 2);

        assert!(root.join("fresh").exists());
        assert!(root.join("older").exists());
        assert!(!root.join("oldest").exists(), "over the count");
        assert!(!root.join("stale").exists(), "over the age");
        assert!(root.join("note.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn blank_text_is_nothing_and_huge_text_is_cut_on_a_character() {
        assert_eq!(usable_text("  \n\t".into()), None);
        assert_eq!(usable_text(" hi ".into()), Some(" hi ".into()));
        // 3 bytes each: the cap falls inside a character.
        let big = "汉".repeat(MAX_TEXT / 3 + 10);
        let cut = usable_text(big).unwrap();
        assert!(cut.len() <= MAX_TEXT && cut.len() > MAX_TEXT - 3);
        assert!(cut.chars().all(|c| c == '汉'));
    }
}
