//! The quick-text receive side (M7.3): the flood and trust gates, the
//! optional clipboard mirror, the `text_received` event and the ack.

use std::time::Instant;

use serde_json::json;
use tauri::{Emitter, Runtime};

use super::inbound::{discovered_name, peer_display_name};
use super::wire::{send_control, PeerHello, TEXT_ACK, TEXT_DROP_THROTTLED, TEXT_DROP_UNTRUSTED};
use crate::consts::MAX_TEXT_BYTES;
use crate::error::Result;
use crate::protocol::AppMessage;
use crate::transport::noise::NoiseSession;
use crate::transport::TransportCtx;

/// Handle an inbound quick text, gated on `TRANSFER_V2` upstream. Emits
/// `text_received` for the inbox, optionally mirrors the text to THIS machine's
/// clipboard, acks, and CLOSES — it never touches disk and never falls through
/// to the file-receive path.
///
/// Clipboard policy (privacy): the mirror happens only when BOTH the receiver's
/// own `clip_share` consent setting is on AND the sender asked for it
/// (`to_clipboard == Some(true)`, the UI's 同时写入对方剪贴板 toggle). The
/// receiver's setting is the master switch — no peer can place text on this
/// machine's clipboard unless the user opted into clipboard sharing — so a
/// sender's request alone is never enough. The write is best-effort: a failure
/// (or a build without the clipboard plugin) is logged, never fatal, and never
/// blocks the ack.
pub(super) async fn handle_text_received<R: Runtime>(
    session: &mut NoiseSession,
    ctx: &TransportCtx<R>,
    device_id: &str,
    text: String,
    to_clipboard: Option<bool>,
    peer: PeerHello,
) -> Result<()> {
    // Bound memory before anything downstream sees the text (M4.5): a single
    // frame already caps the size, but clamp again so the inbox event, the
    // clipboard, and the logs never handle an over-long string.
    let text = clamp_text(text);

    // (1) Flood control (M7.3 hardening): cap how many texts one source may
    // deliver per window. Quick text has NO accept-prompt, so an unthrottled peer
    // could spam the inbox / notifications — this is the text analog of the
    // pairing throttle. A throttled text is DROPPED (not surfaced) but still
    // acked, so the sender resolves rather than hangs and gets no spam oracle.
    if let Some(ip) = session.peer_addr().map(|a| a.ip()) {
        let admitted = ctx
            .text_rate
            .lock()
            .map(|mut r| r.admit(ip, Instant::now()))
            .unwrap_or(true);
        if !admitted {
            log::warn!("quick text from {ip} throttled; dropping");
            ack_text(session, Some(TEXT_DROP_THROTTLED)).await;
            return Ok(());
        }
    }

    // (2) Delivery gate (M7.3 hardening): mirror the file path's recv_policy /
    // trust decision. The file path PARKS an untrusted sender for a prompt; quick
    // text has no prompt, so instead it is DROPPED unless the sender is trusted OR
    // the policy is "all" — text obeys the same "strangers don't get through under
    // ask/trusted" contract files do, and a stranger's note/link is never
    // surfaced unconditionally. This deliberately differs from `should_auto_accept`
    // (which returns false for EVERYONE under "ask", where files PROMPT): text
    // cannot prompt, so "ask"/"trusted" here mean "trusted senders only" rather
    // than "drop everything". A dropped text is still acked (no trust/policy
    // oracle to a stranger); a poisoned lock fails toward the safe side (drop for
    // any non-"all" policy).
    let policy = ctx
        .settings
        .read()
        .map(|s| s.recv_policy.clone())
        .unwrap_or_else(|_| "ask".to_string());
    let is_trusted = ctx
        .trusted
        .read()
        .ok()
        .is_some_and(|t| t.get(device_id).is_some());
    if !is_trusted && policy != "all" {
        log::info!("quick text from {device_id} dropped (untrusted, recv_policy \"{policy}\")");
        // Tell the sender it was dropped. Their `send_text` now fails instead of
        // reporting a delivery that never happened.
        ack_text(session, Some(TEXT_DROP_UNTRUSTED)).await;
        return Ok(());
    }

    // Friendly sender name: the peer's DeviceInfo name (present even for peers
    // absent from discovery), then the discovery table, then the short device
    // id — the same precedence the file-accept prompt uses.
    let sender_name = peer_display_name(peer.name.as_deref(), device_id, || {
        discovered_name(ctx, device_id)
    });

    // Receiver consent: mirror to the clipboard only when the user enabled
    // clip_share AND the sender requested it. Read the setting at fire time so a
    // just-toggled preference takes effect on the very next text.
    let clip_share = ctx.settings.read().map(|s| s.clip_share).unwrap_or(false);
    if clip_share && to_clipboard == Some(true) {
        write_clipboard(ctx, &text);
    }

    // Millisecond unix stamp so the inbox can render a real receive time.
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    log::info!(
        "quick text from {sender_name:?} ({device_id}, {} bytes)",
        text.len()
    );
    let _ = ctx.app.emit(
        "text_received",
        json!({ "deviceId": device_id, "senderName": sender_name, "text": text, "at": at }),
    );

    // Ack so the sender's `send_text` resolves; a failed ack write is only
    // logged (best-effort, like the file path's final ack) — the text already
    // reached the UI, so a lost ack must not turn a delivered text into an error.
    ack_text(session, None).await;
    Ok(())
}

/// Best-effort quick-text ack. The sender's `send_text` resolves on ANY ack, so
/// send one and only LOG a write failure — a text that was delivered (or was
/// deliberately dropped by the trust/flood gates) must never surface to the peer
/// as an error, and a dropped text must not hand the sender a delivery oracle.
/// Ack a quick text. `dropped` carries the reason when it did NOT reach the user,
/// so the sender can fail instead of reporting a delivery that never happened.
async fn ack_text(session: &mut NoiseSession, dropped: Option<&str>) {
    if let Err(e) = send_control(
        session,
        &AppMessage::Ack {
            of: TEXT_ACK.to_string(),
            delivered: Some(dropped.is_none()),
            reason: dropped.map(str::to_string),
        },
    )
    .await
    {
        log::warn!("quick-text ack send failed: {e}");
    }
}

/// Truncate a quick text to [`MAX_TEXT_BYTES`] on a char boundary (M7.3). A
/// single control frame already bounds the incoming size, so this only ever
/// trims a deliberately-oversized push — and never splits a multi-byte char, so
/// the emitted string is always valid UTF-8.
pub(super) fn clamp_text(mut text: String) -> String {
    if text.len() <= MAX_TEXT_BYTES {
        return text;
    }
    let mut end = MAX_TEXT_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

/// Best-effort write to THIS machine's clipboard (M7.3). Uses `try_state`, NOT
/// the `ClipboardExt` accessor, for the same reason as `notify_system`: the ext
/// panics when the plugin is absent, and the integration tests drive the real
/// `handle_incoming` on a mock app with no plugins — a missing clipboard must
/// degrade to a no-op, never take the session down with it.
fn write_clipboard<R: Runtime>(ctx: &TransportCtx<R>, text: &str) {
    use tauri::Manager;
    let Some(clip) = ctx
        .app
        .try_state::<tauri_plugin_clipboard_manager::Clipboard<R>>()
    else {
        log::debug!("clipboard plugin unavailable; skipping clipboard mirror");
        return;
    };
    if let Err(e) = clip.write_text(text.to_string()) {
        log::warn!("clipboard write failed: {e}");
    }
}
