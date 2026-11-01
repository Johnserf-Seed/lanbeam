//! The receive side: reading and checking the manifest, the reply (with
//! resume offsets, M6.4), and the receive engine that streams, verifies and
//! commits each file.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use super::pacing::{wait_while_paused, RateLimiter};
use super::progress::FileEvent;
use super::send_list::to_hex;
use super::wire::{recv_control, recv_frame, send_control};
use super::Manifest;
use crate::consts::{
    CHUNK_IDLE_TIMEOUT, MAX_MANIFEST_FILES, MAX_MANIFEST_NAME_BYTES, MAX_MANIFEST_NAME_TOTAL,
    RECEIVE_SYNC_INTERVAL, TRANSFER_V3,
};
use crate::error::{LanBeamError, Result};
use crate::partials::FileIdentity;
use crate::protocol::{AppMessage, Frame, ResumeOffset};
use crate::sanitize::{self, Decision};
use crate::state::{ConflictAction, TransferControl};
use crate::transport::noise::NoiseSession;
use crate::trust;

/// Read the paged manifest (FileListBegin → entries → FileListEnd).
pub async fn read_manifest(session: &mut NoiseSession) -> Result<Manifest> {
    match recv_control(session).await? {
        AppMessage::FileListBegin {
            transfer_id,
            total_size,
            file_count,
        } => read_manifest_entries(session, transfer_id, total_size, file_count).await,
        other => Err(LanBeamError::Protocol(format!(
            "expected manifest, got {}",
            trust::clamp_peer_text(&format!("{other:?}"))
        ))),
    }
}

/// The entries half of the manifest read — split out because `open_inbound`
/// has already consumed the `FileListBegin` when it dispatches here (M4.2).
pub(super) async fn read_manifest_entries(
    session: &mut NoiseSession,
    transfer_id: String,
    total_size: u64,
    file_count: u32,
) -> Result<Manifest> {
    // Bound the peer-controlled transfer_id BEFORE building the manifest, the
    // same way the entry names below are bounded: this runs pre-trust (the XX
    // handshake accepts any static key), and the id is echoed back in the
    // FileSendReply frame, used as registry keys, and carried in every emitted
    // event — an unbounded id would risk overflowing the reply frame and bloat
    // those maps/payloads before any trust decision (M4.5).
    if transfer_id.len() > MAX_MANIFEST_NAME_BYTES {
        return Err(LanBeamError::Protocol(format!(
            "manifest transfer_id is {} bytes (limit {MAX_MANIFEST_NAME_BYTES})",
            transfer_id.len()
        )));
    }
    // Reject a hostile file_count BEFORE any allocation: the XX handshake accepts
    // any static key, so this code runs pre-trust — an attacker-declared count
    // must not size a reservation or flood the accept prompt (M4.5).
    if file_count > MAX_MANIFEST_FILES {
        return Err(LanBeamError::Protocol(format!(
            "manifest declares {file_count} files (limit {MAX_MANIFEST_FILES})"
        )));
    }
    // Capacity is a hint, not a promise: even an in-limit declaration only
    // pre-reserves up to 1024 entries — the Vec grows normally past that.
    let mut files = Vec::with_capacity((file_count as usize).min(1024));
    // The count clamp alone doesn't bound memory: each entry frame can carry a
    // ~65 KB name, so 10k in-limit entries could still materialize ~650 MB
    // pre-trust. Enforce a per-name cap (nothing longer could ever pass the
    // sanitizer anyway) AND a cumulative byte budget across the manifest,
    // both checked BEFORE the entry is kept (M4.5).
    let mut name_bytes = 0usize;
    loop {
        match recv_control(session).await? {
            AppMessage::FileListEntry { file } => {
                if file.name.len() > MAX_MANIFEST_NAME_BYTES {
                    return Err(LanBeamError::Protocol(format!(
                        "manifest entry name is {} bytes (limit {MAX_MANIFEST_NAME_BYTES})",
                        file.name.len()
                    )));
                }
                // Bound the peer-supplied hash too (finding): the name budget
                // caps `file.name` but NOT `file.sha256`, so 10k in-limit entries
                // could each carry a ~64 KB sha256 string and materialize ~640 MB
                // pre-trust — an unauthenticated memory-amplification DoS. A real
                // SHA-256 is exactly 64 lowercase-hex chars, and the receive loop
                // compares against a 64-char computed hex, so anything longer
                // could never verify. A fixed 64-char cap needs no cumulative
                // accounting (M4.5).
                if file.sha256.as_deref().is_some_and(|h| h.len() > 64) {
                    return Err(LanBeamError::Protocol(
                        "manifest entry sha256 exceeds 64 bytes".into(),
                    ));
                }
                name_bytes += file.name.len();
                if name_bytes > MAX_MANIFEST_NAME_TOTAL {
                    return Err(LanBeamError::Protocol(format!(
                        "manifest name bytes exceed budget (limit {MAX_MANIFEST_NAME_TOTAL})"
                    )));
                }
                files.push(file)
            }
            AppMessage::FileListEnd { .. } => break,
            other => {
                return Err(LanBeamError::Protocol(format!(
                    "bad manifest item {}",
                    trust::clamp_peer_text(&format!("{other:?}"))
                )))
            }
        }
        if files.len() > file_count as usize {
            return Err(LanBeamError::Protocol("manifest overran file_count".into()));
        }
    }
    // The total is the sum of the entries (L17). The declared one is what the
    // accept prompt and the notification show, and nothing tied it to the
    // entries: a sender could ask for "1 KB" and list gigabytes. The per-file
    // sizes are what the receive actually enforces, so they are the truth.
    let listed = files.iter().fold(0u64, |sum, f| sum.saturating_add(f.size));
    if listed != total_size {
        log::warn!(
            "manifest {transfer_id}: declares {total_size} bytes, its files add up to {listed}"
        );
    }
    Ok(Manifest {
        transfer_id,
        total_size: listed,
        files,
    })
}

/// Why a receiver said no, as it travels in `FileSendReply.reason`. Wire values —
/// older builds send exactly these strings — written by the receiver below and
/// read back by the sender in [`peer_reply_error`], so the two cannot drift.
pub(crate) const REPLY_DECLINED: &str = "declined";
pub(crate) const REPLY_BUSY: &str = "busy";
pub(crate) const REPLY_UNSAFE_NAME: &str = "unsafe filename";
pub(crate) const REPLY_DUPLICATE_ID: &str = "duplicate transfer id";
/// The receiver accepted, then cancelled while the transfer waited for a
/// concurrency slot (L15). An older sender reads it as a plain refusal.
pub(crate) const REPLY_CANCELLED: &str = "cancelled";

/// Where a request stands on the receiver while its sender waits for the reply
/// (F4), as it travels in `AppMessage::Pending.state` — written by the receiver
/// ([`send_pending`]) and read back by the sender ([`PendingState::from_wire`]),
/// so the two cannot drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingState {
    /// On screen there, waiting for a person to answer.
    Prompted,
    /// Accepted there, waiting for one of its transfer slots to free up.
    Queued,
}

impl PendingState {
    pub(super) fn as_wire(self) -> &'static str {
        match self {
            PendingState::Prompted => "prompted",
            PendingState::Queued => "queued",
        }
    }

    /// The state a token names — `None` for one a later build sends, which the
    /// sender simply doesn't show.
    pub(super) fn from_wire(token: &str) -> Option<Self> {
        [PendingState::Prompted, PendingState::Queued]
            .into_iter()
            .find(|s| s.as_wire() == token)
    }
}

/// Tell a waiting sender where its request stands (F4) — if it negotiated v3.
/// An older sender is waiting for exactly the reply and would fail on anything
/// else, so it is told nothing. Best-effort: a sender that has gone is noticed
/// by the wait that follows, not here.
pub(super) async fn send_pending(
    session: &mut NoiseSession,
    peer_version: u8,
    transfer_id: &str,
    state: PendingState,
) {
    if peer_version < TRANSFER_V3 {
        return;
    }
    let msg = AppMessage::Pending {
        transfer_id: transfer_id.to_string(),
        state: state.as_wire().to_string(),
    };
    if let Err(e) = send_control(session, &msg).await {
        log::debug!(
            "{transfer_id}: could not say it is {}: {e}",
            state.as_wire()
        );
    }
}

/// What a receiver's "no" means on the sending side (H6). Only a person
/// declining is `Rejected` — the UI then drops the row and says "they
/// declined". It used to be every refusal: a receiver at its concurrency limit
/// or one refusing a file name read as a person saying no. A reason this build
/// doesn't know (or none, from a legacy receiver) is still a refusal.
pub(crate) fn peer_reply_error(reason: Option<&str>) -> LanBeamError {
    match reason {
        Some(REPLY_BUSY) => LanBeamError::PeerBusy,
        Some(REPLY_UNSAFE_NAME) => {
            LanBeamError::UnsafePath("the receiver refused a file name".into())
        }
        Some(REPLY_DUPLICATE_ID) => {
            LanBeamError::Protocol("the receiver already has a transfer with this id".into())
        }
        Some(REPLY_CANCELLED) => LanBeamError::PeerCancelled,
        _ => LanBeamError::Rejected,
    }
}

/// What a receiver's failed `TransferAck` means on the sending side (H5). The
/// token is the receiver's own `ui_code()` — deliberately nothing more detailed,
/// so no local paths cross over. It used to become `Protocol("receiver error:
/// …")`, i.e. "the connection failed", whatever the receiver actually hit: a
/// corrupted file, a full disk and a cancel all read the same.
pub(crate) fn peer_ack_error(token: Option<&str>) -> LanBeamError {
    match token {
        Some("integrity") => LanBeamError::PeerIntegrity,
        Some("io") => LanBeamError::PeerIo,
        Some("cancelled") => LanBeamError::PeerCancelled,
        Some("timeout") => LanBeamError::Timeout,
        Some("unsafe_path") => {
            LanBeamError::UnsafePath("the receiver could not place a file safely".into())
        }
        // Clamp the peer-authored token before it reaches a log (finding): a
        // hostile receiver could otherwise ack with a ~64 KB error string.
        other => LanBeamError::Protocol(format!(
            "receiver error: {}",
            trust::clamp_peer_text(other.unwrap_or_default())
        )),
    }
}

/// Every declared name must pass the pure sanitizer BEFORE we accept the transfer.
pub fn validate_manifest(m: &Manifest) -> Result<()> {
    for f in &m.files {
        if let Decision::Reject(r) = sanitize::validate(&f.name) {
            return Err(LanBeamError::UnsafePath(format!("{}: {:?}", f.name, r)));
        }
    }
    Ok(())
}

/// Send a reply. On reject the transfer ends here. The accept path carries no
/// resume offsets — use [`send_reply_full`] when continuing partials (M6.4).
pub async fn send_reply(
    session: &mut NoiseSession,
    transfer_id: &str,
    accept: bool,
    reason: Option<String>,
) -> Result<()> {
    send_reply_full(session, transfer_id, accept, reason, None).await
}

/// Send a reply with optional SPARSE per-file resume offsets (M6.4): each
/// [`ResumeOffset`] says how many bytes we already hold for one manifest file,
/// so the sender streams only its tail. `None` = start every file at 0 (the
/// send side treats an omitted field identically, keeping legacy peers
/// compatible).
async fn send_reply_full(
    session: &mut NoiseSession,
    transfer_id: &str,
    accept: bool,
    reason: Option<String>,
    offsets: Option<Vec<ResumeOffset>>,
) -> Result<()> {
    send_control(
        session,
        &AppMessage::FileSendReply {
            transfer_id: transfer_id.to_string(),
            accept,
            reason,
            offsets,
        },
    )
    .await
}

/// Per-file receive plan (M6.4/6.5), one entry per manifest file IN ORDER:
/// either continue a persisted partial from an offset, or write a fresh file
/// resolving a name collision with the chosen action.
pub(super) enum FileDisposition {
    /// Continue the partial at `disk_rel` (relative to the download root) from
    /// `offset` bytes — reopened as-is, its prefix folded back into the hasher.
    /// `commit_name`: an overwrite's temp copy, to be renamed onto this name in
    /// the same folder once the transfer succeeds (see `PartialRecord`).
    Resume {
        disk_rel: PathBuf,
        offset: u64,
        commit_name: Option<String>,
    },
    /// A new file at `rel` (auto-organize prefix already applied) — resolve a
    /// collision with `conflict`.
    Fresh {
        rel: PathBuf,
        conflict: ConflictAction,
    },
}

impl FileDisposition {
    /// Bytes the receiver already holds — 0 for a fresh file — which rides
    /// `FileSendReply.offsets` so the sender streams only the tail.
    fn offset(&self) -> u64 {
        match self {
            FileDisposition::Resume { offset, .. } => *offset,
            FileDisposition::Fresh { .. } => 0,
        }
    }
}

/// JSON byte budget for the sparse resume-offset array in a `FileSendReply`
/// (M6.4). Well under the control-frame cap (`MAX_PLAINTEXT` 65519) so the rest
/// of the reply JSON (transfer_id, flags, array punctuation) always fits too.
const RESUME_REPLY_BUDGET: usize = 60_000;

/// Decimal digit count of `n` (`0` → 1) — used to size a resume pair's JSON.
fn dec_digits(mut n: u64) -> usize {
    let mut d = 1;
    while n >= 10 {
        n /= 10;
        d += 1;
    }
    d
}

/// Encode the receiver's per-file resume offsets SPARSELY for the reply (M6.4).
///
/// `eff_offsets[i]` is what file `i` should resume from; only the non-zero
/// entries ride the wire as `(index, offset)` pairs, so a dense O(file_count)
/// vector never overflows the single control frame the reply is (finding: a
/// ~9000-file reply as `Vec<u64>` blew past 65535 and errored the whole
/// transfer, wiping every persisted partial). If even the sparse set would
/// exceed [`RESUME_REPLY_BUDGET`], the overflow files are DROPPED to a fresh
/// start: their `eff_offsets` entry is zeroed IN PLACE — so the receive loop
/// restarts them from 0 in lockstep with what the sender is told — rather than
/// erroring. That costs those files a full re-transfer, never a broken transfer.
pub(super) fn resume_offsets_for_reply(eff_offsets: &mut [u64]) -> Option<Vec<ResumeOffset>> {
    let mut pairs: Vec<ResumeOffset> = Vec::new();
    let mut used = 0usize;
    for (i, off) in eff_offsets.iter_mut().enumerate() {
        if *off == 0 {
            continue;
        }
        // Worst-case JSON for `{"index":I,"offset":O}` plus a `,` separator:
        // the two keys + braces are 21 fixed bytes, the rest is the digits.
        let cost = 21 + dec_digits(i as u64) + dec_digits(*off);
        if used + cost > RESUME_REPLY_BUDGET {
            // No room — restart this file fresh (mirrored locally by zeroing it).
            *off = 0;
            continue;
        }
        used += cost;
        pairs.push(ResumeOffset {
            index: i as u32,
            offset: *off,
        });
    }
    (!pairs.is_empty()).then_some(pairs)
}

/// One file's write progress through a receive, tracked so the error path can
/// persist a resumable partial (M6.4) or clean up. An entry is pushed when a
/// file is opened; on error the accumulator is PRUNED to exactly the files left
/// on disk, so `handle_incoming` persists precisely those.
pub(super) struct FileProgress {
    /// Matches a future manifest entry (sanitized rel + size + hash).
    pub(super) identity: FileIdentity,
    /// On-disk path relative to the download root — reopened verbatim on resume.
    pub(super) disk_rel: String,
    /// Absolute path, for cleanup of a non-resumable partial.
    path: PathBuf,
    /// Resume is only offered for hashed files; a non-hashed partial is deleted
    /// on interruption exactly as before M6.4.
    pub(super) hashed: bool,
    pub(super) bytes_written: u64,
    /// Set while `path` is an overwrite's temp copy: the name, in the same
    /// folder, it replaces once the whole transfer succeeds.
    pub(super) commit_name: Option<String>,
    /// Fully written, synced, and (if hashed) digest-matched. Such a file is
    /// whole, not partial — the error path leaves it on disk instead of deleting
    /// it because a LATER file in the same batch went bad. (Under `overwrite`
    /// "whole" still means "whole, in a temp file": that commit is all-or-nothing
    /// and only happens once the entire transfer succeeds.)
    done: bool,
}

/// Accept + receive with the PRE-M6 semantics: every file fresh, de-dupe on
/// collision, no resume. Kept as the stable entry point for direct callers
/// (unit tests, internal); `handle_incoming` builds a richer plan (auto-organize
/// prefix, conflict policy, resume analysis) and drives [`run_receive_core`].
pub async fn run_receive<F: FnMut(u64, u64)>(
    session: &mut NoiseSession,
    manifest: &Manifest,
    download_root: &Path,
    ctl: &TransferControl,
    on_progress: F,
) -> Result<Vec<PathBuf>> {
    let mut plan = Vec::with_capacity(manifest.files.len());
    for f in &manifest.files {
        match sanitize::validate(&f.name) {
            Decision::Accept(rel) => plan.push(FileDisposition::Fresh {
                rel,
                conflict: ConflictAction::Rename,
            }),
            Decision::Reject(r) => {
                return Err(LanBeamError::UnsafePath(format!("{}: {:?}", f.name, r)))
            }
        }
    }
    let mut progress = Vec::new();
    let mut opened = Vec::new();
    // The pre-M6 entry point: no throttle, no per-file events (production
    // receives go through `run_receive_core` directly from `handle_incoming`).
    run_receive_core(
        session,
        manifest,
        download_root,
        ctl,
        &plan,
        &mut progress,
        &mut opened,
        None,
        &mut |_| {},
        on_progress,
    )
    .await
}

/// The receive engine: reply (with resume `offsets` derived from `plan`), stream
/// each file per its disposition, verify, ack, and return the saved paths.
///
/// `ctl` carries the session's cancel token + pause flag (M6.1/6.2): each chunk
/// parks while paused (no reads → TCP backpressure stalls the sender), and a
/// cancel ends the loop with `Cancelled`.
///
/// `progress` is filled as files are written and, on error, PRUNED to the files
/// still on disk: a hashed file survives an INTERRUPTION (kept for resume, M6.4),
/// while a non-hashed file — or ANY file on a data-integrity failure — is
/// deleted, matching the pre-M6.4 all-or-nothing cleanup. The caller reads the
/// pruned accumulator to persist exactly those partials.
///
/// `opened` is a NON-pruned out-accumulator of every file's identity this
/// session reached — tried to open — in order. The caller uses it as the
/// `remove` set when clearing stale partial records on the error path — scoped
/// to files this session reached, so a still-valid partial for a file the
/// session never got to is left intact rather than wiped (finding-1). A file
/// whose open FAILED is in it too (L16): a partial that can't be reopened kept
/// its record, so every retry offered to resume it, failed the same way, and
/// never got past it.
///
/// `rate_limit` throttles the read loop (M6.7, `None` = unlimited) and `on_file`
/// receives a [`FileEvent`] as each file streams and verifies (M6.8).
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_receive_core<F: FnMut(u64, u64)>(
    session: &mut NoiseSession,
    manifest: &Manifest,
    download_root: &Path,
    ctl: &TransferControl,
    plan: &[FileDisposition],
    progress: &mut Vec<FileProgress>,
    opened: &mut Vec<FileIdentity>,
    rate_limit: Option<u64>,
    // `+ Send` for the same reason as `run_send`: the receive task is spawned.
    on_file: &mut (dyn FnMut(FileEvent<'_>) + Send),
    mut on_progress: F,
) -> Result<Vec<PathBuf>> {
    // Effective per-file resume offsets, then the SPARSE wire form — which may
    // zero an entry that would overflow the reply frame, restarting that file
    // fresh (see `resume_offsets_for_reply`). The loop below reads `offsets`, so
    // any drop is honored locally in lockstep with what the sender is told.
    let mut offsets: Vec<u64> = plan.iter().map(FileDisposition::offset).collect();
    let sparse = resume_offsets_for_reply(&mut offsets);
    send_reply_full(session, &manifest.transfer_id, true, None, sparse).await?;

    let mut limiter = RateLimiter::new(rate_limit);
    let mut saved = Vec::new();
    // Overwrite-mode files stream into a de-duped temp sibling and are renamed
    // onto the real target only if the WHOLE transfer succeeds (see the Ok arm),
    // so an interruption or integrity failure can only ever delete the temp —
    // never the user's existing file (finding: no up-front unlink).
    let mut overwrite_renames: Vec<(PathBuf, PathBuf)> = Vec::new();
    // Bytes already on disk (resumed prefixes) count as delivered, so the bar
    // resumes where it left off rather than restarting at 0.
    let mut received: u64 = offsets.iter().sum::<u64>().min(manifest.total_size);
    let result: Result<()> = async {
        for (i, f) in manifest.files.iter().enumerate() {
            // The file's stable identity — how a FUTURE manifest matches this
            // partial. Recomputed from the (already-validated) manifest name so
            // it is independent of the auto-organize prefix.
            let identity_rel = match sanitize::validate(&f.name) {
                Decision::Accept(p) => p.to_string_lossy().replace('\\', "/"),
                Decision::Reject(r) => {
                    return Err(LanBeamError::UnsafePath(format!("{}: {:?}", f.name, r)))
                }
            };
            let identity = FileIdentity {
                rel: identity_rel,
                size: f.size,
                sha256: f.sha256.clone(),
            };
            let hashed = f.sha256.is_some();
            // Reached: on any failure from here, this file's partial record is
            // dropped and the next attempt starts it fresh (L16).
            opened.push(identity.clone());

            // Open per the plan. Resume reopens the exact recorded partial; a
            // fresh file resolves its collision (rename de-dupes, overwrite
            // replaces) — both uphold the sanitizer's containment invariant.
            let (path, disk_rel, std_file, start_offset, commit_name) = match &plan[i] {
                FileDisposition::Resume {
                    disk_rel,
                    offset: recorded,
                    commit_name,
                } => {
                    // Use the EFFECTIVE offset (`resume_offsets_for_reply` may have
                    // zeroed it to a fresh restart), not the plan's original, so a
                    // dropped file opens its existing bytes and rewrites from 0 —
                    // matching the 0 the sender was told for it.
                    let offset = offsets[i];
                    let (path, file, actual) =
                        sanitize::resolve_and_open_resumable(download_root, disk_rel, offset)?;
                    // The partial's on-disk length must still equal what analysis
                    // RECORDED. Any drift — a shrink OR a growth — means another
                    // local process touched the file during the accept window: a
                    // shrink can't satisfy the promised offset, and a GROWTH is
                    // worse (finding) — the hasher folds only the first `offset`
                    // bytes and the tail streams into `[offset, size)`, so bytes
                    // past `size` survive UNHASHED yet the whole-file SHA-256 still
                    // passes, silently voiding the M6.3 integrity guarantee. Fail
                    // as an interruption so the kept partial is retried, never
                    // saved oversized-but-"verified". Compare against `recorded`,
                    // NOT the effective `offset`: the zeroed restart above
                    // legitimately reopens a nonzero partial with offset 0, and
                    // there `actual == recorded` still holds.
                    if actual != *recorded {
                        return Err(LanBeamError::Io(format!(
                            "partial {} changed on disk ({actual} bytes) since analysis ({recorded})",
                            path.display()
                        )));
                    }
                    // The temp copy of an interrupted overwrite: still owed its
                    // rename onto the file it replaces (M9). Only ever a sibling
                    // — one clean name component, joined to the partial's own
                    // folder — so the containment the open above proved for that
                    // folder covers the rename target too.
                    let commit_name = commit_name.as_deref().filter(|n| is_single_clean_name(n));
                    if let Some(name) = commit_name {
                        overwrite_renames.push((path.clone(), path.with_file_name(name)));
                    }
                    (
                        path,
                        disk_rel.to_string_lossy().replace('\\', "/"),
                        file,
                        offset,
                        commit_name.map(String::from),
                    )
                }
                FileDisposition::Fresh { rel, conflict } => {
                    let (path, file, commit_name) = match conflict {
                        ConflictAction::Overwrite => {
                            // Stream into a de-duped temp sibling; the replace of
                            // the real target happens only in the success arm, so
                            // an abort never destroys the original (finding).
                            let (temp, file, target) =
                                sanitize::resolve_and_open_overwrite(rel, download_root)?;
                            // Nothing there to replace, and the "temp" IS the
                            // target: a file like any other, not one waiting on
                            // a commit. Treated as a temp, an interruption deleted
                            // it even after it had arrived whole (M1).
                            let commit_name = if temp != target {
                                let name = target
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned());
                                overwrite_renames.push((temp.clone(), target));
                                name
                            } else {
                                None
                            };
                            (temp, file, commit_name)
                        }
                        ConflictAction::Rename => {
                            let (path, file) = sanitize::resolve_and_open(rel, download_root)?;
                            (path, file, None)
                        }
                    };
                    (
                        path.clone(),
                        disk_rel_under_root(download_root, &path),
                        file,
                        0u64,
                        commit_name,
                    )
                }
            };

            let mut out = tokio::fs::File::from_std(std_file);
            // Track progress for this file BEFORE the first byte, so an
            // interruption right after open still records the partial (M6.4).
            progress.push(FileProgress {
                identity,
                disk_rel,
                path: path.clone(),
                hashed,
                bytes_written: start_offset,
                commit_name,
                done: false,
            });
            let pi = progress.len() - 1;

            // Integrity digest (M6.3) covers the WHOLE file. On resume the
            // pre-existing prefix is read back and folded in first, so the final
            // compare is over `old-bytes + tail`.
            let mut hasher = f.sha256.as_ref().map(|_| Sha256::new());
            if start_offset > 0 {
                if let Some(h) = hasher.as_mut() {
                    out.seek(SeekFrom::Start(0)).await?;
                    fold_prefix(&mut out, start_offset, h).await?;
                }
                // Position the write cursor at the resume point regardless of
                // whether we folded (a non-hashed resume never happens, but be
                // explicit): writes must APPEND, never overwrite from 0.
                out.seek(SeekFrom::Start(start_offset)).await?;
            }

            let mut remaining = f.size.saturating_sub(start_offset);
            // Bytes written since the last sync — drained every
            // RECEIVE_SYNC_INTERVAL so the final sync_all covers only the last
            // window, not minutes of dirty page cache on slow media.
            let mut unsynced = 0u64;
            while remaining > 0 {
                // Pause gate (M6.2): stop reading while paused → the socket's
                // receive window fills → the sender stalls on backpressure. The
                // park is bounded (finding-4): an auto-resume at the cap moves a
                // chunk before the sender's blocked `write_msg` idle-times-out,
                // and the UI is told so the row un-pauses.
                if wait_while_paused(ctl).await? {
                    on_file(FileEvent::PauseAutoResumed);
                }
                // Idle-bounded: the sender paces the stream, but a peer that
                // stalls (or over-declared f.size) must not wedge this loop
                // forever — Timeout ends the transfer via the error path (M4.5).
                // Cancel (M6.1) wins over the read (biased).
                let frame = tokio::select! {
                    biased;
                    _ = ctl.cancel.cancelled() => return Err(LanBeamError::Cancelled),
                    r = tokio::time::timeout(CHUNK_IDLE_TIMEOUT, recv_frame(session)) => match r {
                        Ok(res) => res?,
                        Err(_) => return Err(LanBeamError::Timeout),
                    }
                };
                match frame {
                    Frame::FileChunk(_idx, bytes) => {
                        // Defense-in-depth (finding-0): with a correct sender every
                        // file's chunks sum to EXACTLY `f.size - start_offset`, so a
                        // single chunk can never exceed this file's `remaining`. If
                        // one does, the sender over-sent for this file and clamping
                        // it (the old `min(remaining)`) would silently drop the
                        // overflow — bytes that belong to the NEXT file's frames —
                        // desyncing the stream. Reject it as a protocol error so the
                        // drift fails cleanly here instead of corrupting a neighbour.
                        if bytes.len() as u64 > remaining {
                            return Err(LanBeamError::Protocol(format!(
                                "file chunk overruns declared size: {}",
                                f.name
                            )));
                        }
                        let take = bytes.len();
                        out.write_all(&bytes[..take]).await?;
                        if let Some(h) = hasher.as_mut() {
                            h.update(&bytes[..take]);
                        }
                        remaining -= take as u64;
                        received += take as u64;
                        progress[pi].bytes_written += take as u64;
                        unsynced += take as u64;
                        if unsynced >= RECEIVE_SYNC_INTERVAL {
                            out.sync_data().await?;
                            unsynced = 0;
                        }
                        on_progress(received, manifest.total_size);
                        on_file(FileEvent::Streaming {
                            index: i,
                            // The sanitized name, as on the prompt (M4).
                            name: &progress[pi].identity.rel,
                            done: progress[pi].bytes_written,
                            total: f.size,
                        });
                        // Pace the read loop (M6.7); a no-op when unthrottled.
                        limiter.throttle(take).await;
                    }
                    Frame::Control(m) => {
                        return Err(LanBeamError::Protocol(format!(
                            "chunk expected, got {}",
                            trust::clamp_peer_text(&format!("{m:?}"))
                        )))
                    }
                }
            }
            out.flush().await?;
            // Verify BEFORE the file is synced + published (M6.3): a mismatch
            // returns through the error path, which — because Integrity is
            // non-resumable — deletes the corrupt partial. `hasher` is Some only
            // when `f.sha256` is Some, by construction.
            if let Some(h) = hasher {
                let got = to_hex(&h.finalize());
                let expected = f.sha256.as_deref().unwrap_or_default();
                if !got.eq_ignore_ascii_case(expected) {
                    return Err(LanBeamError::Integrity(format!(
                        "sha256 mismatch: {}",
                        f.name
                    )));
                }
            }
            out.sync_all().await?;
            // File delivered (M6.8). Reaching here for a hashed file means its
            // digest matched — a mismatch returned above — so `verified` is
            // exactly `f.sha256.is_some()`.
            on_file(FileEvent::Done {
                index: i,
                verified: f.sha256.is_some(),
            });
            progress[pi].done = true;
            saved.push(path);
        }
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            // Commit overwrite files: the whole transfer succeeded (and every
            // hashed file verified), so replace each real target with the temp we
            // streamed into. `rename` replaces the target in one step on every
            // platform (std asks Windows to replace an existing file), but it
            // can still be refused — a read-only target, say — so fall back to
            // remove-then-rename; a crash in that tiny window leaves the
            // fully-synced new bytes at the temp (a "(1)" sibling) — never
            // nothing, and the original was never touched until here.
            // `remove_file` never follows a symlink, so containment holds.
            for (temp, target) in &overwrite_renames {
                let committed = std::fs::rename(temp, target).or_else(|_| {
                    let _ = std::fs::remove_file(target);
                    std::fs::rename(temp, target)
                });
                match committed {
                    Ok(()) => {
                        for s in saved.iter_mut() {
                            if *s == *temp {
                                *s = target.clone();
                            }
                        }
                    }
                    Err(e) => log::warn!(
                        "overwrite commit {} -> {} failed ({e}); kept the received copy at {}",
                        temp.display(),
                        target.display(),
                        temp.display()
                    ),
                }
            }
            // Best-effort ack: every byte is already flushed + synced, so a
            // failed ack write (the sender may have hit ITS deadline and
            // closed first) must not turn a fully-saved transfer into an
            // error on this side too — log it and keep the success.
            if let Err(e) = send_control(
                session,
                &AppMessage::TransferAck {
                    transfer_id: manifest.transfer_id.clone(),
                    ok: true,
                    error: None,
                },
            )
            .await
            {
                log::warn!(
                    "transfer {}: final ack send failed ({e}); files are saved",
                    manifest.transfer_id
                );
            }
            Ok(saved)
        }
        Err(e) => {
            // Best-effort: tell the sender. Then decide each written file's fate
            // (M6.4): keep a hashed partial through an interruption for a later
            // resume, delete a non-hashed one, and — on a data-integrity failure —
            // delete the file whose digest did NOT match while LEAVING its verified
            // siblings alone. (This comment used to say "delete EVERYTHING on a
            // data-integrity failure", which was true, and was the bug: one bad
            // hash took the whole batch with it. See the retain below.) The
            // accumulator is pruned to exactly the files left on disk so the
            // caller persists precisely those.
            let _ = send_control(
                session,
                &AppMessage::TransferAck {
                    transfer_id: manifest.transfer_id.clone(),
                    ok: false,
                    // Send only the COARSE ui_code over the wire, never the
                    // detailed error (finding): receive-path failures embed
                    // absolute local paths ("create C:\\Users\\<name>\\...:
                    // Access is denied"), which would otherwise cross to a
                    // possibly-untrusted peer and land in ITS toast, disclosing
                    // this machine's username and disk layout. The full detail is
                    // still surfaced LOCALLY — the returned `Err(e)` drives this
                    // side's own transfer_error event and log.
                    error: Some(e.ui_code().to_string()),
                },
            )
            .await;
            let keep_resumable = e.keeps_partial();
            progress.retain(|w| {
                // A file that already finished — written, synced, digest matched —
                // is FINISHED. A later file in the batch failing its checksum says
                // nothing about this one (a hash mismatch means corruption in
                // transit; a peer out to lie would simply have sent a matching
                // hash). Deleting good, verified data to punish a neighbour is
                // just data loss, and it used to happen to the whole batch. Leave
                // it on disk, and drop it from the partial accumulator — it is
                // whole, not partial.
                //
                // `overwrite` is the exception, and only looks like one: there
                // "finished" means the bytes are in a TEMP file while the real
                // target still holds the old content. That commit is deliberately
                // all-or-nothing and only runs once the entire transfer succeeds,
                // so an uncommitted temp falls through to the normal rule below —
                // deleted outright, or kept as a resumable partial.
                let uncommitted_temp = overwrite_renames.iter().any(|(t, _)| *t == w.path);
                if w.done && !uncommitted_temp {
                    return false;
                }
                let stays = keep_resumable && w.hashed;
                if !stays {
                    // The file handle unwound with the async block above, so
                    // Windows allows the delete.
                    let _ = std::fs::remove_file(&w.path);
                }
                stays
            });
            Err(e)
        }
    }
}

/// Read exactly `n` bytes from the current position of `file`, folding them into
/// `hasher` — the resume prefix fold-back (M6.4), so the final digest spans the
/// whole file. WHY read it back rather than trust it: the digest must cover
/// EVERY byte we deliver, including the ones a prior session already wrote.
async fn fold_prefix(file: &mut tokio::fs::File, n: u64, hasher: &mut Sha256) -> Result<()> {
    let mut remaining = n;
    let mut buf = vec![0u8; 128 * 1024];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        file.read_exact(&mut buf[..want]).await?;
        hasher.update(&buf[..want]);
        remaining -= want as u64;
    }
    Ok(())
}

/// Whether `name` is a single file name the sanitizer keeps exactly as it is:
/// no separator, no `..`, nothing it would clean. A recorded commit name must
/// be one before it is joined onto a folder (M9) — it comes from our own
/// partials file, which is still no reason to ever rename onto `../x`.
pub(super) fn is_single_clean_name(name: &str) -> bool {
    matches!(
        sanitize::validate(name),
        Decision::Accept(p) if p.components().count() == 1 && p.as_os_str() == name
    )
}

/// The on-disk path of `path` RELATIVE to `download_root`, forward-slashed —
/// the `disk_rel` a partial records so a resume reopens the exact same file.
/// `path` is always `download_root.join(...)`, so the strip cannot fail; the
/// fallback keeps the full path rather than panic.
fn disk_rel_under_root(download_root: &Path, path: &Path) -> String {
    path.strip_prefix(download_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
