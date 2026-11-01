// clippy: reassign is clearer than a many-field struct literal in test setup
#![allow(clippy::field_reassign_with_default)]

//! Integration tests for the accept prompt through the REAL `handle_incoming`:
//! the names it shows are the sanitized ones (M4), it is withdrawn when the
//! sender hangs up while it is up (M12), strangers can't keep opening
//! requests, or pile up more prompts than anyone answers (M18), and a waiting
//! sender hears where its request stands (F4).
//!
//! Lives here (not in lib unit tests) for the same reason as bye_session.rs:
//! a mock-runtime Tauri app links comctl32-v6 imports, and only integration-
//! test targets carry the Windows manifest linker args — see build.rs.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::sync::atomic::AtomicU16;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use tauri::Listener;
use tokio::net::{TcpListener, TcpStream};

use lanbeam_lib::consts::{MAX_FILE_REQUESTS_PER_WINDOW, MAX_PENDING_PROMPTS};
use lanbeam_lib::error::LanBeamError;
use lanbeam_lib::identity::{generate_identity, Identity};
use lanbeam_lib::partials::PartialsStore;
use lanbeam_lib::settings::Settings;
use lanbeam_lib::state::{
    CompletedLog, ConcurrencyGate, PairingState, Parked, ReplyDecision, TransferControl,
    TransferCtlGuard,
};
use lanbeam_lib::transfer::{
    build_send_list, handle_incoming, initiator_hello, run_send, FileEvent, PendingState,
};
use lanbeam_lib::transport::noise::NoiseSession;
use lanbeam_lib::transport::TransportCtx;
use lanbeam_lib::trust::TrustStore;

/// A receiver context over `tmp`, with `recv_policy`.
fn ctx(
    app: tauri::AppHandle<tauri::test::MockRuntime>,
    tmp: &Path,
    recv_policy: &str,
) -> TransportCtx<tauri::test::MockRuntime> {
    let dl_dir = tmp.join("downloads");
    std::fs::create_dir_all(&dl_dir).unwrap();
    let mut settings = Settings::default();
    settings.recv_policy = recv_policy.into();
    TransportCtx {
        app,
        identity: Arc::new(generate_identity().unwrap()),
        tcp_port: Arc::new(AtomicU16::new(0)),
        download_dir: Arc::new(RwLock::new(dl_dir)),
        manual_peers: Default::default(),
        settings: Arc::new(RwLock::new(settings)),
        peers: Arc::new(Mutex::new(HashMap::new())),
        pending: Arc::new(Mutex::new(HashMap::new())),
        in_flight: Arc::new(Mutex::new(HashSet::new())),
        completed: Arc::new(Mutex::new(CompletedLog::new())),
        trusted: Arc::new(RwLock::new(TrustStore::load(tmp.join("trusted.json")))),
        transfers_ctl: Arc::new(Mutex::new(HashMap::new())),
        partials: Arc::new(RwLock::new(PartialsStore::load(tmp.join("partials.json")))),
        concurrency: Arc::new(ConcurrencyGate::new(3)),
        pairing: Arc::new(PairingState::default()),
        text_rate: Default::default(),
        file_rate: Default::default(),
    }
}

/// Capture every payload of `event` on `app`.
fn capture(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    event: &str,
) -> Arc<Mutex<Vec<serde_json::Value>>> {
    let sink: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let s = sink.clone();
    app.listen(event.to_string(), move |e| {
        s.lock()
            .unwrap()
            .push(serde_json::from_str(e.payload()).unwrap())
    });
    sink
}

/// Spawn the receiving side: accept ONE connection on a fresh loopback
/// listener and run `handle_incoming` on it. Returns the address to dial.
async fn spawn_receiver(
    ctx: &TransportCtx<tauri::test::MockRuntime>,
) -> (
    std::net::SocketAddr,
    tokio::task::JoinHandle<lanbeam_lib::error::Result<()>>,
) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ctx2 = ctx.clone();
    let receiver_priv = *ctx.identity.private_bytes();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let sess = NoiseSession::handshake_responder(stream, &receiver_priv)
            .await
            .unwrap();
        handle_incoming(sess, &ctx2).await
    });
    (addr, task)
}

/// Send `file` (renamed on the wire to `wire_name`) from `sender` to `addr`
/// under `id`, with `ctl` — returns the send's result.
async fn send(
    sender: &Identity,
    addr: std::net::SocketAddr,
    file: &Path,
    wire_name: &str,
    id: &str,
    ctl: &TransferControl,
) -> lanbeam_lib::error::Result<()> {
    let noted = Arc::default();
    send_noting(sender, addr, file, wire_name, id, ctl, noted).await
}

/// [`send`], noting what the sender heard while it waited — `"prompted"`,
/// `"queued"` — and `"accepted"`, in order, in `noted`.
async fn send_noting(
    sender: &Identity,
    addr: std::net::SocketAddr,
    file: &Path,
    wire_name: &str,
    id: &str,
    ctl: &TransferControl,
    noted: Arc<Mutex<Vec<&'static str>>>,
) -> lanbeam_lib::error::Result<()> {
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut sess = NoiseSession::handshake_initiator(stream, sender.private_bytes(), None)
        .await
        .unwrap();
    initiator_hello(&mut sess, "Sender".into()).await.unwrap();
    let mut items = build_send_list(&[file.to_path_buf()], false).unwrap();
    items[0].meta.name = wire_name.into();
    let mut on_file = |e: FileEvent<'_>| {
        let note = match e {
            FileEvent::Pending(PendingState::Prompted) => "prompted",
            FileEvent::Pending(PendingState::Queued) => "queued",
            FileEvent::Accepted => "accepted",
            _ => return,
        };
        noted.lock().unwrap().push(note);
    };
    run_send(&mut sess, id, &items, ctl, None, &mut on_file, |_, _| {}).await
}

/// Wait (up to 5 s) for `cond`.
async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !cond() {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// M4 + M12: the prompt shows the name the file will actually get — not the
/// sender's string, whose bidi override makes "photo\u{202E}gnp.exe" READ as
/// "photoexe.png" — and when the sender gives up while it is up, the prompt is
/// withdrawn at once instead of waiting out its 120 seconds for an answer
/// nobody can use.
#[tokio::test]
async fn prompt_shows_sanitized_names_and_is_withdrawn_when_the_sender_hangs_up() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-prompt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let file = tmp.join("a.bin");
    std::fs::write(&file, b"payload").unwrap();

    let app = tauri::test::mock_app();
    let ctx = ctx(app.handle().clone(), &tmp, "ask");
    let requests = capture(app.handle(), "incoming_file_request");
    let errors = capture(app.handle(), "transfer_error");
    let (addr, receiver) = spawn_receiver(&ctx).await;

    let sender = generate_identity().unwrap();
    let ctl = TransferControl::neutral();
    let cancel = ctl.cancel.clone();
    let sending = tokio::spawn({
        let file = file.clone();
        async move {
            send(
                &sender,
                addr,
                &file,
                "photo\u{202E}gnp.exe",
                "t-hangup",
                &ctl,
            )
            .await
        }
    });

    // Wait for the prompt to be up.
    let deadline = Instant::now() + Duration::from_secs(5);
    while requests.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the prompt never came up");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    {
        let reqs = requests.lock().unwrap();
        assert_eq!(reqs[0]["files"][0]["name"], "photognp.exe");
        assert_eq!(reqs[0]["autoAccepted"], false);
    }
    assert_eq!(ctx.pending.lock().unwrap().len(), 1, "the prompt is parked");
    // …with the very payload its event carried, for a window that loads later
    // to fetch (L23).
    {
        let parked: Vec<serde_json::Value> = ctx
            .pending
            .lock()
            .unwrap()
            .values()
            .map(|p| p.request.clone())
            .collect();
        assert_eq!(parked, vec![requests.lock().unwrap()[0].clone()]);
    }

    // The sender cancels while the prompt is up.
    cancel.cancel();
    assert!(matches!(
        sending.await.unwrap(),
        Err(LanBeamError::Cancelled)
    ));
    let res = tokio::time::timeout(Duration::from_secs(5), receiver)
        .await
        .expect("the receive notices the hang-up long before the prompt's 120s")
        .unwrap();
    assert!(
        matches!(res, Err(LanBeamError::ConnectionLost(_))),
        "{res:?}"
    );
    assert!(
        ctx.pending.lock().unwrap().is_empty(),
        "the prompt is withdrawn"
    );
    let errs = errors.lock().unwrap();
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0]["sessionId"], "t-hangup");
    assert_eq!(errs[0]["code"], "connection_lost");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// F4: the sender waiting on a prompt hears that it is up — before anyone has
/// answered — and then that it was accepted.
#[tokio::test]
async fn a_waiting_sender_hears_its_request_is_on_screen() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-shown-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let file = tmp.join("a.bin");
    std::fs::write(&file, b"payload").unwrap();

    let app = tauri::test::mock_app();
    let ctx = ctx(app.handle().clone(), &tmp, "ask");
    let (addr, receiver) = spawn_receiver(&ctx).await;
    let noted: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    let sending = tokio::spawn({
        let (file, noted) = (file.clone(), noted.clone());
        async move {
            let ctl = TransferControl::neutral();
            let sender = generate_identity().unwrap();
            send_noting(&sender, addr, &file, "a.bin", "t-shown", &ctl, noted).await
        }
    });

    until("the sender never heard the prompt was up", || {
        noted.lock().unwrap().contains(&"prompted")
    })
    .await;
    assert_eq!(ctx.pending.lock().unwrap().len(), 1, "nobody has answered");

    // Someone says yes.
    let parked = ctx.pending.lock().unwrap().remove("t-shown").unwrap();
    parked.reply.send(ReplyDecision::plain(true)).unwrap();
    sending.await.unwrap().expect("delivered");
    receiver.await.unwrap().unwrap();
    assert_eq!(*noted.lock().unwrap(), vec!["prompted", "accepted"]);

    let _ = std::fs::remove_dir_all(&tmp);
}

/// F4, and M12 one step on: a receive that has its yes but has to wait for a
/// free slot tells the sender so — and when the sender gives up while it waits,
/// the receiver notices at once, not when a slot frees and its accept meets a
/// closed socket (up to ~145 s later).
#[tokio::test]
async fn a_queued_receive_tells_the_sender_and_notices_it_leave() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-queued-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let file = tmp.join("a.bin");
    std::fs::write(&file, b"payload").unwrap();

    let app = tauri::test::mock_app();
    // "all": accepted without asking anyone, straight to the slots.
    let ctx = ctx(app.handle().clone(), &tmp, "all");
    let queued = capture(app.handle(), "transfer_queued");
    let errors = capture(app.handle(), "transfer_error");
    let _busy: Vec<_> = (0..3)
        .map(|_| ctx.concurrency.try_acquire_receive().expect("a free slot"))
        .collect();
    let (addr, receiver) = spawn_receiver(&ctx).await;
    let noted: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    let ctl = TransferControl::neutral();
    let cancel = ctl.cancel.clone();
    let sending = tokio::spawn({
        let (file, noted) = (file.clone(), noted.clone());
        async move {
            let sender = generate_identity().unwrap();
            send_noting(&sender, addr, &file, "a.bin", "t-queued", &ctl, noted).await
        }
    });

    until("the sender never heard it was queued", || {
        noted.lock().unwrap().contains(&"queued")
    })
    .await;
    assert_eq!(*noted.lock().unwrap(), vec!["queued"], "nobody was asked");
    assert_eq!(
        queued.lock().unwrap().len(),
        1,
        "and the receiver's row says so too"
    );

    // The sender gives up.
    cancel.cancel();
    assert!(matches!(
        sending.await.unwrap(),
        Err(LanBeamError::Cancelled)
    ));
    let res = tokio::time::timeout(Duration::from_secs(5), receiver)
        .await
        .expect("the queued receive notices the sender has gone")
        .unwrap();
    assert!(
        matches!(res, Err(LanBeamError::ConnectionLost(_))),
        "{res:?}"
    );
    let errs = errors.lock().unwrap();
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0]["sessionId"], "t-queued");
    assert_eq!(errs[0]["code"], "connection_lost");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// M18: a stranger past its request budget is declined "busy" before the
/// request is looked at — no prompt, no event — while a device the user
/// paired with, from the very same address, is not counted.
#[tokio::test]
async fn strangers_over_the_request_budget_are_declined_busy() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-budget-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let file = tmp.join("a.bin");
    std::fs::write(&file, b"payload").unwrap();

    let app = tauri::test::mock_app();
    let ctx = ctx(app.handle().clone(), &tmp, "trusted");
    let requests = capture(app.handle(), "incoming_file_request");
    // Spend loopback's whole budget.
    let localhost = IpAddr::V4(Ipv4Addr::LOCALHOST);
    for _ in 0..MAX_FILE_REQUESTS_PER_WINDOW {
        assert!(ctx
            .file_rate
            .lock()
            .unwrap()
            .admit(localhost, Instant::now()));
    }

    let stranger = generate_identity().unwrap();
    let (addr, receiver) = spawn_receiver(&ctx).await;
    let res = send(
        &stranger,
        addr,
        &file,
        "a.bin",
        "t-stranger",
        &TransferControl::neutral(),
    )
    .await;
    assert!(matches!(res, Err(LanBeamError::PeerBusy)), "{res:?}");
    receiver
        .await
        .unwrap()
        .expect("a decline is not a failure here");
    assert!(
        requests.lock().unwrap().is_empty(),
        "nothing reached the UI"
    );

    // Paired (and auto-accepted, so nobody has to answer): not counted.
    let friend = generate_identity().unwrap();
    ctx.trusted
        .write()
        .unwrap()
        .set(friend.device_id(), "Friend".into(), true, None, None);
    let (addr, receiver) = spawn_receiver(&ctx).await;
    send(
        &friend,
        addr,
        &file,
        "a.bin",
        "t-friend",
        &TransferControl::neutral(),
    )
    .await
    .expect("a paired device is not throttled");
    receiver.await.unwrap().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 1);

    let _ = std::fs::remove_dir_all(&tmp);
}

/// M18: with as many prompts waiting as anyone will answer, one more request
/// that would prompt is declined "busy" instead of parking yet another.
#[tokio::test]
async fn a_full_prompt_queue_declines_busy() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-crowd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let file = tmp.join("a.bin");
    std::fs::write(&file, b"payload").unwrap();

    let app = tauri::test::mock_app();
    let ctx = ctx(app.handle().clone(), &tmp, "ask");
    let requests = capture(app.handle(), "incoming_file_request");
    let mut parked = Vec::new();
    for i in 0..MAX_PENDING_PROMPTS {
        let (tx, rx) = tokio::sync::oneshot::channel();
        ctx.pending.lock().unwrap().insert(
            format!("waiting-{i}"),
            Parked {
                generation: i as u64,
                reply: tx,
                request: serde_json::Value::Null,
            },
        );
        parked.push(rx);
    }

    let (addr, receiver) = spawn_receiver(&ctx).await;
    let res = send(
        &generate_identity().unwrap(),
        addr,
        &file,
        "a.bin",
        "t-crowd",
        &TransferControl::neutral(),
    )
    .await;
    assert!(matches!(res, Err(LanBeamError::PeerBusy)), "{res:?}");
    receiver.await.unwrap().unwrap();
    assert!(requests.lock().unwrap().is_empty(), "no prompt parked");
    assert_eq!(ctx.pending.lock().unwrap().len(), MAX_PENDING_PROMPTS);

    let _ = std::fs::remove_dir_all(&tmp);
}

/// L12: an inbound session under the id of one of OUR live transfers is refused
/// before anything reaches the UI. The peer we send to reads that id in our
/// manifest; a session of its own under it would land its events — a failed
/// manifest, a prompt — on our send's row.
#[tokio::test]
async fn an_inbound_session_cannot_borrow_one_of_our_transfer_ids() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-borrow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let file = tmp.join("a.bin");
    std::fs::write(&file, b"payload").unwrap();

    let app = tauri::test::mock_app();
    let ctx = ctx(app.handle().clone(), &tmp, "all");
    let requests = capture(app.handle(), "incoming_file_request");
    let errors = capture(app.handle(), "transfer_error");
    // One of our sends is live under this id.
    let (_ours, _ctl) = TransferCtlGuard::register(&ctx.transfers_ctl, "ours");

    let (addr, receiver) = spawn_receiver(&ctx).await;
    // Even with a name the receiver would refuse, no event may name "ours".
    let res = send(
        &generate_identity().unwrap(),
        addr,
        &file,
        "",
        "ours",
        &TransferControl::neutral(),
    )
    .await;
    assert!(matches!(res, Err(LanBeamError::Protocol(_))), "{res:?}");
    assert!(receiver.await.unwrap().is_err());
    assert!(requests.lock().unwrap().is_empty());
    assert!(
        errors.lock().unwrap().is_empty(),
        "nothing lands on our row"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
