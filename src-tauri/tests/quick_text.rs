// clippy: reassign is clearer than a many-field struct literal in test setup
#![allow(clippy::field_reassign_with_default)]

//! Integration test (M7.3) through the REAL `handle_incoming` quick-text path:
//! a loopback quick text is delivered, `handle_incoming` emits `text_received`
//! (deviceId + senderName + text + at) and acks, so the sender's `send_text`
//! resolves only once the text was received.
//!
//! Lives here (not in lib unit tests) for the same reason as pairing.rs: a
//! mock-runtime Tauri app links comctl32-v6 imports, and only integration-test
//! targets carry the Windows manifest linker args — see build.rs. The mock app
//! registers no clipboard plugin, so the receive path's clipboard mirror (taken
//! here because clip_share is on and the sender asked) degrades to a logged
//! no-op via `try_state` rather than crashing the session — this exercises that.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU16;
use std::sync::{Arc, Mutex, RwLock};

use tauri::Listener;
use tokio::net::{TcpListener, TcpStream};

use lanbeam_lib::consts::TRANSFER_V2;
use lanbeam_lib::identity::{generate_identity, Identity};
use lanbeam_lib::partials::PartialsStore;
use lanbeam_lib::settings::Settings;
use lanbeam_lib::state::{CompletedLog, ConcurrencyGate, PairingState};
use lanbeam_lib::transfer::{handle_incoming, initiator_hello, send_text};
use lanbeam_lib::transport::noise::NoiseSession;
use lanbeam_lib::transport::TransportCtx;
use lanbeam_lib::trust::TrustStore;

/// Build a receiver `TransportCtx` around a mock app, with `clip_share` and
/// `recv_policy` set so the test can drive the receive path's clipboard-consent
/// branch and the quick-text delivery gate (M7.3 hardening).
fn make_host_ctx<R: tauri::Runtime>(
    handle: tauri::AppHandle<R>,
    identity: Arc<Identity>,
    dl_dir: &std::path::Path,
    tmp: &std::path::Path,
    device_name: &str,
    clip_share: bool,
    recv_policy: &str,
) -> TransportCtx<R> {
    let mut settings = Settings::default();
    settings.device_name = device_name.to_string();
    settings.clip_share = clip_share;
    settings.recv_policy = recv_policy.to_string();
    TransportCtx {
        app: handle,
        identity,
        tcp_port: Arc::new(AtomicU16::new(0)),
        download_dir: Arc::new(RwLock::new(dl_dir.to_path_buf())),
        settings: Arc::new(RwLock::new(settings)),
        peers: Arc::new(Mutex::new(HashMap::new())),
        pending: Arc::new(Mutex::new(HashMap::new())),
        in_flight: Arc::new(Mutex::new(HashSet::new())),
        completed: Arc::new(Mutex::new(CompletedLog::new())),
        trusted: Arc::new(RwLock::new(TrustStore::load(tmp.join("trusted.json")))),
        transfers_ctl: Arc::new(Mutex::new(HashMap::new())),
        partials: Arc::new(RwLock::new(PartialsStore::load(tmp.join("partials.json")))),
        concurrency: Arc::new(ConcurrencyGate::new()),
        pairing: Arc::new(PairingState::default()),
        text_rate: Default::default(),
    }
}

/// A quick text delivered end to end: the receiver emits `text_received`
/// carrying the sender's DeviceInfo name, the exact text, its device id, and a
/// receive stamp, then acks — so the sender's `send_text` resolves on delivery.
/// With clip_share on and the sender's request set, the receive path also takes
/// the clipboard branch, which on a plugin-less mock app is a no-op (never a
/// crash) — proving the best-effort degradation.
#[tokio::test]
async fn loopback_quick_text_emits_text_received_and_acks() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-qt-ok-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let dl_dir = tmp.join("downloads");
    std::fs::create_dir_all(&dl_dir).unwrap();

    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = received.clone();
        handle.listen("text_received", move |e| {
            sink.lock().unwrap().push(e.payload().to_string())
        });
    }

    let host_identity = Arc::new(generate_identity().unwrap());
    let host_priv = *host_identity.private_bytes();
    // clip_share ON so the receive path takes the (best-effort) clipboard branch
    // too — the mock app has no clipboard plugin, so it must degrade to a no-op.
    // recv_policy "all" so the delivery gate admits this (untrusted) sender — the
    // gate itself is exercised by `untrusted_text_under_trusted_policy_is_dropped`.
    let ctx = make_host_ctx(
        handle,
        host_identity,
        &dl_dir,
        &tmp,
        "Host Device",
        true,
        "all",
    );

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ctx2 = ctx.clone();
    let responder = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let sess = NoiseSession::handshake_responder(stream, &host_priv)
            .await
            .unwrap();
        handle_incoming(sess, &ctx2).await
    });

    // Sender: unpinned dial + hello + push the text, exactly like the `send_text`
    // command's flow (the pin/no-pin choice is irrelevant to the text path).
    let sender_priv = *generate_identity().unwrap().private_bytes();
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut sess = NoiseSession::handshake_initiator(stream, &sender_priv, None)
        .await
        .unwrap();
    let peer = initiator_hello(&mut sess, "Sender Phone".into())
        .await
        .unwrap();
    assert_eq!(
        peer.version, TRANSFER_V2,
        "two current builds negotiate v2 quick text"
    );
    send_text(&mut sess, "meet at 3pm ✅".into(), Some(true))
        .await
        .expect("send_text resolves once the receiver acks");
    drop(sess);

    responder
        .await
        .unwrap()
        .expect("a delivered quick text is a clean Ok(())");

    // The receive path emitted text_received with the sender name, exact text,
    // a device id, and a millisecond stamp.
    let events = received.lock().unwrap();
    assert_eq!(events.len(), 1, "exactly one text_received");
    let payload: serde_json::Value = serde_json::from_str(&events[0]).unwrap();
    assert_eq!(
        payload["senderName"], "Sender Phone",
        "the sender's DeviceInfo name is surfaced"
    );
    assert_eq!(
        payload["text"], "meet at 3pm ✅",
        "the delivered text is exact"
    );
    assert!(
        payload["deviceId"].as_str().is_some_and(|s| !s.is_empty()),
        "a device id is present: {payload}"
    );
    assert!(
        payload["at"].as_i64().is_some_and(|ms| ms > 0),
        "a receive stamp is present: {payload}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Quick text from an UNTRUSTED sender under the default "trusted" recv_policy is
/// GATED (M7.3 hardening): the receiver never emits `text_received` (a stranger's
/// note/link is not surfaced unconditionally, the same way files don't
/// auto-accept an unknown sender), yet the sender's `send_text` still resolves —
/// the receiver acks the dropped text rather than hanging the peer or handing it
/// a trust/policy oracle.
#[tokio::test]
async fn untrusted_text_under_trusted_policy_is_dropped_but_acked() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-qt-drop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let dl_dir = tmp.join("downloads");
    std::fs::create_dir_all(&dl_dir).unwrap();

    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = received.clone();
        handle.listen("text_received", move |e| {
            sink.lock().unwrap().push(e.payload().to_string())
        });
    }

    let host_identity = Arc::new(generate_identity().unwrap());
    let host_priv = *host_identity.private_bytes();
    // Default "trusted" policy + a sender the trust store has never seen → gated.
    let ctx = make_host_ctx(
        handle,
        host_identity,
        &dl_dir,
        &tmp,
        "Host Device",
        false,
        "trusted",
    );

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ctx2 = ctx.clone();
    let responder = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let sess = NoiseSession::handshake_responder(stream, &host_priv)
            .await
            .unwrap();
        handle_incoming(sess, &ctx2).await
    });

    let sender_priv = *generate_identity().unwrap().private_bytes();
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut sess = NoiseSession::handshake_initiator(stream, &sender_priv, None)
        .await
        .unwrap();
    let peer = initiator_hello(&mut sess, "Stranger".into()).await.unwrap();
    assert_eq!(peer.version, TRANSFER_V2);
    // The send still resolves — the receiver acks even though it drops the text.
    send_text(&mut sess, "click this sketchy link".into(), Some(false))
        .await
        .expect("send_text still resolves on the receiver's ack");
    drop(sess);

    responder
        .await
        .unwrap()
        .expect("a gated (dropped) text is still a clean Ok(())");

    // No text_received: the untrusted stranger's text never reached the inbox.
    assert!(
        received.lock().unwrap().is_empty(),
        "untrusted text under 'trusted' policy is not surfaced"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
