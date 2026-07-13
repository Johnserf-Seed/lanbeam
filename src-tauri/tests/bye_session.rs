//! Integration test: a Bye-only session (what `connect_device`'s identity
//! round-trip produces) through the REAL `handle_incoming` must end cleanly
//! and leave zero trace — no UI events, `pending`/`completed` untouched.
//!
//! Lives here (not in lib unit tests) because building a mock-runtime Tauri
//! app links comctl32-v6 imports, and only integration-test targets can carry
//! the Windows manifest linker args those need — see build.rs.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU16;
use std::sync::{Arc, Mutex, RwLock};

use tauri::Listener;
use tokio::net::{TcpListener, TcpStream};

use lanbeam_lib::consts::TRANSFER_V2;
use lanbeam_lib::identity::generate_identity;
use lanbeam_lib::partials::PartialsStore;
use lanbeam_lib::settings::Settings;
use lanbeam_lib::state::{CompletedLog, ConcurrencyGate, PairingState};
use lanbeam_lib::transfer::{handle_incoming, initiator_hello, send_bye};
use lanbeam_lib::transport::noise::NoiseSession;
use lanbeam_lib::transport::TransportCtx;
use lanbeam_lib::trust::TrustStore;

#[tokio::test]
async fn bye_only_session_leaves_no_events_or_state() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-bye-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    // Record every transfer-lifecycle event the UI could possibly see.
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    for name in [
        "incoming_file_request",
        "transfer_started",
        "transfer_progress",
        "transfer_done",
        "transfer_error",
    ] {
        let sink = seen.clone();
        handle.listen(name, move |_| sink.lock().unwrap().push(name.to_string()));
    }

    // Throwaway identities — never the user's real keychain entry.
    let identity = Arc::new(generate_identity().unwrap());
    let responder_priv = *identity.private_bytes();
    let ctx = TransportCtx {
        app: handle,
        identity,
        tcp_port: Arc::new(AtomicU16::new(0)),
        download_dir: Arc::new(RwLock::new(tmp.clone())),
        settings: Arc::new(RwLock::new(Settings::default())),
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
    };

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ctx2 = ctx.clone();
    let responder = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let sess = NoiseSession::handshake_responder(stream, &responder_priv)
            .await
            .unwrap();
        handle_incoming(sess, &ctx2).await
    });

    // Mirror connect_device: hello exchange, then a deliberate Bye.
    let prober_priv = *generate_identity().unwrap().private_bytes();
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &prober_priv, None)
        .await
        .unwrap();
    let peer = initiator_hello(&mut s, "Prober".into()).await.unwrap();
    // Two current builds negotiate the highest common version — v2 since M7
    // (stage 7.0) appended it to SUPPORTED_VERSIONS.
    assert_eq!(peer.version, TRANSFER_V2);
    assert!(peer.name.is_some(), "responder must introduce itself");
    send_bye(&mut s).await.unwrap();
    drop(s);

    responder
        .await
        .unwrap()
        .expect("a bye-only session must be a clean Ok(())");
    // handle_incoming returned, so any emit it made has already run.
    assert!(
        seen.lock().unwrap().is_empty(),
        "no UI events for a bye-only session: {:?}",
        seen.lock().unwrap()
    );
    assert!(
        ctx.pending.lock().unwrap().is_empty(),
        "pending must stay untouched"
    );
    assert_eq!(
        ctx.completed.lock().unwrap().len(),
        0,
        "completed must stay untouched"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
