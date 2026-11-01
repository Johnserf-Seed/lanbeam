//! Building the send list from the user's selection: the check against the
//! receiver's limits before anything is read (H7), the walk, the optional
//! SHA-256 (M6.3), and EXIF/metadata stripping into temp copies (M9.1).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::SendItem;
use crate::consts::{MAX_MANIFEST_FILES, MAX_MANIFEST_NAME_BYTES, MAX_MANIFEST_NAME_TOTAL};
use crate::error::{LanBeamError, Result};
use crate::exif;
use crate::protocol::FileMeta;
use crate::sanitize::{self, Decision};

// ── build the send list from user-selected paths ──────────────────────
/// Enumerate the files under `paths`, producing one [`SendItem`] each. When
/// `verify_hash` is on (M6.3), every file is additionally SHA-256'd here (an
/// extra full read) so its digest can ride the manifest that precedes the
/// bytes. WHY the caller must run this on the blocking pool: with hashing on,
/// this reads every byte of every file, which must never occupy an async worker.
///
/// No metadata stripping (M9.1) — this is the stable entry for direct callers
/// (tests, internal). The send command uses [`build_send_list_scoped`], which
/// additionally scrubs image metadata into temp copies.
pub fn build_send_list(paths: &[PathBuf], verify_hash: bool) -> Result<Vec<SendItem>> {
    // Delegate to the strip-aware builder with stripping OFF: with `strip_exif`
    // false, `build_item` skips its whole strip block and falls straight through
    // to `item`, and the scratch base is never created or touched — so every
    // returned item is byte-identical to walking with `item` directly. One
    // enumeration to maintain instead of a parallel copy.
    build_send_list_scoped(paths, verify_hash, false, std::path::PathBuf::new())
        .map(|(items, _)| items)
}

/// The strip-aware send-list builder (M9.1): the same enumeration as
/// [`build_send_list`], but when `strip_exif` is on, every JPEG/PNG/WebP is
/// scrubbed of EXIF/ICC/XMP into a temp copy under `scratch_base`, and that copy
/// — not the original — is what the returned item points at, with `size` and
/// (when `verify_hash`) `sha256` computed over the CLEANED bytes so the manifest
/// matches exactly what streams. Non-images, images we can't parse, files over
/// [`exif::MAX_STRIP_BYTES`], and the whole strip-off path are byte-identical to
/// [`build_send_list`].
///
/// Returns the items plus a [`SendScratch`] guard the caller MUST hold until the
/// send finishes: dropping it deletes every temp copy on success OR failure. WHY
/// the caller runs this on the blocking pool — like [`build_send_list`] it reads
/// every file, and now also writes cleaned copies, which must never occupy an
/// async worker.
pub fn build_send_list_scoped(
    paths: &[PathBuf],
    verify_hash: bool,
    strip_exif: bool,
    scratch_base: PathBuf,
) -> Result<(Vec<SendItem>, SendScratch)> {
    let mut ctx = StripCtx::new(strip_exif, scratch_base);
    let mut out = Vec::new();
    walk_selection(paths, &mut |name, abs, meta| {
        out.push(build_item(name, abs, meta, verify_hash, &mut ctx)?);
        Ok(())
    })?;
    Ok((out, ctx.scratch))
}

/// The receiver's manifest limits and name rules, applied to a selection BEFORE
/// anything is hashed, stripped or dialed (H7). Without it, a selection the
/// receiver must refuse was only discovered on the wire: over the file count or
/// the name budget the receiver drops the link mid-manifest ("connection
/// lost"), and an unsavable name came back as a refusal — after the user had
/// picked a device, sat through the hashing of every file, and maybe waited on
/// a prompt. Same constants and the same sanitizer as the receive path, and
/// cheap: metadata only, stopping at the first problem.
pub fn check_send_selection(paths: &[PathBuf]) -> Result<()> {
    check_selection_within(paths, MAX_MANIFEST_FILES, MAX_MANIFEST_NAME_TOTAL)
}

/// [`check_send_selection`] with the two budgets passed in, so a test can hit
/// them with three files instead of ten thousand.
pub(super) fn check_selection_within(
    paths: &[PathBuf],
    max_files: u32,
    max_name_total: usize,
) -> Result<()> {
    let mut count: u32 = 0;
    let mut name_bytes = 0usize;
    walk_selection(paths, &mut |name, _abs, _meta| {
        count += 1;
        if count > max_files {
            return Err(LanBeamError::TooManyFiles(format!(
                "more than {max_files} files"
            )));
        }
        name_bytes += name.len();
        if name_bytes > max_name_total {
            return Err(LanBeamError::TooManyFiles(format!(
                "file names exceed {max_name_total} bytes"
            )));
        }
        if name.len() > MAX_MANIFEST_NAME_BYTES
            || matches!(sanitize::validate(&name), Decision::Reject(_))
        {
            return Err(LanBeamError::UnsafePath(name));
        }
        Ok(())
    })?;
    if count == 0 {
        return Err(LanBeamError::NoFiles);
    }
    Ok(())
}

/// The visitor [`walk_selection`] calls once per file: the manifest name (the
/// relative path the receiver recreates), the absolute path, and its metadata.
type SelectionVisitor<'a> = dyn FnMut(String, PathBuf, &std::fs::Metadata) -> Result<()> + 'a;

/// Every file a selection sends, in send order — the ONE traversal behind both
/// [`check_send_selection`] and [`build_send_list_scoped`], so the cheap
/// pre-check and the real build can never disagree about what is sent. A
/// top-level file goes under its own name, a folder as `folder/…`. Entries
/// INSIDE a folder are judged by their own metadata (a symlink there is neither
/// a file nor a folder, so it is skipped). A visitor error stops the walk.
fn walk_selection(paths: &[PathBuf], visit: &mut SelectionVisitor<'_>) -> Result<()> {
    for p in paths {
        let meta = std::fs::metadata(p)?;
        if meta.is_file() {
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            visit(name, p.clone(), &meta)?;
        } else if meta.is_dir() {
            let base = p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "folder".into());
            walk_dir(p, &base, visit)?;
        }
    }
    Ok(())
}

fn walk_dir(dir: &Path, rel_prefix: &str, visit: &mut SelectionVisitor<'_>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = format!("{rel_prefix}/{name}");
        if meta.is_file() {
            visit(rel, entry.path(), &meta)?;
        } else if meta.is_dir() {
            walk_dir(&entry.path(), &rel, visit)?;
        }
    }
    Ok(())
}

fn item(
    name: String,
    abs: PathBuf,
    meta: &std::fs::Metadata,
    verify_hash: bool,
) -> Result<SendItem> {
    // Precompute the content hash in this same (blocking) pass when the user
    // asked for integrity verification (M6.3). WHY hash here and not fold it
    // during send: the manifest carrying the digest is sent BEFORE any file
    // bytes, so the hash must already exist — that costs one extra full read of
    // every file on the sender, the price of the guarantee. A file mutated
    // between this read and the send simply fails the receiver's check, which
    // is the correct outcome (its bytes no longer match what was promised).
    let sha256 = if verify_hash {
        Some(hash_file(&abs)?)
    } else {
        None
    };
    Ok(SendItem {
        meta: FileMeta {
            name,
            size: meta.len(),
            mtime: mtime_secs(meta),
            mode: 0,
            sha256,
        },
        abs,
    })
}

/// Seconds-since-epoch mtime for `meta`, `0` when the platform can't report it.
/// The manifest `mtime` field, shared by both send-list builders.
fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── EXIF/metadata stripping on send (M9.1) ────────────────────────────
/// Per-send scratch directory for metadata-stripped temp copies (M9.1). Its
/// whole subtree is deleted on drop, so the cleaned copies of the user's photos
/// are removed on EVERY exit path of the send — success, error, or cancel — and
/// a temp copy of a private photo never lingers on disk. Created LAZILY: the
/// directory only appears the first time a file is actually stripped, so a send
/// with nothing to strip leaves nothing behind.
///
/// Ownership: `send_files` holds this guard for the whole `run_send` and drops
/// it when the command returns (all exit paths). If `build_send_list_scoped`
/// itself errors partway, the partially-populated guard is a local there and
/// drops on the `?`, still cleaning up every temp already written.
pub struct SendScratch {
    /// The scratch directory once created; `None` until the first temp is written.
    dir: Option<PathBuf>,
}

impl SendScratch {
    /// A guard owning no directory — the strip-off / nothing-stripped case.
    fn empty() -> Self {
        Self { dir: None }
    }

    /// The scratch directory, if temp copies were written. Observability for
    /// tests asserting the temps are removed once the guard drops.
    #[doc(hidden)]
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }
}

impl Drop for SendScratch {
    fn drop(&mut self) {
        if let Some(dir) = self.dir.take() {
            // Best-effort: a Drop must not panic. The dir is app-private scratch
            // (never the download root), so a rare leftover from a locked/vanished
            // file is harmless and swept with the app cache later.
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// Threads the strip decision, the lazily-created scratch dir, and a per-file
/// counter through the send-list walk (M9.1).
struct StripCtx {
    strip_exif: bool,
    /// The per-transfer scratch directory to create on first use.
    base: PathBuf,
    /// The RAII guard accumulating whatever directory gets created.
    scratch: SendScratch,
    /// Monotonic index for unique temp file names within the scratch dir.
    counter: u32,
}

impl StripCtx {
    fn new(strip_exif: bool, base: PathBuf) -> Self {
        Self {
            strip_exif,
            base,
            scratch: SendScratch::empty(),
            counter: 0,
        }
    }

    /// Write `bytes` to a fresh temp file in the scratch dir (creating the dir on
    /// first call) and return its path. The name is index-based on purpose: the
    /// receiver names the file from the manifest, never from this path, so the
    /// temp's own name and extension are internal only.
    fn write_temp(&mut self, bytes: &[u8], ext: &str) -> Result<PathBuf> {
        if self.scratch.dir.is_none() {
            std::fs::create_dir_all(&self.base)?;
            // On Unix, lock the scratch dir to the owner (0700). The temp copies
            // are plaintext of the user's private photos, and the app-cache /tmp
            // fallback (commands/transfer.rs) can land in a world-traversable
            // directory — 0700 keeps another local user from reading them during
            // the send.
            // No-op on Windows, where %LOCALAPPDATA%/%TEMP% are per-user ACL'd.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&self.base, std::fs::Permissions::from_mode(0o700))?;
            }
            self.scratch.dir = Some(self.base.clone());
        }
        let path = self.base.join(format!("{}.{ext}", self.counter));
        self.counter += 1;
        write_owner_only(&path, bytes)?;
        Ok(path)
    }
}

/// Write `bytes` to `path`, owner-read/write only on Unix (mode 0600) so no other
/// local user can read a stripped copy of a private photo even briefly. On
/// non-Unix this is a plain write — the enclosing scratch dir is already
/// per-user ACL'd there, and Unix mode bits do not apply.
#[cfg(unix)]
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes)?;
    Ok(())
}

/// The lowercased extension of `abs` IF it is a format [`exif::strip_image_metadata`]
/// can attempt — the gate that keeps a non-image (or a HEIC/TIFF/RAW we can't
/// parse) from ever being read into memory just to have the strip decline it.
fn image_strip_ext(abs: &Path) -> Option<String> {
    let ext = abs.extension()?.to_str()?.to_ascii_lowercase();
    exif::is_strippable_ext(&ext).then_some(ext)
}

/// The outcome of attempting a strip on one file (M9.1), carrying the bytes it
/// read so the caller never re-reads the file just to hash it.
enum StripOutcome {
    /// Metadata was actually removed — these CLEANED bytes (distinct from the
    /// file on disk) are what will stream; `size`/`sha256` come from them.
    Cleaned(Vec<u8>),
    /// The file was read but left unchanged (no metadata to remove, or an
    /// unparseable/passthrough image). The ORIGINAL file still streams, but its
    /// already-read bytes ride along so the caller hashes THEM in memory instead
    /// of reading the whole file a second time.
    Unchanged(Vec<u8>),
    /// The file could not be read here at all — defer to the normal [`item`]
    /// path, which reads it (for the hash) and surfaces any error there.
    Unreadable,
}

/// Read `abs` and attempt a metadata strip (M9.1), distinguishing a real strip
/// (needs a cleaned temp copy) from an unchanged passthrough (the original
/// streams) while keeping the bytes it read. A metadata-free image re-encodes
/// byte-identical: returning [`StripOutcome::Unchanged`] with the original buffer
/// lets the caller hash it in memory, avoiding a pointless temp copy AND the
/// second full read the old `Option` shape forced. Stripping is best-effort and
/// never fails the send — a read error defers to [`item`]'s own error handling.
fn try_strip(abs: &Path, ext: &str) -> StripOutcome {
    let Ok(original) = std::fs::read(abs) else {
        return StripOutcome::Unreadable;
    };
    match exif::strip_image_metadata(&original, ext) {
        Some(cleaned) if cleaned != original => StripOutcome::Cleaned(cleaned),
        // No metadata removed (or an unparseable image): the original streams.
        _ => StripOutcome::Unchanged(original),
    }
}

/// Build one [`SendItem`], stripping image metadata into a temp copy first when
/// the strip is on and applies (M9.1). On a real strip the item points at the
/// CLEANED temp file with `size` and (when `verify_hash`) `sha256` recomputed
/// over the cleaned bytes — the consistency contract that keeps the receiver's
/// size/hash checks passing and the chunk stream aligned. Otherwise it is
/// exactly [`item`]'s original-file result.
fn build_item(
    name: String,
    abs: PathBuf,
    meta: &std::fs::Metadata,
    verify_hash: bool,
    ctx: &mut StripCtx,
) -> Result<SendItem> {
    if ctx.strip_exif {
        if let Some(ext) = image_strip_ext(&abs) {
            // Skip absurdly large "images" (a video renamed .jpg) rather than
            // slurp them whole into RAM — send them unstripped instead.
            if meta.len() <= exif::MAX_STRIP_BYTES {
                match try_strip(&abs, &ext) {
                    StripOutcome::Cleaned(cleaned) => {
                        let temp = ctx.write_temp(&cleaned, &ext)?;
                        // Hash the CLEANED bytes in memory (no re-read): they are
                        // the bytes that will stream, so they are what the manifest
                        // must promise.
                        let sha256 = verify_hash.then(|| to_hex(&Sha256::digest(&cleaned)));
                        return Ok(SendItem {
                            meta: FileMeta {
                                name,
                                size: cleaned.len() as u64,
                                mtime: mtime_secs(meta),
                                mode: 0,
                                sha256,
                            },
                            abs: temp,
                        });
                    }
                    StripOutcome::Unchanged(original) => {
                        // Nothing to strip: the ORIGINAL file streams (abs stays
                        // put), but we already hold its bytes — hash THEM here
                        // instead of making [`item`] read the whole file again.
                        // `size` is the buffer we hashed, so size+hash stay
                        // consistent with the bytes `run_send` streams from `abs`.
                        let sha256 = verify_hash.then(|| to_hex(&Sha256::digest(&original)));
                        return Ok(SendItem {
                            meta: FileMeta {
                                name,
                                size: original.len() as u64,
                                mtime: mtime_secs(meta),
                                mode: 0,
                                sha256,
                            },
                            abs,
                        });
                    }
                    // Unreadable here — defer to `item` below, which reads the
                    // file and surfaces the error on the normal path.
                    StripOutcome::Unreadable => {}
                }
            }
        }
    }
    // No strip applied — original path, size and (optional) hash unchanged.
    item(name, abs, meta, verify_hash)
}

/// Stream a file through SHA-256 and return the lowercase-hex digest (M6.3).
/// Synchronous std I/O by design: every caller runs inside `spawn_blocking`,
/// so a multi-gigabyte read never occupies an async worker.
fn hash_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// Lowercase-hex encoding of a byte slice — the wire form for `FileMeta.sha256`.
pub(super) fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
