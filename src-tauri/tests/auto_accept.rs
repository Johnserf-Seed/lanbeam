// clippy: reassign is clearer than a many-field struct literal in test setup
#![allow(clippy::field_reassign_with_default)]

//! Integration test (M4.4): an inbound transfer from a peer the user marked
//! auto-accept (recv_policy "trusted") must complete through the REAL
//! `handle_incoming` WITHOUT any `reply_file_request` — no oneshot is ever
//! parked — while the UI still receives `incoming_file_request` carrying
//! `autoAccepted: true`, and the accepted transfer refreshes the peer's
//! trust-store entry (last_seen bump + DeviceInfo name adoption).
//!
//! Lives here (not in lib unit tests) for the same reason as bye_session.rs:
//! a mock-runtime Tauri app links comctl32-v6 imports, and only integration-
//! test targets carry the Windows manifest linker args — see build.rs.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU16;
use std::sync::{Arc, Mutex, RwLock};

use tauri::Listener;
use tokio::net::{TcpListener, TcpStream};

use lanbeam_lib::identity::generate_identity;
use lanbeam_lib::partials::PartialsStore;
use lanbeam_lib::settings::Settings;
use lanbeam_lib::state::{CompletedLog, ConcurrencyGate, PairingState, TransferControl};
use lanbeam_lib::transfer::{build_send_list, handle_incoming, initiator_hello, run_send};
use lanbeam_lib::transport::noise::NoiseSession;
use lanbeam_lib::transport::TransportCtx;
use lanbeam_lib::trust::TrustStore;

#[tokio::test]
async fn trusted_auto_accept_skips_prompt_and_refreshes_trust() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-autoacc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let dl_dir = tmp.join("downloads");
    std::fs::create_dir_all(&dl_dir).unwrap();
    let src_file = tmp.join("hello.txt");
    let payload = b"auto-accepted payload".to_vec();
    std::fs::write(&src_file, &payload).unwrap();

    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    // Capture the request event payloads — the auto path must still fire one.
    let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = requests.clone();
        handle.listen("incoming_file_request", move |e| {
            sink.lock().unwrap().push(e.payload().to_string())
        });
    }
    // trust_updated must fire when the accepted transfer bumps the entry.
    let trust_events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = trust_events.clone();
        handle.listen("trust_updated", move |e| {
            sink.lock().unwrap().push(e.payload().to_string())
        });
    }

    // The SENDER is trusted with auto-accept BEFORE the transfer, under a
    // stale name — the DeviceInfo exchange must refresh it.
    let sender_identity = generate_identity().unwrap();
    let sender_priv = *sender_identity.private_bytes();
    let sender_id = sender_identity.device_id();
    let trust_path = tmp.join("trusted.json");
    {
        let mut seed = TrustStore::load(trust_path.clone());
        seed.set(sender_id.clone(), "Stale Name".into(), true, None, None);
        seed.save();
    }

    let mut settings = Settings::default();
    settings.recv_policy = "trusted".into();

    let receiver_identity = Arc::new(generate_identity().unwrap());
    let receiver_priv = *receiver_identity.private_bytes();
    let ctx = TransportCtx {
        app: handle,
        identity: receiver_identity,
        tcp_port: Arc::new(AtomicU16::new(0)),
        download_dir: Arc::new(RwLock::new(dl_dir.clone())),
        settings: Arc::new(RwLock::new(settings)),
        peers: Arc::new(Mutex::new(HashMap::new())),
        pending: Arc::new(Mutex::new(HashMap::new())),
        in_flight: Arc::new(Mutex::new(HashSet::new())),
        completed: Arc::new(Mutex::new(CompletedLog::new())),
        trusted: Arc::new(RwLock::new(TrustStore::load(trust_path))),
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
        let sess = NoiseSession::handshake_responder(stream, &receiver_priv)
            .await
            .unwrap();
        handle_incoming(sess, &ctx2).await
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut sess = NoiseSession::handshake_initiator(stream, &sender_priv, None)
        .await
        .unwrap();
    initiator_hello(&mut sess, "Fresh Name".into())
        .await
        .unwrap();
    let items = build_send_list(std::slice::from_ref(&src_file), false).unwrap();
    // The proof of auto-accept: run_send blocks on FileSendReply, and NOBODY
    // calls reply_file_request — acceptance can only come from the policy.
    run_send(
        &mut sess,
        "t-auto",
        &items,
        &TransferControl::neutral(),
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await
    .unwrap();
    drop(sess);

    responder
        .await
        .unwrap()
        .expect("auto-accepted transfer must complete cleanly");

    // The prompt machinery was skipped: nothing ever parked…
    assert!(
        ctx.pending.lock().unwrap().is_empty(),
        "auto-accept must not park a pending prompt"
    );
    // …but the UI was told, with the auto-accept marker set.
    let reqs = requests.lock().unwrap();
    assert_eq!(reqs.len(), 1, "exactly one incoming_file_request");
    let payload_json: serde_json::Value = serde_json::from_str(&reqs[0]).unwrap();
    assert_eq!(payload_json["autoAccepted"], serde_json::Value::Bool(true));
    assert_eq!(payload_json["sessionId"], "t-auto");
    assert_eq!(
        payload_json["deviceId"],
        serde_json::Value::String(sender_id.clone())
    );
    drop(reqs);

    // The accepted transfer refreshed the trust entry: DeviceInfo name adopted,
    // last_seen stamped, auto_accept untouched — and the UI was notified.
    {
        let store = ctx.trusted.read().unwrap();
        let entry = store.get(&sender_id).expect("peer stays trusted");
        assert_eq!(entry.name, "Fresh Name", "DeviceInfo name must be adopted");
        assert!(entry.auto_accept);
        assert!(entry.last_seen >= entry.paired_at);
        assert!(entry.last_seen > 0);
    }
    assert!(
        !trust_events.lock().unwrap().is_empty(),
        "trust_updated must fire on the last_seen bump"
    );

    // And the file actually landed, byte-identical.
    assert_eq!(ctx.completed.lock().unwrap().len(), 1);
    let received = dl_dir.join("hello.txt");
    assert_eq!(std::fs::read(&received).unwrap(), payload);

    let _ = std::fs::remove_dir_all(&tmp);
}
