// clippy: reassign is clearer than a many-field struct literal in test setup
#![allow(clippy::field_reassign_with_default)]

//! Integration tests (M6.4/6.6) through the REAL `handle_incoming`:
//!   * auto-organize places a received file under a `<sender name>/` folder;
//!   * a seeded partial (persisted `partials.json` entry + on-disk bytes)
//!     resumes end to end — byte-identical, hash-verified — and the partial
//!     entry is cleared once the transfer completes.
//!
//! Lives here (not in lib unit tests) for the same reason as auto_accept.rs:
//! a mock-runtime Tauri app links comctl32-v6 imports, and only integration-
//! test targets carry the Windows manifest linker args — see build.rs.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU16;
use std::sync::{Arc, Mutex, RwLock};

use tokio::net::{TcpListener, TcpStream};

use lanbeam_lib::identity::generate_identity;
use lanbeam_lib::partials::{PartialRecord, PartialsStore};
use lanbeam_lib::settings::Settings;
use lanbeam_lib::state::{CompletedLog, ConcurrencyGate, PairingState, TransferControl};
use lanbeam_lib::transfer::{build_send_list, handle_incoming, initiator_hello, run_send};
use lanbeam_lib::transport::noise::NoiseSession;
use lanbeam_lib::transport::TransportCtx;
use lanbeam_lib::trust::TrustStore;

/// Build a receiver `TransportCtx` around a mock app, wiring the given settings,
/// download dir, and (already-populated) partials store path. Generic over the
/// Tauri runtime so it accepts the tests' `MockRuntime` handle.
fn make_ctx<R: tauri::Runtime>(
    handle: tauri::AppHandle<R>,
    settings: Settings,
    dl_dir: &std::path::Path,
    trust_path: std::path::PathBuf,
    partials_path: std::path::PathBuf,
    identity: Arc<lanbeam_lib::identity::Identity>,
) -> TransportCtx<R> {
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
        trusted: Arc::new(RwLock::new(TrustStore::load(trust_path))),
        transfers_ctl: Arc::new(Mutex::new(HashMap::new())),
        partials: Arc::new(RwLock::new(PartialsStore::load(partials_path))),
        concurrency: Arc::new(ConcurrencyGate::new()),
        pairing: Arc::new(PairingState::default()),
        text_rate: Default::default(),
    }
}

/// Auto-organize "device" (M6.6): an auto-accepted transfer lands under a folder
/// named after the sender's DeviceInfo name, not straight in the root.
#[tokio::test]
async fn organize_device_places_file_under_sender_folder() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-org-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let dl_dir = tmp.join("downloads");
    std::fs::create_dir_all(&dl_dir).unwrap();
    let src_file = tmp.join("photo.jpg");
    let payload = b"organized under the device name".to_vec();
    std::fs::write(&src_file, &payload).unwrap();

    let app = tauri::test::mock_app();
    let handle = app.handle().clone();

    let mut settings = Settings::default();
    settings.recv_policy = "all".into(); // auto-accept, no prompt
    settings.organize = "device".into();

    let receiver_identity = Arc::new(generate_identity().unwrap());
    let receiver_priv = *receiver_identity.private_bytes();
    let ctx = make_ctx(
        handle,
        settings,
        &dl_dir,
        tmp.join("trusted.json"),
        tmp.join("partials.json"),
        receiver_identity,
    );

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

    let sender_priv = *generate_identity().unwrap().private_bytes();
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut sess = NoiseSession::handshake_initiator(stream, &sender_priv, None)
        .await
        .unwrap();
    // The sender introduces itself as "MyPhone" — the organize folder name.
    initiator_hello(&mut sess, "MyPhone".into()).await.unwrap();
    let items = build_send_list(std::slice::from_ref(&src_file), false).unwrap();
    run_send(
        &mut sess,
        "t-org",
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
        .expect("organized transfer completes");

    let organized = dl_dir.join("MyPhone").join("photo.jpg");
    assert!(
        organized.exists(),
        "file must land under the sender-name folder"
    );
    assert_eq!(std::fs::read(&organized).unwrap(), payload);
    assert!(
        !dl_dir.join("photo.jpg").exists(),
        "must NOT land straight in the root"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// End-to-end resume (M6.4): a persisted partial (partials.json entry + the
/// matching on-disk prefix) lets a NEW manifest of the same hashed file continue
/// from its offset. The receiver replies with the offset, the sender streams
/// only the tail, and the reassembled file is byte-identical + hash-verified —
/// after which the partial entry is cleared.
#[tokio::test]
async fn seeded_partial_resumes_end_to_end_and_clears() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-e2eresume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let dl_dir = tmp.join("downloads");
    std::fs::create_dir_all(&dl_dir).unwrap();

    let payload: Vec<u8> = (0..250_000u32).map(|i| (i % 251) as u8).collect();
    let offset = 90_000u64;
    let src_file = tmp.join("resumed.bin");
    std::fs::write(&src_file, &payload).unwrap();
    // The whole-file hash the manifest will carry (and the partial's identity).
    let items = build_send_list(std::slice::from_ref(&src_file), true).unwrap();
    let sha256 = items[0].meta.sha256.clone();
    assert!(sha256.is_some(), "verify on must attach a hash");

    // Seed the on-disk prefix and the persisted partial keyed by the SENDER's id.
    std::fs::write(dl_dir.join("resumed.bin"), &payload[..offset as usize]).unwrap();
    let sender_identity = generate_identity().unwrap();
    let sender_priv = *sender_identity.private_bytes();
    let sender_id = sender_identity.device_id();
    let partials_path = tmp.join("partials.json");
    {
        let mut store = PartialsStore::load(partials_path.clone());
        store.set_files(
            &sender_id,
            &[],
            vec![PartialRecord {
                rel: "resumed.bin".into(),
                size: payload.len() as u64,
                sha256: sha256.clone(),
                disk_rel: "resumed.bin".into(),
                bytes_written: offset,
            }],
        );
        store.save();
    }

    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    let mut settings = Settings::default();
    settings.recv_policy = "all".into(); // auto-accept

    let receiver_identity = Arc::new(generate_identity().unwrap());
    let receiver_priv = *receiver_identity.private_bytes();
    let ctx = make_ctx(
        handle,
        settings,
        &dl_dir,
        tmp.join("trusted.json"),
        partials_path,
        receiver_identity,
    );

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
    initiator_hello(&mut sess, "Sender".into()).await.unwrap();
    // run_send honors the resume offset the receiver's reply carries: it seeks to
    // `offset` and streams only the tail.
    run_send(
        &mut sess,
        "t-e2e",
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
        .expect("resumed transfer completes + verifies");

    // Byte-identical delivery (the hash verified inside handle_incoming, else it
    // would have errored).
    let got = std::fs::read(dl_dir.join("resumed.bin")).unwrap();
    assert_eq!(got, payload, "resumed file must be byte-identical");
    // The completed transfer cleared the partial entry.
    assert_eq!(
        ctx.partials.read().unwrap().len(),
        0,
        "partial cleared on completion"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
