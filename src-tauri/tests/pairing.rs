// clippy: reassign is clearer than a many-field struct literal in test setup
#![allow(clippy::field_reassign_with_default)]

//! Integration tests (M7.1/7.2) through the REAL `handle_incoming` pairing path:
//!   * a loopback pair with the right code mutually trusts and emits
//!     `pair_joined` (carrying the SAS), consuming the one-shot code;
//!   * a wrong code is rejected with no trust change and leaves the code live;
//!   * an expired code is rejected and cleared, with no trust change;
//!   * the `connect_by_addr` (IP-direct) transport flow learns the peer's
//!     Device ID + name + SAS from an unpinned handshake.
//!
//! The HOST side runs the production `handle_incoming`; the JOIN side is driven
//! with the same transport helpers `join_by_code`/`connect_by_addr` use, so the
//! wire protocol + host verification + trust + events are all exercised. Lives
//! here (not in lib unit tests) for the same reason as bye_session.rs: a
//! mock-runtime Tauri app links comctl32-v6 imports, and only integration-test
//! targets carry the Windows manifest linker args — see build.rs.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU16;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use tauri::Listener;
use tokio::net::{TcpListener, TcpStream};

use lanbeam_lib::consts::TRANSFER_V2;
use lanbeam_lib::identity::{generate_identity, Identity};
use lanbeam_lib::partials::PartialsStore;
use lanbeam_lib::settings::Settings;
use lanbeam_lib::state::{CompletedLog, ConcurrencyGate, PairingSession, PairingState};
use lanbeam_lib::transfer::{
    handle_incoming, initiator_hello, recv_pair_confirm, send_bye, send_pair_request,
};
use lanbeam_lib::transport::noise::NoiseSession;
use lanbeam_lib::transport::TransportCtx;
use lanbeam_lib::trust::TrustStore;

/// Build a host `TransportCtx` around a mock app, sharing the caller's
/// `pairing` state so the test can plant/inspect the active invitation.
#[allow(clippy::too_many_arguments)]
fn make_host_ctx<R: tauri::Runtime>(
    handle: tauri::AppHandle<R>,
    identity: Arc<Identity>,
    dl_dir: &std::path::Path,
    tmp: &std::path::Path,
    pairing: Arc<PairingState>,
    device_name: &str,
) -> TransportCtx<R> {
    let mut settings = Settings::default();
    settings.device_name = device_name.to_string();
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
        pairing,
        text_rate: Default::default(),
    }
}

/// A correct code mutually trusts: the host stores the joiner under its
/// handshake identity + DeviceInfo name, emits `pair_joined` (with the SAS), and
/// consumes the one-shot code — while the joiner receives an accept carrying the
/// host's name and the SAME SAS (channel binding).
#[tokio::test]
async fn loopback_pairing_mutually_trusts_and_emits_pair_joined() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-pair-ok-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let dl_dir = tmp.join("downloads");
    std::fs::create_dir_all(&dl_dir).unwrap();

    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    let joined: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = joined.clone();
        handle.listen("pair_joined", move |e| {
            sink.lock().unwrap().push(e.payload().to_string())
        });
    }

    let host_identity = Arc::new(generate_identity().unwrap());
    let host_priv = *host_identity.private_bytes();
    let pairing = Arc::new(PairingState::default());
    let ctx = make_host_ctx(
        handle,
        host_identity,
        &dl_dir,
        &tmp,
        pairing.clone(),
        "Host Device",
    );

    // Plant the active invitation, exactly as `start_pairing` would.
    *pairing.session.lock().unwrap() = Some(PairingSession {
        code: "482913".into(),
        expires: Instant::now() + Duration::from_secs(600),
    });

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

    // Joiner: unpinned dial (TOFU) + hello + present the code, exactly like
    // `join_by_code`.
    let joiner_identity = generate_identity().unwrap();
    let joiner_priv = *joiner_identity.private_bytes();
    let joiner_id = joiner_identity.device_id();
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut sess = NoiseSession::handshake_initiator(stream, &joiner_priv, None)
        .await
        .unwrap();
    let sas = sess.sas().to_string();
    let peer = initiator_hello(&mut sess, "Joiner Device".into())
        .await
        .unwrap();
    assert_eq!(
        peer.version, TRANSFER_V2,
        "two current builds negotiate v2 pairing"
    );
    send_pair_request(&mut sess, "482913").await.unwrap();
    let (accept, host_name, reason) = recv_pair_confirm(&mut sess).await.unwrap();
    assert!(accept, "the right code must be accepted");
    assert_eq!(
        host_name, "Host Device",
        "confirm carries the host's name for the joiner's entry"
    );
    assert!(reason.is_none());
    drop(sess);

    responder
        .await
        .unwrap()
        .expect("a successful pair is a clean Ok(())");

    // Host trusts the joiner: stored under its handshake identity + DeviceInfo
    // name, auto_accept OFF (pairing establishes identity, not a standing grant).
    {
        let store = ctx.trusted.read().unwrap();
        let entry = store
            .get(&joiner_id)
            .expect("the joiner is now trusted by the host");
        assert_eq!(entry.name, "Joiner Device");
        assert!(
            !entry.auto_accept,
            "pairing must not silently grant prompt-free receipt"
        );
        assert!(entry.paired_at > 0);
    }
    // The one-shot code was consumed.
    assert!(
        pairing.session.lock().unwrap().is_none(),
        "a successful pair clears the code"
    );

    // pair_joined fired with the joiner + the SAS (both sides derive the same one).
    let events = joined.lock().unwrap();
    assert_eq!(events.len(), 1, "exactly one pair_joined");
    let payload: serde_json::Value = serde_json::from_str(&events[0]).unwrap();
    assert_eq!(payload["deviceId"], serde_json::Value::String(joiner_id));
    assert_eq!(payload["name"], "Joiner Device");
    assert_eq!(
        payload["sas"],
        serde_json::Value::String(sas),
        "SAS is the channel binding"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// A wrong code is rejected, no trust is granted, and the invitation stays live
/// so the legitimate joiner can still redeem it.
#[tokio::test]
async fn wrong_code_rejects_without_trust_and_keeps_code() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-pair-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let dl_dir = tmp.join("downloads");
    std::fs::create_dir_all(&dl_dir).unwrap();

    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    let joined: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = joined.clone();
        handle.listen("pair_joined", move |e| {
            sink.lock().unwrap().push(e.payload().to_string())
        });
    }

    let host_identity = Arc::new(generate_identity().unwrap());
    let host_priv = *host_identity.private_bytes();
    let pairing = Arc::new(PairingState::default());
    let ctx = make_host_ctx(
        handle,
        host_identity,
        &dl_dir,
        &tmp,
        pairing.clone(),
        "Host Device",
    );
    *pairing.session.lock().unwrap() = Some(PairingSession {
        code: "111111".into(),
        expires: Instant::now() + Duration::from_secs(600),
    });

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

    let joiner_priv = *generate_identity().unwrap().private_bytes();
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut sess = NoiseSession::handshake_initiator(stream, &joiner_priv, None)
        .await
        .unwrap();
    initiator_hello(&mut sess, "Guesser".into()).await.unwrap();
    send_pair_request(&mut sess, "999999").await.unwrap();
    let (accept, _name, reason) = recv_pair_confirm(&mut sess).await.unwrap();
    assert!(!accept, "a wrong code must be declined");
    assert!(reason.is_some(), "a decline carries a reason");
    drop(sess);

    responder
        .await
        .unwrap()
        .expect("a rejected pair is still a clean Ok(())");

    assert!(
        ctx.trusted.read().unwrap().list().is_empty(),
        "a wrong code grants no trust"
    );
    assert!(
        pairing.session.lock().unwrap().is_some(),
        "a wrong guess must NOT consume the live invitation"
    );
    assert!(
        joined.lock().unwrap().is_empty(),
        "no pair_joined on a rejected code"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// An expired code is rejected even when the digits are right, and the stale
/// invitation is cleared in passing — no trust is granted.
#[tokio::test]
async fn expired_code_rejects_and_clears() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-pair-exp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let dl_dir = tmp.join("downloads");
    std::fs::create_dir_all(&dl_dir).unwrap();

    let app = tauri::test::mock_app();
    let handle = app.handle().clone();

    let host_identity = Arc::new(generate_identity().unwrap());
    let host_priv = *host_identity.private_bytes();
    let pairing = Arc::new(PairingState::default());
    let ctx = make_host_ctx(
        handle,
        host_identity,
        &dl_dir,
        &tmp,
        pairing.clone(),
        "Host Device",
    );
    // Already past its deadline (a code minted, then never redeemed in time).
    let expired = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    *pairing.session.lock().unwrap() = Some(PairingSession {
        code: "482913".into(),
        expires: expired,
    });

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

    let joiner_priv = *generate_identity().unwrap().private_bytes();
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut sess = NoiseSession::handshake_initiator(stream, &joiner_priv, None)
        .await
        .unwrap();
    initiator_hello(&mut sess, "Latecomer".into())
        .await
        .unwrap();
    send_pair_request(&mut sess, "482913").await.unwrap();
    let (accept, _name, _reason) = recv_pair_confirm(&mut sess).await.unwrap();
    assert!(
        !accept,
        "the right digits on an expired code must still be declined"
    );
    drop(sess);

    responder
        .await
        .unwrap()
        .expect("clean Ok(()) even on an expired-code reject");

    assert!(
        ctx.trusted.read().unwrap().list().is_empty(),
        "an expired code grants no trust"
    );
    assert!(
        pairing.session.lock().unwrap().is_none(),
        "an expired invitation is cleared"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// A `PairRequest` arriving while NO invitation is active must NOT grow the
/// per-source failure map (finding 2): there is nothing to brute-force at an idle
/// host, so recording it would let a stranger spamming PairRequests accumulate
/// map entries without bound. The request is still politely declined.
#[tokio::test]
async fn no_session_pair_request_does_not_grow_rate_map() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-pair-nosess-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let dl_dir = tmp.join("downloads");
    std::fs::create_dir_all(&dl_dir).unwrap();

    let app = tauri::test::mock_app();
    let handle = app.handle().clone();

    let host_identity = Arc::new(generate_identity().unwrap());
    let host_priv = *host_identity.private_bytes();
    let pairing = Arc::new(PairingState::default());
    // Deliberately plant NO invitation: `pairing.session` stays None.
    let ctx = make_host_ctx(
        handle,
        host_identity,
        &dl_dir,
        &tmp,
        pairing.clone(),
        "Host Device",
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

    let joiner_priv = *generate_identity().unwrap().private_bytes();
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut sess = NoiseSession::handshake_initiator(stream, &joiner_priv, None)
        .await
        .unwrap();
    initiator_hello(&mut sess, "Prober".into()).await.unwrap();
    send_pair_request(&mut sess, "123456").await.unwrap();
    let (accept, _name, _reason) = recv_pair_confirm(&mut sess).await.unwrap();
    assert!(!accept, "a PairRequest at an idle host is declined");
    drop(sess);

    responder
        .await
        .unwrap()
        .expect("clean Ok(()) even with no active invitation");

    assert_eq!(
        pairing.rate.lock().unwrap().tracked(),
        0,
        "a no-session PairRequest must not record a failure (the unbounded-growth vector)"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The IP-direct flow (`connect_by_addr`, M7.2): an unpinned dial + hello + Bye
/// against a live host learns its Device ID, DeviceInfo name, and SAS — the
/// exact pieces the command stores as a manual peer and returns to the UI.
#[tokio::test]
async fn connect_by_addr_flow_learns_peer_identity() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-direct-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let dl_dir = tmp.join("downloads");
    std::fs::create_dir_all(&dl_dir).unwrap();

    let app = tauri::test::mock_app();
    let handle = app.handle().clone();

    let host_identity = Arc::new(generate_identity().unwrap());
    let host_priv = *host_identity.private_bytes();
    let host_id = host_identity.device_id();
    let pairing = Arc::new(PairingState::default());
    let ctx = make_host_ctx(handle, host_identity, &dl_dir, &tmp, pairing, "Direct Host");

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

    let joiner_priv = *generate_identity().unwrap().private_bytes();
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut sess = NoiseSession::handshake_initiator(stream, &joiner_priv, None)
        .await
        .unwrap();
    let sas = sess.sas().to_string();
    // The Device ID the command records is base64url of the learned static key.
    let learned_id = base64url_id(sess.remote_static());
    let peer = initiator_hello(&mut sess, "Dialer".into()).await.unwrap();
    send_bye(&mut sess).await.unwrap();
    drop(sess);

    responder
        .await
        .unwrap()
        .expect("a bye-only identity round-trip is clean");

    assert_eq!(
        learned_id, host_id,
        "the unpinned handshake learns the host's Device ID"
    );
    assert_eq!(
        peer.name.as_deref(),
        Some("Direct Host"),
        "the host's DeviceInfo name is learned"
    );
    assert_eq!(
        sas.len(),
        6,
        "a 6-digit SAS is available to show for the out-of-band check"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// base64url (no pad) of a 32-byte static key — the Device ID form the pairing /
/// direct-connect commands compute from `remote_static()`.
fn base64url_id(key: &[u8; 32]) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD.encode(key)
}
