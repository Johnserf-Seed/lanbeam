// clippy: reassign is clearer than a many-field struct literal in test setup
#![allow(clippy::field_reassign_with_default)]

//! Cross-stage integration test (M7.1 + M7.3): the two v2 features COMPOSE over
//! a single, reused host `TransportCtx`. First a device pairs (7.1) — the host
//! stores it in the trust store and consumes the one-shot code — and THEN, on a
//! fresh session against the SAME host state, that device pushes a quick text
//! (7.3) which is delivered and acked. This is the one seam the isolated
//! `pairing.rs` / `quick_text.rs` tests don't touch: it proves the shared host
//! state (`trusted`, `in_flight`, `concurrency`, `pairing`) survives one M7
//! session kind and still serves the next, so the pairing dispatch never wedges
//! the later text dispatch on the same `handle_incoming` entry point.
//!
//! Lives here (not in lib unit tests) for the same reason as pairing.rs: a
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
    handle_incoming, initiator_hello, recv_pair_confirm, send_pair_request, send_text,
};
use lanbeam_lib::transport::noise::NoiseSession;
use lanbeam_lib::transport::TransportCtx;
use lanbeam_lib::trust::TrustStore;

/// Build the host `TransportCtx` once and reuse it across both sessions, so the
/// trust granted by pairing is still present when the quick text arrives. Shares
/// the caller's `pairing` state so the test can plant the active invitation.
fn make_host_ctx<R: tauri::Runtime>(
    handle: tauri::AppHandle<R>,
    identity: Arc<Identity>,
    dl_dir: &std::path::Path,
    tmp: &std::path::Path,
    pairing: Arc<PairingState>,
) -> TransportCtx<R> {
    let mut settings = Settings::default();
    settings.device_name = "Host Device".to_string();
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

/// Pair, then quick-text, against ONE host `TransportCtx`. The pair mutually
/// trusts the joiner and consumes the code; the subsequent text from the same
/// device is delivered and acked — proving the two M7 features share the host's
/// `handle_incoming` state without interfering.
#[tokio::test]
async fn pair_then_quick_text_share_one_host_ctx() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-pair-text-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let dl_dir = tmp.join("downloads");
    std::fs::create_dir_all(&dl_dir).unwrap();

    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    let texts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = texts.clone();
        handle.listen("text_received", move |e| {
            sink.lock().unwrap().push(e.payload().to_string())
        });
    }

    let host_identity = Arc::new(generate_identity().unwrap());
    let host_priv = *host_identity.private_bytes();
    let pairing = Arc::new(PairingState::default());
    let ctx = make_host_ctx(handle, host_identity, &dl_dir, &tmp, pairing.clone());

    // Plant the active invitation, exactly as `start_pairing` would.
    *pairing.session.lock().unwrap() = Some(PairingSession {
        code: "482913".into(),
        expires: Instant::now() + Duration::from_secs(600),
    });

    // The joiner keeps its identity between the two sessions — it is the SAME
    // device pairing and then texting.
    let joiner_identity = generate_identity().unwrap();
    let joiner_priv = *joiner_identity.private_bytes();
    let joiner_id = joiner_identity.device_id();

    // ── session 1: pair (M7.1) ──────────────────────────────────────────────
    {
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

        let stream = TcpStream::connect(addr).await.unwrap();
        let mut sess = NoiseSession::handshake_initiator(stream, &joiner_priv, None)
            .await
            .unwrap();
        let peer = initiator_hello(&mut sess, "Joiner Device".into())
            .await
            .unwrap();
        assert_eq!(peer.version, TRANSFER_V2, "two current builds negotiate v2");
        send_pair_request(&mut sess, "482913").await.unwrap();
        let (accept, host_name, _reason) = recv_pair_confirm(&mut sess).await.unwrap();
        assert!(accept, "the right code pairs");
        assert_eq!(host_name, "Host Device");
        drop(sess);

        responder
            .await
            .unwrap()
            .expect("a successful pair is a clean Ok(())");
    }

    // The code is spent — and NOTHING is trusted yet. Redeeming a code proves the
    // code, not the device holding it; the SAS compare is what proves that, and
    // it happens in front of a human, in the UI.
    assert!(
        ctx.trusted.read().unwrap().list().is_empty(),
        "the pairing handshake must not trust anyone on its own"
    );
    assert!(
        pairing.session.lock().unwrap().is_none(),
        "the code was consumed by the pair"
    );

    // Stand in for the UI: the user compared the two SAS screens, they matched,
    // and PairModal recorded the trust through `set_trusted` — which, like the
    // trust circle, turns auto-accept on with it. This is the step that used to
    // happen behind the user's back inside the handshake.
    {
        let mut store = ctx.trusted.write().unwrap();
        store.set(joiner_id.clone(), "Joiner Device".into(), true, None, None);
    }

    // ── session 2: quick text (M7.3) over the SAME host ctx ─────────────────
    let listener2 = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr2 = listener2.local_addr().unwrap();
    let ctx3 = ctx.clone();
    let responder = tokio::spawn(async move {
        let (stream, _) = listener2.accept().await.unwrap();
        let sess = NoiseSession::handshake_responder(stream, &host_priv)
            .await
            .unwrap();
        handle_incoming(sess, &ctx3).await
    });

    let stream = TcpStream::connect(addr2).await.unwrap();
    let mut sess = NoiseSession::handshake_initiator(stream, &joiner_priv, None)
        .await
        .unwrap();
    let peer = initiator_hello(&mut sess, "Joiner Device".into())
        .await
        .unwrap();
    assert_eq!(
        peer.version, TRANSFER_V2,
        "the text session still negotiates v2"
    );
    send_text(&mut sess, "paired — now here's a link".into(), Some(false))
        .await
        .expect("send_text resolves once the receiver acks");
    drop(sess);

    responder
        .await
        .unwrap()
        .expect("a delivered quick text is a clean Ok(())");

    // The text arrived through the same host that just paired this device.
    let events = texts.lock().unwrap();
    assert_eq!(events.len(), 1, "exactly one text_received after the pair");
    let payload: serde_json::Value = serde_json::from_str(&events[0]).unwrap();
    assert_eq!(
        payload["text"], "paired — now here's a link",
        "the delivered text is exact"
    );
    assert_eq!(
        payload["deviceId"],
        serde_json::Value::String(joiner_id),
        "the text is attributed to the paired device's id"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
