//! TCP transport: a listener that answers inbound Noise handshakes, and a dialer
//! that opens an authenticated channel to a peer. DESIGN §1.2, §3.5.

mod frame;
pub mod noise;

use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use tauri::{AppHandle, Emitter, Runtime, Wry};
use tokio::net::{TcpListener, TcpStream};

use crate::consts::CONNECT_TIMEOUT;
use crate::discovery::PeerTable;
use crate::error::{LanBeamError, Result};
use crate::identity::Identity;
use crate::settings::Settings;
use crate::state::{
    CompletedLog, ConcurrencyGate, NetDegraded, PairingState, PendingMap, TextRateLimiter,
    TransfersCtl,
};
use crate::trust::TrustStore;
use noise::NoiseSession;

/// Bind the transfer listener, falling back to an ephemeral port on ANY bind error.
/// Returns the listener plus the original bind error when the fallback fired, so
/// callers can surface the degradation (fixed-port firewall rules stop matching)
/// instead of silently swallowing it.
pub async fn bind_tcp_with_fallback(port: u16) -> Result<(TcpListener, Option<String>)> {
    match TcpListener::bind(("0.0.0.0", port)).await {
        Ok(l) => Ok((l, None)),
        Err(first) => TcpListener::bind(("0.0.0.0", 0))
            .await
            .map(|l| (l, Some(first.to_string())))
            .map_err(|e| LanBeamError::Bind(e.to_string())),
    }
}

/// Decode a base64url Device ID into the 32-byte static public key (pin target).
pub fn decode_device_id(id: &str) -> Option<[u8; 32]> {
    URL_SAFE_NO_PAD.decode(id).ok()?.try_into().ok()
}

/// Generic over the Tauri runtime so integration tests can drive the real
/// `handle_incoming` with the mock runtime; production code always uses the
/// default (`Wry`).
pub struct TransportCtx<R: Runtime = Wry> {
    pub app: AppHandle<R>,
    pub identity: Arc<Identity>,
    pub tcp_port: Arc<AtomicU16>,
    /// Download root for inbound sessions — locked (M5.2) so a
    /// `set_download_dir` reaches the NEXT session without a restart;
    /// `handle_incoming` snapshots it at session start and never holds the
    /// lock across an await.
    pub download_dir: Arc<RwLock<PathBuf>>,
    /// Live settings — the hello exchange reads `device_name` at session time
    /// so a rename reaches the very next transfer (M4.2).
    pub settings: Arc<RwLock<Settings>>,
    /// Discovery table — fallback source for a friendly sender name when the
    /// peer's `DeviceInfo` is absent (legacy v1 peers) (M4.2).
    pub peers: Arc<Mutex<PeerTable>>,
    pub pending: Arc<Mutex<PendingMap>>,
    /// Inbound session ids in flight for the WHOLE life of their session —
    /// `pending` only covers the prompt window, and auto-accepted sessions
    /// never park there at all, so this set is what actually prevents a
    /// reused sender-chosen transfer_id from running concurrently with a
    /// live session (see `transfer::InFlightGuard`).
    pub in_flight: Arc<Mutex<HashSet<String>>>,
    pub completed: Arc<Mutex<CompletedLog>>,
    /// Trust store — consulted before the accept prompt so trusted peers (or
    /// everyone, under the "all" policy) skip it entirely (M4.4).
    pub trusted: Arc<RwLock<TrustStore>>,
    /// Live cancel/pause controls per in-flight session (M6.1/6.2). Shared with
    /// `AppState` (unlike `in_flight`, which only the listener touches) so the
    /// receive path registers a control here and the cancel/pause/resume
    /// commands can reach a running receive.
    pub transfers_ctl: TransfersCtl,
    /// Persisted resume state (M6.4): the receive path records a partial when a
    /// receive interrupts and clears it on completion. Shared with `AppState`
    /// so `discard_partials` clears it too.
    pub partials: Arc<RwLock<crate::partials::PartialsStore>>,
    /// Global concurrency cap (M6.7). Shared with `AppState` so an inbound
    /// receive draws a slot from the same gate the outbound send does — the cap
    /// bounds all transfers, both directions, together.
    pub concurrency: Arc<ConcurrencyGate>,
    /// Active pairing invitation + failure throttle (M7.1). Shared with
    /// `AppState` so an inbound `PairRequest` here matches against the code the
    /// `start_pairing` command minted, and records failed guesses in the
    /// per-source throttle. See [`PairingState`].
    pub pairing: Arc<PairingState>,
    /// Per-source-IP inbound quick-text throttle (M7.3 hardening). Listener-only,
    /// like `in_flight` — quick text has no accept-prompt, so this shared limiter
    /// is what stops one LAN peer from flooding the inbox with `text_received`
    /// events. See [`TextRateLimiter`].
    pub text_rate: Arc<Mutex<TextRateLimiter>>,
}

// Manual impl: `#[derive(Clone)]` would demand `R: Clone`, which the mock
// test runtime doesn't implement — every field here is an Arc/handle clone.
impl<R: Runtime> Clone for TransportCtx<R> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            identity: self.identity.clone(),
            tcp_port: self.tcp_port.clone(),
            download_dir: self.download_dir.clone(),
            settings: self.settings.clone(),
            peers: self.peers.clone(),
            pending: self.pending.clone(),
            in_flight: self.in_flight.clone(),
            completed: self.completed.clone(),
            trusted: self.trusted.clone(),
            transfers_ctl: self.transfers_ctl.clone(),
            partials: self.partials.clone(),
            concurrency: self.concurrency.clone(),
            pairing: self.pairing.clone(),
            text_rate: self.text_rate.clone(),
        }
    }
}

/// Spawn the TCP listener on `port` (the settings-resolved choice — M5.2; the
/// caller maps the `0` sentinel to the default before this). Stores the actual
/// bound port in `tcp_port` (so discovery announces it) and answers each
/// inbound connection with a responder handshake.
/// A port fallback is pushed into `degraded` BEFORE the emit — the emit fires
/// during `setup()`, before the webview exists, so the recorded entry (served
/// by `get_net_status`) is what actually reaches the user (M4.6).
pub fn spawn_listener(ctx: TransportCtx, degraded: Arc<Mutex<Vec<NetDegraded>>>, port: u16) {
    tauri::async_runtime::spawn(async move {
        let (listener, fallback_cause) = match bind_tcp_with_fallback(port).await {
            Ok(x) => x,
            Err(e) => {
                // No listener at all: sending still works, receiving is dead.
                log::error!("listener bind failed (inbound transfers disabled): {e}");
                return;
            }
        };
        if let Ok(addr) = listener.local_addr() {
            ctx.tcp_port.store(addr.port(), Ordering::Relaxed);
            log::info!("transfer listener on {addr}");
            if let Some(cause) = &fallback_cause {
                // Degraded, not broken: discovery announces the real port so peers
                // still connect — but fixed-port firewall allowances stop matching.
                // Surface it to the UI instead of leaving it log-only (M4.6).
                let detail = format!(
                    "TCP port {port} unavailable ({cause}); using ephemeral port {}",
                    addr.port()
                );
                log::warn!("{detail}");
                let event = NetDegraded {
                    kind: "tcp_port_fallback".into(),
                    detail,
                };
                if let Ok(mut list) = degraded.lock() {
                    list.push(event.clone());
                }
                let _ = ctx.app.emit("net_degraded", &event);
            }
        }
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let _ = stream.set_nodelay(true);
                    let c = ctx.clone();
                    tokio::spawn(async move {
                        match NoiseSession::handshake_responder(stream, c.identity.private_bytes())
                            .await
                        {
                            Ok(sess) => {
                                if let Err(e) = crate::transfer::handle_incoming(sess, &c).await {
                                    log::warn!("incoming from {peer} ended: {e}");
                                }
                            }
                            Err(e) => {
                                log::warn!("inbound handshake failed from {peer}: {e}");
                            }
                        }
                    });
                }
                Err(e) => log::warn!("accept error: {e}"),
            }
        }
    });
}

/// Dial a peer and open an authenticated Noise channel, pinning `expected_id`.
pub async fn connect(
    addr: Ipv4Addr,
    port: u16,
    local_private: &[u8; 32],
    expected_id: [u8; 32],
) -> Result<NoiseSession> {
    let stream = dial(addr, port, CONNECT_TIMEOUT).await?;
    let _ = stream.set_nodelay(true);
    NoiseSession::handshake_initiator(stream, local_private, Some(expected_id)).await
}

/// Dial a peer WITHOUT pinning a Device ID (M7.1/7.2). Used by pairing and
/// IP-direct connect, where the peer's identity is not known in advance —
/// it is learned from the handshake (`remote_static`) and verified out of band
/// via the SAS (TOFU). The Noise XX handshake still authenticates the channel
/// to whatever static key the peer presents; only the compare-against-expected
/// step is skipped. The caller must surface the SAS so a MITM is detectable.
pub async fn connect_unpinned(
    addr: Ipv4Addr,
    port: u16,
    local_private: &[u8; 32],
) -> Result<NoiseSession> {
    let stream = dial(addr, port, CONNECT_TIMEOUT).await?;
    let _ = stream.set_nodelay(true);
    NoiseSession::handshake_initiator(stream, local_private, None).await
}

/// TCP connect with a deadline. WHY: a black-holed peer (stale discovery entry,
/// firewall silently dropping the SYN) otherwise hangs the dialer for the OS
/// TCP timeout — minutes on some stacks.
async fn dial(addr: Ipv4Addr, port: u16, deadline: Duration) -> Result<TcpStream> {
    bounded_connect(TcpStream::connect((addr, port)), deadline).await
}

/// The deadline half of [`dial`], generic over the connect future — so tests
/// can prove the timeout mapping deterministically (a never-resolving future)
/// instead of depending on some address being unrouteable, which TUN-style
/// proxies on dev machines quietly break by answering for arbitrary IPs.
async fn bounded_connect<F>(connecting: F, deadline: Duration) -> Result<TcpStream>
where
    F: std::future::Future<Output = std::io::Result<TcpStream>>,
{
    match tokio::time::timeout(deadline, connecting).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(e)) => Err(LanBeamError::Io(e.to_string())),
        Err(_) => Err(LanBeamError::Timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::NOISE_PARAMS;
    use snow::Builder;

    fn keypair() -> ([u8; 32], [u8; 32]) {
        let kp = Builder::new(NOISE_PARAMS.parse().unwrap())
            .generate_keypair()
            .unwrap();
        (
            kp.public.as_slice().try_into().unwrap(),
            kp.private.as_slice().try_into().unwrap(),
        )
    }

    #[tokio::test]
    async fn loopback_handshake_same_sas_and_echo() {
        let (a_pub, a_priv) = keypair();
        let (b_pub, b_priv) = keypair();

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut sess = NoiseSession::handshake_responder(stream, &b_priv)
                .await
                .unwrap();
            let msg = sess.read_msg().await.unwrap().unwrap();
            sess.write_msg(&msg).await.unwrap();
            (sess.sas().to_string(), *sess.remote_static())
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        let mut client = NoiseSession::handshake_initiator(stream, &a_priv, Some(b_pub))
            .await
            .unwrap();
        client.write_msg(b"ping").await.unwrap();
        let echo = client.read_msg().await.unwrap().unwrap();
        assert_eq!(echo, b"ping"); // encrypted round-trip

        let (server_sas, server_saw) = server.await.unwrap();
        assert_eq!(client.sas(), server_sas); // both ends derive the SAME 6-digit SAS
        assert_eq!(client.sas().len(), 6);
        assert_eq!(*client.remote_static(), b_pub); // client authenticated B
        assert_eq!(server_saw, a_pub); // server learned A's identity
    }

    /// The deadline path must map to `LanBeamError::Timeout` — proven with a
    /// connect future that never resolves, so the result does not depend on
    /// any address being unrouteable from the test machine.
    #[tokio::test]
    async fn dial_deadline_maps_to_timeout_error() {
        let res = bounded_connect(std::future::pending(), Duration::from_millis(50)).await;
        assert!(matches!(res, Err(LanBeamError::Timeout)), "got {res:?}");
    }

    /// Real-network variant against a blackhole address. IGNORED by default:
    /// on this dev machine a TUN-style proxy answers for ARBITRARY IPs (observed:
    /// local 198.18.0.1 "connecting" 10.255.255.1 instantly), so no address is
    /// reliably unrouteable. Run manually on a plain LAN with
    /// `cargo test -- --ignored dial_timeout_on_blackholed_address`.
    #[tokio::test]
    #[ignore = "needs a network where 10.255.255.1 is actually blackholed (no TUN proxy)"]
    async fn dial_timeout_on_blackholed_address() {
        let res = dial(
            Ipv4Addr::new(10, 255, 255, 1),
            9,
            Duration::from_millis(250),
        )
        .await;
        assert!(matches!(res, Err(LanBeamError::Timeout)), "got {res:?}");
    }

    #[tokio::test]
    async fn pin_mismatch_is_rejected() {
        let (_a_pub, a_priv) = keypair();
        let (_b_pub, b_priv) = keypair();
        let (wrong_pub, _wrong_priv) = keypair();

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Responder is B; its handshake will hang waiting for msg3 that never comes.
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = NoiseSession::handshake_responder(stream, &b_priv).await;
            }
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        // Initiator pins the WRONG public key → must reject before completing.
        let res = NoiseSession::handshake_initiator(stream, &a_priv, Some(wrong_pub)).await;
        assert!(matches!(res, Err(LanBeamError::IdentityMismatch)));
    }

    /// `bind_tcp_with_fallback(0)` binds cleanly on an ephemeral port and reports
    /// no fallback cause — the happy path.
    #[tokio::test]
    async fn bind_tcp_with_fallback_no_cause_on_success() {
        let (listener, cause) = bind_tcp_with_fallback(0).await.unwrap();
        assert!(cause.is_none(), "clean bind must not report a cause");
        assert_ne!(listener.local_addr().unwrap().port(), 0);
    }

    /// When the requested port is already taken, the helper falls back to an
    /// ephemeral port and surfaces the original bind error as the cause.
    #[tokio::test]
    async fn bind_tcp_with_fallback_reports_cause_on_conflict() {
        let held = TcpListener::bind(("0.0.0.0", 0)).await.unwrap();
        let taken = held.local_addr().unwrap().port();

        let (listener, cause) = bind_tcp_with_fallback(taken).await.unwrap();
        assert!(
            cause.is_some(),
            "conflict must surface the original bind error"
        );
        assert_ne!(
            listener.local_addr().unwrap().port(),
            taken,
            "fallback must not reuse the contended port"
        );
    }

    #[test]
    fn decode_device_id_roundtrips_32_bytes() {
        let raw = [7u8; 32];
        let id = URL_SAFE_NO_PAD.encode(raw);
        assert_eq!(decode_device_id(&id), Some(raw));
    }

    #[test]
    fn decode_device_id_rejects_wrong_length() {
        let short = URL_SAFE_NO_PAD.encode([1u8; 16]);
        assert_eq!(decode_device_id(&short), None);
        let long = URL_SAFE_NO_PAD.encode([1u8; 33]);
        assert_eq!(decode_device_id(&long), None);
    }

    #[test]
    fn decode_device_id_rejects_invalid_base64() {
        assert_eq!(decode_device_id("not valid base64!!!"), None);
    }

    /// The success arm of `bounded_connect`: a future that resolves to a real
    /// connected stream passes straight through as `Ok`.
    #[tokio::test]
    async fn bounded_connect_passes_through_connected_stream() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let res = bounded_connect(TcpStream::connect(addr), Duration::from_secs(5)).await;
        assert!(res.is_ok(), "got {res:?}");
        accept.await.unwrap();
    }

    /// A connect error (inner `Err`, before the deadline) maps to
    /// `LanBeamError::Io`, not `Timeout`.
    #[tokio::test]
    async fn bounded_connect_maps_inner_error_to_io() {
        let res = bounded_connect(
            async {
                Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    "refused",
                ))
            },
            Duration::from_secs(5),
        )
        .await;
        assert!(matches!(res, Err(LanBeamError::Io(_))), "got {res:?}");
    }

    /// `connect` dials, pins the peer's Device ID, and authenticates the channel
    /// end to end over loopback — exercising `dial` + `bounded_connect` + the
    /// pinned initiator path.
    #[tokio::test]
    async fn connect_pins_and_authenticates_peer() {
        let (a_pub, a_priv) = keypair();
        let (b_pub, b_priv) = keypair();

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            NoiseSession::handshake_responder(stream, &b_priv)
                .await
                .map(|s| *s.remote_static())
        });

        let client = connect(Ipv4Addr::LOCALHOST, addr.port(), &a_priv, b_pub)
            .await
            .unwrap();
        assert_eq!(*client.remote_static(), b_pub); // pinned + authenticated B

        let server_saw = server.await.unwrap().unwrap();
        assert_eq!(server_saw, a_pub); // responder learned A's identity
    }

    /// `connect_unpinned` completes the same authenticated channel WITHOUT
    /// pinning — the peer identity is learned from the handshake (TOFU path).
    #[tokio::test]
    async fn connect_unpinned_learns_peer_identity() {
        let (a_pub, a_priv) = keypair();
        let (b_pub, b_priv) = keypair();

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            NoiseSession::handshake_responder(stream, &b_priv)
                .await
                .map(|s| *s.remote_static())
        });

        let client = connect_unpinned(Ipv4Addr::LOCALHOST, addr.port(), &a_priv)
            .await
            .unwrap();
        assert_eq!(*client.remote_static(), b_pub); // learned B from the handshake

        let server_saw = server.await.unwrap().unwrap();
        assert_eq!(server_saw, a_pub);
    }
}
