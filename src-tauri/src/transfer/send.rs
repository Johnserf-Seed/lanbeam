//! The send side: the manifest, the wait for the receiver's answer, the chunk
//! stream, and the final ack.

use std::io::SeekFrom;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::pacing::{wait_while_paused, RateLimiter};
use super::progress::FileEvent;
use super::receive::{peer_ack_error, peer_reply_error, PendingState};
use super::wire::{recv_control, send_control};
use super::SendItem;
use crate::consts::{
    ACK_DRAIN_RATE, ACK_TIMEOUT, ACK_TIMEOUT_MAX, CHUNK_IDLE_TIMEOUT, MANIFEST_TIMEOUT,
    MAX_FILE_DATA, REPLY_TIMEOUT,
};
use crate::error::{LanBeamError, Result};
use crate::protocol::{self, AppMessage, ResumeOffset};
use crate::state::TransferControl;
use crate::transport::noise::NoiseSession;
use crate::trust;

/// Send the manifest, await acceptance, then stream every file chunked.
///
/// `ctl` carries the session's cancel token + pause flag (M6.1/6.2): each
/// chunk parks while paused and a cancel ends the loop promptly with
/// `Cancelled` (the caller then drops the session, so the receiver aborts too).
///
/// `rate_limit` is an optional byte-per-second cap (M6.7, `None` = unlimited),
/// and `on_file` receives a [`FileEvent`] as each file streams and finishes
/// (M6.8) — both no-ops for callers that pass `None`/`&mut |_| {}`.
pub async fn run_send<F: FnMut(u64, u64)>(
    session: &mut NoiseSession,
    transfer_id: &str,
    items: &[SendItem],
    ctl: &TransferControl,
    rate_limit: Option<u64>,
    // `+ Send`: `handle_incoming`'s receive task and the async send command both
    // hold this across `.await`, so the boxed sink must be Send for the future.
    on_file: &mut (dyn FnMut(FileEvent<'_>) + Send),
    mut on_progress: F,
) -> Result<()> {
    let total_size: u64 = items.iter().map(|i| i.meta.size).sum();
    let mut limiter = RateLimiter::new(rate_limit);

    // The manifest preamble is small control traffic; bound it as ONE unit —
    // mirroring the receiver, which reads the whole opening under this same
    // deadline — so a peer that stops reading cannot wedge these writes on
    // TCP backpressure (M4.5).
    let preamble = async {
        send_control(
            session,
            &AppMessage::FileListBegin {
                transfer_id: transfer_id.to_string(),
                total_size,
                file_count: items.len() as u32,
            },
        )
        .await?;
        for it in items {
            send_control(
                session,
                &AppMessage::FileListEntry {
                    file: it.meta.clone(),
                },
            )
            .await?;
        }
        send_control(
            session,
            &AppMessage::FileListEnd {
                transfer_id: transfer_id.to_string(),
            },
        )
        .await
    };
    // Cancel wins over the wait (finding), same as the chunk loop below: a user
    // who cancels while this is parked must free the ConcurrencyGate slot at
    // once, not after the peer finally answers. The cancel arm returns and the
    // caller drops `session`, so nothing reuses the mutable borrow afterwards.
    tokio::select! {
        biased;
        _ = ctl.cancel.cancelled() => return Err(LanBeamError::Cancelled),
        r = tokio::time::timeout(MANIFEST_TIMEOUT, preamble) => match r {
            Ok(res) => res?,
            Err(_) => return Err(LanBeamError::Timeout),
        }
    }

    // Bounded reply wait (M4.5): a live receiver ALWAYS answers — its accept
    // prompt auto-declines at 120s — so only a peer that vanished silently
    // (sleep, power loss, WiFi drop with no RST) leaves this read pending,
    // and idle keepalive-free TCP would never notice on its own. NOT
    // ACK_TIMEOUT: that equals the prompt window exactly, so a user accepting
    // at the deadline would race the sender's timeout.
    // Cancel wins over the reply wait (finding): without this a cancel issued
    // while the receiver's accept prompt is up is not observed until the first
    // chunk after the peer answers — up to REPLY_TIMEOUT (150s) with the slot
    // held. Mirrors the chunk loop's biased select.
    // A v3 receiver may first say where the request stands (F4) — on its screen,
    // or queued there for a slot — any number of times; the wait goes on under
    // the same deadline, so no stream of notices can stretch it.
    let deadline = tokio::time::Instant::now() + REPLY_TIMEOUT;
    let reply = loop {
        let msg = tokio::select! {
            biased;
            _ = ctl.cancel.cancelled() => return Err(LanBeamError::Cancelled),
            r = tokio::time::timeout_at(deadline, recv_control(session)) => match r {
                Ok(res) => res?,
                Err(_) => return Err(LanBeamError::Timeout),
            }
        };
        match msg {
            AppMessage::Pending { state, .. } => {
                if let Some(state) = PendingState::from_wire(&state) {
                    on_file(FileEvent::Pending(state));
                }
            }
            other => break other,
        }
    };
    // Per-file resume offsets (M6.4): a receiver that holds a verified prefix of
    // some files asks us to stream only the tail. `None`/absent (legacy receiver
    // or a fresh transfer) means "start every file at 0". The whole-file SHA-256
    // already rode the manifest (build_send_list hashes the WHOLE file), so the
    // receiver verifies old-bytes + tail — nothing to re-hash here.
    let sparse = match reply {
        AppMessage::FileSendReply {
            accept: true,
            offsets,
            ..
        } => offsets.unwrap_or_default(),
        AppMessage::FileSendReply {
            accept: false,
            reason,
            ..
        } => {
            return Err(peer_reply_error(reason.as_deref()));
        }
        other => {
            return Err(LanBeamError::Protocol(format!(
                "expected reply, got {}",
                trust::clamp_peer_text(&format!("{other:?}"))
            )))
        }
    };
    // The receiver said yes. Only NOW is the request accepted — the connection
    // and the SAS existed long before, while the receiver's prompt was up.
    on_file(FileEvent::Accepted);
    // Densify the SPARSE reply into a per-file offset vector. Guard every pair:
    // an out-of-range index (a malformed/oversized reply) is ignored, and each
    // offset is clamped to its file size so a hostile value can only skip within
    // the file, never seek past its end. A duplicate index keeps the last pair.
    let mut offsets = vec![0u64; items.len()];
    for ResumeOffset { index, offset } in sparse {
        if let Some(slot) = offsets.get_mut(index as usize) {
            *slot = offset.min(items[index as usize].meta.size);
        }
    }

    // Seed progress with the bytes the receiver already holds so the bar reflects
    // total delivered, not just what THIS session streams.
    let mut sent: u64 = offsets.iter().sum::<u64>().min(total_size);
    // Per-chunk write deadline. On a RESUMED transfer the receiver re-reads and
    // SHA-256-folds each file's existing prefix from disk BEFORE it reads the
    // tail off the socket, so it stops draining this loop's writes for the
    // fold's duration; on slow media a multi-GB prefix can outlast the fixed
    // CHUNK_IDLE_TIMEOUT (60s), timing out the send on every retry so the file
    // can never complete (finding). Grant extra time scaled by the total
    // resumed bytes at the same drain rate as `ack_deadline`, capped identically
    // so a genuinely dead receiver is still bounded. Because TCP pipelining
    // hides WHICH write races the fold (a small tail may push it onto a later
    // file's chunk), the allowance covers every chunk of the transfer, not just
    // the resumed file's first. For a fresh send or a legacy peer every offset
    // is 0, so this is exactly CHUNK_IDLE_TIMEOUT — no behavior change.
    let resume_grace = Duration::from_secs(offsets.iter().sum::<u64>() / ACK_DRAIN_RATE);
    let chunk_write_deadline = (CHUNK_IDLE_TIMEOUT + resume_grace).min(ACK_TIMEOUT_MAX);
    let mut buf = vec![0u8; MAX_FILE_DATA];
    for (i, it) in items.iter().enumerate() {
        // Already clamped to the file size when densified above.
        let offset = offsets[i];
        // Stage disk reads through a 1 MiB BufReader so the loop pulls ~16 wire
        // chunks per blocking-pool hop instead of one hop per 64 KiB frame — a
        // measurable win on 10 GbE where the per-chunk round-trip no longer
        // hides behind the wire (finding). The seek below discards the (empty)
        // buffer harmlessly; it runs once per file, before any read.
        let mut f =
            tokio::io::BufReader::with_capacity(1 << 20, tokio::fs::File::open(&it.abs).await?);
        if offset > 0 {
            f.seek(SeekFrom::Start(offset)).await?;
        }
        let mut index = 0u32;
        // Per-file byte counter for the M6.8 progress event; seeded at the
        // resume offset so a resumed file's bar reflects total delivered.
        let mut file_sent = offset;
        // Stream EXACTLY the manifest's declared tail (`meta.size - offset`), NOT
        // the file's live byte count (finding-0). The receiver slices the chunk
        // stream by `meta.size` alone with no per-file end marker, so if a file
        // mutated since `build_send_list` changed how many bytes we put on the
        // wire, the drift would bleed across the file boundary: a GROWN file's
        // tail would be consumed as the NEXT file's data, a SHRUNK one would
        // leave the receiver reading on into the next file's frames — corrupting
        // or aborting files that never changed. Bounding here keeps total wire
        // bytes == sum(meta.size) regardless of concurrent mutation, so a changed
        // file fails ONLY its own SHA-256 check, never desyncs its neighbours.
        let mut remaining = it.meta.size.saturating_sub(offset);
        while remaining > 0 {
            // Pause gate (M6.2): park before producing the next chunk so a
            // paused sender simply stops writing → TCP backpressure stalls the
            // peer. A cancel while parked ends the loop promptly. The park is
            // bounded (finding-4): if it auto-resumes at the cap, tell the UI so
            // a >MAX_PAUSE_PARK pause never leaves the receiver idling into an
            // abort — it resumes here first.
            if wait_while_paused(ctl).await? {
                on_file(FileEvent::PauseAutoResumed);
            }
            // Never read past the declared tail: a file that GREW would otherwise
            // leak its extra bytes into the next file's frames.
            let want = remaining.min(buf.len() as u64) as usize;
            let n = f.read(&mut buf[..want]).await?;
            if n == 0 {
                // EOF before `meta.size`: the file SHRANK since the manifest was
                // built. Fail THIS file locally (identified by name) instead of
                // streaming a short count that would pull the next file's frames
                // early and desync every file after it (finding-0).
                return Err(LanBeamError::Io(format!(
                    "{}: file is shorter than its manifest size ({} of {} bytes)",
                    it.meta.name,
                    it.meta.size - remaining,
                    it.meta.size,
                )));
            }
            // Write deadline: a receiver that accepts then stops reading (hung
            // app, zero TCP window) must not wedge this loop on backpressure —
            // the same idle bound the receive side applies between chunks (M4.5).
            // Cancel (M6.1) wins over the write (biased); the caller then drops
            // the session, so the receiver's read fails through its error path.
            let frame = protocol::encode_file_chunk(index, &buf[..n])?;
            tokio::select! {
                biased;
                _ = ctl.cancel.cancelled() => return Err(LanBeamError::Cancelled),
                r = tokio::time::timeout(chunk_write_deadline, session.write_msg(&frame)) => match r {
                    Ok(res) => res?,
                    Err(_) => return Err(LanBeamError::Timeout),
                }
            }
            index += 1;
            sent += n as u64;
            file_sent += n as u64;
            remaining -= n as u64;
            on_progress(sent, total_size);
            on_file(FileEvent::Streaming {
                index: i,
                name: &it.meta.name,
                done: file_sent,
                total: it.meta.size,
            });
            // Pace the write loop after accounting the chunk (M6.7); a no-op
            // when unthrottled.
            limiter.throttle(n).await;
        }
        // This file's bytes are all on the wire (M6.8). `verified` on the send
        // side means an integrity digest was attached — the peer verifies it.
        on_file(FileEvent::Done {
            index: i,
            verified: it.meta.sha256.is_some(),
        });
    }

    // Bounded ack wait: the receiver may still be flushing/syncing large files
    // after the last chunk, so the deadline scales with the transfer size —
    // but a dead peer must not pin this task (and the whole send_files
    // command) forever (M4.5). See `ack_deadline`.
    // Cancel wins over the ack wait too (finding): `ack_deadline` scales to
    // ACK_TIMEOUT_MAX (15 min) for slow media, so a cancel here must not leave
    // the slot pinned that long. Note the semantic edge: every byte is already
    // on the wire, so cancelling now reports Cancelled on this side while the
    // receiver may still verify + save and show success — acceptable, the peer
    // already has the data, and it matches the existing cancel-drops-session
    // behavior.
    let ack = tokio::select! {
        biased;
        _ = ctl.cancel.cancelled() => return Err(LanBeamError::Cancelled),
        r = tokio::time::timeout(ack_deadline(total_size), recv_control(session)) => match r {
            Ok(res) => res?,
            Err(_) => return Err(LanBeamError::Timeout),
        }
    };
    match ack {
        AppMessage::TransferAck { ok: true, .. } => Ok(()),
        AppMessage::TransferAck {
            ok: false, error, ..
        } => Err(peer_ack_error(error.as_deref())),
        other => Err(LanBeamError::Protocol(format!(
            "expected ack, got {}",
            trust::clamp_peer_text(&format!("{other:?}"))
        ))),
    }
}

/// The sender's deadline for the final `TransferAck`, scaled with the transfer
/// size. WHY not a fixed 120s: the receiver flushes + syncs every file before
/// acking, and slow media (USB stick, SD card, SMR disk) can take minutes to
/// drain gigabytes of page cache even though the wire was fast — a fixed
/// deadline misreported such transfers as timeouts while the receiver saved
/// everything. Base `ACK_TIMEOUT` plus the drain time at `ACK_DRAIN_RATE`
/// (~20 MB/s), capped at `ACK_TIMEOUT_MAX` (15 min) so a dead peer still
/// cannot pin the send task for long (M4.5).
pub(super) fn ack_deadline(total_size: u64) -> Duration {
    let drain = Duration::from_secs(total_size / ACK_DRAIN_RATE);
    (ACK_TIMEOUT + drain).min(ACK_TIMEOUT_MAX)
}
