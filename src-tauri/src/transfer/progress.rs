//! Progress reporting (M6.8): the events the chunk loops hand their `on_file`
//! sink, the sink that turns them into UI events, and the throttled
//! whole-transfer `transfer_progress` emit.

use serde_json::json;
use tauri::{AppHandle, Emitter, Runtime};

use super::receive::PendingState;

/// Events the send/receive chunk loops hand to their `on_file` sink (M6.8):
/// per-file progress + completion, plus one transfer-level pause signal. The
/// caller turns these into `transfer_file_progress` / `transfer_file_done` /
/// `transfer_resumed` events; tests and the pre-M6.8 internal entry points pass
/// a no-op sink (`&mut |_| {}`). WHY the pause signal rides this same sink: it
/// already carries the session id + app handle to both loops, so reusing it
/// avoids threading a second emit path (and a fourth closure) through
/// `run_send`/`run_receive_core`.
pub enum FileEvent<'a> {
    /// `done` of `total` bytes delivered for manifest file `index`. `name` is
    /// the sender-declared leaf/rel path (the receiver's own on-disk name may
    /// differ after de-dupe; the UI keys the per-file row by `index`).
    Streaming {
        index: usize,
        name: &'a str,
        done: u64,
        total: u64,
    },
    /// File `index` finished. `verified` is the SHA-256 outcome (M6.3): on the
    /// RECEIVE side `true` means a digest was present AND matched (a mismatch
    /// never reaches here — it errors the transfer first); on the SEND side it
    /// means a digest was ATTACHED (the peer will verify it). Both reduce to
    /// "this file carried a verified hash" — exactly `sha256.is_some()` at a
    /// successful completion.
    Done { index: usize, verified: bool },
    /// The session's pause hit its [`MAX_PAUSE_PARK`] cap and AUTO-RESUMED
    /// (finding-4) — pause is bounded so it can never outlive the peer's idle
    /// timeout. NOT per-file: the sink emits `transfer_resumed` so the UI clears
    /// its optimistic `paused` flag (there is no other backend pause event).
    ///
    /// [`MAX_PAUSE_PARK`]: crate::consts::MAX_PAUSE_PARK
    PauseAutoResumed,
    /// SEND side only: the receiver answered `FileSendReply{accept:true}`. NOT
    /// per-file either: the sink emits `transfer_accepted`. Until this, the
    /// receiver's prompt is up and the sender's SAS is the thing to compare —
    /// `transfer_started` (the connection is open) used to be read as "they
    /// accepted", which closed the sender's SAS before anyone could look at it.
    Accepted,
    /// SEND side only: before answering, the receiver said where the request
    /// stands (F4). The sink emits `transfer_pending`.
    Pending(PendingState),
}

/// Build the per-file event sink (M6.8) shared by `send_files` and
/// `handle_incoming`: emits `transfer_file_progress` — throttled to a
/// whole-percent change PER FILE, mirroring [`emit_progress`]'s aggregate
/// throttle — and `transfer_file_done`. Returned as a closure for
/// `run_send`/`run_receive_core`'s `on_file` parameter.
pub fn file_event_sink<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> impl FnMut(FileEvent<'_>) {
    // Track the last emitted percent for the CURRENT file; reset when the index
    // advances so each file's bar starts fresh at 0.
    let mut last_index = usize::MAX;
    let mut last_pct = -1i64;
    move |event| match event {
        FileEvent::Streaming {
            index,
            name,
            done,
            total,
        } => {
            if index != last_index {
                last_index = index;
                last_pct = -1;
            }
            let pct = whole_percent(done, total);
            if pct != last_pct {
                last_pct = pct;
                let _ = app.emit(
                    "transfer_file_progress",
                    json!({
                        "sessionId": session_id,
                        "fileIndex": index,
                        "fileName": name,
                        "done": done,
                        "total": total,
                        "percent": pct,
                    }),
                );
            }
        }
        FileEvent::Done { index, verified } => {
            let _ = app.emit(
                "transfer_file_done",
                json!({ "sessionId": session_id, "fileIndex": index, "verified": verified }),
            );
        }
        FileEvent::PauseAutoResumed => {
            // Pause is time-limited (finding-4): the loop resumed on its own so
            // the peer's idle timeout could never fire. Tell the UI so its
            // optimistic pause flag clears instead of lying about a paused row
            // whose bytes are moving again.
            let _ = app.emit(
                "transfer_resumed",
                json!({ "sessionId": session_id, "reason": "pause_timeout" }),
            );
        }
        FileEvent::Accepted => {
            // The receiver said yes: the UI's "waiting for them to accept" (and
            // the SAS it keeps on screen for comparing) ends here, not at
            // transfer_started.
            let _ = app.emit(
                "transfer_accepted",
                json!({ "sessionId": session_id, "direction": "send" }),
            );
        }
        FileEvent::Pending(state) => {
            // Still waiting, and now the UI can say on what: a person, or a
            // free slot on a receiver that already said yes.
            let _ = app.emit(
                "transfer_pending",
                json!({ "sessionId": session_id, "direction": "send", "state": state.as_wire() }),
            );
        }
    }
}

/// `done` of `total` as a whole percent; an empty total is complete.
fn whole_percent(done: u64, total: u64) -> i64 {
    if total == 0 {
        100
    } else {
        ((done as u128 * 100) / total as u128) as i64
    }
}

/// Throttled progress emit — only fires on a whole-percent change.
pub fn emit_progress<R: Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    direction: &str,
    done: u64,
    total: u64,
    last_pct: &mut i64,
) {
    let pct = whole_percent(done, total);
    if pct != *last_pct {
        *last_pct = pct;
        let _ = app.emit(
            "transfer_progress",
            json!({ "sessionId": session_id, "direction": direction, "done": done, "total": total, "percent": pct }),
        );
    }
}
