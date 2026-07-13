//! ⭐ Security-critical: the single choke point for every received-file write.
//! Split ([FIX-1]) into a PURE `validate` (lexical decision, no filesystem access)
//! and `resolve_and_open` (write-time I/O with a canonicalize-and-contain check).
//!
//! LOAD-BEARING INVARIANT (§5.1.1): if `validate` returns `Accept(rel)`, then `rel`
//! is relative and contains ONLY `Normal` components (no `..`, no root, no prefix) —
//! so `download_root.join(rel)` can never escape the root. `resolve_and_open` adds a
//! runtime canonicalize-and-contain check as defense-in-depth (e.g. a pre-existing
//! symlink inside the root pointing outward). DESIGN §5.1.
#![allow(dead_code)] // wired into the receive loop in M3-B

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::error::{LanBeamError, Result};

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    "CONIN$", "CONOUT$", "CLOCK$",
];
const MAX_COMPONENTS: usize = 16;
const MAX_COMPONENT_LEN: usize = 200;
const MAX_TOTAL_LEN: usize = 4096;

#[derive(Debug, PartialEq)]
pub enum Decision {
    /// A validated RELATIVE path (Normal components only). Not yet resolved to disk.
    Accept(PathBuf),
    Reject(RejectReason),
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum RejectReason {
    NullByte,
    Empty,
    AbsolutePath,
    DrivePrefix,
    NoValidComponents,
    TooManyComponents,
    TooLong,
    EscapesDownloadRoot,
}

/// Pure lexical validation. No filesystem access, no side effects.
pub fn validate(name: &str) -> Decision {
    use RejectReason::*;

    // Step 0 — reject NUL anywhere.
    if name.contains('\0') {
        return Decision::Reject(NullByte);
    }
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Decision::Reject(Empty);
    }
    if trimmed.len() > MAX_TOTAL_LEN {
        return Decision::Reject(TooLong);
    }

    // Step 1 — reject absolute / drive / UNC on the RAW string.
    let b = trimmed.as_bytes();
    if b[0] == b'/' || b[0] == b'\\' {
        return Decision::Reject(AbsolutePath); // also UNC \\ and //
    }
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return Decision::Reject(DrivePrefix); // "C:\x" AND "C:x"
    }

    // Step 2 — split on BOTH separators; drop "", ".", ".."; sanitize each survivor.
    let mut parts: Vec<String> = Vec::new();
    for raw in trimmed.split(['/', '\\']) {
        match clean_component(raw) {
            None => continue,
            Some(s) => {
                if s.len() > MAX_COMPONENT_LEN {
                    return Decision::Reject(TooLong);
                }
                parts.push(s);
            }
        }
    }
    if parts.is_empty() {
        return Decision::Reject(NoValidComponents);
    }
    if parts.len() > MAX_COMPONENTS {
        return Decision::Reject(TooManyComponents);
    }

    let mut rel = PathBuf::new();
    for p in &parts {
        rel.push(p);
    }
    Decision::Accept(rel)
}

fn clean_component(comp: &str) -> Option<String> {
    if comp.is_empty() || comp == "." || comp == ".." {
        return None;
    }
    // Strip bidi/zero-width (RLO spoofing); map illegal chars → '_' (':' kills NTFS ADS).
    let s: String = comp
        .chars()
        .filter(|c| !is_bidi_or_zero_width(*c))
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '/' | '\\') {
                '_'
            } else {
                c
            }
        })
        .collect();
    // Windows silently strips trailing dots/spaces → do it ourselves ("evil.bat." masquerade).
    let s = s.trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() {
        return None;
    }
    // Reserved device name — matched on a NORMALIZED stem: Windows ignores trailing
    // spaces/dots and folds superscript ¹²³ when resolving device names. [red-team #2,#3]
    if is_reserved_stem(&s) {
        return Some(format!("_{s}"));
    }
    Some(s)
}

fn is_reserved_stem(s: &str) -> bool {
    let raw = s.split('.').next().unwrap_or("");
    let stem: String = raw
        .trim_matches([' ', '.'])
        .chars()
        .map(|c| match c {
            '\u{00B9}' => '1',
            '\u{00B2}' => '2',
            '\u{00B3}' => '3',
            other => other,
        })
        .collect();
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(&stem))
}

fn is_bidi_or_zero_width(c: char) -> bool {
    matches!(
        c as u32,
        0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x2069 | 0xFEFF
    )
}

/// Create + verify the parent tree of `candidate` under the canonical `base`.
/// The single containment gate shared by every open mode ([`resolve_and_open`],
/// [`resolve_and_open_resumable`], [`resolve_and_open_overwrite`]): it proves no
/// real ancestor and the (created) parent resolves OUTSIDE the root before any
/// file is opened, so a pre-planted junction/symlink cannot redirect the write.
fn ensure_parent_in_root(candidate: &Path, base: &Path) -> Result<()> {
    if let Some(parent) = candidate.parent() {
        // [red-team #1] Check the DEEPEST EXISTING ancestor's real path BEFORE creating
        // anything — else a pre-planted junction inside the root would let create_dir_all
        // materialize directories outside the root.
        let mut anc = parent.to_path_buf();
        while !exists_nofollow(&anc) {
            match anc.parent() {
                Some(p) => anc = p.to_path_buf(),
                None => break,
            }
        }
        if exists_nofollow(&anc) {
            let real = anc
                .canonicalize()
                .map_err(|e| LanBeamError::UnsafePath(format!("canonicalize ancestor: {e}")))?;
            if !real.starts_with(base) {
                return Err(LanBeamError::UnsafePath("escapes download root".into()));
            }
        }
        std::fs::create_dir_all(parent)
            .map_err(|e| LanBeamError::UnsafePath(format!("create dirs: {e}")))?;
        let real_parent = parent
            .canonicalize()
            .map_err(|e| LanBeamError::UnsafePath(format!("canonicalize parent: {e}")))?;
        if !real_parent.starts_with(base) {
            return Err(LanBeamError::UnsafePath("escapes download root".into()));
        }
    }
    Ok(())
}

/// Post-open TOCTOU guard: the just-opened `target` must still resolve inside
/// `base`, else the caller must not write to it. Shared by every open mode.
fn opened_inside_root(target: &Path, base: &Path) -> bool {
    matches!(target.canonicalize(), Ok(real) if real.starts_with(base))
}

/// Write-time resolution: create the parent tree, verify containment, and open the
/// file with `create_new` (never follow/overwrite an existing symlink). De-dupes on
/// collision and retries if a concurrent transfer wins the race ([FIX-12]).
pub fn resolve_and_open(rel: &Path, download_root: &Path) -> Result<(PathBuf, File)> {
    let base = download_root
        .canonicalize()
        .map_err(|e| LanBeamError::UnsafePath(format!("download root: {e}")))?;
    let candidate = download_root.join(rel);
    ensure_parent_in_root(&candidate, &base)?;

    for _ in 0..10_000 {
        let target = dedupe_on_collision(&candidate);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(f) => {
                // [red-team #5] Close the check→open TOCTOU window: the opened file must
                // still resolve inside the root, else remove it and reject.
                if opened_inside_root(&target, &base) {
                    return Ok((target, f));
                }
                drop(f);
                let _ = std::fs::remove_file(&target);
                return Err(LanBeamError::UnsafePath("escapes download root".into()));
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue, // race → re-dedupe
            Err(e) => {
                return Err(LanBeamError::Io(format!(
                    "create {}: {e}",
                    target.display()
                )))
            }
        }
    }
    Err(LanBeamError::Io("too many name collisions".into()))
}

/// Overwrite-mode resolution (M6.5, `conflict = "overwrite"`): stream into a
/// de-duped TEMP sibling (exactly like [`resolve_and_open`]) and return it
/// together with the INTENDED final target `download_root.join(rel)`. The caller
/// renames temp → target only AFTER the whole transfer has succeeded and (for a
/// hashed file) verified.
///
/// WHY not open the target directly: the previous version unlinked the existing
/// file up front, so an interrupted or integrity-failed receive — whose cleanup
/// deletes the partial — destroyed BOTH the original and the half-written
/// replacement, silent permanent data loss from an ordinary interruption
/// (finding). Streaming into a temp means an abort can only ever delete the
/// temp; the user's existing file is untouched until the atomic replace.
///
/// Containment is inherited: the temp is opened through [`resolve_and_open`]'s
/// full guard, and the target's parent is the same in-root directory. The caller
/// replaces via `remove_file` + `rename`, and `remove_file` never follows a
/// symlink planted at the target, so overwrite still cannot clobber anything
/// OUTSIDE the download root.
pub fn resolve_and_open_overwrite(
    rel: &Path,
    download_root: &Path,
) -> Result<(PathBuf, File, PathBuf)> {
    let (temp, file) = resolve_and_open(rel, download_root)?;
    let target = download_root.join(rel);
    Ok((temp, file, target))
}

/// Resume-mode resolution (M6.4): open EXACTLY `download_root.join(disk_rel)`
/// for continued writing — the existing partial if present, else a fresh file —
/// WITHOUT truncating and WITHOUT a de-dupe suffix, because resume must land on
/// the very bytes we persisted. Returns the file plus its current on-disk length
/// (the authoritative resume point).
///
/// Security: the same parent-containment gate as every other mode, PLUS an
/// explicit refusal to resume through a symlink at the target — `create(true)`
/// would otherwise follow a dangling link and materialize a file outside the
/// root before the post-open check could reject it. `disk_rel` is always a path
/// WE authored in a prior session (from a validated manifest name), never
/// peer-supplied, so this only defends against another local process planting a
/// link at our partial path. `existing_len_hint` is informational; the returned
/// length is what the caller resumes from.
pub fn resolve_and_open_resumable(
    download_root: &Path,
    disk_rel: &Path,
    existing_len_hint: u64,
) -> Result<(PathBuf, File, u64)> {
    let base = download_root
        .canonicalize()
        .map_err(|e| LanBeamError::UnsafePath(format!("download root: {e}")))?;
    let target = download_root.join(disk_rel);
    ensure_parent_in_root(&target, &base)?;

    // Refuse to open THROUGH a symlink: unlike create_new (which fails on any
    // existing entry), create(true) follows a link and would create/write its
    // target. Drop the link's own metadata check before opening.
    if is_symlink(&target) {
        return Err(LanBeamError::UnsafePath(
            "refusing to resume through a symlink".into(),
        ));
    }
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&target)
        .map_err(|e| LanBeamError::Io(format!("open partial {}: {e}", target.display())))?;
    // Post-open containment (TOCTOU): the opened path must resolve inside root.
    // Reject WITHOUT touching the file if it escaped (it is a pre-existing entry,
    // not ours to delete).
    if !opened_inside_root(&target, &base) {
        drop(f);
        return Err(LanBeamError::UnsafePath("escapes download root".into()));
    }
    let actual = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = existing_len_hint; // the caller cross-checks against `actual`
    Ok((target, f, actual))
}

/// Symlink test (does NOT follow): a dangling link still reports `true`.
fn is_symlink(p: &Path) -> bool {
    p.symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Sanitize an untrusted display string (a peer name, a date) into a SINGLE
/// safe path component for an auto-organize subfolder (M6.6). Reuses the
/// per-component cleaner — path separators become `_`, traversal/reserved names
/// are neutralized — and falls back to `fallback` when nothing safe remains, so
/// the organize prefix can never introduce a separator or escape the root.
pub fn sanitize_component(name: &str, fallback: &str) -> String {
    clean_component(name).unwrap_or_else(|| fallback.to_string())
}

/// Existence test that does NOT follow symlinks (a dangling link still counts as present).
fn exists_nofollow(p: &Path) -> bool {
    p.symlink_metadata().is_ok()
}

fn dedupe_on_collision(candidate: &Path) -> PathBuf {
    if !exists_nofollow(candidate) {
        return candidate.to_path_buf();
    }
    let parent = candidate
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let stem = candidate
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = candidate
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    for i in 1..=10_000u32 {
        let cand = parent.join(format!("{stem} ({i}){ext}"));
        if !exists_nofollow(&cand) {
            return cand;
        }
    }
    parent.join(format!("{stem} (dup){ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Component;

    fn accept_path(name: &str) -> String {
        match validate(name) {
            Decision::Accept(p) => p.to_string_lossy().replace('\\', "/"),
            other => panic!("expected Accept for {name:?}, got {other:?}"),
        }
    }
    fn reject(name: &str) -> RejectReason {
        match validate(name) {
            Decision::Reject(r) => r,
            other => panic!("expected Reject for {name:?}, got {other:?}"),
        }
    }

    #[test]
    fn san_matrix_validate() {
        use RejectReason::*;
        // Benign + subdirectories (SAN-01..04)
        assert_eq!(accept_path("report.pdf"), "report.pdf");
        assert_eq!(accept_path("My Photo.jpg"), "My Photo.jpg");
        assert_eq!(accept_path("docs/report.txt"), "docs/report.txt");
        assert_eq!(accept_path("photos/2026/img.jpg"), "photos/2026/img.jpg");
        // Normalizations (SAN-05..09)
        assert_eq!(accept_path("a//b.txt"), "a/b.txt");
        assert_eq!(accept_path("./readme.md"), "readme.md");
        assert_eq!(accept_path("../x"), "x");
        assert_eq!(accept_path("a/../../x"), "a/x");
        assert_eq!(accept_path("..\\x"), "x");
        // Absolute / drive / UNC (SAN-10..14)
        assert_eq!(reject("/x"), AbsolutePath);
        assert_eq!(reject("/etc/passwd"), AbsolutePath);
        assert_eq!(reject("C:\\x"), DrivePrefix);
        assert_eq!(reject("C:x"), DrivePrefix);
        assert_eq!(reject("\\\\host\\share\\x"), AbsolutePath);
        // Trailing dot/space (SAN-15..16)
        assert_eq!(accept_path("evil.bat."), "evil.bat");
        assert_eq!(accept_path("evil.exe   "), "evil.exe");
        // Reserved names (SAN-17..19)
        assert_eq!(accept_path("CON"), "_CON");
        assert_eq!(accept_path("con.txt"), "_con.txt");
        assert_eq!(accept_path("LPT1"), "_LPT1");
        // NTFS ADS (SAN-20..21)
        assert_eq!(accept_path("a.txt:evil"), "a.txt_evil");
        assert_eq!(accept_path("a.txt::$DATA"), "a.txt__$DATA");
        // Null / empty (SAN-22..25)
        assert_eq!(reject("bad\0.txt"), NullByte);
        assert_eq!(reject(""), Empty);
        assert_eq!(reject("   "), Empty);
        assert_eq!(reject("..."), NoValidComponents);
        // Bidi spoof (SAN-26): U+202E stripped
        assert_eq!(accept_path("photo\u{202E}gnp.exe"), "photognp.exe");
        // Illegal chars (SAN-27)
        assert_eq!(accept_path("a<b>c*d?.txt"), "a_b_c_d_.txt");
        // Reserved $-names ([FIX-9])
        assert_eq!(accept_path("CLOCK$"), "_CLOCK$");
        assert_eq!(accept_path("CONIN$"), "_CONIN$");
    }

    // Red-team regressions: reserved-device stem normalization.
    #[test]
    fn reserved_stem_normalization() {
        // [#2] trailing space before the extension keeps the device stem
        assert_eq!(accept_path("NUL .txt"), "_NUL .txt");
        assert_eq!(accept_path("CON .txt"), "_CON .txt");
        // [#3] superscript COM¹/LPT² fold to the ASCII device digit
        assert!(accept_path("COM\u{00B9}").starts_with('_'));
        assert!(accept_path("LPT\u{00B2}.log").starts_with('_'));
        assert!(accept_path("com\u{00B3}").starts_with('_'));
        // unaffected: a normal name whose extension-less stem is not reserved
        assert_eq!(accept_path("summary.txt"), "summary.txt");
    }

    // SAN-29 — LanDrop's exact Startup-folder RCE payload. Every `..` is dropped; the
    // file is confined to a harmless nested tree under the download root.
    #[test]
    fn san29_startup_attack_is_neutralized() {
        let payload = "..\\..\\..\\..\\..\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup\\evil.exe";
        assert_eq!(
            accept_path(payload),
            "AppData/Roaming/Microsoft/Windows/Start Menu/Programs/Startup/evil.exe"
        );
        // and it has zero ParentDir components
        if let Decision::Accept(rel) = validate(payload) {
            assert!(rel.components().all(|c| matches!(c, Component::Normal(_))));
        }
    }

    // INV-1.1: resolve_and_open confines writes to the download root; dedup works.
    #[test]
    fn resolve_confines_and_dedupes() {
        let root = std::env::temp_dir().join(format!("lanbeam-san-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let base = root.canonicalize().unwrap();

        // benign nested path lands inside the root
        let rel = match validate("docs/report.txt") {
            Decision::Accept(p) => p,
            _ => panic!(),
        };
        let (p1, _f) = resolve_and_open(&rel, &root).unwrap();
        assert!(p1.canonicalize().unwrap().starts_with(&base));

        // SAN-29 payload resolves INSIDE the root (never the real Startup folder)
        let rel29 = match validate("..\\..\\..\\Startup\\evil.exe") {
            Decision::Accept(p) => p,
            _ => panic!(),
        };
        let (p29, _f) = resolve_and_open(&rel29, &root).unwrap();
        assert!(p29.canonicalize().unwrap().starts_with(&base));

        // collision de-dupes to "report (1).txt"
        let (p2, _f) = resolve_and_open(&rel, &root).unwrap();
        assert_ne!(p1, p2);
        assert!(p2.file_name().unwrap().to_string_lossy().contains("(1)"));

        let _ = std::fs::remove_dir_all(&root);
    }

    // M6.5 overwrite mode: resolution streams into a de-duped TEMP sibling and
    // reports the intended final target — it must NOT unlink the existing file
    // (the caller renames onto it only after a fully-verified transfer, so an
    // aborted overwrite can never destroy the original — finding).
    #[test]
    fn resolve_overwrite_opens_temp_and_leaves_original() {
        let root = std::env::temp_dir().join(format!("lanbeam-ow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let base = root.canonicalize().unwrap();

        let rel = match validate("docs/report.txt") {
            Decision::Accept(p) => p,
            _ => panic!(),
        };
        // Seed an existing file whose bytes must survive resolution.
        let (p1, _f) = resolve_and_open(&rel, &root).unwrap();
        std::fs::write(&p1, b"old content").unwrap();

        let (temp, _f2, target) = resolve_and_open_overwrite(&rel, &root).unwrap();
        // Streams into a SIBLING temp, never the original path.
        assert_ne!(
            temp, p1,
            "overwrite must stream into a temp, not the original"
        );
        assert!(temp.canonicalize().unwrap().starts_with(&base));
        // The reported target IS the original path (the caller renames onto it).
        assert_eq!(target, p1, "the final target is the requested path");
        // Crucially, the original is untouched — no up-front unlink.
        assert_eq!(
            std::fs::read(&p1).unwrap(),
            b"old content",
            "overwrite resolution must not destroy the existing file"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    // M6.4 resume mode: opens the existing partial and reports its length; a
    // second open of a fresh name creates it at length 0.
    #[test]
    fn resolve_resumable_opens_existing_and_reports_length() {
        let root = std::env::temp_dir().join(format!("lanbeam-rs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let base = root.canonicalize().unwrap();

        let rel = match validate("sub/partial.bin") {
            Decision::Accept(p) => p,
            _ => panic!(),
        };
        // No file yet → resumable creates it, length 0.
        let (path, _f, len0) = resolve_and_open_resumable(&root, &rel, 0).unwrap();
        assert_eq!(len0, 0);
        assert!(path.canonicalize().unwrap().starts_with(&base));
        // Seed some bytes, reopen → the reported length is the on-disk length.
        std::fs::write(&path, vec![9u8; 4096]).unwrap();
        let (path2, _f2, len1) = resolve_and_open_resumable(&root, &rel, 4096).unwrap();
        assert_eq!(path, path2);
        assert_eq!(
            len1, 4096,
            "resumable must report the existing partial length"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    // The organize-prefix sanitizer yields exactly one safe component (or the
    // fallback), never a separator or traversal.
    #[test]
    fn sanitize_component_yields_one_safe_token() {
        assert_eq!(sanitize_component("John's PC", "device"), "John's PC");
        // separators collapse to underscores → still a single component
        assert_eq!(sanitize_component("a/b\\c", "device"), "a_b_c");
        assert_eq!(
            sanitize_component("..", "device"),
            "device",
            "traversal falls back"
        );
        assert_eq!(
            sanitize_component("   ", "device"),
            "device",
            "blank falls back"
        );
        assert_eq!(sanitize_component("2026-07-12", "date"), "2026-07-12");
        // reserved device stem is neutralized, never returned bare
        assert!(sanitize_component("CON", "device").starts_with('_'));
    }

    // Length-limit rejects: total-length gate fires before splitting (line 59).
    #[test]
    fn reject_total_length_over_limit() {
        use RejectReason::*;
        // One oversized token, no separators → total-length check trips first.
        let huge = "a".repeat(MAX_TOTAL_LEN + 1);
        assert_eq!(reject(&huge), TooLong);
    }

    // Per-component length gate (line 78): total stays under the total cap, but a
    // single surviving component exceeds MAX_COMPONENT_LEN.
    #[test]
    fn reject_single_component_over_limit() {
        use RejectReason::*;
        let long_comp = "b".repeat(MAX_COMPONENT_LEN + 1);
        assert_eq!(long_comp.len(), MAX_COMPONENT_LEN + 1);
        assert!(long_comp.len() <= MAX_TOTAL_LEN);
        assert_eq!(reject(&long_comp), TooLong);
        // A component exactly at the per-component limit is accepted.
        let at_comp_limit = "b".repeat(MAX_COMPONENT_LEN);
        assert_eq!(accept_path(&at_comp_limit), at_comp_limit);
    }

    // Too-many-components gate (line 88): 17 survivors exceed MAX_COMPONENTS = 16.
    #[test]
    fn reject_too_many_components() {
        use RejectReason::*;
        let over = vec!["a"; MAX_COMPONENTS + 1].join("/");
        assert_eq!(reject(&over), TooManyComponents);
        // Exactly MAX_COMPONENTS survivors is accepted.
        let at = vec!["a"; MAX_COMPONENTS].join("/");
        assert_eq!(accept_path(&at), at);
        // Empty / dot components between separators are dropped, so they don't count.
        let with_dots = "a/./a/./a"; // 3 real survivors
        assert_eq!(accept_path(with_dots), "a/a/a");
    }

    // dedupe_on_collision: an extension-less name de-dupes to "name (1)", "name (2)"
    // on repeated collisions — exercises the numbered-suffix loop with an empty ext.
    #[test]
    fn resolve_dedupes_extensionless_name() {
        let root = std::env::temp_dir().join(format!("lanbeam-ddx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let base = root.canonicalize().unwrap();

        let rel = match validate("README") {
            Decision::Accept(p) => p,
            _ => panic!(),
        };
        let (p1, _f1) = resolve_and_open(&rel, &root).unwrap();
        assert!(p1.canonicalize().unwrap().starts_with(&base));
        assert_eq!(p1.file_name().unwrap().to_string_lossy(), "README");

        let (p2, _f2) = resolve_and_open(&rel, &root).unwrap();
        assert_ne!(p1, p2);
        assert_eq!(p2.file_name().unwrap().to_string_lossy(), "README (1)");

        let (p3, _f3) = resolve_and_open(&rel, &root).unwrap();
        assert_ne!(p2, p3);
        assert_eq!(p3.file_name().unwrap().to_string_lossy(), "README (2)");

        let _ = std::fs::remove_dir_all(&root);
    }

    // dedupe preserves the extension across the numbered suffix (stem/ext split).
    #[test]
    fn resolve_dedupe_preserves_extension() {
        let root = std::env::temp_dir().join(format!("lanbeam-ddext-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let rel = match validate("photo.jpg") {
            Decision::Accept(p) => p,
            _ => panic!(),
        };
        let (_p1, _f1) = resolve_and_open(&rel, &root).unwrap();
        let (p2, _f2) = resolve_and_open(&rel, &root).unwrap();
        assert_eq!(p2.file_name().unwrap().to_string_lossy(), "photo (1).jpg");

        let _ = std::fs::remove_dir_all(&root);
    }

    // resumable open onto a name with a subdirectory prefix creates the parent tree
    // and reports the growing on-disk length across reopens (M6.4 authoritative point).
    #[test]
    fn resolve_resumable_reports_partial_growth() {
        let root = std::env::temp_dir().join(format!("lanbeam-rsg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let base = root.canonicalize().unwrap();

        let rel = match validate("nested/deep/part.bin") {
            Decision::Accept(p) => p,
            _ => panic!(),
        };
        let (path, _f, len0) = resolve_and_open_resumable(&root, &rel, 0).unwrap();
        assert_eq!(len0, 0);
        assert!(path.canonicalize().unwrap().starts_with(&base));

        std::fs::write(&path, vec![1u8; 123]).unwrap();
        // The hint is informational; the returned length is the true on-disk length.
        let (path2, _f2, len1) = resolve_and_open_resumable(&root, &rel, 999).unwrap();
        assert_eq!(path, path2);
        assert_eq!(len1, 123);

        let _ = std::fs::remove_dir_all(&root);
    }

    use proptest::prelude::*;
    proptest! {
        // AC-SAN-INV: for ANY input, an Accept is always a relative path made of only
        // Normal components — so join(root, rel) can never escape the download root.
        #[test]
        fn accept_is_relative_and_normal_only(s in any::<String>()) {
            if let Decision::Accept(rel) = validate(&s) {
                prop_assert!(rel.is_relative());
                for c in rel.components() {
                    prop_assert!(matches!(c, Component::Normal(_)), "non-normal component: {:?}", c);
                }
                prop_assert!(rel.components().count() <= MAX_COMPONENTS);
            }
        }

        // AC-SAN-INV (resume): the resumable open must uphold the SAME containment
        // invariant as create_new — for ANY input that validates, the opened
        // partial resolves INSIDE the download root. The organize prefix is folded
        // in too (a sanitized component), mirroring the real receive path.
        #[test]
        fn resumable_open_stays_inside_root(s in any::<String>(), org in any::<String>()) {
            if let Decision::Accept(rel) = validate(&s) {
                let root = std::env::temp_dir()
                    .join(format!("lanbeam-rsprop-{}-{}", std::process::id(), rand_tag()));
                let _ = std::fs::create_dir_all(&root);
                if let Ok(base) = root.canonicalize() {
                    // Prefix like organize=device would: one sanitized component.
                    let prefix = sanitize_component(&org, "device");
                    let disk_rel = Path::new(&prefix).join(&rel);
                    if let Ok((path, _f, _len)) = resolve_and_open_resumable(&root, &disk_rel, 0) {
                        let real = path.canonicalize().expect("opened file canonicalizes");
                        prop_assert!(real.starts_with(&base), "resume escaped root: {:?}", real);
                    }
                }
                let _ = std::fs::remove_dir_all(&root);
            }
        }
    }

    /// A cheap unique tag so concurrent proptest cases don't collide on one temp
    /// dir (proptest runs many cases per test; the pid alone is not enough).
    fn rand_tag() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }
}
