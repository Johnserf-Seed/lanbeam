
use super::*;
use crate::consts::{NOISE_PARAMS, TRANSFER_V1, TRANSFER_V2};
use snow::Builder;
use tokio::net::{TcpListener, TcpStream};

fn priv_key() -> [u8; 32] {
    Builder::new(NOISE_PARAMS.parse().unwrap())
        .generate_keypair()
        .unwrap()
        .private
        .as_slice()
        .try_into()
        .unwrap()
}

/// Full CURRENT flow (M4.2): hello/device-info exchange on both ends, then
/// the file dialogue — bytes must arrive identical and both sides must
/// have learned the other's friendly name.
#[tokio::test]
async fn loopback_file_transfer_is_byte_identical() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-xfer-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect(); // spans many chunks
    let src_file = src_dir.join("movie.bin");
    std::fs::write(&src_file, &payload).unwrap();

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dl = dl_dir.clone();

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let (manifest, peer) = match open_inbound(&mut s, "Receiver".into()).await.unwrap() {
            InboundOpen::Files { manifest, peer } => (manifest, peer),
            InboundOpen::Bye => panic!("expected a file session, got bye"),
            InboundOpen::Pair { .. } => panic!("expected a file session, got pair"),
            InboundOpen::Text { .. } => panic!("expected a file session, got text"),
        };
        // Two current builds negotiate the highest common version — v2 now
        // that M7 appended it to SUPPORTED_VERSIONS (stage 7.0).
        assert_eq!(peer.version, TRANSFER_V2);
        assert_eq!(
            peer.name.as_deref(),
            Some("Sender"),
            "sender's DeviceInfo name"
        );
        validate_manifest(&manifest).unwrap();
        run_receive(
            &mut s,
            &manifest,
            &dl,
            &TransferControl::neutral(),
            |_, _| {},
        )
        .await
        .unwrap()
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let peer = initiator_hello(&mut s, "Sender".into()).await.unwrap();
    assert_eq!(peer.version, TRANSFER_V2);
    assert_eq!(
        peer.name.as_deref(),
        Some("Receiver"),
        "receiver's DeviceInfo name"
    );
    let items = build_send_list(std::slice::from_ref(&src_file), false).unwrap();
    run_send(
        &mut s,
        "t-1",
        &items,
        &TransferControl::neutral(),
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await
    .unwrap();

    let saved = receiver.await.unwrap();
    assert_eq!(saved.len(), 1);
    let got = std::fs::read(&saved[0]).unwrap();
    assert_eq!(got, payload, "received bytes must be identical");
    // and it landed inside the download root
    assert!(saved[0]
        .canonicalize()
        .unwrap()
        .starts_with(dl_dir.canonicalize().unwrap()));

    let _ = std::fs::remove_dir_all(&tmp);
}

/// finding-0: a file that is LONGER than its manifest `size` at send time
/// must stream ONLY the declared bytes — its tail must never leak into the
/// next file's frames. Simulate the `build_send_list`→`run_send` TOCTOU by
/// understating file A's `meta.size` after the list is built; the adjacent
/// file B must arrive byte-identical (no cross-file desync).
#[tokio::test]
async fn sender_file_longer_than_manifest_does_not_misslice_next_file() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-longer-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let a_payload: Vec<u8> = (0..100u32).map(|i| (i % 251) as u8).collect();
    let b_payload = b"adjacent file B must be byte-identical".to_vec();
    let a_file = src_dir.join("a.bin");
    let b_file = src_dir.join("b.bin");
    std::fs::write(&a_file, &a_payload).unwrap();
    std::fs::write(&b_file, &b_payload).unwrap();

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dl = dl_dir.clone();

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let manifest = match open_inbound(&mut s, "Receiver".into()).await.unwrap() {
            InboundOpen::Files { manifest, .. } => manifest,
            _ => panic!("expected a file session"),
        };
        validate_manifest(&manifest).unwrap();
        run_receive(
            &mut s,
            &manifest,
            &dl,
            &TransferControl::neutral(),
            |_, _| {},
        )
        .await
        .unwrap()
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let _ = initiator_hello(&mut s, "Sender".into()).await.unwrap();
    let mut items = build_send_list(&[a_file.clone(), b_file.clone()], false).unwrap();
    // TOCTOU: A grew after the manifest was built — declare fewer bytes than
    // it now holds. The stream must still carry exactly the declared count.
    let declared_a = 60u64;
    items[0].meta.size = declared_a;
    run_send(
        &mut s,
        "t-longer",
        &items,
        &TransferControl::neutral(),
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await
    .unwrap();

    let saved = receiver.await.unwrap();
    assert_eq!(saved.len(), 2, "both files delivered");
    // A is truncated to exactly its declared size — the tail never crossed
    // the wire, so it could not bleed into B's frames.
    assert_eq!(
        std::fs::read(&saved[0]).unwrap(),
        a_payload[..declared_a as usize].to_vec(),
        "file A streams exactly its declared size"
    );
    // The adjacent file B is untouched by A's drift.
    assert_eq!(
        std::fs::read(&saved[1]).unwrap(),
        b_payload,
        "adjacent file B is byte-identical, not mis-sliced"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// finding-0: a file that is SHORTER than its manifest `size` at send time
/// must fail LOCALLY on the sender (identified by name) rather than stream a
/// short count that pulls the next file's frames early. Simulate by
/// overstating file A's `meta.size`; `run_send` must error on A, never
/// silently mis-slice B into a "successful" receive.
#[tokio::test]
async fn sender_file_shorter_than_manifest_fails_that_file_locally() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-shorter-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let a_payload = b"short file A".to_vec();
    let b_payload = b"file B never mis-sliced".to_vec();
    let a_file = src_dir.join("a.bin");
    let b_file = src_dir.join("b.bin");
    std::fs::write(&a_file, &a_payload).unwrap();
    std::fs::write(&b_file, &b_payload).unwrap();

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dl = dl_dir.clone();

    // The receiver accepts, then its receive ends in error when the sender
    // aborts — never a silently mis-sliced success.
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let manifest = match open_inbound(&mut s, "Receiver".into()).await.unwrap() {
            InboundOpen::Files { manifest, .. } => manifest,
            _ => panic!("expected a file session"),
        };
        validate_manifest(&manifest).unwrap();
        run_receive(
            &mut s,
            &manifest,
            &dl,
            &TransferControl::neutral(),
            |_, _| {},
        )
        .await
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let _ = initiator_hello(&mut s, "Sender".into()).await.unwrap();
    let mut items = build_send_list(&[a_file.clone(), b_file.clone()], false).unwrap();
    // TOCTOU: A was truncated after the manifest was built — declare MORE
    // bytes than it now holds.
    items[0].meta.size = a_payload.len() as u64 + 40;
    let res = run_send(
        &mut s,
        "t-shorter",
        &items,
        &TransferControl::neutral(),
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await;
    // Sender fails, localized to file A by name — the proof it did NOT
    // stream a short count that would pull B's frames early.
    match res {
        Err(LanBeamError::Io(msg)) => {
            assert!(
                msg.contains("a.bin"),
                "error names the offending file: {msg}"
            );
            assert!(
                msg.contains("shorter"),
                "error explains the shortfall: {msg}"
            );
        }
        other => panic!("expected a localized Io error for a.bin, got {other:?}"),
    }
    // Close the connection so the receiver's pending read resolves promptly
    // instead of idling to its chunk timeout.
    drop(s);

    let recv = receiver.await.unwrap();
    assert!(
        recv.is_err(),
        "receiver must not report a mis-sliced success"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Legacy-receive tolerance (M4.2): a v1 sender opens with the manifest
/// and never says Hello — the receiver must run the old flow unchanged,
/// with no DeviceInfo name and the version pinned to 1.
#[tokio::test]
async fn legacy_v1_sender_without_hello_still_delivers() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-legacy-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let payload = b"old peer, old ways".to_vec();
    let src_file = src_dir.join("legacy.txt");
    std::fs::write(&src_file, &payload).unwrap();

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dl = dl_dir.clone();

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let (manifest, peer) = match open_inbound(&mut s, "Receiver".into()).await.unwrap() {
            InboundOpen::Files { manifest, peer } => (manifest, peer),
            InboundOpen::Bye => panic!("expected a file session, got bye"),
            InboundOpen::Pair { .. } => panic!("expected a file session, got pair"),
            InboundOpen::Text { .. } => panic!("expected a file session, got text"),
        };
        assert_eq!(peer.version, 1, "legacy peers are pinned to v1");
        assert!(
            peer.name.is_none(),
            "legacy peers never introduce themselves"
        );
        validate_manifest(&manifest).unwrap();
        run_receive(
            &mut s,
            &manifest,
            &dl,
            &TransferControl::neutral(),
            |_, _| {},
        )
        .await
        .unwrap()
    });

    // A pre-M4.2 sender: straight to run_send, no hello exchange.
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let items = build_send_list(std::slice::from_ref(&src_file), false).unwrap();
    run_send(
        &mut s,
        "t-legacy",
        &items,
        &TransferControl::neutral(),
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await
    .unwrap();

    let saved = receiver.await.unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(std::fs::read(&saved[0]).unwrap(), payload);

    let _ = std::fs::remove_dir_all(&tmp);
}

/// A v1 RESPONDER errors out on our Hello and drops the link — the
/// initiator must surface that as `PeerTooOld` (UI code `peer_too_old`),
/// not as a generic protocol/io failure.
#[tokio::test]
async fn hello_to_v1_responder_surfaces_peer_too_old() {
    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Mirror the pre-M4.2 receiver: read_manifest as the very first step.
    let old_responder = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let res = read_manifest(&mut s).await;
        assert!(res.is_err(), "a Hello must confuse the old receiver");
        // session drops here — exactly what the old error path did
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let res = initiator_hello(&mut s, "Sender".into()).await;
    assert!(
        matches!(res, Err(LanBeamError::PeerTooOld(_))),
        "expected PeerTooOld, got {res:?}"
    );
    old_responder.await.unwrap();
}

/// A hostile peer's over-long `DeviceInfo` name is clamped to 63 chars at
/// the hello boundary — on BOTH the responder (`open_inbound`) and the
/// initiator (`initiator_hello`) — so no downstream surface (accept prompt,
/// OS notification title, event payload, logs) ever sees an unbounded
/// string pre-trust (M5.4).
#[tokio::test]
async fn over_long_peer_names_are_clamped_at_the_hello_boundary() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-clampname-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let src_file = src_dir.join("f.bin");
    std::fs::write(&src_file, b"hi").unwrap();

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dl = dl_dir.clone();
    let long_recv = "r".repeat(200);

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let (manifest, peer) = match open_inbound(&mut s, long_recv).await.unwrap() {
            InboundOpen::Files { manifest, peer } => (manifest, peer),
            InboundOpen::Bye => panic!("expected a file session, got bye"),
            InboundOpen::Pair { .. } => panic!("expected a file session, got pair"),
            InboundOpen::Text { .. } => panic!("expected a file session, got text"),
        };
        // The sender's 200-char DeviceInfo name reached us clamped to 63.
        assert_eq!(peer.name.as_deref().map(|n| n.chars().count()), Some(63));
        validate_manifest(&manifest).unwrap();
        run_receive(
            &mut s,
            &manifest,
            &dl,
            &TransferControl::neutral(),
            |_, _| {},
        )
        .await
        .unwrap()
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let peer = initiator_hello(&mut s, "s".repeat(200)).await.unwrap();
    // The receiver's 200-char DeviceInfo name reached us clamped to 63.
    assert_eq!(peer.name.as_deref().map(|n| n.chars().count()), Some(63));
    let items = build_send_list(std::slice::from_ref(&src_file), false).unwrap();
    run_send(
        &mut s,
        "t-clamp",
        &items,
        &TransferControl::neutral(),
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await
    .unwrap();

    receiver.await.unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The pairing code comparison (M7.1) accepts an exact match and rejects a
/// wrong code or a length mismatch — the non-short-circuiting loop is a
/// timing property, so the test just pins the boolean contract.
#[test]
fn code_matches_accepts_exact_rejects_otherwise() {
    assert!(code_matches("482913", "482913"));
    assert!(!code_matches("482913", "482914"), "one wrong digit rejects");
    assert!(!code_matches("482913", "48291"), "a short code rejects");
    assert!(!code_matches("482913", "4829130"), "a long code rejects");
    assert!(
        !code_matches("000000", "999999"),
        "an all-different code rejects"
    );
    assert!(
        code_matches("", ""),
        "the degenerate empty-vs-empty is equal"
    );
}

/// The date folder follows the LOCAL calendar (H8): 07:30 on Oct 30 in UTC+8
/// is still Oct 29 in UTC, and the folder must say the 30th.
#[test]
fn date_folder_uses_the_local_date() {
    use chrono::{FixedOffset, TimeZone};
    let utc8 = FixedOffset::east_opt(8 * 3600).unwrap();
    let t = utc8.with_ymd_and_hms(2026, 10, 30, 7, 30, 0).unwrap();
    assert_eq!(ymd_in(t), "2026-10-30");
    assert_eq!(ymd_in(t.with_timezone(&chrono::Utc)), "2026-10-29");
    // And the live path produces the same shape.
    let today = today_ymd();
    assert_eq!(today.len(), 10, "{today}");
    assert_eq!(&today[4..5], "-");
}

/// A code typed the way the host DISPLAYS it ("482 913"), with a dash, or
/// through a CJK input method (full-width digits) reduces to the six ASCII
/// digits the host compares against; anything that is not a digit is gone.
#[test]
fn normalize_pair_code_reads_a_code_the_way_people_type_it() {
    assert_eq!(normalize_pair_code("482 913"), "482913");
    assert_eq!(normalize_pair_code(" 482-913 "), "482913");
    assert_eq!(normalize_pair_code("４８２９１３"), "482913");
    assert_eq!(normalize_pair_code("４８２ 913"), "482913");
    assert_eq!(normalize_pair_code("abc"), "", "no digits, no code");
    assert_eq!(normalize_pair_code(""), "");
    // Other scripts' digits are not codes: only ASCII and full-width count.
    assert_eq!(normalize_pair_code("٤٨٢"), "");
    assert!(code_matches("482913", &normalize_pair_code("482 913")));
}

// ── quick text (M7.3) ─────────────────────────────────────────────────

/// The receiver's clamp (M7.3) trims an over-long text to `MAX_TEXT_BYTES`
/// WITHOUT splitting a multi-byte char, and leaves an in-limit text alone —
/// so the emitted string is always valid UTF-8 no matter what a peer sends.
#[test]
fn clamp_text_truncates_on_char_boundary() {
    // In-limit text is returned verbatim.
    let small = "hello ✅".to_string();
    assert_eq!(clamp_text(small.clone()), small);

    // Build a string that OVERRUNS the cap with a 3-byte char (✅ = 3 bytes)
    // straddling the boundary, so a naive byte-truncate would panic / split it.
    let mut s = "a".repeat(MAX_TEXT_BYTES - 1);
    s.push('✅'); // now MAX_TEXT_BYTES - 1 + 3 = MAX_TEXT_BYTES + 2 bytes
    let clamped = clamp_text(s);
    assert!(clamped.len() <= MAX_TEXT_BYTES, "must not exceed the cap");
    // The trailing multi-byte char was dropped whole (not sliced), leaving
    // only the ASCII prefix — and the result is valid UTF-8 by construction.
    assert_eq!(clamped.len(), MAX_TEXT_BYTES - 1);
    assert!(clamped.bytes().all(|b| b == b'a'));
}

/// Full transfer-layer round-trip (M7.3): the initiator's `send_text` puts a
/// `TextSend` on the wire and blocks for the ack; the responder's
/// `open_inbound` surfaces the payload (text + `to_clipboard` + negotiated v2
/// + sender name) and, once it acks, `send_text` resolves. Proves the ack
/// round-trip and that `to_clipboard` is carried faithfully.
#[tokio::test]
async fn loopback_text_send_acks_and_dispatches_payload() {
    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let (text, to_clipboard, peer) =
            match open_inbound(&mut s, "Receiver".into()).await.unwrap() {
                InboundOpen::Text {
                    text,
                    to_clipboard,
                    peer,
                } => (text, to_clipboard, peer),
                InboundOpen::Files { .. } => panic!("expected text, got files"),
                InboundOpen::Pair { .. } => panic!("expected text, got pair"),
                InboundOpen::Bye => panic!("expected text, got bye"),
            };
        assert_eq!(text, "ping ✅");
        assert_eq!(
            to_clipboard,
            Some(true),
            "the sender's request rides the wire"
        );
        assert_eq!(
            peer.version, TRANSFER_V2,
            "two current builds negotiate v2 quick text"
        );
        assert_eq!(peer.name.as_deref(), Some("Sender"));
        // Stand in for `handle_text_received`'s ack so send_text can resolve.
        send_control(
            &mut s,
            &AppMessage::Ack {
                of: "text".into(),
                delivered: None,
                reason: None,
            },
        )
        .await
        .unwrap();
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let peer = initiator_hello(&mut s, "Sender".into()).await.unwrap();
    assert_eq!(peer.version, TRANSFER_V2);
    send_text(&mut s, "ping ✅".into(), Some(true))
        .await
        .expect("send_text resolves on ack");

    receiver.await.unwrap();
}

/// `send_text` refuses a payload too large to ride one Noise control frame
/// (M7.3) — cleanly, BEFORE writing a byte, so an over-long text fails here
/// instead of deep in `write_msg` (and the peer never sees a partial frame).
#[tokio::test]
async fn send_text_refuses_oversized_payload() {
    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    // The responder only needs to complete the handshake; the guard fires
    // before send_text writes anything, so no control read ever happens.
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let huge = "z".repeat(MAX_TEXT_BYTES * 4);
    let res = send_text(&mut s, huge, Some(false)).await;
    assert!(
        matches!(res, Err(LanBeamError::Protocol(_))),
        "oversized text must be rejected: {res:?}"
    );

    drop(s);
    receiver.await.unwrap();
}

/// The receive-side v2 gate (M7.3): a peer that negotiated v1 but (wrongly)
/// sends a `TextSend` is refused with a clean `Protocol` error — a v1 peer was
/// never offered quick text, so its premature/forged variant must fail this
/// one session, never be acted on.
#[tokio::test]
async fn text_send_from_v1_negotiated_peer_is_refused() {
    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        open_inbound(&mut s, "Receiver".into()).await
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    // Advertise ONLY v1, then push a v2-only TextSend — exactly the mismatch
    // the gate guards against. (We don't read the responder's hello back; its
    // small writes buffer, and we only care that open_inbound rejects.)
    send_control(
        &mut s,
        &AppMessage::Hello {
            versions: vec![TRANSFER_V1],
        },
    )
    .await
    .unwrap();
    send_control(&mut s, &device_info_msg("OldPeer".into()))
        .await
        .unwrap();
    send_control(
        &mut s,
        &AppMessage::TextSend {
            text: "hi".into(),
            to_clipboard: Some(true),
        },
    )
    .await
    .unwrap();

    match receiver.await.unwrap() {
        Err(LanBeamError::Protocol(msg)) => assert!(msg.contains("v1"), "{msg}"),
        Err(e) => panic!("expected a Protocol error, got {e:?}"),
        Ok(_) => panic!("a v1-negotiated TextSend must be refused, not accepted"),
    }
}

/// The send-side v2 gate input (M7.3): dialing a peer that advertises only v1
/// negotiates version 1, so the `send_text` command's `peer.version <
/// TRANSFER_V2` guard refuses to put a `TextSend` on the wire — a v1 (M4–M6)
/// peer never receives a v2 message.
#[tokio::test]
async fn send_text_gate_refuses_v1_negotiated_peer() {
    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    // A responder that advertises ONLY v1 (an M4–M6 peer), replicating the
    // responder half of the hello exchange with a v1-only version list.
    let responder = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        match recv_control(&mut s).await.unwrap() {
            AppMessage::Hello { .. } => {}
            other => panic!("expected hello, got {other:?}"),
        }
        match recv_control(&mut s).await.unwrap() {
            AppMessage::DeviceInfo { .. } => {}
            other => panic!("expected device info, got {other:?}"),
        }
        send_control(
            &mut s,
            &AppMessage::Hello {
                versions: vec![TRANSFER_V1],
            },
        )
        .await
        .unwrap();
        send_control(&mut s, &device_info_msg("OldPeer".into()))
            .await
            .unwrap();
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let peer = initiator_hello(&mut s, "Dialer".into()).await.unwrap();
    assert_eq!(
        peer.version, TRANSFER_V1,
        "an old peer pins the session to v1"
    );
    // This is exactly the guard the `send_text` command applies before it
    // ever calls `transfer::send_text`, so a v1 peer is never handed the variant.
    assert!(
        peer.version < TRANSFER_V2,
        "a v1 peer must fail the send_text gate"
    );

    responder.await.unwrap();
}

// NOTE: the Bye-only end-to-end test (real `handle_incoming`, mock-runtime
// app, event + state assertions) lives in tests/bye_session.rs — see the
// module-visibility comment in lib.rs for why it cannot live here.

/// A hostile `file_count` must abort the manifest read up front — before
/// any entry is read, any Vec is sized from it, or the user is prompted.
#[tokio::test]
async fn oversized_manifest_rejected_before_entries() {
    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        read_manifest(&mut s).await
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    send_control(
        &mut s,
        &AppMessage::FileListBegin {
            transfer_id: "t-dos".into(),
            total_size: u64::MAX,
            file_count: MAX_MANIFEST_FILES + 1,
        },
    )
    .await
    .unwrap();

    match receiver.await.unwrap() {
        Err(LanBeamError::Protocol(msg)) => assert!(msg.contains("limit"), "{msg}"),
        Err(e) => panic!("expected Protocol error, got {e:?}"),
        Ok(_) => panic!("oversized manifest was accepted"),
    }
}

/// A second session reusing an in-flight transfer_id must be refused WITHOUT
/// touching the first session's pending entry — a malicious peer must not
/// be able to force-decline someone else's accept prompt.
#[tokio::test]
async fn duplicate_transfer_id_refused_and_first_prompt_survives() {
    let pending: Mutex<PendingMap> = Mutex::new(PendingMap::new());
    let (tx1, rx1) = oneshot::channel();
    let (tx2, rx2) = oneshot::channel();

    assert!(
        park_pending(&pending, "t-dup", tx1).is_some(),
        "first park must win the slot"
    );
    assert!(
        park_pending(&pending, "t-dup", tx2).is_none(),
        "duplicate must be refused"
    );

    // The FIRST prompt is still answerable through the map…
    let (_, survivor) = pending
        .lock()
        .unwrap()
        .remove("t-dup")
        .expect("first entry survives");
    survivor.send(ReplyDecision::plain(true)).unwrap();
    assert!(
        rx1.await.unwrap().accept,
        "first session receives ITS answer"
    );
    // …and the duplicate's sender was dropped, never parked anywhere.
    assert!(rx2.await.is_err(), "duplicate channel must be dead");
}

/// The reply-races-successor window (M4 finding): after session 1's entry
/// was answered (removed by `reply_file_request`) and a successor re-parked
/// the SAME sender-chosen id, session 1's post-await cleanup must NOT
/// remove the successor's live entry — only the owning generation may.
#[tokio::test]
async fn stale_cleanup_cannot_remove_successors_reparked_prompt() {
    let pending: Mutex<PendingMap> = Mutex::new(PendingMap::new());
    let (tx1, _rx1) = oneshot::channel();
    let gen1 = park_pending(&pending, "t-gen", tx1).expect("first park");

    // reply_file_request answers session 1: unconditional remove-and-send.
    let (_, answered) = pending
        .lock()
        .unwrap()
        .remove("t-gen")
        .expect("entry parked");
    let _ = answered.send(ReplyDecision::plain(true));

    // Before session 1's task wakes, a successor reuses the id.
    let (tx2, rx2) = oneshot::channel();
    let gen2 = park_pending(&pending, "t-gen", tx2).expect("successor parks the vacant slot");
    assert_ne!(gen1, gen2, "generations must be unique");

    // Session 1's cleanup runs with ITS generation: a no-op now.
    remove_pending_if_owner(&pending, "t-gen", gen1);
    // The successor's prompt is intact and still answerable…
    let (_, survivor) = pending
        .lock()
        .unwrap()
        .remove("t-gen")
        .expect("successor's entry must survive the stale cleanup");
    survivor.send(ReplyDecision::plain(true)).unwrap();
    assert!(rx2.await.unwrap().accept, "successor receives ITS answer");

    // …and the owner's cleanup DOES remove when the generation matches.
    let (tx3, _rx3) = oneshot::channel();
    let gen3 = park_pending(&pending, "t-gen", tx3).expect("slot free again");
    remove_pending_if_owner(&pending, "t-gen", gen3);
    assert!(
        !pending.lock().unwrap().contains_key("t-gen"),
        "owner removal works"
    );
}

/// The whole-lifetime in-flight registry: a claimed id refuses duplicates
/// until the guard drops, then the id is claimable again.
#[test]
fn in_flight_guard_claims_refuses_and_releases() {
    let set: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
    let guard = InFlightGuard::claim(&set, "t-live").expect("first claim wins");
    assert!(
        InFlightGuard::claim(&set, "t-live").is_none(),
        "duplicate refused while held"
    );
    assert!(
        InFlightGuard::claim(&set, "t-other").is_some(),
        "unrelated ids unaffected"
    );
    drop(guard);
    assert!(
        InFlightGuard::claim(&set, "t-live").is_some(),
        "released on drop"
    );
}

/// The ack deadline scales with the transfer size at ACK_DRAIN_RATE above
/// the ACK_TIMEOUT base, and is capped at ACK_TIMEOUT_MAX.
#[test]
fn ack_deadline_scales_with_size_and_caps() {
    assert_eq!(ack_deadline(0), ACK_TIMEOUT);
    // 12 GiB at ~20 MB/s ≈ 10 min of drain — base + drain, below the cap.
    let mid = ack_deadline(12 * 1024 * 1024 * 1024);
    assert!(mid > ACK_TIMEOUT && mid < ACK_TIMEOUT_MAX, "got {mid:?}");
    // Anything huge saturates at the cap.
    assert_eq!(ack_deadline(u64::MAX), ACK_TIMEOUT_MAX);
}

/// A single manifest entry whose name exceeds the per-name byte cap must
/// abort the read — the sanitizer would reject it later anyway, but the
/// bytes must never be kept pre-trust (M4.5).
#[tokio::test]
async fn oversized_entry_name_rejected_during_manifest_read() {
    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        read_manifest(&mut s).await
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    send_control(
        &mut s,
        &AppMessage::FileListBegin {
            transfer_id: "t-name".into(),
            total_size: 1,
            file_count: 1,
        },
    )
    .await
    .unwrap();
    send_control(
        &mut s,
        &AppMessage::FileListEntry {
            file: FileMeta {
                name: "a".repeat(MAX_MANIFEST_NAME_BYTES + 1),
                size: 1,
                mtime: 0,
                mode: 0,
                sha256: None,
            },
        },
    )
    .await
    .unwrap();

    match receiver.await.unwrap() {
        Err(LanBeamError::Protocol(msg)) => assert!(msg.contains("limit"), "{msg}"),
        Err(e) => panic!("expected Protocol error, got {e:?}"),
        Ok(_) => panic!("oversized name was accepted"),
    }
}

/// The peer-controlled `transfer_id` is bounded like file names: an over-long
/// id in `FileListBegin` is rejected during the manifest read, before it can
/// be echoed into the reply frame (overflowing it) or used as a registry key /
/// event payload pre-trust.
#[tokio::test]
async fn oversized_transfer_id_rejected_during_manifest_read() {
    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        read_manifest(&mut s).await
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    send_control(
        &mut s,
        &AppMessage::FileListBegin {
            transfer_id: "t".repeat(MAX_MANIFEST_NAME_BYTES + 1),
            total_size: 1,
            file_count: 1,
        },
    )
    .await
    .unwrap();

    match receiver.await.unwrap() {
        Err(LanBeamError::Protocol(msg)) => {
            assert!(
                msg.contains("transfer_id") && msg.contains("limit"),
                "{msg}"
            )
        }
        Err(e) => panic!("expected Protocol error, got {e:?}"),
        Ok(_) => panic!("oversized transfer_id was accepted"),
    }
}

/// L17: the manifest's total is what its files add up to, whatever the
/// sender declared — a "1 KB" request listing gigabytes reads as gigabytes.
#[tokio::test]
async fn manifest_total_is_the_sum_of_its_files() {
    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        read_manifest(&mut s).await
    });
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let entry = |name: &str, size: u64| AppMessage::FileListEntry {
        file: FileMeta {
            name: name.into(),
            size,
            mtime: 0,
            mode: 0,
            sha256: None,
        },
    };
    for msg in [
        AppMessage::FileListBegin {
            transfer_id: "t-total".into(),
            total_size: 1024,
            file_count: 2,
        },
        entry("a.bin", 3 << 30),
        entry("b.bin", 5),
        AppMessage::FileListEnd {
            transfer_id: "t-total".into(),
        },
    ] {
        send_control(&mut s, &msg).await.unwrap();
    }
    let m = receiver.await.unwrap().expect("a well-formed manifest");
    assert_eq!(m.total_size, (3 << 30) + 5);
}

/// Entries that each pass the per-name cap must still trip the CUMULATIVE
/// name-byte budget — the count clamp alone would admit ~40 MB of names.
#[tokio::test]
async fn manifest_name_byte_budget_rejected() {
    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        read_manifest(&mut s).await
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    // Exactly one entry past the budget: N max-length names fill it, the
    // (N+1)-th pushes over. Every name is individually in-limit.
    let n = MAX_MANIFEST_NAME_TOTAL / MAX_MANIFEST_NAME_BYTES + 1;
    send_control(
        &mut s,
        &AppMessage::FileListBegin {
            transfer_id: "t-budget".into(),
            total_size: 1,
            file_count: n as u32,
        },
    )
    .await
    .unwrap();
    let name = "b".repeat(MAX_MANIFEST_NAME_BYTES);
    for _ in 0..n {
        let sent = send_control(
            &mut s,
            &AppMessage::FileListEntry {
                file: FileMeta {
                    name: name.clone(),
                    size: 1,
                    mtime: 0,
                    mode: 0,
                    sha256: None,
                },
            },
        )
        .await;
        if sent.is_err() {
            break; // receiver already refused and closed
        }
    }

    match receiver.await.unwrap() {
        Err(LanBeamError::Protocol(msg)) => assert!(msg.contains("budget"), "{msg}"),
        Err(e) => panic!("expected Protocol error, got {e:?}"),
        Ok(_) => panic!("over-budget manifest was accepted"),
    }
}

/// When the sender dies mid-file, the half-written partial must be deleted
/// too — before M4.5 only fully-completed files were cleaned up.
#[tokio::test]
async fn partial_file_is_deleted_when_sender_dies_mid_file() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-partial-{}", std::process::id()));
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&dl_dir).unwrap();

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dl = dl_dir.clone();

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let manifest = read_manifest(&mut s).await.unwrap();
        run_receive(
            &mut s,
            &manifest,
            &dl,
            &TransferControl::neutral(),
            |_, _| {},
        )
        .await
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let declared = 100_000u64;
    send_control(
        &mut s,
        &AppMessage::FileListBegin {
            transfer_id: "t-partial".into(),
            total_size: declared,
            file_count: 1,
        },
    )
    .await
    .unwrap();
    send_control(
        &mut s,
        &AppMessage::FileListEntry {
            file: FileMeta {
                name: "half.bin".into(),
                size: declared,
                mtime: 0,
                mode: 0,
                sha256: None,
            },
        },
    )
    .await
    .unwrap();
    send_control(
        &mut s,
        &AppMessage::FileListEnd {
            transfer_id: "t-partial".into(),
        },
    )
    .await
    .unwrap();
    match recv_control(&mut s).await.unwrap() {
        AppMessage::FileSendReply { accept: true, .. } => {}
        other => panic!("expected accept, got {other:?}"),
    }
    // Stream less than the declared size, then vanish mid-file.
    s.write_msg(&protocol::encode_file_chunk(0, &vec![7u8; 30_000]).unwrap())
        .await
        .unwrap();
    drop(s);

    let res = receiver.await.unwrap();
    assert!(res.is_err(), "truncated stream must fail the receive");
    let leftovers: Vec<_> = std::fs::read_dir(&dl_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(leftovers.is_empty(), "partial leaked: {leftovers:?}");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The notification body's size text (M5.4): locale-neutral digits + unit
/// symbols, one decimal only while it adds information.
#[test]
fn human_size_formats_across_units() {
    assert_eq!(human_size(0), "0 B");
    assert_eq!(human_size(1023), "1023 B");
    assert_eq!(human_size(1024), "1.0 KB");
    assert_eq!(human_size(1536), "1.5 KB");
    assert_eq!(human_size(10 * 1024 * 1024), "10 MB");
    assert_eq!(human_size(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    assert_eq!(human_size(u64::MAX), "16777216 TB"); // saturates at TB, never panics
}

#[test]
fn traversal_manifest_is_rejected_before_accept() {
    let m = Manifest {
        transfer_id: "t".into(),
        total_size: 1,
        files: vec![
            FileMeta {
                name: "ok.txt".into(),
                size: 1,
                mtime: 0,
                mode: 0,
                sha256: None,
            },
            FileMeta {
                name: "..\\..\\Startup\\evil.exe".into(),
                size: 1,
                mtime: 0,
                mode: 0,
                sha256: None,
            },
        ],
    };
    // "..\\..\\Startup\\evil.exe" validates (—.. dropped—) so it is NOT rejected here;
    // the security guarantee is that it stays INSIDE the root. A genuinely-rejectable
    // name (absolute) must abort the whole manifest:
    let m2 = Manifest {
        transfer_id: "t".into(),
        total_size: 1,
        files: vec![FileMeta {
            name: "C:\\Windows\\x".into(),
            size: 1,
            mtime: 0,
            mode: 0,
            sha256: None,
        }],
    };
    assert!(validate_manifest(&m).is_ok()); // traversal name is normalized, not rejected
    assert!(validate_manifest(&m2).is_err()); // absolute path is rejected
}

/// Cancelling a live transfer ends BOTH sides promptly (M6.1): the cancelled
/// sender returns `Cancelled` (UI code "cancelled"), and the receiver —
/// whose connection the sender then drops — fails through its normal error
/// path, leaving no partial file behind.
#[tokio::test]
async fn cancel_ends_both_sides_promptly() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-cancel-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    // Large enough to span many chunks so the cancel lands mid-stream, and
    // far bigger than any socket buffer so the sender cannot race to
    // completion before the cancel trips.
    let payload = vec![7u8; 4 * 1024 * 1024];
    let src_file = src_dir.join("big.bin");
    std::fs::write(&src_file, &payload).unwrap();

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dl = dl_dir.clone();

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let manifest = match open_inbound(&mut s, "Receiver".into()).await.unwrap() {
            InboundOpen::Files { manifest, .. } => manifest,
            InboundOpen::Bye => panic!("expected a file session"),
            InboundOpen::Pair { .. } => panic!("expected a file session, got pair"),
            InboundOpen::Text { .. } => panic!("expected a file session, got text"),
        };
        validate_manifest(&manifest).unwrap();
        run_receive(
            &mut s,
            &manifest,
            &dl,
            &TransferControl::neutral(),
            |_, _| {},
        )
        .await
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    initiator_hello(&mut s, "Sender".into()).await.unwrap();
    let items = build_send_list(std::slice::from_ref(&src_file), false).unwrap();

    // Trip the cancel token as soon as the first chunk lands, so the NEXT
    // loop iteration's biased select returns Cancelled mid-stream.
    let ctl = TransferControl::neutral();
    let trip = ctl.cancel.clone();
    let send_res = run_send(
        &mut s,
        "t-cancel",
        &items,
        &ctl,
        None,
        &mut |_| {},
        move |sent, _| {
            if sent > 0 {
                trip.cancel();
            }
        },
    )
    .await;
    assert!(
        matches!(send_res, Err(LanBeamError::Cancelled)),
        "sender: {send_res:?}"
    );
    assert_eq!(LanBeamError::Cancelled.ui_code(), "cancelled");

    // Dropping the sender session closes the connection; the receiver's read
    // then fails rather than completing.
    drop(s);
    let recv_res = receiver.await.unwrap();
    assert!(recv_res.is_err(), "receiver must abort, got {recv_res:?}");
    // The half-written partial was cleaned up by the receive error path.
    let leftovers: Vec<_> = std::fs::read_dir(&dl_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(leftovers.is_empty(), "partial leaked: {leftovers:?}");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// A paused sender parks without moving any bytes; resuming wakes it and the
/// transfer completes byte-identical (M6.2).
#[tokio::test]
async fn pause_then_resume_completes() {
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;

    let tmp = std::env::temp_dir().join(format!("lanbeam-pause-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let src_file = src_dir.join("paused.bin");
    std::fs::write(&src_file, &payload).unwrap();

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dl = dl_dir.clone();

    // Count what the receiver has taken, to prove the pause actually held.
    let received = Arc::new(AtomicU64::new(0));
    let rc = received.clone();
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let manifest = match open_inbound(&mut s, "Receiver".into()).await.unwrap() {
            InboundOpen::Files { manifest, .. } => manifest,
            InboundOpen::Bye => panic!("expected a file session"),
            InboundOpen::Pair { .. } => panic!("expected a file session, got pair"),
            InboundOpen::Text { .. } => panic!("expected a file session, got text"),
        };
        validate_manifest(&manifest).unwrap();
        run_receive(
            &mut s,
            &manifest,
            &dl,
            &TransferControl::neutral(),
            move |recv, _| {
                rc.store(recv, Ordering::SeqCst);
            },
        )
        .await
    });

    // Pause the SENDER before it streams any chunk.
    let ctl = TransferControl::neutral();
    ctl.paused.store(true, Ordering::SeqCst);
    let resume_flag = ctl.paused.clone();
    let resume_notify = ctl.resume_notify.clone();

    let sender = tokio::spawn(async move {
        let stream = TcpStream::connect(addr).await.unwrap();
        let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
            .await
            .unwrap();
        initiator_hello(&mut s, "Sender".into()).await.unwrap();
        let items = build_send_list(std::slice::from_ref(&src_file), false).unwrap();
        run_send(
            &mut s,
            "t-pause",
            &items,
            &ctl,
            None,
            &mut |_| {},
            |_, _| {},
        )
        .await
    });

    // Ample time for a paused sender to have streamed something if pause
    // were a no-op — it must not.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        received.load(Ordering::SeqCst),
        0,
        "paused sender must send nothing"
    );

    // Resume: clear the flag and wake the parked loop.
    resume_flag.store(false, Ordering::SeqCst);
    resume_notify.notify_waiters();

    sender.await.unwrap().expect("send completes after resume");
    let saved = receiver
        .await
        .unwrap()
        .expect("receive completes after resume");
    assert_eq!(saved.len(), 1);
    assert_eq!(
        std::fs::read(&saved[0]).unwrap(),
        payload,
        "bytes identical after pause/resume"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The pause park's cap MUST stay comfortably under the peer's per-chunk
/// idle deadline (finding-4): the WHOLE point of the bound is that an
/// auto-resume lands early enough that the next chunk moves before the peer
/// (blocked in `recv_frame` / `write_msg`) times out. If this margin ever
/// eroded, a maxed-out pause would resume too late and the peer would still
/// abort — the silent-failure this fix removes.
#[test]
fn pause_park_cap_stays_under_idle_timeout() {
    assert!(
        MAX_PAUSE_PARK < CHUNK_IDLE_TIMEOUT,
        "the park cap must be under the idle timeout"
    );
    assert!(
        CHUNK_IDLE_TIMEOUT - MAX_PAUSE_PARK >= Duration::from_secs(5),
        "keep a margin for scheduling jitter + TCP send-buffer drain on resume"
    );
}

/// A pause left alone past the cap AUTO-RESUMES instead of parking forever
/// (finding-4): the helper returns `Ok(true)` (so the loop emits
/// `transfer_resumed`) and clears the flag exactly as `resume_transfer`
/// would. This is what makes a >60s pause SURVIVE rather than let the peer's
/// 60s idle deadline abort it. Uses a tiny cap so the real-time wait is
/// milliseconds, not the 50s production value.
#[tokio::test]
async fn pause_park_auto_resumes_at_cap() {
    let ctl = TransferControl::neutral();
    ctl.paused.store(true, Ordering::SeqCst);
    let cap = Duration::from_millis(40);
    let t0 = Instant::now();
    let auto = wait_while_paused_capped(&ctl, cap).await.unwrap();
    assert!(auto, "the cap must report an auto-resume so the UI is told");
    assert!(
        !ctl.paused.load(Ordering::SeqCst),
        "auto-resume must clear the flag"
    );
    assert!(t0.elapsed() >= cap, "must not resume before the cap");
}

/// A genuine `resume_transfer` before the cap wins: the helper wakes
/// promptly and reports `Ok(false)` (a NORMAL resume, not the cap), so the
/// UI is not told of a phantom auto-resume (finding-4). The large cap proves
/// the notify — not the timer — ended the park.
#[tokio::test]
async fn pause_park_genuine_resume_returns_false() {
    let ctl = TransferControl::neutral();
    ctl.paused.store(true, Ordering::SeqCst);
    let waker = ctl.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        waker.paused.store(false, Ordering::SeqCst);
        waker.resume_notify.notify_waiters();
    });
    let t0 = Instant::now();
    let auto = wait_while_paused_capped(&ctl, Duration::from_secs(30))
        .await
        .unwrap();
    assert!(!auto, "a real resume is not an auto-resume");
    assert!(!ctl.paused.load(Ordering::SeqCst));
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "the notify, not the cap, ended the park"
    );
}

/// Cancel still wins over a parked pause even with the cap in place
/// (finding-4 must not regress M6.1): a cancel issued while paused ends the
/// park immediately with `Cancelled`, never waiting out the cap.
#[tokio::test]
async fn pause_park_cancel_wins_over_cap() {
    let ctl = TransferControl::neutral();
    ctl.paused.store(true, Ordering::SeqCst);
    ctl.cancel.cancel();
    let t0 = Instant::now();
    let res = wait_while_paused_capped(&ctl, Duration::from_secs(30)).await;
    assert!(
        matches!(res, Err(LanBeamError::Cancelled)),
        "cancel must win: {res:?}"
    );
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "cancel must not wait out the cap"
    );
}

/// With verify_hash on the sender attaches a SHA-256 to each manifest entry
/// and the receiver verifies it before delivering — a byte-identical
/// transfer must pass the check and land the file (M6.3).
#[tokio::test]
async fn hashed_transfer_verifies_and_delivers() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-hashok-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let payload: Vec<u8> = (0..250_000u32).map(|i| (i % 251) as u8).collect(); // many chunks
    let src_file = src_dir.join("verified.bin");
    std::fs::write(&src_file, &payload).unwrap();

    // verify_hash ON: the digest is computed in the same build pass.
    let items = build_send_list(std::slice::from_ref(&src_file), true).unwrap();
    assert!(
        items[0]
            .meta
            .sha256
            .as_deref()
            .is_some_and(|h| h.len() == 64),
        "verify_hash on must attach a lowercase-hex sha256"
    );

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dl = dl_dir.clone();

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let manifest = read_manifest(&mut s).await.unwrap();
        assert!(
            manifest.files[0].sha256.is_some(),
            "receiver must see the hash"
        );
        run_receive(
            &mut s,
            &manifest,
            &dl,
            &TransferControl::neutral(),
            |_, _| {},
        )
        .await
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    run_send(
        &mut s,
        "t-hashok",
        &items,
        &TransferControl::neutral(),
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await
    .unwrap();

    let saved = receiver.await.unwrap().expect("verified receive completes");
    assert_eq!(saved.len(), 1);
    assert_eq!(
        std::fs::read(&saved[0]).unwrap(),
        payload,
        "delivered bytes identical"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// A file whose bytes don't match the sender-declared SHA-256 is rejected
/// and its partial deleted, surfacing as an `Integrity` error (UI code
/// "integrity") — the in-flight corruption case (M6.3).
#[tokio::test]
async fn corrupted_bytes_fail_sha256_and_clean_up() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-hashbad-{}", std::process::id()));
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&dl_dir).unwrap();

    // Declare the digest of the ORIGINAL content but stream tampered bytes
    // of the same length — exactly what a flipped bit on the wire looks like.
    let good = vec![7u8; 6000];
    let mut hasher = Sha256::new();
    hasher.update(&good);
    let declared = to_hex(&hasher.finalize());
    let tampered = vec![8u8; good.len()];

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dl = dl_dir.clone();

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let manifest = read_manifest(&mut s).await.unwrap();
        run_receive(
            &mut s,
            &manifest,
            &dl,
            &TransferControl::neutral(),
            |_, _| {},
        )
        .await
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let size = good.len() as u64;
    send_control(
        &mut s,
        &AppMessage::FileListBegin {
            transfer_id: "t-bad".into(),
            total_size: size,
            file_count: 1,
        },
    )
    .await
    .unwrap();
    send_control(
        &mut s,
        &AppMessage::FileListEntry {
            file: FileMeta {
                name: "corrupt.bin".into(),
                size,
                mtime: 0,
                mode: 0,
                sha256: Some(declared),
            },
        },
    )
    .await
    .unwrap();
    send_control(
        &mut s,
        &AppMessage::FileListEnd {
            transfer_id: "t-bad".into(),
        },
    )
    .await
    .unwrap();
    match recv_control(&mut s).await.unwrap() {
        AppMessage::FileSendReply { accept: true, .. } => {}
        other => panic!("expected accept, got {other:?}"),
    }
    s.write_msg(&protocol::encode_file_chunk(0, &tampered).unwrap())
        .await
        .unwrap();

    let res = receiver.await.unwrap();
    assert!(
        matches!(res, Err(LanBeamError::Integrity(_))),
        "hash mismatch must fail with Integrity, got {res:?}"
    );
    assert_eq!(
        LanBeamError::Integrity(String::new()).ui_code(),
        "integrity"
    );
    // The corrupted partial was removed by the receive error path.
    let leftovers: Vec<_> = std::fs::read_dir(&dl_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(
        leftovers.is_empty(),
        "corrupted partial leaked: {leftovers:?}"
    );
    // …and the SENDER learns what happened (H5): the ack carries the
    // receiver's code, which maps back to "their copy failed its checksum —
    // send it again", not "the connection failed".
    match recv_control(&mut s).await.unwrap() {
        AppMessage::TransferAck {
            ok: false, error, ..
        } => {
            let e = peer_ack_error(error.as_deref());
            assert!(matches!(e, LanBeamError::PeerIntegrity), "got {e:?}");
            assert_eq!(e.ui_code(), "peer_integrity");
        }
        other => panic!("expected a failed ack, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

/// How a scripted receiver answers a real `run_send`.
enum Script {
    /// `FileSendReply { accept: false, reason }`.
    Refuse(Option<&'static str>),
    /// Accept, take every byte, then `TransferAck { ok: false, error }`.
    AcceptThenFail(&'static str),
}

/// Drive the REAL `run_send` against a hand-scripted receiver; return what
/// the sender ends with and the session-level events it reported, in order.
/// Scripted, because a real receiver only says "busy" after its concurrency
/// gate has waited out most of the sender's 150 s reply window.
async fn send_against(script: Script, tag: &str) -> (Result<()>, Vec<&'static str>) {
    let tmp = std::env::temp_dir().join(format!("lanbeam-script-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let file = tmp.join("note.txt");
    std::fs::write(&file, b"hello there").unwrap();

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let manifest = match open_inbound(&mut s, "Receiver".into()).await.unwrap() {
            InboundOpen::Files { manifest, .. } => manifest,
            _ => panic!("expected a file session"),
        };
        match script {
            Script::Refuse(reason) => {
                send_reply(
                    &mut s,
                    &manifest.transfer_id,
                    false,
                    reason.map(String::from),
                )
                .await
                .unwrap();
            }
            Script::AcceptThenFail(token) => {
                send_reply(&mut s, &manifest.transfer_id, true, None)
                    .await
                    .unwrap();
                let total: u64 = manifest.files.iter().map(|f| f.size).sum();
                let mut got = 0u64;
                while got < total {
                    if let Frame::FileChunk(_, data) = recv_frame(&mut s).await.unwrap() {
                        got += data.len() as u64;
                    }
                }
                send_control(
                    &mut s,
                    &AppMessage::TransferAck {
                        transfer_id: manifest.transfer_id.clone(),
                        ok: false,
                        error: Some(token.into()),
                    },
                )
                .await
                .unwrap();
            }
        }
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let _ = initiator_hello(&mut s, "Sender".into()).await.unwrap();
    let items = build_send_list(&[file], false).unwrap();
    let mut events: Vec<&'static str> = Vec::new();
    let res = run_send(
        &mut s,
        "t-script",
        &items,
        &TransferControl::neutral(),
        None,
        &mut |e| {
            events.push(match e {
                FileEvent::Accepted => "accepted",
                FileEvent::Streaming { .. } => "streaming",
                FileEvent::Done { .. } => "done",
                FileEvent::PauseAutoResumed => "resumed",
            })
        },
        |_, _| {},
    )
    .await;
    receiver.await.unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
    (res, events)
}

/// A receiver's "no" maps to what it meant (H6). Only "declined" — or a
/// legacy receiver's reasonless no — is a person saying no; a full
/// receiver used to read as one too.
#[tokio::test]
async fn sender_learns_why_a_receiver_refused() {
    let (res, events) = send_against(Script::Refuse(Some(REPLY_BUSY)), "busy").await;
    assert!(matches!(res, Err(LanBeamError::PeerBusy)), "{res:?}");
    assert!(events.is_empty(), "nothing was accepted: {events:?}");
    let (res, _) = send_against(Script::Refuse(Some(REPLY_UNSAFE_NAME)), "unsafe").await;
    assert!(matches!(res, Err(LanBeamError::UnsafePath(_))), "{res:?}");
    let (res, _) = send_against(Script::Refuse(Some(REPLY_DECLINED)), "declined").await;
    assert!(matches!(res, Err(LanBeamError::Rejected)), "{res:?}");
    let (res, _) = send_against(Script::Refuse(None), "legacy").await;
    assert!(matches!(res, Err(LanBeamError::Rejected)), "{res:?}");
    // L15: accepted, then cancelled while it queued for a slot.
    let (res, _) = send_against(Script::Refuse(Some(REPLY_CANCELLED)), "queued-cancel").await;
    assert!(matches!(res, Err(LanBeamError::PeerCancelled)), "{res:?}");
}

/// The sender reports "accepted" when the receiver accepted — after the
/// reply, before the first byte, exactly once (H10) — and a failed ack says
/// what failed on the receiver's side (H5).
#[tokio::test]
async fn sender_reports_acceptance_then_the_receivers_failure() {
    let (res, events) = send_against(Script::AcceptThenFail("io"), "ack-io").await;
    assert!(matches!(res, Err(LanBeamError::PeerIo)), "{res:?}");
    assert_eq!(events.first(), Some(&"accepted"), "{events:?}");
    assert_eq!(
        events.iter().filter(|e| **e == "accepted").count(),
        1,
        "{events:?}"
    );
    assert!(events.contains(&"streaming"), "{events:?}");
    let (res, _) = send_against(Script::AcceptThenFail("cancelled"), "ack-cancel").await;
    assert!(matches!(res, Err(LanBeamError::PeerCancelled)), "{res:?}");
}

/// Every code a receiver can put in its ack maps back to the sender-side
/// kind that says what happened over there.
#[test]
fn peer_ack_codes_map_back_to_what_happened() {
    assert!(matches!(
        peer_ack_error(Some("integrity")),
        LanBeamError::PeerIntegrity
    ));
    assert!(matches!(peer_ack_error(Some("io")), LanBeamError::PeerIo));
    assert!(matches!(
        peer_ack_error(Some("cancelled")),
        LanBeamError::PeerCancelled
    ));
    assert!(matches!(
        peer_ack_error(Some("timeout")),
        LanBeamError::Timeout
    ));
    assert!(matches!(
        peer_ack_error(Some("unsafe_path")),
        LanBeamError::UnsafePath(_)
    ));
    assert!(matches!(
        peer_ack_error(Some("protocol")),
        LanBeamError::Protocol(_)
    ));
    assert!(matches!(peer_ack_error(None), LanBeamError::Protocol(_)));
    // A hostile receiver's novel can't reach a log unclamped.
    match peer_ack_error(Some(&"x".repeat(100_000))) {
        LanBeamError::Protocol(m) => assert!(m.len() < 1_000, "{} bytes", m.len()),
        other => panic!("{other:?}"),
    }
}

/// The receiver's rules, applied to a selection before anything is hashed
/// or dialed (H7): an empty folder, a name the receiver can't save, too many
/// files and too many name bytes each fail with their own kind.
#[test]
fn send_selection_is_checked_against_the_receivers_rules() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-selcheck-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("empty")).unwrap();
    std::fs::create_dir_all(tmp.join("three")).unwrap();
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(tmp.join("three").join(n), b"x").unwrap();
    }

    check_send_selection(&[tmp.join("three")]).unwrap();
    assert!(matches!(
        check_send_selection(&[tmp.join("empty")]),
        Err(LanBeamError::NoFiles)
    ));
    // A 241-byte segment: one over what the receiver accepts.
    let long = tmp.join("a".repeat(241));
    std::fs::write(&long, b"x").unwrap();
    assert!(matches!(
        check_send_selection(&[long]),
        Err(LanBeamError::UnsafePath(_))
    ));
    // The budgets, at a size a test can afford.
    assert!(matches!(
        check_selection_within(&[tmp.join("three")], 2, usize::MAX),
        Err(LanBeamError::TooManyFiles(_))
    ));
    check_selection_within(&[tmp.join("three")], 3, usize::MAX).unwrap();
    assert!(matches!(
        check_selection_within(&[tmp.join("three")], 3, 20),
        Err(LanBeamError::TooManyFiles(_))
    ));
    let _ = std::fs::remove_dir_all(&tmp);
}

/// verify_hash OFF omits the digest entirely (no downgrade protection is
/// claimed), while ON attaches a 64-char lowercase-hex one (M6.3).
#[test]
fn verify_hash_off_omits_sha256() {
    let dir = std::env::temp_dir().join(format!("lanbeam-hashopt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("a.bin");
    std::fs::write(&f, b"content to hash").unwrap();

    let off = build_send_list(std::slice::from_ref(&f), false).unwrap();
    assert_eq!(
        off[0].meta.sha256, None,
        "verify_hash off must send no hash"
    );

    let on = build_send_list(std::slice::from_ref(&f), true).unwrap();
    assert!(
        on[0].meta.sha256.as_deref().is_some_and(|h| {
            h.len() == 64
                && h.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        }),
        "verify_hash on must send a lowercase-hex sha256"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ── EXIF/metadata stripping on send (M9.1) ────────────────────────────

/// A minimal but parseable JPEG carrying an EXIF APP1 segment, a COM
/// "content" segment, and an SOS scan (with the EOI in its entropy tail so
/// img-parts re-encodes it faithfully) — the same fixture shape `exif.rs`
/// uses, inlined here because that module's helper is private to its test mod.
fn jpeg_with_exif() -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8]; // SOI
    let exif_contents: Vec<u8> = [b"Exif\x00\x00".as_slice(), &[0xAB; 24]].concat();
    out.push(0xFF);
    out.push(0xE1); // APP1
    out.extend_from_slice(&((exif_contents.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&exif_contents);
    let com = b"PIXELS".as_slice();
    out.push(0xFF);
    out.push(0xFE); // COM
    out.extend_from_slice(&((com.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(com);
    let sos_params = [0x01u8, 0x01, 0x00, 0x00, 0x3F, 0x00];
    out.push(0xFF);
    out.push(0xDA); // SOS
    out.extend_from_slice(&((sos_params.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&sos_params);
    out.extend_from_slice(&[0xAA, 0xBB, 0xCC]); // scan bytes
    out.extend_from_slice(&[0xFF, 0xD9]); // EOI (carried with the scan)
    out
}

/// Strip ON: an image is rewritten to a CLEANED temp copy under the scratch
/// dir, with the manifest `size` and `sha256` computed over the stripped
/// bytes (the consistency contract) — and dropping the returned guard deletes
/// every temp, so a cleaned copy of the user's photo never lingers.
#[test]
fn strip_on_rewrites_item_to_cleaned_temp_and_guard_cleans_up() {
    let dir = std::env::temp_dir().join(format!("lanbeam-strip-on-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let photo = dir.join("holiday.jpg");
    let original = jpeg_with_exif();
    std::fs::write(&photo, &original).unwrap();
    let scratch_base = dir.join("scratch");
    // What the strip must produce, computed independently of the builder.
    let expected = exif::strip_image_metadata(&original, "jpg").unwrap();
    assert_ne!(expected, original, "fixture must actually change on strip");

    let (items, scratch) = build_send_list_scoped(
        std::slice::from_ref(&photo),
        true, // verify_hash
        true, // strip_exif
        scratch_base.clone(),
    )
    .unwrap();

    assert_eq!(items.len(), 1);
    let it = &items[0];
    assert_eq!(it.meta.name, "holiday.jpg", "manifest keeps the real name");
    assert_ne!(
        it.abs, photo,
        "the item must point at the temp, not the source"
    );
    assert!(
        it.abs.starts_with(&scratch_base),
        "temp must live under the scratch dir, got {:?}",
        it.abs
    );
    // Size + hash describe the STRIPPED bytes that will actually stream.
    let streamed = std::fs::read(&it.abs).unwrap();
    assert_eq!(streamed, expected, "temp holds the cleaned bytes");
    assert_eq!(
        it.meta.size,
        expected.len() as u64,
        "size is over stripped bytes"
    );
    assert_eq!(
        it.meta.sha256.as_deref(),
        Some(to_hex(&Sha256::digest(&expected)).as_str()),
        "sha256 is over stripped bytes"
    );
    // The streamed bytes carry no EXIF, and the original is untouched on disk.
    assert!(
        !streamed.windows(6).any(|w| w == b"Exif\x00\x00"),
        "streamed bytes must be EXIF-free"
    );
    assert_eq!(
        std::fs::read(&photo).unwrap(),
        original,
        "source is never modified"
    );

    // Dropping the guard removes the whole scratch subtree.
    let scratch_dir = scratch.dir().map(|p| p.to_path_buf());
    assert!(scratch_dir.as_deref().is_some_and(|p| p.exists()));
    drop(scratch);
    assert!(
        scratch_dir.is_some_and(|p| !p.exists()),
        "temp copies must be gone after the guard drops"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Strip OFF: the item is byte-identical to the plain builder — original
/// path, original size, hash over the ORIGINAL bytes — and no scratch dir is
/// ever created.
#[test]
fn strip_off_keeps_original_untouched() {
    let dir = std::env::temp_dir().join(format!("lanbeam-strip-off-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let photo = dir.join("holiday.jpg");
    let original = jpeg_with_exif();
    std::fs::write(&photo, &original).unwrap();
    let scratch_base = dir.join("scratch");

    let (items, scratch) = build_send_list_scoped(
        std::slice::from_ref(&photo),
        true,  // verify_hash
        false, // strip_exif OFF
        scratch_base.clone(),
    )
    .unwrap();

    let it = &items[0];
    assert_eq!(it.abs, photo, "strip off must send the original path");
    assert_eq!(it.meta.size, original.len() as u64);
    assert_eq!(
        it.meta.sha256.as_deref(),
        Some(to_hex(&Sha256::digest(&original)).as_str()),
        "hash is over the original bytes"
    );
    assert!(scratch.dir().is_none(), "no strip → no scratch dir");
    assert!(!scratch_base.exists(), "no scratch dir created on disk");

    let _ = std::fs::remove_dir_all(&dir);
}

/// An unsupported format (HEIC) passes through UNCHANGED even with strip on —
/// we never claim to scrub what img-parts cannot parse, and never read it.
#[test]
fn strip_passes_unsupported_heic_through_unchanged() {
    let dir = std::env::temp_dir().join(format!("lanbeam-strip-heic-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let photo = dir.join("clip.heic");
    // Arbitrary non-JPEG bytes; a real HEIC would likewise be unparsed here.
    let original = vec![0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9];
    std::fs::write(&photo, &original).unwrap();
    let scratch_base = dir.join("scratch");

    let (items, scratch) = build_send_list_scoped(
        std::slice::from_ref(&photo),
        true, // verify_hash
        true, // strip_exif ON, but HEIC is unsupported
        scratch_base.clone(),
    )
    .unwrap();

    let it = &items[0];
    assert_eq!(it.abs, photo, "unsupported format sends the original path");
    assert_eq!(it.meta.size, original.len() as u64);
    assert_eq!(
        it.meta.sha256.as_deref(),
        Some(to_hex(&Sha256::digest(&original)).as_str()),
        "hash is over the untouched original"
    );
    assert!(scratch.dir().is_none(), "unsupported format writes no temp");
    assert!(!scratch_base.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

/// Strip ON but the image carries NO metadata (finding-3): it streams
/// UNCHANGED from its original path — no temp copy — and its `sha256` is over
/// the original bytes. The Unchanged path hashes the buffer `try_strip`
/// already read instead of re-reading the whole file for the hash.
#[test]
fn strip_on_metadata_free_image_streams_original_without_temp() {
    let dir = std::env::temp_dir().join(format!("lanbeam-strip-clean-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let photo = dir.join("clean.jpg");
    // A structurally valid JPEG with no EXIF/XMP: SOI · COM · SOS(scan·EOI).
    let mut original = vec![0xFF, 0xD8]; // SOI
    let com = b"PIXELS".as_slice();
    original.push(0xFF);
    original.push(0xFE); // COM
    original.extend_from_slice(&((com.len() + 2) as u16).to_be_bytes());
    original.extend_from_slice(com);
    let sos_params = [0x01u8, 0x01, 0x00, 0x00, 0x3F, 0x00];
    original.push(0xFF);
    original.push(0xDA); // SOS
    original.extend_from_slice(&((sos_params.len() + 2) as u16).to_be_bytes());
    original.extend_from_slice(&sos_params);
    original.extend_from_slice(&[0xAA, 0xBB, 0xCC]); // entropy scan
    original.extend_from_slice(&[0xFF, 0xD9]); // EOI
    std::fs::write(&photo, &original).unwrap();
    // Precondition: the strip leaves this file byte-identical (nothing to remove).
    assert_eq!(
        exif::strip_image_metadata(&original, "jpg").as_deref(),
        Some(original.as_slice()),
        "a metadata-free JPEG must re-encode unchanged"
    );
    let scratch_base = dir.join("scratch");

    let (items, scratch) = build_send_list_scoped(
        std::slice::from_ref(&photo),
        true, // verify_hash
        true, // strip_exif ON
        scratch_base.clone(),
    )
    .unwrap();

    let it = &items[0];
    assert_eq!(
        it.abs, photo,
        "a metadata-free image streams from its original path, not a temp"
    );
    assert_eq!(it.meta.size, original.len() as u64);
    assert_eq!(
        it.meta.sha256.as_deref(),
        Some(to_hex(&Sha256::digest(&original)).as_str()),
        "hash is over the original bytes"
    );
    assert!(scratch.dir().is_none(), "no change → no temp copy written");
    assert!(!scratch_base.exists(), "no scratch dir created");

    let _ = std::fs::remove_dir_all(&dir);
}

// ── resume + conflict policy (M6.4/6.5) ───────────────────────────────

/// Drive a full send → planned-receive over loopback and return the
/// receiver's result. The sender streams `items` (its `run_send` honors the
/// resume offsets the receiver's reply carries); the receiver reads the
/// manifest and runs `run_receive_core` with `plan` (same order + length as
/// the files). Mirrors the manifest-only harness the hash tests use.
async fn loopback_planned(
    items: Vec<SendItem>,
    transfer_id: &'static str,
    dl: PathBuf,
    plan: Vec<FileDisposition>,
) -> Result<Vec<PathBuf>> {
    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let manifest = read_manifest(&mut s).await.unwrap();
        let mut progress = Vec::new();
        let mut opened = Vec::new();
        run_receive_core(
            &mut s,
            &manifest,
            &dl,
            &TransferControl::neutral(),
            &plan,
            &mut progress,
            &mut opened,
            None,
            &mut |_| {},
            |_, _| {},
        )
        .await
    });
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    run_send(
        &mut s,
        transfer_id,
        &items,
        &TransferControl::neutral(),
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await
    .expect("send completes");
    drop(s);
    receiver.await.unwrap()
}

/// A seeded partial (first `offset` bytes already on disk) resumes to a
/// byte-identical file whose whole-file SHA-256 matches the manifest's — the
/// sender streams only the tail, the receiver folds the prefix back into the
/// hasher before appending (M6.4).
#[tokio::test]
async fn resume_completes_partial_byte_identical() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-resume-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let payload: Vec<u8> = (0..250_000u32).map(|i| (i % 251) as u8).collect();
    let offset = 100_000u64; // tail spans many chunks
    let src_file = src_dir.join("resumed.bin");
    std::fs::write(&src_file, &payload).unwrap();
    // Seed the partial: the first `offset` bytes already on disk.
    std::fs::write(dl_dir.join("resumed.bin"), &payload[..offset as usize]).unwrap();

    // verify ON so the WHOLE-file hash rides the manifest (resume needs it).
    let items = build_send_list(std::slice::from_ref(&src_file), true).unwrap();
    let expected_hash = items[0].meta.sha256.clone();

    let plan = vec![FileDisposition::Resume {
        disk_rel: PathBuf::from("resumed.bin"),
        offset,
        commit_name: None,
    }];
    let saved = loopback_planned(items, "t-resume", dl_dir.clone(), plan)
        .await
        .expect("resume completes");
    assert_eq!(saved.len(), 1);
    let got = std::fs::read(&saved[0]).unwrap();
    assert_eq!(got, payload, "resumed file must be byte-identical");
    let mut h = Sha256::new();
    h.update(&got);
    assert_eq!(
        Some(to_hex(&h.finalize())),
        expected_hash,
        "final hash matches the declared one"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The overwrite policy replaces the existing file at the EXACT path — no
/// de-dupe sibling, the stale bytes gone (M6.5).
#[tokio::test]
async fn overwrite_conflict_replaces_existing() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-owrecv-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let payload = b"the fresh incoming content".to_vec();
    let src_file = src_dir.join("clash.bin");
    std::fs::write(&src_file, &payload).unwrap();
    std::fs::write(
        dl_dir.join("clash.bin"),
        b"stale older bytes, longer than the new file",
    )
    .unwrap();

    let items = build_send_list(std::slice::from_ref(&src_file), false).unwrap();
    let plan = vec![FileDisposition::Fresh {
        rel: PathBuf::from("clash.bin"),
        conflict: ConflictAction::Overwrite,
    }];
    let saved = loopback_planned(items, "t-ow", dl_dir.clone(), plan)
        .await
        .expect("overwrite completes");
    assert_eq!(saved.len(), 1);
    assert_eq!(
        saved[0],
        dl_dir.join("clash.bin"),
        "overwrite reuses the exact path"
    );
    assert_eq!(
        std::fs::read(&saved[0]).unwrap(),
        payload,
        "existing file replaced"
    );
    assert!(
        !dl_dir.join("clash (1).bin").exists(),
        "overwrite must not de-dupe"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// An overwrite that FAILS mid-receive (here a hash mismatch, standing in for
/// any interruption) must leave the user's existing file intact: the fix
/// streams into a de-duped temp and replaces the target only on full success,
/// so the failure path deletes just the temp — closing the data-loss hole
/// where an aborted overwrite destroyed both the original and the partial.
#[tokio::test]
async fn overwrite_failure_preserves_original() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-owfail-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let original = b"PRECIOUS original bytes that must survive an aborted overwrite".to_vec();
    std::fs::write(dl_dir.join("clash.bin"), &original).unwrap();
    let src_file = src_dir.join("clash.bin");
    std::fs::write(&src_file, b"incoming replacement").unwrap();

    // Hashed send, but corrupt the manifest's expected hash so the receiver's
    // verify fails AFTER it has streamed the tail into its temp — driving the
    // failure/cleanup path for an overwrite file.
    let mut items = build_send_list(std::slice::from_ref(&src_file), true).unwrap();
    items[0].meta.sha256 = Some("00".repeat(32)); // 64 hex zeros — never matches
    let plan = vec![FileDisposition::Fresh {
        rel: PathBuf::from("clash.bin"),
        conflict: ConflictAction::Overwrite,
    }];
    // Inline loopback that TOLERATES the send erroring: the receiver rejects
    // with ok:false on the hash mismatch, so run_send returns Err — expected
    // here, unlike loopback_planned which `.expect`s a clean send.
    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dl = dl_dir.clone();
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let manifest = read_manifest(&mut s).await.unwrap();
        let mut progress = Vec::new();
        let mut opened = Vec::new();
        run_receive_core(
            &mut s,
            &manifest,
            &dl,
            &TransferControl::neutral(),
            &plan,
            &mut progress,
            &mut opened,
            None,
            &mut |_| {},
            |_, _| {},
        )
        .await
    });
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let _ = run_send(
        &mut s,
        "t-owfail",
        &items,
        &TransferControl::neutral(),
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await;
    drop(s);
    let recv = receiver.await.unwrap();
    assert!(recv.is_err(), "a hash mismatch must fail the receive");

    // The whole point of the fix: the original is UNTOUCHED (never unlinked).
    assert_eq!(
        std::fs::read(dl_dir.join("clash.bin")).unwrap(),
        original,
        "an aborted overwrite must not destroy the existing file"
    );
    // The failed temp is cleaned up (Integrity failure deletes it).
    assert!(
        !dl_dir.join("clash (1).bin").exists(),
        "the failed overwrite temp must be cleaned up"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// `loopback_planned`, for a transfer expected to FAIL: the send's own
/// error is ignored, and the receive's result comes back with what it left
/// in its progress and reached-files accumulators.
async fn loopback_failing(
    items: Vec<SendItem>,
    transfer_id: &'static str,
    dl: PathBuf,
    plan: Vec<FileDisposition>,
) -> (Result<Vec<PathBuf>>, Vec<FileProgress>, Vec<FileIdentity>) {
    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let manifest = read_manifest(&mut s).await.unwrap();
        let mut progress = Vec::new();
        let mut opened = Vec::new();
        let res = run_receive_core(
            &mut s,
            &manifest,
            &dl,
            &TransferControl::neutral(),
            &plan,
            &mut progress,
            &mut opened,
            None,
            &mut |_| {},
            |_, _| {},
        )
        .await;
        (res, progress, opened)
    });
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    let _ = run_send(
        &mut s,
        transfer_id,
        &items,
        &TransferControl::neutral(),
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await;
    drop(s);
    receiver.await.unwrap()
}

/// M1: under the overwrite policy a file with NOTHING to replace is written
/// straight to its own name — it is not a temp copy waiting on a commit — so
/// a later file failing must leave it alone, exactly as under "rename". It
/// was treated as an uncommitted temp and deleted, finished or not.
#[tokio::test]
async fn overwrite_failure_keeps_finished_files_that_replaced_nothing() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-m1-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let first = src_dir.join("first.bin");
    let second = src_dir.join("second.bin");
    std::fs::write(&first, b"arrived whole").unwrap();
    std::fs::write(&second, b"will fail its checksum").unwrap();

    let mut items = build_send_list(&[first, second], true).unwrap();
    assert_eq!(items[1].meta.name, "second.bin");
    items[1].meta.sha256 = Some("00".repeat(32));
    let plan = vec![
        FileDisposition::Fresh {
            rel: PathBuf::from("first.bin"),
            conflict: ConflictAction::Overwrite,
        },
        FileDisposition::Fresh {
            rel: PathBuf::from("second.bin"),
            conflict: ConflictAction::Overwrite,
        },
    ];
    let (res, _, _) = loopback_failing(items, "t-m1", dl_dir.clone(), plan).await;
    assert!(matches!(res, Err(LanBeamError::Integrity(_))), "{res:?}");
    assert_eq!(
        std::fs::read(dl_dir.join("first.bin")).unwrap(),
        b"arrived whole",
        "the finished file that replaced nothing stays"
    );
    assert!(!dl_dir.join("second.bin").exists(), "the corrupt one goes");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// L16: a partial that can't be reopened as recorded (here: it shrank on
/// disk) counts as reached, so the caller drops its record and the next
/// attempt starts the file fresh — instead of offering the same doomed
/// resume on every retry.
#[tokio::test]
async fn a_partial_that_cannot_be_reopened_loses_its_record() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-l16-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let src_file = src_dir.join("r.bin");
    std::fs::write(&src_file, vec![1u8; 1000]).unwrap();
    // The record says 100 bytes; only 50 are there now.
    std::fs::write(dl_dir.join("r.bin"), vec![1u8; 50]).unwrap();

    let items = build_send_list(std::slice::from_ref(&src_file), true).unwrap();
    let plan = vec![FileDisposition::Resume {
        disk_rel: PathBuf::from("r.bin"),
        offset: 100,
        commit_name: None,
    }];
    let (res, progress, opened) = loopback_failing(items, "t-l16", dl_dir, plan).await;
    assert!(res.is_err(), "the drifted partial fails the receive");
    assert!(progress.is_empty(), "nothing kept for a resume");
    assert_eq!(
        opened.iter().map(|o| o.rel.as_str()).collect::<Vec<_>>(),
        ["r.bin"],
        "reached, so its stale record is removed"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// M9, the recording half: an overwrite interrupted mid-file keeps its temp
/// copy for a resume AND remembers which file that copy is to replace.
#[tokio::test]
async fn interrupted_overwrite_records_the_name_it_replaces() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-m9rec-{}", std::process::id()));
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&dl_dir).unwrap();
    std::fs::write(dl_dir.join("clash.bin"), b"the original").unwrap();

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let declared = 100_000u64;
    let sender = tokio::spawn(async move {
        let stream = TcpStream::connect(addr).await.unwrap();
        let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
            .await
            .unwrap();
        for msg in [
            AppMessage::FileListBegin {
                transfer_id: "t-m9rec".into(),
                total_size: declared,
                file_count: 1,
            },
            AppMessage::FileListEntry {
                file: FileMeta {
                    name: "clash.bin".into(),
                    size: declared,
                    mtime: 0,
                    mode: 0,
                    sha256: Some("ab".repeat(32)),
                },
            },
            AppMessage::FileListEnd {
                transfer_id: "t-m9rec".into(),
            },
        ] {
            send_control(&mut s, &msg).await.unwrap();
        }
        match recv_control(&mut s).await.unwrap() {
            AppMessage::FileSendReply { accept: true, .. } => {}
            other => panic!("expected accept, got {other:?}"),
        }
        s.write_msg(&protocol::encode_file_chunk(0, &vec![7u8; 30_000]).unwrap())
            .await
            .unwrap();
        drop(s); // vanish mid-file
    });

    let (stream, _) = listener.accept().await.unwrap();
    let mut s = NoiseSession::handshake_responder(stream, &b_priv)
        .await
        .unwrap();
    let manifest = read_manifest(&mut s).await.unwrap();
    let plan = vec![FileDisposition::Fresh {
        rel: PathBuf::from("clash.bin"),
        conflict: ConflictAction::Overwrite,
    }];
    let mut progress = Vec::new();
    let mut opened = Vec::new();
    let res = run_receive_core(
        &mut s,
        &manifest,
        &dl_dir,
        &TransferControl::neutral(),
        &plan,
        &mut progress,
        &mut opened,
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await;
    sender.await.unwrap();

    assert!(res.is_err(), "a truncated stream fails the receive");
    assert_eq!(progress.len(), 1, "the temp copy is kept for a resume");
    assert_eq!(progress[0].disk_rel, "clash (1).bin");
    assert_eq!(progress[0].commit_name.as_deref(), Some("clash.bin"));
    assert_eq!(
        std::fs::read(dl_dir.join("clash.bin")).unwrap(),
        b"the original",
        "nothing is replaced until the whole transfer succeeds"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// M9, the resuming half: the continued temp copy ends where the overwrite
/// was always going to put it — on the original's name — instead of staying
/// a "(1)" second copy beside it.
#[tokio::test]
async fn resumed_overwrite_finishes_on_the_name_it_replaces() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-m9res-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let payload: Vec<u8> = (0..250_000u32).map(|i| (i % 251) as u8).collect();
    let offset = 100_000u64;
    let src_file = src_dir.join("clash.bin");
    std::fs::write(&src_file, &payload).unwrap();
    std::fs::write(dl_dir.join("clash.bin"), b"the original").unwrap();
    std::fs::write(dl_dir.join("clash (1).bin"), &payload[..offset as usize]).unwrap();

    let items = build_send_list(std::slice::from_ref(&src_file), true).unwrap();
    let plan = vec![FileDisposition::Resume {
        disk_rel: PathBuf::from("clash (1).bin"),
        offset,
        commit_name: Some("clash.bin".into()),
    }];
    let saved = loopback_planned(items, "t-m9res", dl_dir.clone(), plan)
        .await
        .expect("the resume completes");
    assert_eq!(saved, vec![dl_dir.join("clash.bin")]);
    assert_eq!(std::fs::read(dl_dir.join("clash.bin")).unwrap(), payload);
    assert!(
        !dl_dir.join("clash (1).bin").exists(),
        "no second copy left behind"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// A recorded commit name is only ever a sibling's name: anything that is
/// not exactly one clean name component is ignored, and the resumed file
/// simply stays where it is.
#[tokio::test]
async fn a_commit_name_is_only_ever_one_clean_name() {
    assert!(is_single_clean_name("clash.bin"));
    assert!(!is_single_clean_name("../clash.bin"));
    assert!(!is_single_clean_name("sub/clash.bin"));
    assert!(!is_single_clean_name(""));
    assert!(
        !is_single_clean_name("photo\u{202E}gnp.exe"),
        "a name the sanitizer would change is not clean"
    );

    let tmp = std::env::temp_dir().join(format!("lanbeam-m9esc-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let payload = b"0123456789abcdef".to_vec();
    let src_file = src_dir.join("x.bin");
    std::fs::write(&src_file, &payload).unwrap();
    std::fs::write(dl_dir.join("x (1).bin"), &payload[..4]).unwrap();
    let items = build_send_list(std::slice::from_ref(&src_file), true).unwrap();
    let plan = vec![FileDisposition::Resume {
        disk_rel: PathBuf::from("x (1).bin"),
        offset: 4,
        commit_name: Some("../x.bin".into()),
    }];
    let saved = loopback_planned(items, "t-m9esc", dl_dir.clone(), plan)
        .await
        .expect("the resume completes");
    assert_eq!(saved, vec![dl_dir.join("x (1).bin")], "left where it was");
    assert!(
        !tmp.join("x.bin").exists(),
        "nothing renamed out of the folder"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// M2: a partial recorded under one download folder is not resumed in
/// another — joined to the NEW folder its path names some other file. One
/// recorded before the field existed, or under this folder, still resumes.
#[test]
fn a_partial_from_another_download_folder_is_not_resumed() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-m2an-{}", std::process::id()));
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&dl_dir).unwrap();
    std::fs::write(dl_dir.join("big.bin"), vec![0u8; 400]).unwrap();
    let manifest = Manifest {
        transfer_id: "t-m2".into(),
        total_size: 1000,
        files: vec![FileMeta {
            name: "big.bin".into(),
            size: 1000,
            mtime: 0,
            mode: 0,
            sha256: Some("ab".repeat(32)),
        }],
    };
    let analyse = |root: Option<String>| {
        let store = RwLock::new(crate::partials::PartialsStore::load(tmp.join("p.json")));
        store.write().unwrap().set_files(
            "dev",
            &[],
            vec![PartialRecord {
                rel: "big.bin".into(),
                size: 1000,
                sha256: Some("ab".repeat(32)),
                disk_rel: "big.bin".into(),
                bytes_written: 400,
                commit_name: None,
                root,
            }],
        );
        analyze_files(&manifest, &dl_dir, None, "dev", &store)
    };
    let elsewhere = tmp.join("old-downloads").to_string_lossy().into_owned();
    assert!(
        analyse(Some(elsewhere))[0].resume.is_none(),
        "another folder"
    );
    assert!(
        analyse(None)[0].resume.is_some(),
        "a record from before the field"
    );
    let here = dl_dir.to_string_lossy().into_owned();
    assert!(analyse(Some(here))[0].resume.is_some(), "this folder");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The rename policy keeps both files: the incoming one de-dupes to
/// `clash (1).bin` and the original is untouched (M6.5, the pre-M6 default).
#[tokio::test]
async fn rename_conflict_dedupes_and_keeps_original() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-rn-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();
    let original = b"original, keep me".to_vec();
    std::fs::write(dl_dir.join("clash.bin"), &original).unwrap();
    let payload = b"incoming under a new name".to_vec();
    let src_file = src_dir.join("clash.bin");
    std::fs::write(&src_file, &payload).unwrap();

    let items = build_send_list(std::slice::from_ref(&src_file), false).unwrap();
    let plan = vec![FileDisposition::Fresh {
        rel: PathBuf::from("clash.bin"),
        conflict: ConflictAction::Rename,
    }];
    let saved = loopback_planned(items, "t-rn", dl_dir.clone(), plan)
        .await
        .expect("rename completes");
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0], dl_dir.join("clash (1).bin"), "rename de-dupes");
    assert_eq!(
        std::fs::read(&saved[0]).unwrap(),
        payload,
        "incoming lands under the new name"
    );
    assert_eq!(
        std::fs::read(dl_dir.join("clash.bin")).unwrap(),
        original,
        "the original file is untouched"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// A HASHED receive interrupted mid-file KEEPS the partial on disk and
/// records it for resume (M6.4) — the reverse of the non-hashed cleanup the
/// `partial_file_is_deleted_when_sender_dies_mid_file` test proves.
#[tokio::test]
async fn interrupted_hashed_receive_keeps_partial_and_records_it() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-keep-{}", std::process::id()));
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&dl_dir).unwrap();

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    let declared = 100_000u64;
    // A dummy but present hash → the file is "hashed", so its partial is
    // resumable and must survive the interruption. The hash is never checked
    // (we die before completion), so its value is irrelevant here.
    let dummy_hash = "ab".repeat(32);
    let sender = tokio::spawn(async move {
        let stream = TcpStream::connect(addr).await.unwrap();
        let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
            .await
            .unwrap();
        send_control(
            &mut s,
            &AppMessage::FileListBegin {
                transfer_id: "t-keep".into(),
                total_size: declared,
                file_count: 1,
            },
        )
        .await
        .unwrap();
        send_control(
            &mut s,
            &AppMessage::FileListEntry {
                file: FileMeta {
                    name: "half.bin".into(),
                    size: declared,
                    mtime: 0,
                    mode: 0,
                    sha256: Some(dummy_hash),
                },
            },
        )
        .await
        .unwrap();
        send_control(
            &mut s,
            &AppMessage::FileListEnd {
                transfer_id: "t-keep".into(),
            },
        )
        .await
        .unwrap();
        match recv_control(&mut s).await.unwrap() {
            AppMessage::FileSendReply { accept: true, .. } => {}
            other => panic!("expected accept, got {other:?}"),
        }
        // Stream well under the declared size, then vanish.
        s.write_msg(&protocol::encode_file_chunk(0, &vec![7u8; 30_000]).unwrap())
            .await
            .unwrap();
        drop(s);
    });

    let (stream, _) = listener.accept().await.unwrap();
    let mut s = NoiseSession::handshake_responder(stream, &b_priv)
        .await
        .unwrap();
    let manifest = read_manifest(&mut s).await.unwrap();
    let plan = vec![FileDisposition::Fresh {
        rel: PathBuf::from("half.bin"),
        conflict: ConflictAction::Rename,
    }];
    let mut progress = Vec::new();
    let mut opened = Vec::new();
    let res = run_receive_core(
        &mut s,
        &manifest,
        &dl_dir,
        &TransferControl::neutral(),
        &plan,
        &mut progress,
        &mut opened,
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await;
    sender.await.unwrap();

    assert!(res.is_err(), "a truncated stream must fail the receive");
    // The hashed partial is KEPT on disk with exactly the streamed bytes…
    let partial = dl_dir.join("half.bin");
    assert!(partial.exists(), "hashed partial must be kept for resume");
    assert_eq!(std::fs::metadata(&partial).unwrap().len(), 30_000);
    // …and the accumulator recorded it for `handle_incoming` to persist.
    assert_eq!(progress.len(), 1, "the interrupted file is recorded");
    assert!(progress[0].hashed);
    assert_eq!(progress[0].bytes_written, 30_000);
    assert_eq!(progress[0].identity.rel, "half.bin");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// finding-1: an interruption that opens only the FIRST of two manifest
/// files records ONLY that file in the `opened` accumulator — never the
/// second, unreached file — so `handle_incoming`'s error-path removal is
/// scoped to reached files and cannot wipe a still-valid partial for a file
/// this session never got to.
#[tokio::test]
async fn interrupted_receive_scopes_opened_to_reached_files() {
    let tmp = std::env::temp_dir().join(format!("lanbeam-scope-{}", std::process::id()));
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&dl_dir).unwrap();

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    let first = 100_000u64;
    let dummy_hash = "cd".repeat(32);
    let dummy_hash2 = dummy_hash.clone();
    let sender = tokio::spawn(async move {
        let stream = TcpStream::connect(addr).await.unwrap();
        let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
            .await
            .unwrap();
        send_control(
            &mut s,
            &AppMessage::FileListBegin {
                transfer_id: "t-scope".into(),
                total_size: first + 50_000,
                file_count: 2,
            },
        )
        .await
        .unwrap();
        for (name, size, hash) in [
            ("a.bin", first, dummy_hash),
            ("b.bin", 50_000u64, dummy_hash2),
        ] {
            send_control(
                &mut s,
                &AppMessage::FileListEntry {
                    file: FileMeta {
                        name: name.into(),
                        size,
                        mtime: 0,
                        mode: 0,
                        sha256: Some(hash),
                    },
                },
            )
            .await
            .unwrap();
        }
        send_control(
            &mut s,
            &AppMessage::FileListEnd {
                transfer_id: "t-scope".into(),
            },
        )
        .await
        .unwrap();
        match recv_control(&mut s).await.unwrap() {
            AppMessage::FileSendReply { accept: true, .. } => {}
            other => panic!("expected accept, got {other:?}"),
        }
        // Stream only PART of the first file, then vanish — the receiver
        // never opens the second file.
        s.write_msg(&protocol::encode_file_chunk(0, &vec![7u8; 20_000]).unwrap())
            .await
            .unwrap();
        drop(s);
    });

    let (stream, _) = listener.accept().await.unwrap();
    let mut s = NoiseSession::handshake_responder(stream, &b_priv)
        .await
        .unwrap();
    let manifest = read_manifest(&mut s).await.unwrap();
    let plan = vec![
        FileDisposition::Fresh {
            rel: PathBuf::from("a.bin"),
            conflict: ConflictAction::Rename,
        },
        FileDisposition::Fresh {
            rel: PathBuf::from("b.bin"),
            conflict: ConflictAction::Rename,
        },
    ];
    let mut progress = Vec::new();
    let mut opened = Vec::new();
    let res = run_receive_core(
        &mut s,
        &manifest,
        &dl_dir,
        &TransferControl::neutral(),
        &plan,
        &mut progress,
        &mut opened,
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await;
    sender.await.unwrap();

    assert!(res.is_err(), "a truncated stream must fail the receive");
    // Only the reached file is in the removal-scope accumulator; the
    // second, never-opened file's identity must NOT appear (so its stored
    // partial would survive the error-path removal).
    assert_eq!(
        opened.len(),
        1,
        "only the opened file is in the remove scope"
    );
    assert_eq!(opened[0].rel, "a.bin");
    assert!(
        !opened.iter().any(|id| id.rel == "b.bin"),
        "the unreached file is never in scope"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// finding-0: the resume reply is SPARSE (only non-zero offsets, keyed by
/// index) and BOUNDED — a resume set too large to fit one control frame
/// drops its overflow files to a fresh start (offset 0) instead of encoding
/// a dense vector that overflows the frame and errors the whole transfer.
#[test]
fn resume_offsets_sparse_and_bounded_to_one_frame() {
    // Sparse: only the non-zero entries ride the wire, keyed by index.
    let mut eff = vec![0u64, 4096, 0, 1_000_000];
    let sparse = resume_offsets_for_reply(&mut eff).expect("some offsets");
    assert_eq!(
        sparse,
        vec![
            ResumeOffset {
                index: 1,
                offset: 4096
            },
            ResumeOffset {
                index: 3,
                offset: 1_000_000
            },
        ]
    );
    assert_eq!(
        eff,
        vec![0, 4096, 0, 1_000_000],
        "in-budget offsets are untouched"
    );

    // All-zero (a fully-fresh plan) puts nothing on the wire.
    let mut all_fresh = vec![0u64; 5];
    assert!(resume_offsets_for_reply(&mut all_fresh).is_none());

    // Oversized: far more resumed files than fit one frame. The kept pairs
    // must (a) still encode within one control frame, (b) be fewer than the
    // total, and (c) leave every dropped file zeroed so the receive loop
    // restarts it fresh in lockstep with the offset-0 the sender is told.
    let n = 10_000usize;
    let mut many = vec![1_000_000u64; n];
    let sparse = resume_offsets_for_reply(&mut many).expect("some kept");
    let msg = AppMessage::FileSendReply {
        transfer_id: "t".into(),
        accept: true,
        reason: None,
        offsets: Some(sparse.clone()),
    };
    let enc = protocol::encode_control(&msg).unwrap();
    assert!(
        enc.len() <= crate::consts::MAX_PLAINTEXT,
        "the sparse reply must fit one frame, got {} bytes",
        enc.len()
    );
    let kept = sparse.len();
    assert!(
        kept < n,
        "not all {n} files fit; the overflow drops to fresh"
    );
    let still: Vec<ResumeOffset> = many
        .iter()
        .enumerate()
        .filter(|(_, &o)| o != 0)
        .map(|(i, &o)| ResumeOffset {
            index: i as u32,
            offset: o,
        })
        .collect();
    assert_eq!(
        sparse, still,
        "kept wire pairs are exactly the still-resumed entries"
    );
    assert_eq!(
        many.iter().filter(|&&o| o == 0).count(),
        n - kept,
        "every non-kept file is zeroed for a fresh restart"
    );
}

/// finding-2: a transfer cancelled while QUEUED on a full concurrency gate
/// aborts immediately — the biased select the send/receive gate sites use
/// lets the cancel win over an acquire that would otherwise never complete.
#[tokio::test]
async fn queued_transfer_cancel_wins_over_full_gate() {
    use std::sync::Arc;

    let gate = Arc::new(crate::state::ConcurrencyGate::new(1));
    let _held = gate.try_acquire_receive().expect("first slot is free");
    assert!(
        gate.try_acquire_receive().is_none(),
        "the gate is now full — the next transfer queues"
    );

    let ctl = TransferControl::neutral();
    ctl.cancel.cancel(); // cancelled while parked on the gate

    let outcome: Result<()> = tokio::select! {
        biased;
        _ = ctl.cancel.cancelled() => Err(LanBeamError::Cancelled),
        _g = gate.acquire_receive() => Ok(()),
    };
    assert!(
        matches!(outcome, Err(LanBeamError::Cancelled)),
        "a cancel issued while queued must abort before the slot ever frees"
    );
}

// ── per-file progress (M6.8) ──────────────────────────────────────────

/// A multi-file loopback must emit a `Streaming` event for EVERY file
/// (indices only advancing, matching manifest order) and a `Done` event per
/// file in order — each `verified` because the files ride whole-file hashes
/// that the receiver checks (M6.3). Proves the detail drawer (6.9) gets
/// correctly-indexed per-file signal.
#[tokio::test]
async fn per_file_progress_events_report_each_file_in_order() {
    use std::sync::Arc;

    let tmp = std::env::temp_dir().join(format!("lanbeam-fileprog-{}", std::process::id()));
    let src_dir = tmp.join("src");
    let dl_dir = tmp.join("downloads");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dl_dir).unwrap();

    // Three distinct files, each spanning several chunks so every file emits
    // at least one Streaming event; verify ON so each Done is verified=true.
    let mut src_files = Vec::new();
    for k in 0..3u32 {
        let payload: Vec<u8> = (0..80_000u32).map(|i| ((i + k) % 251) as u8).collect();
        let f = src_dir.join(format!("f{k}.bin"));
        std::fs::write(&f, &payload).unwrap();
        src_files.push(f);
    }
    let items = build_send_list(&src_files, true).unwrap();

    let a_priv = priv_key();
    let b_priv = priv_key();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dl = dl_dir.clone();

    // Capture per-file events on the RECEIVE side; the borrowed name is
    // dropped (we assert on indices + verified, which is all the UI keys on).
    let stream_idx = Arc::new(Mutex::new(Vec::<usize>::new()));
    let done = Arc::new(Mutex::new(Vec::<(usize, bool)>::new()));
    let s_idx = stream_idx.clone();
    let d = done.clone();

    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut s = NoiseSession::handshake_responder(stream, &b_priv)
            .await
            .unwrap();
        let manifest = read_manifest(&mut s).await.unwrap();
        let plan: Vec<FileDisposition> = manifest
            .files
            .iter()
            .map(|f| match sanitize::validate(&f.name) {
                Decision::Accept(rel) => FileDisposition::Fresh {
                    rel,
                    conflict: ConflictAction::Rename,
                },
                Decision::Reject(_) => panic!("unexpected reject for {}", f.name),
            })
            .collect();
        let mut progress = Vec::new();
        let mut opened = Vec::new();
        let mut on_file = |e: FileEvent<'_>| match e {
            FileEvent::Streaming { index, .. } => s_idx.lock().unwrap().push(index),
            FileEvent::Done { index, verified } => d.lock().unwrap().push((index, verified)),
            FileEvent::PauseAutoResumed | FileEvent::Accepted => {}
        };
        run_receive_core(
            &mut s,
            &manifest,
            &dl,
            &TransferControl::neutral(),
            &plan,
            &mut progress,
            &mut opened,
            None,
            &mut on_file,
            |_, _| {},
        )
        .await
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut s = NoiseSession::handshake_initiator(stream, &a_priv, None)
        .await
        .unwrap();
    run_send(
        &mut s,
        "t-fileprog",
        &items,
        &TransferControl::neutral(),
        None,
        &mut |_| {},
        |_, _| {},
    )
    .await
    .expect("send completes");
    drop(s);

    let saved = receiver.await.unwrap().expect("receive completes");
    assert_eq!(saved.len(), 3);

    // Every file emitted at least one progress event…
    let s_idx = stream_idx.lock().unwrap();
    for k in 0..3usize {
        assert!(
            s_idx.contains(&k),
            "file {k} must emit at least one progress event: {s_idx:?}"
        );
    }
    // …and the indices only ever advance (files stream in manifest order).
    assert!(
        s_idx.windows(2).all(|w| w[0] <= w[1]),
        "indices must be monotonic: {s_idx:?}"
    );

    // One Done per file, in order, each verified (hash present + matched).
    let done = done.lock().unwrap();
    assert_eq!(
        *done,
        vec![(0, true), (1, true), (2, true)],
        "each file done+verified, in order"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
