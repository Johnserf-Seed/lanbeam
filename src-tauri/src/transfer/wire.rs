//! The session's wire helpers: control frames, the opening dialogue
//! (Hello/DeviceInfo exchange, M4.2), and the joiner's half of pairing (M7.1)
//! and the sender's half of a quick text (M7.3).

use std::sync::RwLock;

use super::receive::read_manifest_entries;
use super::Manifest;
use crate::consts::{MANIFEST_TIMEOUT, MAX_PLAINTEXT, TRANSFER_V2};
use crate::error::{LanBeamError, Result};
use crate::protocol::{self, AppMessage, Frame};
use crate::settings::Settings;
use crate::transport::noise::NoiseSession;
use crate::trust;

// ── control-frame helpers ─────────────────────────────────────────────
pub(super) async fn send_control(session: &mut NoiseSession, msg: &AppMessage) -> Result<()> {
    session.write_msg(&protocol::encode_control(msg)?).await
}

pub(super) async fn recv_frame(session: &mut NoiseSession) -> Result<Frame> {
    let pt = session
        .read_msg()
        .await?
        // The peer hung up mid-conversation (cancelled, quit, or crashed) — the
        // connection is gone, nothing the peer said was malformed.
        .ok_or_else(|| LanBeamError::ConnectionLost("the peer closed the connection".into()))?;
    protocol::decode(&pt)
}

pub(super) async fn recv_control(session: &mut NoiseSession) -> Result<AppMessage> {
    match recv_frame(session).await? {
        Frame::Control(m) => Ok(m),
        Frame::FileChunk(..) => Err(LanBeamError::Protocol("expected control, got chunk".into())),
    }
}

// ── opening dialogue (Hello/DeviceInfo exchange, M4.2) ────────────────
/// What the post-handshake exchange established about the peer, kept for the
/// session (the receive path uses `name` for the accept prompt; `version` is
/// the gate every post-v1 capability must check before sending new variants).
#[derive(Debug)]
pub struct PeerHello {
    /// Negotiated protocol version — the highest both sides speak. Legacy
    /// peers that never sent a `Hello` are pinned to 1.
    pub version: u8,
    /// The peer's self-reported friendly name (`None` for legacy-v1 peers).
    pub name: Option<String>,
}

/// The friendly name we introduce ourselves with, read at call time — not
/// snapshotted at startup — so a rename reaches the very next session.
pub fn local_device_name(settings: &RwLock<Settings>) -> String {
    settings
        .read()
        .map(|s| s.device_name.clone())
        // Poisoned lock: introducing ourselves with the OS default beats
        // failing the whole session over a display name.
        .unwrap_or_else(|_| crate::settings::default_device_name())
}

/// Our `DeviceInfo`. Platform and app version are process constants; only the
/// name varies, which keeps callers to a single settings read.
pub(super) fn device_info_msg(name: String) -> AppMessage {
    AppMessage::DeviceInfo {
        name,
        platform: std::env::consts::OS.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Say goodbye explicitly. WHY a dedicated frame instead of just closing:
/// an identity check (`connect_by_addr`) must end in a way the responder can tell
/// apart from a crashed peer — a `Bye` is swallowed silently (no prompt, no
/// events), a bare close is an error.
pub async fn send_bye(session: &mut NoiseSession) -> Result<()> {
    send_control(session, &AppMessage::Bye).await
}

/// Initiator side of the opening dialogue: introduce ourselves (`Hello` +
/// `DeviceInfo`), then read the responder's introduction back.
///
/// Every send/receive here runs before any prompt, so the whole exchange is
/// bounded like the manifest read (M4.5).
pub async fn initiator_hello(session: &mut NoiseSession, my_name: String) -> Result<PeerHello> {
    match tokio::time::timeout(MANIFEST_TIMEOUT, initiator_hello_inner(session, my_name)).await {
        Ok(res) => res,
        Err(_) => Err(LanBeamError::Timeout),
    }
}

async fn initiator_hello_inner(session: &mut NoiseSession, my_name: String) -> Result<PeerHello> {
    // Send both introduction frames, then wait for the peer's first reply.
    // WHY the error mapping: a v1 responder expects the manifest as the FIRST
    // control message, so our `Hello` makes it error out and drop the link —
    // once the Hello is out, a broken/closed channel means "the peer does not
    // speak the version handshake", which the UI must present as "update the
    // other device" (code `peer_too_old`), not as a generic failure. A Hello
    // that could not even be SENT says nothing about the peer's version: the
    // connection was already gone (L10), and that is what it reports.
    send_control(
        session,
        &AppMessage::Hello {
            versions: protocol::SUPPORTED_VERSIONS.to_vec(),
        },
    )
    .await?;
    let first: Result<AppMessage> = async {
        send_control(session, &device_info_msg(my_name)).await?;
        recv_control(session).await
    }
    .await;
    let hello = first.map_err(|e| match e {
        LanBeamError::Io(_) | LanBeamError::Protocol(_) | LanBeamError::ConnectionLost(_) => {
            LanBeamError::PeerTooOld(format!("peer dropped the version handshake ({e})"))
        }
        other => other,
    })?;
    let version = match hello {
        AppMessage::Hello { versions } => {
            protocol::negotiate_version(&versions).ok_or_else(|| {
                LanBeamError::PeerTooOld(format!(
                    "no common protocol version (peer speaks {versions:?})"
                ))
            })?
        }
        other => {
            return Err(LanBeamError::Protocol(format!(
                "expected hello, got {}",
                trust::clamp_peer_text(&format!("{other:?}"))
            )))
        }
    };
    // Past the Hello the peer provably speaks the new dialogue — from here on,
    // an unexpected message is a genuine protocol error, not an old build.
    let name = match recv_control(session).await? {
        AppMessage::DeviceInfo {
            name,
            platform,
            app_version,
        } => {
            // Clamp the peer's self-declared name at the trust boundary (M5.4):
            // the XX handshake accepts any static key, so this runs pre-trust —
            // an unbounded name would otherwise flow into the notification
            // title, event payloads, and logs. Same rule as `trust::clamp_name`.
            let name = trust::clamp_name(&name);
            log::debug!("peer introduced itself: {name:?} ({platform}, app {app_version})");
            Some(name)
        }
        other => {
            return Err(LanBeamError::Protocol(format!(
                "expected device info, got {}",
                trust::clamp_peer_text(&format!("{other:?}"))
            )))
        }
    };
    Ok(PeerHello { version, name })
}

/// Outcome of the responder-side opening dialogue.
pub enum InboundOpen {
    /// The peer is sending files — the manifest is fully read.
    Files { manifest: Manifest, peer: PeerHello },
    /// The peer redeemed a pairing code (M7.1): its `PairRequest.code` (and the
    /// joiner's own listen `port`, when a current build sent one), carried up
    /// so `handle_incoming` can match it against the active pairing session and
    /// reply `PairConfirm`. No trust is granted anywhere on this path — the UI
    /// records it only after a human compares the SAS. v2-gated — a peer that
    /// negotiated v1 never reaches this arm.
    Pair {
        code: String,
        port: Option<u16>,
        peer: PeerHello,
    },
    /// The peer pushed a quick text (M7.3): its `TextSend` payload, carried up so
    /// `handle_incoming` can emit `text_received`, optionally mirror it to the
    /// clipboard, and ack. v2-gated — a peer that negotiated v1 never reaches
    /// this arm (the dispatch refuses its premature/forged variant).
    Text {
        text: String,
        to_clipboard: Option<bool>,
        peer: PeerHello,
    },
    /// The peer only wanted the identity/SAS round-trip (`connect_by_addr`)
    /// and said `Bye` — close cleanly: no error, no prompt, no events.
    Bye,
}

/// Responder side of the opening dialogue. Tolerates BOTH generations: a
/// legacy v1 peer opens with `FileListBegin` (it never sends `Hello`), a
/// current peer introduces itself first. WHY keep the legacy path: pre-Hello
/// builds already exist on our own devices, and receiving from them must keep
/// working even though this build always initiates with `Hello`.
pub async fn open_inbound(session: &mut NoiseSession, my_name: String) -> Result<InboundOpen> {
    let peer;
    let opening = match recv_control(session).await? {
        // Legacy peer: the first control frame is already the manifest head.
        first @ AppMessage::FileListBegin { .. } => {
            peer = PeerHello {
                version: 1,
                name: None,
            };
            first
        }
        AppMessage::Hello { versions } => {
            let version = protocol::negotiate_version(&versions).ok_or_else(|| {
                LanBeamError::PeerTooOld(format!(
                    "no common protocol version (peer speaks {versions:?})"
                ))
            })?;
            let name = match recv_control(session).await? {
                AppMessage::DeviceInfo {
                    name,
                    platform,
                    app_version,
                } => {
                    // Clamp the peer's self-declared name at the trust boundary
                    // (M5.4): this runs pre-trust (XX accepts any static key), so
                    // bound it here before it reaches the accept prompt, the OS
                    // notification title, the event payload, and the logs. Same
                    // rule as `trust::clamp_name`.
                    let name = trust::clamp_name(&name);
                    log::debug!("peer introduced itself: {name:?} ({platform}, app {app_version})");
                    Some(name)
                }
                other => {
                    return Err(LanBeamError::Protocol(format!(
                        "expected device info, got {}",
                        trust::clamp_peer_text(&format!("{other:?}"))
                    )))
                }
            };
            // Introduce ourselves back, then the dialogue proper begins.
            send_control(
                session,
                &AppMessage::Hello {
                    versions: protocol::SUPPORTED_VERSIONS.to_vec(),
                },
            )
            .await?;
            send_control(session, &device_info_msg(my_name)).await?;
            peer = PeerHello { version, name };
            recv_control(session).await?
        }
        other => {
            return Err(LanBeamError::Protocol(format!(
                "expected hello or manifest, got {}",
                trust::clamp_peer_text(&format!("{other:?}"))
            )))
        }
    };
    // Post-Hello dispatch: the peer's FIRST control message after the opening
    // dialogue selects the session kind. `PairRequest` (M7.1) and `TextSend`
    // (M7.3) are gated on the negotiated `peer.version >= TRANSFER_V2` — a peer
    // pinned to v1 was never offered either capability, so its (forged/premature)
    // variant is refused rather than acted on. For ANY v2 variant arriving from a
    // peer that (wrongly) negotiated v1, the arm returns a clean `Protocol` error:
    // the Debug format never panics, so a malformed or premature first message can
    // only fail this one session, never the listener.
    match opening {
        AppMessage::FileListBegin {
            transfer_id,
            total_size,
            file_count,
        } => {
            let manifest =
                read_manifest_entries(session, transfer_id, total_size, file_count).await?;
            Ok(InboundOpen::Files { manifest, peer })
        }
        AppMessage::PairRequest { code, port } => {
            if peer.version < TRANSFER_V2 {
                return Err(LanBeamError::Protocol(
                    "pair request from a peer that negotiated v1".into(),
                ));
            }
            Ok(InboundOpen::Pair { code, port, peer })
        }
        AppMessage::TextSend { text, to_clipboard } => {
            if peer.version < TRANSFER_V2 {
                return Err(LanBeamError::Protocol(
                    "text send from a peer that negotiated v1".into(),
                ));
            }
            Ok(InboundOpen::Text {
                text,
                to_clipboard,
                peer,
            })
        }
        AppMessage::Bye => Ok(InboundOpen::Bye),
        other => Err(LanBeamError::Protocol(format!(
            "expected manifest or bye, got {}",
            trust::clamp_peer_text(&format!("{other:?}"))
        ))),
    }
}

// ── pairing wire helpers (M7.1) ───────────────────────────────────────
/// Send a `PairRequest` presenting the pairing `code` (M7.1). v2-gated — the
/// joiner (`join_by_code`) checks the negotiated version before calling this,
/// so a v1 peer is never handed the variant.
/// `port` is this device's own TCP listen port, so the host can record us as a
/// dialable manual peer; `None` only in callers that have no listener (tests).
pub async fn send_pair_request(
    session: &mut NoiseSession,
    code: &str,
    port: Option<u16>,
) -> Result<()> {
    send_control(
        session,
        &AppMessage::PairRequest {
            code: code.to_string(),
            port,
        },
    )
    .await
}

/// Await the responder's `PairConfirm` (M7.1), bounded like the manifest reply
/// (the host answers immediately — the code IS the authorization, no human
/// prompt on its side). Returns `(accepted, responder name, decline reason)`;
/// the name is clamped at this trust boundary before it can reach the store.
pub async fn recv_pair_confirm(
    session: &mut NoiseSession,
) -> Result<(bool, String, Option<String>)> {
    let msg = match tokio::time::timeout(MANIFEST_TIMEOUT, recv_control(session)).await {
        Ok(res) => res?,
        Err(_) => return Err(LanBeamError::Timeout),
    };
    match msg {
        AppMessage::PairConfirm {
            accept,
            name,
            reason,
            // Clamp `reason` at this trust boundary the same way `name` is
            // (finding): join_by_code surfaces it as the command error the
            // pairing UI renders, so an unbounded peer string must not reach it.
        } => Ok((
            accept,
            trust::clamp_name(&name),
            reason.map(|r| trust::clamp_peer_text(&r)),
        )),
        other => Err(LanBeamError::Protocol(format!(
            "expected pair confirm, got {}",
            trust::clamp_peer_text(&format!("{other:?}"))
        ))),
    }
}

// ── quick-text wire helpers (M7.3) ────────────────────────────────────
/// The tag the quick-text receiver acks with (`Ack { of }`). One constant so the
/// receiver's reply and any future matcher can never drift apart.
pub(super) const TEXT_ACK: &str = "text";
/// Coarse reasons a quick text did not reach the user, carried on the ack. Stable
/// tokens the sender maps to a typed error — never shown raw.
pub(super) const TEXT_DROP_UNTRUSTED: &str = "untrusted";
pub(super) const TEXT_DROP_THROTTLED: &str = "throttled";

/// Push one quick text to a peer over an already-open session (M7.3). v2-gated —
/// the `send_text` command checks the negotiated version before calling this, so
/// a v1 peer is never handed the variant. `to_clipboard` rides along as the
/// SENDER's request ("also put this on your clipboard"); whether it actually
/// lands there is the receiver's call, gated on ITS `clip_share` consent.
///
/// Bounded end-to-end (M4.5): the write-then-await-ack round-trip is small
/// control traffic, so it runs under one deadline — a peer that takes the bytes
/// but never acks (crash, hung app) cannot pin this task. Refuses up front if the
/// encoded frame would overflow one Noise message, so an over-long text fails
/// cleanly here rather than deep inside `write_msg`.
pub async fn send_text(
    session: &mut NoiseSession,
    text: String,
    to_clipboard: Option<bool>,
) -> Result<()> {
    let frame = protocol::encode_control(&AppMessage::TextSend { text, to_clipboard })?;
    // Escaping can push a text that is under MAX_TEXT_BYTES past the frame (a
    // `"` or newline costs 2 bytes, a control char 6), so this is a separate check.
    if frame.len() > MAX_PLAINTEXT {
        return Err(LanBeamError::TextTooLong);
    }
    let acked = tokio::time::timeout(MANIFEST_TIMEOUT, async {
        session.write_msg(&frame).await?;
        recv_control(session).await
    })
    .await;
    match acked {
        // An ack that says it did NOT deliver is not a success. This function's
        // whole contract is "resolves only once the peer confirms it received it"
        // — and the peer drops a stranger's text (there is no prompt to park it
        // in). It used to ack that drop anyway, so the sender reported success for
        // a message that was thrown away on arrival.
        Ok(Ok(AppMessage::Ack {
            delivered: Some(false),
            reason,
            ..
        })) => Err(match reason.as_deref() {
            Some(TEXT_DROP_THROTTLED) => LanBeamError::TextThrottled,
            _ => LanBeamError::TextRefused,
        }),
        // Delivered — or a peer too old to tell us either way, where an ack meant
        // exactly this.
        Ok(Ok(AppMessage::Ack { .. })) => Ok(()),
        Ok(Ok(other)) => Err(LanBeamError::Protocol(format!(
            "expected text ack, got {}",
            trust::clamp_peer_text(&format!("{other:?}"))
        ))),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(LanBeamError::Timeout),
    }
}
