//! App orchestration for an inbound connection. `handle_incoming` reads the
//! opening dialogue and dispatches on it; a file request then goes through the
//! flood and duplicate guards, the receive policy and its prompt, the
//! resume/collision analysis and the concurrency gate, with the lifecycle
//! events emitted along the way.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::json;
use tauri::{Emitter, Runtime};
use tokio::sync::oneshot;

use super::organize::{FolderLang, Organize};
use super::pairing_host::handle_pair_request;
use super::progress::{emit_progress, file_event_sink};
use super::receive::{
    run_receive_core, send_pending, send_reply, validate_manifest, FileDisposition, FileProgress,
    PendingState, REPLY_BUSY, REPLY_CANCELLED, REPLY_DECLINED, REPLY_DUPLICATE_ID,
    REPLY_UNSAFE_NAME,
};
use super::text::handle_text_received;
use super::wire::{local_device_name, open_inbound, InboundOpen};
use super::Manifest;
use crate::consts::{MANIFEST_TIMEOUT, REPLY_TIMEOUT};
use crate::error::{LanBeamError, Result};
use crate::partials::{FileIdentity, PartialRecord};
use crate::sanitize::{self, Decision};
use crate::state::{ConflictAction, Parked, PendingMap, ReplyDecision, TransferCtlGuard};
use crate::transport::noise::NoiseSession;
use crate::transport::TransportCtx;
use crate::trust;

/// The name to show for a peer: what it called itself, trimmed, if that is
/// anything; else `fallback` (the discovery table's name, say); else the first
/// eight characters of its device id — never empty.
pub(crate) fn peer_display_name(
    declared: Option<&str>,
    device_id: &str,
    fallback: impl FnOnce() -> Option<String>,
) -> String {
    declared
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(String::from)
        .or_else(fallback)
        .unwrap_or_else(|| device_id.chars().take(8).collect())
}

/// The name the discovery table has for `device_id`, if it is there.
pub(super) fn discovered_name<R: Runtime>(
    ctx: &TransportCtx<R>,
    device_id: &str,
) -> Option<String> {
    ctx.peers
        .lock()
        .ok()
        .and_then(|t| t.get(device_id).map(|p| p.name.clone()))
}

/// The file-request throttle (M18): a device in the trust store passes; anyone
/// else is counted against its source address. A poisoned lock admits — the
/// throttle is a nuisance guard, and failing it closed would stop every
/// transfer from strangers for the rest of the run.
fn admit_file_request<R: Runtime>(
    ctx: &TransportCtx<R>,
    session: &NoiseSession,
    device_id: &str,
) -> bool {
    let known = ctx
        .trusted
        .read()
        .map(|t| t.get(device_id).is_some())
        .unwrap_or(false);
    if known {
        return true;
    }
    let Some(ip) = session.peer_addr().map(|a| a.ip()) else {
        return true;
    };
    ctx.file_rate
        .lock()
        .map(|mut r| r.admit(ip, Instant::now()))
        .unwrap_or(true)
}

/// Monotonic stamp for parked prompts — see [`park_pending`]. Process-wide so
/// two entries can never share a generation, whatever their ids.
static PENDING_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Park the accept/decline sender under `transfer_id` — unless one is already
/// parked there. WHY refuse instead of replace: `pending` is keyed by the
/// SENDER-chosen transfer_id, so honoring a duplicate would drop the victim's
/// oneshot sender and force-decline someone else's live prompt (M4.5).
/// Returns the entry's generation on success — the token the parking session
/// must present to [`remove_pending_if_owner`] later — or `None` (caller must
/// reject the session) on duplicate or when the lock is poisoned: a panicked
/// bookkeeping task means the map is unusable, and declining beats cascading
/// the panic.
pub(super) fn park_pending(
    pending: &Mutex<PendingMap>,
    transfer_id: &str,
    reply: oneshot::Sender<ReplyDecision>,
    request: serde_json::Value,
) -> Option<u64> {
    let Ok(mut map) = pending.lock() else {
        return None;
    };
    match map.entry(transfer_id.to_string()) {
        std::collections::hash_map::Entry::Occupied(_) => None,
        std::collections::hash_map::Entry::Vacant(slot) => {
            let generation = PENDING_GENERATION.fetch_add(1, Ordering::Relaxed);
            slot.insert(Parked {
                generation,
                reply,
                request,
            });
            Some(generation)
        }
    }
}

/// Owner-only removal of a parked prompt: the entry goes away only when its
/// generation matches the one this session parked. WHY not remove by id:
/// `reply_file_request` vacates the slot the moment it answers, so a
/// successor session reusing the same sender-chosen id may have re-parked in
/// the interim — an id-only removal would drop the successor's live oneshot
/// and force-decline its prompt without any user input (M4.5).
pub(super) fn remove_pending_if_owner(
    pending: &Mutex<PendingMap>,
    transfer_id: &str,
    generation: u64,
) {
    if let Ok(mut map) = pending.lock() {
        if map
            .get(transfer_id)
            .is_some_and(|p| p.generation == generation)
        {
            map.remove(transfer_id);
        }
    }
}

/// RAII claim on an inbound session id, held for the WHOLE life of
/// `handle_incoming`. WHY `pending` isn't enough: it only covers the prompt
/// window — the auto-accept path never parks at all, and even a prompted
/// session vacates its entry before `run_receive` starts — so without this
/// set, two sessions reusing one sender-chosen transfer_id would run
/// concurrently and conflate every event and completed entry keyed by it.
/// Drop releases the id on every exit path (decline, error, success).
pub(super) struct InFlightGuard<'a> {
    set: &'a Mutex<HashSet<String>>,
    id: String,
}

impl<'a> InFlightGuard<'a> {
    /// Atomically claim `id`. `None` when it is already in flight — or when
    /// the lock is poisoned: an unusable registry must reject, same contract
    /// as [`park_pending`].
    pub(super) fn claim(set: &'a Mutex<HashSet<String>>, id: &str) -> Option<Self> {
        let Ok(mut s) = set.lock() else {
            return None;
        };
        if !s.insert(id.to_string()) {
            return None;
        }
        Some(Self {
            set,
            id: id.to_string(),
        })
    }
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut s) = self.set.lock() {
            s.remove(&self.id);
        }
    }
}

/// Refresh a trusted peer's bookkeeping after an ACCEPTED transfer (M4.4):
/// bump `last_seen`, adopt a fresher DeviceInfo name, persist, and tell the
/// UI. A no-op for peers not in the store — see `TrustStore::touch_seen`.
/// Mutation + snapshot happen under the write guard; the file write runs on
/// the blocking pool and the emit fires after the guard drops — this runs on
/// a tokio worker inside `handle_incoming`, and disk I/O under the trust lock
/// would stall every concurrent session's policy check and the UI's sync
/// trust commands (M4.6).
fn touch_trusted<R: Runtime>(ctx: &TransportCtx<R>, device_id: &str, name: Option<&str>) {
    let (snapshot, list) = {
        // Poisoned lock: skip the bookkeeping rather than cascade the panic —
        // the transfer itself must proceed regardless.
        let Ok(mut store) = ctx.trusted.write() else {
            return;
        };
        if !store.touch_seen(device_id, name) {
            return;
        }
        (store.snapshot(), store.list())
    };
    trust::persist_async(snapshot);
    trust::emit_updated(&ctx.app, list);
}

/// Fire an OS notification if the `notif_system` setting (read at fire time)
/// allows it (M5.4). Content is deliberately LOCALE-NEUTRAL: the backend has
/// no i18n layer, so instead of hardcoding sentences in any one language the
/// title is the peer's display name and the body is symbols + numbers only
/// ("↓ 3 · 1.2 MB") — digits, arrows and unit symbols read the same in every
/// locale, and the direction/meaning lives in the glyph, not in words.
#[cfg(desktop)]
fn notify_system<R: Runtime>(ctx: &TransportCtx<R>, title: &str, body: &str) {
    use tauri::Manager;
    let enabled = ctx.settings.read().map(|s| s.notif_system).unwrap_or(true);
    if !enabled {
        return;
    }
    // try_state, NOT NotificationExt::notification(): the ext accessor panics
    // when the plugin is absent, and the integration tests drive the real
    // `handle_incoming` on a mock app with no plugins — a missing notifier
    // must degrade to silence, never take the session down with it.
    let Some(n) = ctx
        .app
        .try_state::<tauri_plugin_notification::Notification<R>>()
    else {
        return;
    };
    if let Err(e) = n.builder().title(title).body(body).show() {
        log::warn!("system notification failed: {e}");
    }
}

/// Show the just-received files in the file manager, if the user asked for it
/// (`auto_open_folder`, read at fire time).
///
/// That setting had a switch, a command, a persisted field and its own line in
/// the diagnostics export — and nothing anywhere that acted on it. Rust read it
/// only to hand it back to the UI so the switch could render its own state. It
/// also defaults ON, so every install has been promising this and never doing it.
#[cfg(desktop)]
fn reveal_if_asked<R: Runtime>(ctx: &TransportCtx<R>, path: Option<&std::path::Path>) {
    use tauri::Manager;
    let enabled = ctx
        .settings
        .read()
        .map(|s| s.auto_open_folder)
        .unwrap_or(false);
    if !enabled {
        return;
    }
    let Some(path) = path else { return };
    // try_state, NOT OpenerExt::opener(): the ext accessor panics when the plugin
    // is absent, and the integration tests drive the real `handle_incoming` on a
    // mock app with no plugins — a missing opener must degrade to silence, never
    // take the session down with it. (Same reason `notify_system` does it.)
    let Some(opener) = ctx.app.try_state::<tauri_plugin_opener::Opener<R>>() else {
        return;
    };
    if let Err(e) = opener.reveal_item_in_dir(path) {
        log::warn!("auto-open: reveal {} failed: {e}", path.display());
    }
}

/// Mobile stub — see `notify_system`.
#[cfg(not(desktop))]
fn reveal_if_asked<R: Runtime>(_ctx: &TransportCtx<R>, _path: Option<&std::path::Path>) {}

/// Mobile stub — the notification plugin is a desktop-only dependency here,
/// and a no-op keeps every call site free of cfg noise.
#[cfg(not(desktop))]
fn notify_system<R: Runtime>(_ctx: &TransportCtx<R>, _title: &str, _body: &str) {}

/// Human-readable byte size ("824 B", "1.2 MB", "40 GB"), 1024-based, one
/// decimal under 10. Feeds the OS notification body and the share landing page,
/// so it stays symbols + numbers — see [`notify_system`] for the
/// locale-neutrality contract.
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit]) // one decimal while it matters
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// Handle one inbound connection: opening dialogue (hello exchange, or a
/// legacy manifest) → validate → decide via the receive policy (auto-accept
/// or ask the UI) → receive, emitting lifecycle events. Runs in the
/// listener's per-connection task.
pub async fn handle_incoming<R: Runtime>(
    mut session: NoiseSession,
    ctx: &TransportCtx<R>,
) -> Result<()> {
    // Bounded: the whole opening dialogue (hello exchange + manifest) is small
    // control traffic, so a peer that opens a session and trickles (or stalls)
    // must not hold this task open (M4.5).
    let my_name = local_device_name(&ctx.settings);
    let opened =
        match tokio::time::timeout(MANIFEST_TIMEOUT, open_inbound(&mut session, my_name)).await {
            Ok(res) => res?,
            Err(_) => return Err(LanBeamError::Timeout),
        };
    let device_id = URL_SAFE_NO_PAD.encode(session.remote_static());
    let (manifest, peer) = match opened {
        // `connect_by_addr`'s identity/SAS round-trip ends here by design:
        // the peer introduced itself and said Bye — nothing user-visible
        // happened, so the UI sees no prompt, no events, no state.
        InboundOpen::Bye => {
            log::debug!("clean bye from {device_id} (identity round-trip)");
            return Ok(());
        }
        // A pairing redemption (M7.1) — handled and CLOSED here, BEFORE any
        // incoming_file_request path: match the code, reply PairConfirm, hand
        // the SAS to the UI, and return. Never falls through to the file flow,
        // and never grants trust (that is the UI's call after the SAS compare).
        InboundOpen::Pair { code, port, peer } => {
            return handle_pair_request(&mut session, ctx, &device_id, code, port, peer).await;
        }
        // A quick text (M7.3) — handled and CLOSED here, BEFORE any file path:
        // emit `text_received`, optionally mirror to the clipboard (receiver
        // consent), ack, and return. Never touches disk, never falls through.
        InboundOpen::Text {
            text,
            to_clipboard,
            peer,
        } => {
            return handle_text_received(&mut session, ctx, &device_id, text, to_clipboard, peer)
                .await;
        }
        InboundOpen::Files { manifest, peer } => (manifest, peer),
    };
    let session_id = manifest.transfer_id.clone();
    // When the sender's reply clock (REPLY_TIMEOUT) began: it started waiting the
    // moment it finished sending this manifest — i.e. now. Both the accept prompt
    // and any concurrency-gate queue below spend against this same budget, so the
    // gate wait is bounded by what's LEFT of it (finding), not a fresh 150s.
    let received_at = Instant::now();
    // Friendly sender name for the accept prompt: the peer's self-declared
    // DeviceInfo name is best (it exists even for peers absent from the
    // discovery table), the table name is next, and the shortened device id
    // always exists. The negotiated version is what later capabilities gate on.
    let sender_name = peer_display_name(peer.name.as_deref(), &device_id, || {
        discovered_name(ctx, &device_id)
    });
    log::info!(
        "inbound session {session_id} from {sender_name:?} ({device_id}, proto v{})",
        peer.version
    );

    // Flood guard (M18), before anything costs: a stranger opening request
    // after request would otherwise get a disk check per listed file, an event
    // and — on the prompt path — a system notification, every time. Paired
    // devices are known and not counted. Declined "busy", without a word to
    // the UI: there is nothing to show anyone.
    if !admit_file_request(ctx, &session, &device_id) {
        log::warn!("declining {session_id} from {device_id}: too many requests from its address");
        let _ = send_reply(&mut session, &session_id, false, Some(REPLY_BUSY.into())).await;
        return Ok(());
    }

    // Whole-lifetime duplicate guard: claimed BEFORE any event fires and held
    // until this function returns, so a reused sender-chosen id is politely
    // refused whether the live session is auto-accepted, prompting, or already
    // mid-transfer — see `InFlightGuard`. An id one of OUR transfers is using
    // is refused the same way (L12): the peer we are sending to learns that id
    // from our manifest, and an inbound session under it would land its events
    // — a failure, a prompt — on our send's row. (Sending to ourselves is
    // refused at the dial, so this is never the same transfer seen twice.)
    let ours = ctx
        .transfers_ctl
        .lock()
        .map(|m| m.contains_key(&session_id))
        .unwrap_or(true);
    let claimed = if ours {
        None
    } else {
        InFlightGuard::claim(&ctx.in_flight, &session_id)
    };
    let Some(_in_flight) = claimed else {
        log::warn!(
            "rejecting inbound transfer from {device_id}: id {session_id} is already in flight"
        );
        let _ = send_reply(
            &mut session,
            &session_id,
            false,
            Some(REPLY_DUPLICATE_ID.into()),
        )
        .await;
        // Deliberately NO transfer_error emit here: the UI keys sessions by this
        // id, and an event would clobber the live session's still-valid state.
        return Err(LanBeamError::Protocol(format!(
            "duplicate transfer id {session_id}"
        )));
    };

    // Reject unsafe manifests before involving the user — after the claim, so
    // the event below can only ever name a session that is this one (L12).
    if let Err(e) = validate_manifest(&manifest) {
        let _ = send_reply(
            &mut session,
            &session_id,
            false,
            Some(REPLY_UNSAFE_NAME.into()),
        )
        .await;
        let _ = ctx.app.emit(
            "transfer_error",
            json!({ "sessionId": session_id, "error": e.to_string(), "code": e.ui_code() }),
        );
        return Err(e);
    }

    // Receive policy (M4.4): decide BEFORE parking a prompt whether this peer
    // skips it — "all" auto-accepts anyone on the LAN, "trusted" only peers
    // the user explicitly marked auto-accept, "ask" always prompts. A poisoned
    // lock (settings or trust) also prompts: when in doubt, ask the human.
    let policy = ctx
        .settings
        .read()
        .map(|s| s.recv_policy.clone())
        .unwrap_or_else(|_| "ask".to_string());
    let trusted_entry = ctx
        .trusted
        .read()
        .ok()
        .and_then(|t| t.get(&device_id).cloned());
    let auto_accepted = trust::should_auto_accept(&policy, trusted_entry.as_ref());

    // Snapshot the download root NOW (M5.2): a concurrent `set_download_dir`
    // must not split this session's files across two folders (and the resume /
    // collision analysis below must agree with the eventual write). The lock is
    // never held across an await — the receive path gets a plain path.
    let download_root = match ctx.download_dir.read() {
        Ok(d) => d.clone(),
        // Poisoned: the last-written value is still the user's choice.
        Err(e) => e.into_inner().clone(),
    };

    // Auto-organize (M6.6): the subfolder each file in this transfer is nested
    // under — the sanitized sender name, today's date, or the file's kind (F7).
    // Decided once so a transfer spanning midnight all lands in one folder.
    let (conflict_policy, organize, ui_lang) = ctx
        .settings
        .read()
        .map(|s| (s.conflict.clone(), s.organize.clone(), s.ui_lang.clone()))
        .unwrap_or_else(|_| ("ask".to_string(), "none".to_string(), String::new()));
    let organize = match organize.as_str() {
        "device" => Organize::Folder(PathBuf::from(sanitize::sanitize_component(
            &sender_name,
            "device",
        ))),
        "date" => Organize::Folder(PathBuf::from(today_ymd())),
        "type" => Organize::ByType(FolderLang::from_setting(&ui_lang)),
        _ => Organize::Flat, // "none" + forward-compat fallback
    };

    // Per-file resume + collision analysis (M6.4/6.5): where each file would
    // land, whether a persisted partial lets it continue, and whether a fresh
    // copy would collide with an existing file. Run on the blocking pool
    // (finding): it does 1-2 synchronous std::fs stats PER FILE, and a 10k-file
    // manifest on a network share or spun-down disk would otherwise pin an async
    // worker for tens of seconds, stalling other sessions before the prompt even
    // fires. `manifest` moves in and back out; the roots/id are cloned since the
    // receive path still needs them, and `ctx.partials` is an `Arc` clone.
    let partials = ctx.partials.clone();
    let analysis_root = download_root.clone();
    let analysis_dev = device_id.clone();
    let (analyses, manifest) = tauri::async_runtime::spawn_blocking(move || {
        let a = analyze_files(
            &manifest,
            &analysis_root,
            &organize,
            &analysis_dev,
            &partials,
        );
        (a, manifest)
    })
    .await
    .map_err(|e| LanBeamError::Io(format!("file analysis task failed: {e}")))?;

    // Every name the prompt shows is the SANITIZED one (M4) — what the file
    // will actually be called. The sender's own string can carry bidi controls
    // and zero-width characters: "photo\u{202E}gnp.exe" reads as "photoexe.png"
    // on the prompt while the file saved is "photognp.exe".
    let files: Vec<_> = analyses
        .iter()
        .zip(&manifest.files)
        .map(|(a, f)| json!({ "name": a.identity.rel, "size": f.size }))
        .collect();
    // Files a fresh write would collide with — the ConflictModal's input (M6.5).
    let conflicts: Vec<_> = analyses
        .iter()
        .filter(|a| a.collides)
        .map(|a| a.identity.rel.clone())
        .collect();
    // Files continuing a partial (M6.4) — informational for the UI.
    let resuming: Vec<_> = analyses
        .iter()
        .filter_map(|a| {
            a.resume
                .as_ref()
                .map(|r| json!({ "name": a.identity.rel, "offset": r.offset }))
        })
        .collect();
    let request_payload = json!({
        "sessionId": session_id,
        "deviceId": device_id,
        "sas": session.sas(),
        "totalSize": manifest.total_size,
        "fileCount": manifest.files.len(),
        "files": files,
        // Additive (M4.2): older frontends simply ignore it.
        "senderName": sender_name,
        // Additive (M4.4): `true` means the policy already accepted — the UI
        // should toast/log instead of prompting (a reply would go nowhere).
        "autoAccepted": auto_accepted,
        // Additive (M6.5): names that collide with existing files, plus the
        // active policy — when it is "ask" and this list is non-empty the UI
        // shows the ConflictModal and echoes the choice via `reply_file_request`.
        "conflicts": conflicts,
        "conflictPolicy": conflict_policy,
        // Additive (M6.4): files that will continue from a partial (offset > 0).
        "resuming": resuming,
    });

    // Auto-accept answers the ACCEPT question ("do I want anything from this
    // device") — it does not answer the COLLISION question ("what do I do with my
    // existing file"). Those are two settings and two decisions. A user who set
    // 冲突策略 = 每次询问 asked to be asked, and silently answering "rename" on
    // their behalf was worst precisely where it is most likely to bite: a trusted
    // device with auto-accept on is the MAIN path, so under the default settings
    // (recv_policy "trusted" + conflict "ask") the modal could never appear at all.
    // So: still no accept prompt, but do stop and ask about the collision.
    let ask_collision = auto_accepted && conflict_policy == "ask" && !conflicts.is_empty();

    let (accept, conflict_choice) = if auto_accepted && !ask_collision {
        // The prompt machinery is skipped entirely: nothing parked in
        // `pending`, no oneshot, no 120s window. The event still fires so
        // the UI can surface WHAT was auto-accepted and from whom.
        log::info!("auto-accepting {session_id} from {device_id} (recv_policy \"{policy}\")");
        let _ = ctx.app.emit("incoming_file_request", request_payload);
        (true, None)
    } else {
        // A queue of unanswered prompts has an upper bound too (M18) — nobody
        // answers sixteen at once, and each is another notification.
        let crowded = ctx
            .pending
            .lock()
            .map(|p| p.len() >= crate::consts::MAX_PENDING_PROMPTS)
            .unwrap_or(false);
        if crowded {
            log::warn!("declining {session_id} from {device_id}: too many prompts already waiting");
            let _ = send_reply(&mut session, &session_id, false, Some(REPLY_BUSY.into())).await;
            return Ok(());
        }
        // Ask the UI and wait for `reply_file_request` (timeout = decline).
        // The in-flight guard above already rejected duplicates, so a `None`
        // here means the pending lock is poisoned — decline defensively.
        let (tx, rx) = oneshot::channel();
        let Some(generation) = park_pending(&ctx.pending, &session_id, tx, request_payload.clone())
        else {
            log::warn!(
                "rejecting inbound transfer from {device_id}: cannot park prompt for {session_id}"
            );
            let _ = send_reply(
                &mut session,
                &session_id,
                false,
                Some(REPLY_DUPLICATE_ID.into()),
            )
            .await;
            // Deliberately NO transfer_error emit here: the UI keys sessions by this
            // id, and an event would clobber the victim's still-valid prompt state.
            return Err(LanBeamError::Protocol(format!(
                "duplicate transfer id {session_id}"
            )));
        };
        let _ = ctx.app.emit("incoming_file_request", request_payload);
        // OS toast beside the in-app prompt (M5.4): with close-to-tray on by
        // default the window is often hidden, and an unanswered prompt
        // auto-declines at 120s — the toast is what makes that window real.
        // Only the PROMPT path notifies; auto-accepted sessions surface via
        // the completion toast below instead of pinging twice.
        notify_system(
            ctx,
            &sender_name,
            &format!(
                "↓ {} · {}",
                manifest.files.len(),
                human_size(manifest.total_size)
            ),
        );
        // And tell the sender it is on screen now (F4), so its "waiting for
        // them" can say what it is waiting on.
        send_pending(
            &mut session,
            peer.version,
            &session_id,
            PendingState::Prompted,
        )
        .await;

        // Wait for the answer — or for the sender to stop waiting (M12): it
        // cancelled, quit, or lost the network. Nothing else would notice until
        // someone answered a prompt with nobody on the other end, which then
        // failed. `closed` reads nothing, so the answer winning leaves the stream
        // untouched for the reply.
        let answer = tokio::select! {
            r = tokio::time::timeout(crate::consts::PROMPT_TIMEOUT, rx) => r,
            () = session.closed() => {
                remove_pending_if_owner(&ctx.pending, &session_id, generation);
                let e = LanBeamError::ConnectionLost("the sender hung up before an answer".into());
                log::info!("{session_id}: {e}; withdrawing the prompt");
                let _ = ctx.app.emit(
                    "transfer_error",
                    json!({ "sessionId": session_id, "error": e.to_string(), "code": e.ui_code() }),
                );
                return Err(e);
            }
        };
        let decision = match answer {
            Ok(Ok(d)) => d,
            // Nobody answered. What that MEANS depends on what was asked. An
            // accept prompt unanswered is a "no" — silence must never become
            // consent. But an auto-accepted transfer already has its yes; only
            // the collision choice was outstanding, so throwing the whole
            // transfer away over an unanswered follow-up would punish the user
            // for stepping out. Fall back to the safe, non-destructive rename —
            // the same thing this path did before it learned to ask at all.
            _ if ask_collision => ReplyDecision {
                accept: true,
                conflict: Some(ConflictAction::Rename),
            },
            _ => ReplyDecision::plain(false),
        };
        // Owner-only cleanup: on the answered path `reply_file_request` already
        // vacated the slot, and the generation check keeps a racing successor's
        // re-parked entry (same sender-chosen id) alive.
        remove_pending_if_owner(&ctx.pending, &session_id, generation);
        (decision.accept, decision.conflict)
    };

    if !accept {
        let _ = send_reply(
            &mut session,
            &session_id,
            false,
            Some(REPLY_DECLINED.into()),
        )
        .await;
        // `code` lets the UI show "you declined" instead of a failure state
        // without string-matching `error` (M4.5). The string stays unchanged.
        let _ = ctx.app.emit(
            "transfer_error",
            json!({ "sessionId": session_id, "error": "declined", "code": "declined" }),
        );
        return Ok(());
    }

    // Accepted — by policy or by hand: refresh the peer's trust bookkeeping.
    // `last_seen` feeds the UI's trusted list, and the DeviceInfo name follows
    // renames without re-pairing. Peers absent from the store are untouched.
    touch_trusted(ctx, &device_id, peer.name.as_deref());

    let _ = ctx.app.emit(
        "transfer_started",
        json!({ "sessionId": session_id, "direction": "receive", "totalSize": manifest.total_size }),
    );

    // Resolve the collision action for THIS transfer (M6.5): "rename"/"overwrite"
    // apply directly; "ask" (and any unknown value) honors the modal choice, or
    // falls back to the safe rename when none was made — the auto-accept path,
    // which never prompts, always lands here as Rename, so nothing is ever
    // overwritten without an explicit instruction. Resume takes precedence over
    // this per file (a resumed file is the same file continuing, not a new
    // collision), which the plan below already encodes.
    let action = match conflict_policy.as_str() {
        "overwrite" => ConflictAction::Overwrite,
        "rename" => ConflictAction::Rename,
        _ => conflict_choice.unwrap_or(ConflictAction::Rename),
    };
    let plan: Vec<FileDisposition> = analyses
        .iter()
        .map(|a| match &a.resume {
            Some(r) => FileDisposition::Resume {
                disk_rel: r.disk_rel.clone(),
                offset: r.offset,
                commit_name: r.commit_name.clone(),
            },
            None => FileDisposition::Fresh {
                rel: a.organized_rel.clone(),
                conflict: action,
            },
        })
        .collect();
    let all_identities: Vec<FileIdentity> = analyses.iter().map(|a| a.identity.clone()).collect();

    let app = ctx.app.clone();
    let sid = session_id.clone();
    let mut last_pct = -1i64;
    // Register live cancel/pause controls for the receive chunk phase
    // (M6.1/6.2); the guard removes them on every exit path below, so a
    // cancel/pause of this id after it finishes is a graceful no-op.
    let (_ctl_guard, ctl) = TransferCtlGuard::register(&ctx.transfers_ctl, &session_id);

    // Concurrency gate (M6.7): bound how many transfers stream at once, across
    // both directions (the send side draws from the same gate). Acquired AFTER
    // the accept decision — so the prompt still showed and trust was refreshed —
    // and held for the whole receive; a lowered cap applies only to transfers
    // not yet past this gate. While the gate is full the transfer QUEUES here;
    // its accept reply is sent inside `run_receive_core`, so the queue is bounded
    // by what remains of the sender's `REPLY_TIMEOUT` (finding): if the gate
    // can't drain before that deadline, sending an accept would land on a socket
    // the sender already closed, so we decline "busy" instead and let both peers
    // resolve deterministically. Our own sends still waiting on THEIR receivers
    // don't count here (see `ConcurrencyGate`). The rate cap (M6.7) is read at
    // the same point.
    let rate_limit = ctx
        .settings
        .read()
        .map(|s| crate::settings::rate_limit_bytes_per_sec(&s.rate_limit))
        .unwrap_or(None);
    let _slot = match ctx.concurrency.try_acquire_receive() {
        Some(guard) => guard,
        None => {
            let _ = ctx.app.emit(
                "transfer_queued",
                json!({ "sessionId": session_id, "direction": "receive" }),
            );
            // The sender has its yes, and is now waiting on a free slot here —
            // not on anyone to answer. Tell it so (F4).
            send_pending(
                &mut session,
                peer.version,
                &session_id,
                PendingState::Queued,
            )
            .await;
            // A cancel issued while QUEUED must win over the wait (finding-2):
            // without this select the acquire ignores the already-registered
            // cancel token, so the transfer stays parked and only aborts once a
            // slot frees and it reaches the chunk loop — after sending an accept
            // reply. Emit the error so the UI row resolves, then bail.
            //
            // The acquire is also bounded by what's LEFT of the sender's reply
            // clock (finding): the prompt already spent part of REPLY_TIMEOUT, so
            // waiting a fresh 150s here would overshoot the sender's deadline and
            // send our accept into a dead socket. `saturating_sub` keeps the
            // deadline non-negative (an accept at the wire) and leaves a small
            // margin so the reply itself still fits. On expiry we decline "busy":
            // the sender then resolves to a deterministic Rejected instead of a
            // confusing Timeout/io pair, and the queued UI row settles on a local
            // Timeout error.
            let gate_deadline = REPLY_TIMEOUT
                .saturating_sub(received_at.elapsed())
                .saturating_sub(Duration::from_secs(5));
            tokio::select! {
                biased;
                _ = ctl.cancel.cancelled() => {
                    // Say so (L15): the sender is still waiting for the reply,
                    // and a dropped connection read there as "connection lost".
                    let _ = send_reply(&mut session, &session_id, false, Some(REPLY_CANCELLED.into())).await;
                    let _ = ctx.app.emit(
                        "transfer_error",
                        json!({ "sessionId": session_id, "error": LanBeamError::Cancelled.to_string(), "code": LanBeamError::Cancelled.ui_code() }),
                    );
                    return Err(LanBeamError::Cancelled);
                }
                // The sender stopped waiting while this sat in the queue (M12's
                // case, one step later): it cancelled, quit or lost the network.
                // Say so now — not once a slot frees and the accept meets a dead
                // socket.
                () = session.closed() => {
                    let e = LanBeamError::ConnectionLost("the sender hung up while the transfer was queued".into());
                    log::info!("{session_id}: {e}");
                    let _ = ctx.app.emit(
                        "transfer_error",
                        json!({ "sessionId": session_id, "error": e.to_string(), "code": e.ui_code() }),
                    );
                    return Err(e);
                }
                r = tokio::time::timeout(gate_deadline, ctx.concurrency.acquire_receive()) => match r {
                    Ok(g) => g,
                    Err(_) => {
                        let _ = send_reply(&mut session, &session_id, false, Some(REPLY_BUSY.into())).await;
                        let _ = ctx.app.emit(
                            "transfer_error",
                            json!({ "sessionId": session_id, "error": LanBeamError::Timeout.to_string(), "code": LanBeamError::Timeout.ui_code() }),
                        );
                        return Err(LanBeamError::Timeout);
                    }
                },
            }
        }
    };

    // Per-file progress/completion events (M6.8).
    let mut on_file = file_event_sink(ctx.app.clone(), session_id.clone());
    // Filled as files are written; on error, pruned to the partials still on
    // disk so we persist exactly those for a later resume (M6.4).
    let mut progress: Vec<FileProgress> = Vec::new();
    // Every file this session actually OPENED (non-pruned), scoping the error
    // path's stale-record removal to files we reached (finding-1).
    let mut opened: Vec<FileIdentity> = Vec::new();
    let saved = run_receive_core(
        &mut session,
        &manifest,
        &download_root,
        &ctl,
        &plan,
        &mut progress,
        &mut opened,
        rate_limit,
        &mut on_file,
        move |recv, total| emit_progress(&app, &sid, "receive", recv, total, &mut last_pct),
    )
    .await;

    match saved {
        Ok(paths) => {
            // Delivered: clear any persisted partials for these files (a resumed
            // transfer that just completed, or a no-op when there were none).
            persist_partials(ctx, &device_id, &all_identities, Vec::new());
            let names: Vec<_> = paths
                .iter()
                .map(|p| {
                    p.file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
                .collect();
            let file_count = paths.len();
            // Kept before `paths` moves into the completed log below.
            let first_saved = paths.first().cloned();
            // Poisoned lock = graceful no-op, not a cascade panic (the invariant
            // every other registry access in this file upholds): a panicked
            // holder of `completed` must degrade to a skipped history entry, not
            // crash the per-connection receive task. The transfer is already
            // saved; the completed-log insert is only bookkeeping for `reveal`.
            if let Ok(mut log) = ctx.completed.lock() {
                log.insert(session_id.clone(), paths);
            }
            let _ = ctx.app.emit(
                "transfer_done",
                json!({ "sessionId": session_id, "direction": "receive", "savedNames": names }),
            );
            // Completion toast for receives (M5.4) — the transfer may have run
            // for minutes with the window hidden in the tray. Send completions
            // stay quiet: the user just drove that flow in the foreground.
            notify_system(
                ctx,
                &sender_name,
                &format!("✓ {} · {}", file_count, human_size(manifest.total_size)),
            );
            reveal_if_asked(ctx, first_saved.as_deref());
            Ok(())
        }
        Err(e) => {
            // A failed receive is at least as worth interrupting for as a
            // successful one — and it used to be the only one that said nothing
            // at all, so a transfer that ran for ten minutes and then threw its
            // bytes away did it in total silence. Locale-neutral like the rest
            // (a cross, a count, a size); the WHY is the in-app toast's job,
            // where there is an i18n layer to say it in the user's language.
            notify_system(
                ctx,
                &sender_name,
                &format!(
                    "✕ {} · {}",
                    manifest.files.len(),
                    human_size(manifest.total_size)
                ),
            );
            // Record resumable partials (M6.4): `progress` was pruned by
            // `run_receive_core` to the files still on disk (hashed files kept
            // through an interruption; a corrupt file deleted, its verified
            // siblings left alone), so persist exactly those and drop the rest.
            // `set_files` replaces this manifest's identities, so a corrupt or
            // deleted file's stale partial is cleared even when nothing is kept.
            let kept: Vec<PartialRecord> = progress
                .iter()
                .map(|w| PartialRecord {
                    rel: w.identity.rel.clone(),
                    size: w.identity.size,
                    sha256: w.identity.sha256.clone(),
                    disk_rel: w.disk_rel.clone(),
                    bytes_written: w.bytes_written,
                    commit_name: w.commit_name.clone(),
                    root: Some(download_root.to_string_lossy().into_owned()),
                })
                .collect();
            // Scope removal to files this session OPENED (finding-1), NOT every
            // manifest identity: an interruption that never reached a later file
            // must leave that file's still-valid persisted partial intact instead
            // of wiping it (which would force a full re-download and orphan its
            // on-disk bytes). Files opened-then-deleted (non-hashed / integrity
            // failure) are in `opened` too, so their stale records still clear.
            persist_partials(ctx, &device_id, &opened, kept);
            let _ = ctx.app.emit(
                "transfer_error",
                json!({ "sessionId": session_id, "error": e.to_string(), "code": e.ui_code() }),
            );
            Err(e)
        }
    }
}

/// One file's resume + collision analysis, computed before the accept prompt so
/// the reply can carry offsets (M6.4) and the event can list conflicts (M6.5).
pub(super) struct FileAnalysis {
    /// Stable identity (sanitized rel + size + hash) — the partials-store key.
    identity: FileIdentity,
    /// Where a FRESH copy lands (auto-organize applied, pre-de-dupe).
    organized_rel: PathBuf,
    /// Set when a valid persisted partial lets this file continue instead of
    /// starting over.
    pub(super) resume: Option<ResumeFrom>,
    /// A fresh write would collide with an existing file (feeds the conflict UI).
    collides: bool,
}

/// A persisted partial a file can continue from (M6.4) — `PartialRecord`'s
/// fields that the receive needs.
pub(super) struct ResumeFrom {
    disk_rel: PathBuf,
    offset: u64,
    commit_name: Option<String>,
}

/// Analyze every manifest file for resume (M6.4) and collision (M6.5). Resume is
/// offered ONLY for a hashed file whose stored partial still matches the bytes
/// on disk (length == recorded, not a symlink) — the whole-file hash is what
/// makes continuing `old-bytes + tail` verifiable and safe against a sender that
/// ignores offsets, so a non-hashed file always starts fresh. Names are assumed
/// already validated by `validate_manifest`.
pub(super) fn analyze_files(
    manifest: &Manifest,
    download_root: &Path,
    organize: &Organize,
    device_id: &str,
    partials: &RwLock<crate::partials::PartialsStore>,
) -> Vec<FileAnalysis> {
    manifest
        .files
        .iter()
        .map(|f| {
            let rel = match sanitize::validate(&f.name) {
                Decision::Accept(p) => p,
                // validate_manifest already passed; keep a harmless empty rel so
                // one odd entry cannot panic the analysis (the write path rejects).
                Decision::Reject(_) => PathBuf::new(),
            };
            let identity = FileIdentity {
                rel: rel.to_string_lossy().replace('\\', "/"),
                size: f.size,
                sha256: f.sha256.clone(),
            };
            let organized_rel = organize.place(&rel);
            // Resume only a hashed file with a matching, intact on-disk partial,
            // written under the folder we are receiving into now: a partial left
            // in the old folder is not the file of that name in the new one (M2).
            let resume = if f.sha256.is_some() {
                partials
                    .read()
                    .ok()
                    .and_then(|s| s.lookup(device_id, &identity))
                    .filter(|rec| {
                        rec.root
                            .as_deref()
                            .is_none_or(|r| Path::new(r) == download_root)
                    })
                    .filter(|rec| {
                        let disk = download_root.join(&rec.disk_rel);
                        match std::fs::symlink_metadata(&disk) {
                            Ok(m) if !m.file_type().is_symlink() => m.len() == rec.bytes_written,
                            _ => false,
                        }
                    })
                    .map(|rec| ResumeFrom {
                        disk_rel: PathBuf::from(&rec.disk_rel),
                        offset: rec.bytes_written,
                        commit_name: rec.commit_name.clone(),
                    })
            } else {
                None
            };
            // A fresh (non-resumed) write collides when its target already exists.
            let collides = resume.is_none()
                && std::fs::symlink_metadata(download_root.join(&organized_rel)).is_ok();
            FileAnalysis {
                identity,
                organized_rel,
                resume,
                collides,
            }
        })
        .collect()
}

/// Persist the resume-state change for `device_id` after a receive (M6.4):
/// `remove` = this manifest's identities, `keep` = the partials still on disk
/// (empty on success). Mutates under the write guard, snapshots, and writes on
/// the blocking pool — no disk I/O under the lock (the trust-store pattern).
fn persist_partials<R: Runtime>(
    ctx: &TransportCtx<R>,
    device_id: &str,
    remove: &[FileIdentity],
    keep: Vec<PartialRecord>,
) {
    let snapshot = {
        // Poisoned lock: skip — losing resume state costs at most a re-download,
        // never a cascade of the panic.
        let Ok(mut store) = ctx.partials.write() else {
            return;
        };
        if !store.set_files(device_id, remove, keep) {
            return; // nothing changed — no needless write
        }
        store.snapshot()
    };
    crate::partials::persist_async(snapshot);
}

/// Today's LOCAL date as `YYYY-MM-DD` for the auto-organize date folder (M6.6).
/// Local, not UTC (H8): it used to be UTC — hand-rolled to avoid a date crate —
/// so in UTC+8 everything received before 08:00 went into YESTERDAY's folder.
/// The local offset needs the OS time-zone database, which is what chrono's
/// `clock` feature reads.
pub(super) fn today_ymd() -> String {
    ymd_in(chrono::Local::now())
}

/// `YYYY-MM-DD` of an instant in its own time zone — split out so a test can
/// pin both the instant and the offset.
pub(super) fn ymd_in<Tz: chrono::TimeZone>(t: chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    t.format("%Y-%m-%d").to_string()
}
