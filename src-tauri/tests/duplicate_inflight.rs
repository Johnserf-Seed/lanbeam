// clippy: reassign is clearer than a many-field struct literal in test setup
#![allow(clippy::field_reassign_with_default)]

//! Integration test (M4 hardening): the duplicate-transfer_id guard must span
//! the WHOLE session lifetime, not just the prompt window. Under recv_policy
//! "all" nothing is ever parked in `pending`, so before the in-flight registry
//! a peer could open two sessions with one sender-chosen id and conflate every
//! event/completed entry keyed by it. Here: a second session reusing the id of
//! a live, auto-accepted, MID-TRANSFER session is politely declined (reason
//! "duplicate transfer id", no `transfer_error` emit) — and once the first
//! session completes, the id becomes usable again.
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
use lanbeam_lib::protocol::{self, AppMessage, FileMeta, Frame};
use lanbeam_lib::settings::Settings;
use lanbeam_lib::state::{CompletedLog, ConcurrencyGate, PairingState, TransferControl};
use lanbeam_lib::transfer::{build_send_list, handle_incoming, run_send};
use lanbeam_lib::transport::noise::NoiseSession;
use lanbeam_lib::transport::TransportCtx;
use lanbeam_lib::trust::TrustStore;

async fn write_control(s: &mut NoiseSession, msg: &AppMessage) {
    s.write_msg(&protocol::encode_control(msg).unwrap())
        .await
        .unwrap();
}

async fn read_control(s: &mut NoiseSession) -> AppMessage {
    let pt = s
        .read_msg()
        .await
        .unwrap()
        .expect("peer closed unexpectedly");
    match protocol::decode(&pt).unwrap() {
        Frame::Control(m) => m,
        other => panic!("expected control frame, got {other:?}"),
    }
}

#[tokio::test]
async fn duplicate_id_rejected_while_live_and_freed_after_completion() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-dupflight-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let dl_dir = tmp.join("downloads");
    std::fs::create_dir_all(&dl_dir).unwrap();
    let src_file = tmp.join("later.txt");
    std::fs::write(&src_file, b"third session, same id").unwrap();

    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = requests.clone();
        handle.listen("incoming_file_request", move |e| {
            sink.lock().unwrap().push(e.payload().to_string())
        });
    }
    let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = errors.clone();
        handle.listen("transfer_error", move |e| {
            sink.lock().unwrap().push(e.payload().to_string())
        });
    }

    // "all" auto-accepts ANY peer: the prompt machinery (and its `pending`
    // duplicate check) is never involved — only the in-flight registry guards.
    let mut settings = Settings::default();
    settings.recv_policy = "all".into();

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
    let (hs_tx, mut hs_rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let c = ctx2.clone();
            let h = tokio::spawn(async move {
                let sess = NoiseSession::handshake_responder(stream, &receiver_priv)
                    .await
                    .unwrap();
                handle_incoming(sess, &c).await
            });
            if hs_tx.send(h).is_err() {
                return;
            }
        }
    });

    let sender_priv = *generate_identity().unwrap().private_bytes();

    // ── Session A: legacy-style manifest, auto-accepted, then STALLS mid-file.
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut sa = NoiseSession::handshake_initiator(stream, &sender_priv, None)
        .await
        .unwrap();
    let h_a = hs_rx.recv().await.unwrap();
    write_control(
        &mut sa,
        &AppMessage::FileListBegin {
            transfer_id: "t-dup-live".into(),
            total_size: 8,
            file_count: 1,
        },
    )
    .await;
    write_control(
        &mut sa,
        &AppMessage::FileListEntry {
            file: FileMeta {
                name: "half.bin".into(),
                size: 8,
                mtime: 0,
                mode: 0,
                sha256: None,
            },
        },
    )
    .await;
    write_control(
        &mut sa,
        &AppMessage::FileListEnd {
            transfer_id: "t-dup-live".into(),
        },
    )
    .await;
    match read_control(&mut sa).await {
        AppMessage::FileSendReply { accept: true, .. } => {} // policy accepted, id now in flight
        other => panic!("expected auto-accept, got {other:?}"),
    }
    // Half the declared bytes: session A is now live and mid-transfer.
    sa.write_msg(&protocol::encode_file_chunk(0, &[1, 2, 3, 4]).unwrap())
        .await
        .unwrap();

    // ── Session B: SAME id while A is in flight → polite decline, no events.
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut sb = NoiseSession::handshake_initiator(stream, &sender_priv, None)
        .await
        .unwrap();
    let h_b = hs_rx.recv().await.unwrap();
    write_control(
        &mut sb,
        &AppMessage::FileListBegin {
            transfer_id: "t-dup-live".into(),
            total_size: 1,
            file_count: 1,
        },
    )
    .await;
    write_control(
        &mut sb,
        &AppMessage::FileListEntry {
            file: FileMeta {
                name: "dup.bin".into(),
                size: 1,
                mtime: 0,
                mode: 0,
                sha256: None,
            },
        },
    )
    .await;
    write_control(
        &mut sb,
        &AppMessage::FileListEnd {
            transfer_id: "t-dup-live".into(),
        },
    )
    .await;
    match read_control(&mut sb).await {
        AppMessage::FileSendReply {
            accept: false,
            reason,
            ..
        } => {
            assert_eq!(reason.as_deref(), Some("duplicate transfer id"));
        }
        other => panic!("expected duplicate decline, got {other:?}"),
    }
    drop(sb);
    assert!(
        h_b.await.unwrap().is_err(),
        "duplicate session must end in an error"
    );

    // ── Session A finishes normally: the duplicate never disturbed it.
    sa.write_msg(&protocol::encode_file_chunk(1, &[5, 6, 7, 8]).unwrap())
        .await
        .unwrap();
    match read_control(&mut sa).await {
        AppMessage::TransferAck { ok: true, .. } => {}
        other => panic!("expected ok ack, got {other:?}"),
    }
    drop(sa);
    h_a.await
        .unwrap()
        .expect("first session must complete cleanly");

    // ── Session C: the guard released the id, so it is claimable again.
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut sc = NoiseSession::handshake_initiator(stream, &sender_priv, None)
        .await
        .unwrap();
    let h_c = hs_rx.recv().await.unwrap();
    let items = build_send_list(std::slice::from_ref(&src_file), false).unwrap();
    run_send(
        &mut sc,
        "t-dup-live",
        &items,
        &TransferControl::neutral(),
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await
    .expect("id must be reusable once the first session ended");
    drop(sc);
    h_c.await
        .unwrap()
        .expect("reused-id session must complete cleanly");

    // Only the two ACCEPTED sessions surfaced to the UI; the duplicate emitted
    // neither a request nor a transfer_error (which would clobber A's row).
    assert_eq!(
        requests.lock().unwrap().len(),
        2,
        "requests for A and C only"
    );
    assert!(
        errors.lock().unwrap().is_empty(),
        "no transfer_error: {:?}",
        errors.lock().unwrap()
    );
    assert!(
        ctx.pending.lock().unwrap().is_empty(),
        "auto-accept never parks"
    );
    assert!(
        ctx.in_flight.lock().unwrap().is_empty(),
        "registry drained after all sessions"
    );
    assert_eq!(
        ctx.completed.lock().unwrap().len(),
        1,
        "same id → one completed entry"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
