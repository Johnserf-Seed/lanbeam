//! The host side of pairing (M7.1): checking the code a joiner presents,
//! answering it, and handing the SAS to the UI. No trust is granted here.

use std::time::Instant;

use serde_json::json;
use tauri::{Emitter, Runtime};

use super::inbound::peer_display_name;
use super::wire::{local_device_name, send_control, PeerHello};
use crate::error::{LanBeamError, Result};
use crate::protocol::AppMessage;
use crate::transport::noise::NoiseSession;
use crate::transport::TransportCtx;

/// Handle an inbound pairing redemption, gated on `TRANSFER_V2` upstream. Runs
/// the whole exchange and CLOSES the session — it never falls through to the
/// file-receive path.
///
/// Order: (1) per-source rate gate — a source over the failure cap is DROPPED
/// without even checking the code, so the throttle, not the 6 digits, is the
/// real brute-force defense; (2) match the presented code against the active
/// invitation (constant-time-ish, consumed on success); (3a) on a match, record
/// the joiner as a dialable manual peer (its source IP + the listen `port` it
/// declared), reply `PairConfirm{accept:true}`, and emit `pair_joined` with the
/// SAS — NO trust is written here, the UI does that once a human has compared
/// the SAS; (3b) on a miss, record the failure, reply `PairConfirm{accept:false}`
/// with a reason, and change nothing. A reject is a clean `Ok(())` for the
/// listener (like a declined file), not an error.
pub(super) async fn handle_pair_request<R: Runtime>(
    session: &mut NoiseSession,
    ctx: &TransportCtx<R>,
    device_id: &str,
    code: String,
    port: Option<u16>,
    peer: PeerHello,
) -> Result<()> {
    let peer_ip = session.peer_addr().map(|a| a.ip());
    let now = Instant::now();
    // Tolerate the way a human reads a code off a screen ("482 913", full-width
    // digits from a CJK IME): the display groups the digits, so the compare must
    // not punish typing them that way — every miss costs rate-limit budget.
    let code = normalize_pair_code(&code);

    // (1) Rate gate — atomic check-and-RESERVE under one lock hold. A limited
    // source is DROPPED (no reply): answering would hand a brute-forcer a fast,
    // cheap accept/reject oracle. The join side surfaces the dropped connection as
    // a plain failure. Reserving the attempt here (rather than a read-only check
    // now and a failure recorded later) is what closes the concurrent-burst race:
    // N simultaneous PairRequests from one IP each see the reservations of the
    // ones before them, so at most MAX_PAIR_FAILURES pass within a window instead
    // of all N reading a stale count-0. A poisoned lock fails open, as before. The
    // reservation is provisional — a miss against a live code keeps it (the real
    // failure), a successful pair clears it, and a request at an IDLE host rolls
    // it back below, so spamming an idle host still can't grow the map or drain
    // the per-source budget.
    if let Some(ip) = peer_ip {
        let allowed = ctx
            .pairing
            .rate
            .lock()
            .map(|mut r| r.reserve_attempt(ip, now))
            .unwrap_or(true);
        if !allowed {
            log::warn!("pairing: rate-limited PairRequest from {ip}; dropping");
            return Err(LanBeamError::Protocol("pairing attempts throttled".into()));
        }
    }

    // (2) Match against the active invitation. A poisoned lock fails toward
    // reject. Decide first (borrowing the guard), THEN mutate — a match consumes
    // the code (single-use) and an expired invitation is cleared in passing;
    // computing the `(clear, ok, had_session)` tuple up front releases the read
    // borrow before the write. `had_session` gates failure recording below: a
    // PairRequest arriving while NO invitation is active has nothing to
    // brute-force, so it must not be recorded (that is the unbounded-growth
    // amplification vector — spam PairRequests at an idle host to grow the map).
    let (matched, had_session) = match ctx.pairing.session.lock() {
        Ok(mut guard) => {
            let (clear, ok, present) = match guard.as_ref() {
                // Expired on EITHER clock: the monotonic one is the liveness
                // deadline, the wall clock catches a code that slept through its
                // ten minutes (Instant does not advance during sleep).
                Some(sess)
                    if now >= sess.expires || std::time::SystemTime::now() >= sess.expires_at =>
                {
                    (true, false, true)
                }
                Some(sess) if code_matches(&sess.code, &code) => (true, true, true),
                Some(_) => (false, false, true),
                None => (false, false, false),
            };
            if clear {
                *guard = None;
            }
            (ok, present)
        }
        Err(_) => (false, false),
    };

    let my_name = local_device_name(&ctx.settings);
    if matched {
        // (3a) The code was right, so the protocol accepts — but NO trust is
        // granted here. Accepting a code and trusting a device are two different
        // decisions, and only the second one needs a human: the SAS below is the
        // sole defence against a machine-in-the-middle, and a code nobody read is
        // not a check. `pair_joined` hands the UI the SAS; the UI records trust
        // (`set_trusted`) once the user confirms it matches the other screen.
        let joiner_name = peer_display_name(peer.name.as_deref(), device_id, || None);
        if let Some(ip) = peer_ip {
            if let Ok(mut r) = ctx.pairing.rate.lock() {
                r.clear(&ip);
            }
        }
        // Remember how to reach the joiner. Pairing exists for peers discovery
        // cannot see; without this the host would trust a device it could never
        // dial (the joiner records the host symmetrically in `join_by_code`).
        // The source IP is authenticated by the TCP connection; the port is the
        // joiner's self-declared listener, defaulting for older builds. Not
        // trust — just an address, exactly like `connect_by_addr`.
        if let Some(std::net::IpAddr::V4(ip)) = peer_ip {
            if let Ok(mut manual) = ctx.manual_peers.lock() {
                crate::state::insert_manual_peer(
                    &mut manual,
                    device_id.to_string(),
                    crate::state::ManualPeer {
                        name: joiner_name.clone(),
                        addr: ip,
                        port: port.unwrap_or(crate::consts::DEFAULT_TCP_PORT),
                        last_used: Instant::now(),
                    },
                );
            }
            // The device list is replaced wholesale from this payload on the
            // UI side, so emit the MERGED list (discovery + manual), never
            // discovery alone.
            let _ = ctx.app.emit(
                "devices_updated",
                crate::discovery::devices_payload(
                    &ctx.peers,
                    &ctx.manual_peers,
                    &ctx.identity.device_id(),
                ),
            );
        }
        send_control(
            session,
            &AppMessage::PairConfirm {
                accept: true,
                name: my_name,
                reason: None,
            },
        )
        .await?;
        // Hand the UI who redeemed the code and the SAS to compare. This is the
        // ONLY route by which a pairing becomes trust on this side — if the user
        // never confirms, nothing was trusted.
        let _ = ctx.app.emit(
            "pair_joined",
            json!({ "deviceId": device_id, "name": joiner_name, "sas": session.sas() }),
        );
        log::info!(
            "pairing: {device_id} ({joiner_name:?}) redeemed the code; awaiting SAS confirm"
        );
        Ok(())
    } else {
        // (3b) No trust change. The failure was already recorded at the gate
        // (the reservation) so a guesser stays throttled — but that only belongs
        // when an invitation was actually active: a wrong code against a live code
        // is a real guess, whereas a PairRequest at an idle host is nothing to
        // brute-force, so keeping its reservation would let a stranger grow the
        // per-source failure map without ever touching a real code. Roll the
        // reservation back there to remove that amplification vector; the map is
        // also hard-capped.
        if !had_session {
            if let Some(ip) = peer_ip {
                if let Ok(mut r) = ctx.pairing.rate.lock() {
                    r.unreserve(&ip);
                }
            }
        }
        send_control(
            session,
            &AppMessage::PairConfirm {
                accept: false,
                name: my_name,
                reason: Some("pairing code rejected".into()),
            },
        )
        .await?;
        log::info!("pairing: rejected PairRequest from {device_id} (bad or expired code)");
        Ok(())
    }
}

/// Reduce a pairing code to its ASCII digits (M7.1): drops grouping spaces or
/// dashes a human copied off a screen ("482 913"), and folds full-width digits
/// (`４８２` from a CJK IME) onto ASCII. Shared by the joiner (`pick_code`) and
/// the host (`handle_pair_request`), so a code typed the way it is DISPLAYED
/// matches — and an older joiner that still sends the spaced form is accepted.
/// Everything else is dropped, so the result is either empty or digits only.
pub fn normalize_pair_code(raw: &str) -> String {
    raw.chars()
        .filter_map(|c| match c {
            '0'..='9' => Some(c),
            // U+FF10..=U+FF19: FULLWIDTH DIGIT ZERO..NINE
            '\u{FF10}'..='\u{FF19}' => char::from_u32(c as u32 - 0xFF10 + '0' as u32),
            _ => None,
        })
        .collect()
}

/// Length-checked, non-short-circuiting equality for a pairing code (M7.1):
/// compares every byte so the time taken does not leak how many leading
/// characters matched — a brute-forcer must not learn "the first N were right"
/// from a faster reject. Not the primary defense (the per-source throttle is),
/// just no free timing oracle. The length check leaks only the code's length,
/// which is fixed and public (6 digits).
pub(super) fn code_matches(expected: &str, got: &str) -> bool {
    let a = expected.as_bytes();
    let b = got.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}
