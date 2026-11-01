//! Sending (quick text, M7.3, and files), answering incoming requests, the
//! interrupted partials a receive leaves behind, and live transfer control
//! (M6.1/6.2).

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Emitter, Manager, State};

use super::devices::resolve_peer;
use crate::error::LanBeamError;
use crate::settings;
use crate::state::{AppState, ConflictAction, ReplyDecision};
use crate::{transfer, transport};

// ── quick text (M7.3) ──────────────────────────────────────────────────

/// Send a short text or link to a device. Connects to the peer and delivers the
/// text, resolving only once the peer confirms it received it. `alsoClipboard`
/// asks the receiver to also place the text on their clipboard — whether it
/// actually lands there is the receiver's choice (their clipboard-sharing
/// setting must allow it). Errors if the text is empty or too long, the device
/// is unknown, or the peer is too old to receive text.
#[tauri::command]
pub async fn send_text(
    state: State<'_, AppState>,
    device_id: String,
    text: String,
    also_clipboard: bool,
) -> Result<(), LanBeamError> {
    // Reject empty / oversized input BEFORE opening a socket — the size cap
    // mirrors the receiver's clamp so a text that would be truncated on arrival
    // is refused here instead (M7.3).
    if text.trim().is_empty() {
        return Err(LanBeamError::Protocol("no text to send".into()));
    }
    if text.len() > crate::consts::MAX_TEXT_BYTES {
        return Err(LanBeamError::TextTooLong);
    }
    let (addr, port, expected) = resolve_peer(&state, &device_id)?;
    let local_private = *state.identity.private_bytes();
    let mut session = transport::connect(addr, port, &local_private, expected).await?;
    let my_name = transfer::local_device_name(&state.settings);
    let peer = transfer::initiator_hello(&mut session, my_name).await?;
    // Quick text is a v2 capability (M7.3): a peer that only speaks v1 (M4–M6)
    // must never be handed the variant, so refuse cleanly before sending.
    if peer.version < crate::consts::TRANSFER_V2 {
        return Err(LanBeamError::PeerTooOld(
            "peer too old for quick text".into(),
        ));
    }
    // The receiver acks after emitting the text, so this resolves on delivery.
    transfer::send_text(&mut session, text, Some(also_clipboard)).await
}

/// Send files/folders to a device. Returns the transfer id; progress and
/// completion are reported via `transfer_*` events. `stripExif` removes photo
/// metadata (location, camera, time) from JPEG/PNG/WebP images before they leave
/// this device — the per-send override the confirm dialog offers.
#[tauri::command]
pub async fn send_files(
    app: AppHandle,
    state: State<'_, AppState>,
    device_id: String,
    paths: Vec<String>,
    strip_exif: bool,
    transfer_id: Option<String>,
) -> Result<String, LanBeamError> {
    let (addr, port, expected) = resolve_peer(&state, &device_id)?;
    // One id up front: it names the cancel/pause controls, the events, AND the
    // per-transfer scratch dir the metadata strip writes temp copies into.
    //
    // The UI chooses it (H11), so its own record of what it sent — names,
    // sizes, the paths behind 再次发送 — is tied to exactly this session. It
    // used to be matched to the first `sas_code` for the same device, and every
    // send that failed before one (offline peer, unreadable file, cancel while
    // queued) left its record queued for the NEXT send to that device. A caller
    // that sends none gets one minted here.
    let transfer_id = match transfer_id {
        // Canonical lowercase-hyphenated — what crypto.randomUUID() produces, so
        // the events carry the very string the UI keyed its record by.
        Some(id) => uuid::Uuid::parse_str(&id)
            .map_err(|_| LanBeamError::Protocol("transfer id is not a UUID".into()))?
            .hyphenated()
            .to_string(),
        None => uuid::Uuid::new_v4().to_string(),
    };
    // A live id must not be reused: the control registry keeps the FIRST owner,
    // so a second send under it could never be cancelled or paused.
    if state
        .transfers_ctl
        .lock()
        .map(|m| m.contains_key(&transfer_id))
        .unwrap_or(false)
    {
        return Err(LanBeamError::Protocol("transfer id already in use".into()));
    }
    // Snapshot the integrity setting NOW (M6.3): if on, the build hashes every
    // file, so the whole build — the directory walk, the hashing, and the M9.1
    // metadata strip — runs on the blocking pool to keep large reads/writes off
    // the async runtime.
    let verify_hash = state.settings.read().map(|s| s.verify_hash).unwrap_or(true);
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    // Per-transfer scratch dir for stripped temp copies (M9.1): app-private
    // cache, NOT the download dir, and its own subfolder so concurrent sends
    // never collide. The returned guard deletes it (and every temp) on drop.
    let scratch_base = send_scratch_root(&app).join(&transfer_id);
    let (items, _scratch) = tauri::async_runtime::spawn_blocking(move || {
        // Refuse what the receiver would refuse — too many files, a name it
        // can't save — BEFORE hashing a single file (H7).
        transfer::check_send_selection(&paths)?;
        transfer::build_send_list_scoped(&paths, verify_hash, strip_exif, scratch_base)
    })
    .await
    .map_err(|e| LanBeamError::Io(format!("send list build task failed: {e}")))??;
    // `_scratch` lives to the end of this command (all exit paths below), so the
    // stripped temp copies are cleaned up whether the send succeeds, errors, or
    // is cancelled — a temp copy of the user's photo never lingers on disk.
    // The pre-check already said NoFiles for an empty selection; files can still
    // vanish between that walk and this one.
    if items.is_empty() {
        return Err(LanBeamError::NoFiles);
    }
    let total: u64 = items.iter().map(|i| i.meta.size).sum();

    // Register live cancel/pause controls (M6.1/6.2) keyed by this session id.
    // The guard removes them on every exit path below, so cancel/pause/resume
    // of a finished id is a graceful no-op. Registered before the dial so a
    // cancel during setup is honored at the first chunk.
    let (_ctl_guard, ctl) =
        crate::state::TransferCtlGuard::register(&state.transfers_ctl, &transfer_id);

    // Concurrency gate (M6.7): cap simultaneously-streaming transfers across both
    // directions (the receive side draws from the same gate). Acquired BEFORE the
    // dial, so a queued send opens no socket until a slot frees; a lowered cap
    // applies to sends not yet past this gate. Until the receiver accepts, the
    // slot doesn't hold back receives (see `ConcurrencyGate`). The rate cap
    // rides into run_send below.
    let rate_limit = state
        .settings
        .read()
        .map(|s| settings::rate_limit_bytes_per_sec(&s.rate_limit))
        .unwrap_or(None);
    let slot = match state.concurrency.try_acquire_send() {
        Some(guard) => guard,
        None => {
            // The device too (M25): a queued send is the first the transfer
            // list hears of this session, and without it the row has no peer.
            let _ = app.emit(
                "transfer_queued",
                serde_json::json!({ "sessionId": transfer_id, "direction": "send", "deviceId": device_id }),
            );
            // A cancel issued while QUEUED must win over the wait: without this
            // biased select the acquire ignores the already-registered cancel
            // token, so a cancelled send stays parked and only aborts once a slot
            // frees — after dialing/prompting the peer. Emit the error (mirroring
            // the res-error path below) so the UI resolves, then bail before any
            // socket is opened.
            tokio::select! {
                biased;
                _ = ctl.cancel.cancelled() => {
                    let _ = app.emit(
                        "transfer_error",
                        serde_json::json!({ "sessionId": transfer_id, "error": LanBeamError::Cancelled.to_string(), "code": LanBeamError::Cancelled.ui_code() }),
                    );
                    return Err(LanBeamError::Cancelled);
                }
                g = state.concurrency.acquire_send() => g,
            }
        }
    };

    let local_private = *state.identity.private_bytes();
    // A dial/handshake failure here must emit a terminal `transfer_error` for
    // this session id: when the concurrency gate was full we already announced
    // `transfer_queued {sessionId}`, so the UI holds a queued row keyed by it —
    // without this it would dangle forever (the peer went offline while queued,
    // the handshake timed out). Mirrors the cancel-while-queued branch above and
    // the post-`transfer_started` error path below. In the non-queued path the
    // frontend never learned this id (it only learns it on Ok), so the extra
    // event has no row to resolve and is harmless.
    let mut session = match transport::connect(addr, port, &local_private, expected).await {
        Ok(s) => s,
        Err(e) => {
            let _ = app.emit(
                "transfer_error",
                serde_json::json!({ "sessionId": transfer_id, "error": e.to_string(), "code": e.ui_code() }),
            );
            return Err(e);
        }
    };

    // [FIX-2] Show the SAS on the sender so the user can compare it against the
    // code the receiver's accept prompt shows — matching codes ⇒ no man-in-the-middle.
    let _ = app.emit(
        "sas_code",
        serde_json::json!({ "sessionId": transfer_id, "sas": session.sas(), "deviceId": device_id }),
    );
    let _ = app.emit(
        "transfer_started",
        serde_json::json!({ "sessionId": transfer_id, "direction": "send", "totalSize": total, "fileCount": items.len() }),
    );

    let app2 = app.clone();
    let sid = transfer_id.clone();
    let mut last_pct = -1i64;
    let my_name = transfer::local_device_name(&state.settings);
    // Per-file progress/completion events (M6.8), shared impl with the receive side.
    // The accept is also when this send starts counting against receives.
    let mut sink = transfer::file_event_sink(app.clone(), transfer_id.clone());
    let mut on_file = |event: transfer::FileEvent<'_>| {
        if matches!(event, transfer::FileEvent::Accepted) {
            slot.streaming();
        }
        sink(event);
    };
    let res = async {
        // Introduce ourselves and learn the peer's version BEFORE the manifest
        // (M4.2) — the receiver shows our friendly name on its accept prompt,
        // and future capabilities gate on the negotiated version. A peer that
        // drops the exchange surfaces as `peer_too_old` below.
        let peer = transfer::initiator_hello(&mut session, my_name).await?;
        log::debug!(
            "peer {device_id} answered hello (proto v{}, name {:?})",
            peer.version,
            peer.name
        );
        transfer::run_send(
            &mut session,
            &transfer_id,
            &items,
            &ctl,
            rate_limit,
            &mut on_file,
            move |sent, total| {
                transfer::emit_progress(&app2, &sid, "send", sent, total, &mut last_pct);
            },
        )
        .await
    }
    .await;

    match res {
        Ok(()) => {
            let _ = app.emit(
                "transfer_done",
                serde_json::json!({ "sessionId": transfer_id, "direction": "send" }),
            );
            Ok(transfer_id)
        }
        Err(e) => {
            // `code` distinguishes the peer politely declining ("declined") from
            // genuine failures ("timeout"/"io"/"protocol") — the `error` string
            // is unchanged and stays non-contractual (M4.5).
            let _ = app.emit(
                "transfer_error",
                serde_json::json!({ "sessionId": transfer_id, "error": e.to_string(), "code": e.ui_code() }),
            );
            Err(e)
        }
    }
}

/// Where the per-send scratch copies (stripped photos, M9.1) live.
pub(crate) fn send_scratch_root(app: &AppHandle) -> PathBuf {
    app.path()
        .app_cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("lanbeam")
        .join("send-scratch")
}

/// Delete the scratch folders of sends that ended before `started` (L18). A
/// send removes its own folder when it ends, but a crash or a kill skips that —
/// and those are copies of the user's photos, left in a cache nothing else
/// ever reads. Folders from this run are left alone.
pub(crate) fn sweep_send_scratch(root: &Path, started: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t < started);
        if stale {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Answer an incoming file request (fires the receive task's awaited oneshot).
/// Removal is deliberately unconditional — whatever prompt is parked under
/// this id right now is the one the user just answered; the parking session's
/// own cleanup is generation-checked instead (see `transfer::inbound::park_pending`).
///
/// `conflict` (M6.5) is the ConflictModal's choice — `"rename"` or `"overwrite"`
/// — folded into the SAME reply so there is one ordered answer per prompt. It is
/// consulted only when the receive-side `conflict` policy is `"ask"` and the
/// transfer actually collides; anything else (including a bare accept from a
/// frontend that predates the modal) leaves it `None`, and the receiver falls
/// back to the safe rename — it never overwrites without this explicit choice.
#[tauri::command]
pub fn reply_file_request(
    state: State<'_, AppState>,
    session_id: String,
    accept: bool,
    conflict: Option<String>,
) {
    let conflict = conflict.as_deref().and_then(ConflictAction::parse);
    // Take the sender out and DROP the pending guard BEFORE sending: the oneshot
    // send never runs under the lock (a latent footgun if that path ever grows),
    // and a poisoned lock is a graceful no-op — same discipline as `park_pending`
    // / `remove_pending_if_owner`, not a panic on the accept/decline command.
    let taken = {
        let Ok(mut pending) = state.pending.lock() else {
            return;
        };
        pending.remove(&session_id)
    };
    if let Some(parked) = taken {
        let _ = parked.reply.send(ReplyDecision { accept, conflict });
    }
}

/// The requests still waiting for an answer, as their `incoming_file_request`
/// payloads (L23). A request that arrived before the window had loaded — at
/// login, with LanBeam starting minimized — emitted its event to nobody; the
/// window asks for these once it is up, or the prompt would never show and
/// the request would time out unanswered.
#[tauri::command]
pub fn pending_requests(state: State<'_, AppState>) -> Vec<serde_json::Value> {
    state
        .pending
        .lock()
        .map(|m| m.values().map(|p| p.request.clone()).collect())
        .unwrap_or_default()
}

/// One interrupted file still on disk, waiting to be resumed or discarded.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialDto {
    pub device_id: String,
    /// The file's name as it appears in the download folder.
    pub name: String,
    /// Bytes already on disk.
    pub written: u64,
    /// What the whole file will be.
    pub size: u64,
}

/// Every interrupted receive still holding bytes on disk.
///
/// This exists because a half-written file is INVISIBLE. It is saved under its
/// FINAL name — `holiday.mp4`, 1.2 GB of a 4 GB file — so the download folder
/// shows something that looks perfectly normal and plays for thirty seconds. The
/// backend has known about these all along (it is what makes resume work) and
/// `discard_partials` could already clean them up; nothing ever asked it what it
/// was holding, so the UI could neither show them nor offer to.
#[tauri::command]
pub fn list_partials(state: State<'_, AppState>) -> Vec<PartialDto> {
    let Ok(store) = state.partials.read() else {
        return Vec::new();
    };
    store
        .entries()
        .into_iter()
        .map(|(device_id, rec)| PartialDto {
            device_id,
            // `disk_rel` is where the bytes actually are (organize-by-date/device
            // folds a prefix in); its leaf is what the user sees in the folder.
            name: rec
                .disk_rel
                .rsplit('/')
                .next()
                .unwrap_or(&rec.rel)
                .to_string(),
            written: rec.bytes_written,
            size: rec.size,
        })
        .collect()
}

/// Discard the persisted resume state for a peer (M6.4): forget every partial
/// LanBeam kept for `device_id` and delete the half-written files from disk.
/// The counterpart to letting an interrupted transfer resume — the user chooses
/// to start clean instead. A no-op for a peer with no partials.
#[tauri::command]
pub fn discard_partials(state: State<'_, AppState>, device_id: String) {
    // Take the records out under the write guard, snapshot, then do disk I/O and
    // persist AFTER the guard drops — the trust-store pattern (no I/O under lock).
    let (removed, snapshot) = {
        let Ok(mut store) = state.partials.write() else {
            return;
        };
        let removed = store.clear_device(&device_id);
        if removed.is_empty() {
            return; // nothing to discard — no save
        }
        (removed, store.snapshot())
    };
    // Snapshot the download root to resolve each partial's on-disk path; never
    // hold the settings/dir lock across the deletes.
    let current = state.download_dir.read().ok().map(|d| d.clone());
    for rec in &removed {
        if let Some(path) = partial_on_disk(rec, current.as_deref()) {
            let _ = std::fs::remove_file(path);
        }
    }
    crate::partials::persist_async(snapshot);
}

/// The file a partial record names, if it is still the partial LanBeam wrote:
/// under the folder it was written into (`root`; a record older than that field
/// can only be taken to be under `current`), a regular file, and exactly the
/// length recorded. Anything else is somebody's file that happens to have the
/// name — after a download-folder change the same relative path names a file
/// in the NEW folder, and a discard used to delete it (M2).
fn partial_on_disk(
    rec: &crate::partials::PartialRecord,
    current: Option<&Path>,
) -> Option<PathBuf> {
    let root = match rec.root.as_deref() {
        Some(r) => PathBuf::from(r),
        None => current?.to_path_buf(),
    };
    let path = root.join(&rec.disk_rel);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    (meta.is_file() && meta.len() == rec.bytes_written).then_some(path)
}

// ── transfer control (M6.1/6.2) ────────────────────────────────────────

/// Cancel an in-flight transfer by its session id. Works in either direction
/// and ends the transfer promptly on both peers. Always safe to call: a no-op
/// if the id is unknown (the transfer already finished or never started).
#[tauri::command]
pub fn cancel_transfer(state: State<'_, AppState>, session_id: String) {
    // Trip the session's cancel token; the chunk loop returns early and the
    // dropped connection makes the peer fail through its normal error path.
    crate::state::cancel_transfer_ctl(&state.transfers_ctl, &session_id);
}

/// Pause an in-flight transfer by its session id. The transfer holds where it
/// is until `resume_transfer` is called for the same id. A no-op if the id is
/// unknown.
#[tauri::command]
pub fn pause_transfer(state: State<'_, AppState>, session_id: String) {
    // Session-local: the loop stops moving bytes → TCP backpressure stalls the
    // peer, with no protocol message sent.
    crate::state::set_transfer_paused_ctl(&state.transfers_ctl, &session_id, true);
}

/// Resume a transfer previously paused with `pause_transfer`. A no-op if the id
/// is unknown or the transfer was not paused.
#[tauri::command]
pub fn resume_transfer(state: State<'_, AppState>, session_id: String) {
    crate::state::set_transfer_paused_ctl(&state.transfers_ctl, &session_id, false);
}

#[tauri::command]
pub fn get_download_dir(state: State<'_, AppState>) -> String {
    // Poisoned lock: an empty string beats panicking the invoke — same
    // contract as `list_trusted`.
    state
        .download_dir
        .read()
        .map(|d| d.display().to_string())
        .unwrap_or_default()
}

/// Open a path (a received file, or the download folder) with the OS default
/// handler.
///
/// WHY A COMMAND, not the opener plugin's JS `openPath`: that command is
/// SCOPE-GATED, and its scope is a static allow-list. Our download folder is
/// user-configurable to anywhere on disk, so the only static scope that would
/// actually work is `"**"` — i.e. handing the webview a blanket "open any file
/// on this machine" capability. The plugin's RUST api takes the same action
/// without granting that authority to the frontend, so the capability stays
/// narrow (reveal-only) and this stays the one audited door.
///
/// Existence is checked FIRST so a vanished file reports `NotFound` — the UI
/// then says "that file is gone" instead of blaming a stale record for what
/// might really be an open failure.
#[tauri::command]
pub fn open_local_path(app: AppHandle, path: String) -> Result<(), LanBeamError> {
    use tauri_plugin_opener::OpenerExt;
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(LanBeamError::NotFound(path));
    }
    app.opener()
        .open_path(&path, None::<&str>)
        .map_err(|e| LanBeamError::Io(format!("open {path}: {e}")))
}

/// Absolute paths saved by a completed inbound transfer (for reveal/open in the UI).
#[tauri::command]
pub fn reveal_received(state: State<'_, AppState>, session_id: String) -> Vec<String> {
    // Poisoned lock: an empty list beats panicking the invoke — same contract as
    // `list_trusted` / `get_download_dir`.
    let Ok(log) = state.completed.lock() else {
        return Vec::new();
    };
    log.get(&session_id)
        .map(|v| v.iter().map(|p| p.display().to_string()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L18: scratch folders older than this run are removed; this run's stay.
    #[test]
    fn sweep_removes_scratch_left_by_an_earlier_run() {
        let root = std::env::temp_dir().join(format!("lanbeam-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("old-send")).unwrap();
        std::fs::write(root.join("old-send").join("IMG.jpg"), b"x").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let started = std::time::SystemTime::now();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::create_dir_all(root.join("this-run")).unwrap();

        sweep_send_scratch(&root, started);
        assert!(!root.join("old-send").exists(), "left by a crashed run");
        assert!(root.join("this-run").exists(), "a send of this run");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M2: a discard deletes a partial only in the folder it was written to,
    /// and only while it is still the length recorded. After a download-folder
    /// change the same relative name in the new folder is somebody's file.
    #[test]
    fn discard_only_touches_the_partial_it_recorded() {
        let tmp = std::env::temp_dir().join(format!("lanbeam-m2d-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (old, new) = (tmp.join("old"), tmp.join("new"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("a.bin"), vec![0u8; 400]).unwrap();
        std::fs::write(new.join("a.bin"), vec![0u8; 400]).unwrap();
        let rec = crate::partials::PartialRecord {
            rel: "a.bin".into(),
            size: 1000,
            sha256: None,
            disk_rel: "a.bin".into(),
            bytes_written: 400,
            commit_name: None,
            root: Some(old.to_string_lossy().into_owned()),
        };
        assert_eq!(partial_on_disk(&rec, Some(&new)), Some(old.join("a.bin")));

        let legacy = crate::partials::PartialRecord { root: None, ..rec };
        assert_eq!(
            partial_on_disk(&legacy, Some(&new)),
            Some(new.join("a.bin")),
            "a record from before the field: the current folder"
        );
        assert_eq!(partial_on_disk(&legacy, None), None);
        std::fs::write(new.join("a.bin"), vec![0u8; 1000]).unwrap();
        assert_eq!(
            partial_on_disk(&legacy, Some(&new)),
            None,
            "not the length recorded: not ours"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
