//! Application message layer (post-handshake). Each transport plaintext is
//! `kind(1) || body`: CONTROL = JSON `AppMessage`, FILE_CHUNK = `index:u32 BE || bytes`.
//! The file manifest is PAGED ([FIX-4]) so a large file list never overflows one frame.
//! DESIGN §1.3–1.4. Wired into the send/receive loops in M3-B.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::consts::{KIND_CONTROL, KIND_FILE_CHUNK, MAX_FILE_DATA, TRANSFER_V1, TRANSFER_V2};
use crate::error::{LanBeamError, Result};

/// Transfer versions this build speaks, advertised in `Hello` (M4.2) and
/// negotiated to the highest common. APPENDING a version (never renumbering)
/// keeps older peers working: they still find their version in the overlap and
/// the session degrades to that feature set instead of dying. `TRANSFER_V2` is
/// M7 (pairing + quick text); it rides here so two current builds negotiate 2
/// while an M4–M6 peer still negotiates `TRANSFER_V1`.
pub const SUPPORTED_VERSIONS: &[u8] = &[TRANSFER_V1, TRANSFER_V2];

/// Highest protocol version both sides speak. `None` means no overlap — the
/// session cannot proceed and must fail with a version error, never a guess.
pub fn negotiate_version(peer_versions: &[u8]) -> Option<u8> {
    peer_versions
        .iter()
        .copied()
        .filter(|v| SUPPORTED_VERSIONS.contains(v))
        .max()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FileMeta {
    /// sender's leaf/relative path — the RECEIVER sanitizes this (never trusted).
    pub name: String,
    pub size: u64,
    pub mtime: i64,
    pub mode: u32,
    /// Lowercase-hex SHA-256 of the file's contents, set only when the SENDER's
    /// `verify_hash` setting was on (M6.3). Additive + `#[serde(default)]`, so a
    /// legacy peer — or a sender with verification off — omits it and the
    /// receiver simply skips the check. WHY no downgrade protection is claimed:
    /// the same peer authors the manifest that carries (or drops) this hash, so
    /// a hostile sender could always omit it; this guards against accidental
    /// corruption on the wire/disk, not a lying peer.
    #[serde(default)]
    pub sha256: Option<String>,
}

/// One resumed file's offset in a [`AppMessage::FileSendReply`] (M6.4). The
/// reply carries these SPARSELY — one pair per file the receiver already holds
/// a prefix of, keyed by manifest `index` — rather than a dense per-file vector,
/// so a many-thousand-file manifest's reply never overflows the single control
/// frame it rides in.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ResumeOffset {
    /// Manifest file index (0-based) this offset applies to.
    pub index: u32,
    /// Bytes the receiver already holds for that file — the sender seeks here
    /// and streams only the tail.
    pub offset: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum AppMessage {
    Hello {
        versions: Vec<u8>,
    },
    DeviceInfo {
        name: String,
        platform: String,
        app_version: String,
    },
    /// Paged manifest: begin → many entries → end. ([FIX-4])
    FileListBegin {
        transfer_id: String,
        total_size: u64,
        file_count: u32,
    },
    FileListEntry {
        file: FileMeta,
    },
    FileListEnd {
        transfer_id: String,
    },
    FileSendReply {
        transfer_id: String,
        accept: bool,
        reason: Option<String>,
        /// SPARSE per-file resume offsets (M6.4): one [`ResumeOffset`] per file
        /// the receiver already holds a prefix of, so the sender seeks there and
        /// streams only the tail. Files absent from the list start at 0. Sparse
        /// (not a dense per-file vector) so the reply stays inside one control
        /// frame no matter the file count. Additive + `#[serde(default)]` — a
        /// legacy receiver omits it (the sender then starts every file at 0),
        /// and a legacy sender ignores it (serde drops the unknown field), so
        /// the field is bidirectionally safe. WHY the sender only trusts these
        /// for hashed files: see `run_receive`'s resume gating — the whole-file
        /// SHA-256 is what proves the reassembled `old-bytes + tail` is correct,
        /// which also makes resume safe against a sender that predates offsets.
        #[serde(default)]
        offsets: Option<Vec<ResumeOffset>>,
    },
    TransferAck {
        transfer_id: String,
        ok: bool,
        error: Option<String>,
    },
    /// In-band pairing request (M7.1), gated on `TRANSFER_V2`: the initiator that
    /// redeemed a pairing code presents it so the responder can match it against
    /// the code it minted, after which both sides store each other's fingerprint
    /// in the trust store. Only ever sent to a peer whose `Hello` advertised 2 —
    /// a v1 (M4–M6) peer never receives it (see `transfer::PeerHello::version`).
    PairRequest {
        code: String,
    },
    /// The responder's answer to a [`AppMessage::PairRequest`] (M7.1): `accept`
    /// on a code match — carrying the responder's friendly `name` for the
    /// initiator's trust entry — or a decline with an optional human `reason`.
    /// Also gated on `TRANSFER_V2`.
    PairConfirm {
        accept: bool,
        name: String,
        reason: Option<String>,
    },
    /// Quick-text push (M7.3), gated on `TRANSFER_V2`: one control frame carrying
    /// a short text/link. Only ever sent to a peer whose `Hello` advertised 2, so
    /// a v1 (M4–M6) peer never receives it. `to_clipboard` is the sender's
    /// per-send REQUEST ("also put this on your clipboard", the UI's
    /// 同时写入对方剪贴板 toggle): additive + `#[serde(default)]`, so a peer that
    /// omits it decodes to `None`. It is only a request — the receiver still gates
    /// the clipboard write on its OWN `clip_share` consent setting (see
    /// `transfer::handle_text_received`), so the receiver always has the final say.
    TextSend {
        text: String,
        #[serde(default)]
        to_clipboard: Option<bool>,
    },
    /// Acknowledges a control message by a short tag (M7.3): the quick-text
    /// receiver replies `Ack { of: "text" }` once it has emitted the text, so the
    /// sender's `send_text` resolves only after delivery. Gated on `TRANSFER_V2`.
    Ack {
        of: String,
    },
    Error {
        code: String,
        message: String,
    },
    Bye,
}

/// A decoded transport plaintext frame.
#[derive(Debug, PartialEq)]
pub enum Frame {
    Control(AppMessage),
    /// (chunk index, file bytes)
    FileChunk(u32, Vec<u8>),
}

/// Encode a CONTROL message: `0x01 || json`.
pub fn encode_control(msg: &AppMessage) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(msg).map_err(|e| LanBeamError::Protocol(e.to_string()))?;
    let mut out = Vec::with_capacity(json.len() + 1);
    out.push(KIND_CONTROL);
    out.extend_from_slice(&json);
    Ok(out)
}

/// Encode a FILE_CHUNK: `0x02 || index:u32 BE || bytes` (bytes must be <= MAX_FILE_DATA).
pub fn encode_file_chunk(index: u32, bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() > MAX_FILE_DATA {
        return Err(LanBeamError::Protocol(
            "file chunk exceeds MAX_FILE_DATA".into(),
        ));
    }
    let mut out = Vec::with_capacity(bytes.len() + 5);
    out.push(KIND_FILE_CHUNK);
    out.extend_from_slice(&index.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(out)
}

/// Decode one transport plaintext frame by its leading kind byte.
pub fn decode(plaintext: &[u8]) -> Result<Frame> {
    let (&kind, body) = plaintext
        .split_first()
        .ok_or_else(|| LanBeamError::Protocol("empty frame".into()))?;
    match kind {
        KIND_CONTROL => {
            let msg = serde_json::from_slice(body)
                .map_err(|e| LanBeamError::Protocol(format!("bad control json: {e}")))?;
            Ok(Frame::Control(msg))
        }
        KIND_FILE_CHUNK => {
            if body.len() < 4 {
                return Err(LanBeamError::Protocol("truncated file chunk header".into()));
            }
            let index = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
            Ok(Frame::FileChunk(index, body[4..].to_vec()))
        }
        other => Err(LanBeamError::Protocol(format!(
            "unknown frame kind {other:#04x}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_roundtrip() {
        let msg = AppMessage::FileSendReply {
            transfer_id: "t-1".into(),
            accept: true,
            reason: None,
            offsets: None,
        };
        let enc = encode_control(&msg).unwrap();
        assert_eq!(enc[0], KIND_CONTROL);
        assert_eq!(decode(&enc).unwrap(), Frame::Control(msg));
    }

    #[test]
    fn paged_manifest_roundtrip() {
        let begin = AppMessage::FileListBegin {
            transfer_id: "t-1".into(),
            total_size: 1234,
            file_count: 2,
        };
        let entry = AppMessage::FileListEntry {
            file: FileMeta {
                name: "docs/a.txt".into(),
                size: 10,
                mtime: 0,
                mode: 0,
                sha256: None,
            },
        };
        for m in [
            begin,
            entry,
            AppMessage::FileListEnd {
                transfer_id: "t-1".into(),
            },
        ] {
            let enc = encode_control(&m).unwrap();
            assert_eq!(decode(&enc).unwrap(), Frame::Control(m));
        }
    }

    #[test]
    fn file_chunk_roundtrip() {
        let bytes = vec![7u8; 1000];
        let enc = encode_file_chunk(42, &bytes).unwrap();
        assert_eq!(enc[0], KIND_FILE_CHUNK);
        assert_eq!(decode(&enc).unwrap(), Frame::FileChunk(42, bytes));
    }

    #[test]
    fn chunk_too_large_rejected() {
        assert!(encode_file_chunk(0, &vec![0u8; MAX_FILE_DATA + 1]).is_err());
    }

    #[test]
    fn unknown_kind_is_error() {
        assert!(decode(&[0x09, 1, 2, 3]).is_err());
        assert!(decode(&[]).is_err());
    }

    /// The resume offsets (M6.4) ride `FileSendReply` as an additive field:
    /// present values round-trip, and a reply written by a legacy receiver
    /// (no `offsets` key) decodes with `offsets == None` — the sender then
    /// starts every file at 0, so the field is bidirectionally safe.
    #[test]
    fn file_send_reply_offsets_are_additive_and_backward_compatible() {
        let with = AppMessage::FileSendReply {
            transfer_id: "t-r".into(),
            accept: true,
            reason: None,
            offsets: Some(vec![
                ResumeOffset {
                    index: 1,
                    offset: 4096,
                },
                ResumeOffset {
                    index: 2,
                    offset: 1_000_000,
                },
            ]),
        };
        let enc = encode_control(&with).unwrap();
        assert_eq!(decode(&enc).unwrap(), Frame::Control(with));

        // A legacy receiver's reply carries no `offsets` key at all.
        let legacy = br#"{"t":"file_send_reply","transfer_id":"t-r","accept":true,"reason":null}"#;
        let mut frame = vec![KIND_CONTROL];
        frame.extend_from_slice(legacy);
        match decode(&frame).unwrap() {
            Frame::Control(AppMessage::FileSendReply {
                offsets, accept, ..
            }) => {
                assert!(accept);
                assert_eq!(offsets, None, "a missing offsets key must default to None");
            }
            other => panic!("expected FileSendReply, got {other:?}"),
        }
    }

    /// The opening-dialogue messages (M4.2) must round-trip like every other
    /// control message, and negotiation must pick the shared version — or none.
    #[test]
    fn hello_exchange_roundtrip_and_negotiation() {
        for m in [
            AppMessage::Hello {
                versions: SUPPORTED_VERSIONS.to_vec(),
            },
            AppMessage::DeviceInfo {
                name: "Johns-PC".into(),
                platform: "windows".into(),
                app_version: "0.1.0".into(),
            },
            AppMessage::Bye,
        ] {
            let enc = encode_control(&m).unwrap();
            assert_eq!(decode(&enc).unwrap(), Frame::Control(m));
        }
        assert_eq!(negotiate_version(&[TRANSFER_V1]), Some(TRANSFER_V1));
        // a FUTURE peer advertising more versions still overlaps on ours
        assert_eq!(negotiate_version(&[TRANSFER_V1, 7]), Some(TRANSFER_V1));
        assert_eq!(negotiate_version(&[9]), None);
        assert_eq!(negotiate_version(&[]), None);
    }

    /// The M7 gate (stage 7.0): with `SUPPORTED_VERSIONS = [1, 2]`, two current
    /// builds negotiate the v2 feature level, while an M4–M6 peer that speaks
    /// only v1 pins the session to 1 — always the highest COMMON version, never
    /// a guess. This is what every v2-only send (pairing, text) checks before it
    /// puts a `PairRequest`/`TextSend` on the wire.
    #[test]
    fn negotiate_picks_highest_common_transfer_version() {
        assert_eq!(
            negotiate_version(&[TRANSFER_V1, TRANSFER_V2]),
            Some(TRANSFER_V2)
        );
        assert_eq!(negotiate_version(&[TRANSFER_V1]), Some(TRANSFER_V1));
        // Order-independent: the peer may advertise its versions in any order.
        assert_eq!(
            negotiate_version(&[TRANSFER_V2, TRANSFER_V1]),
            Some(TRANSFER_V2)
        );
    }

    /// The M7 pairing variants (gated on `TRANSFER_V2`) round-trip through the
    /// control frame like every other `AppMessage` — both the accept form
    /// (carrying the responder's name) and the decline form (carrying a reason).
    #[test]
    fn pair_messages_roundtrip() {
        for m in [
            AppMessage::PairRequest {
                code: "482913".into(),
            },
            AppMessage::PairConfirm {
                accept: true,
                name: "Johns-PC".into(),
                reason: None,
            },
            AppMessage::PairConfirm {
                accept: false,
                name: "Johns-PC".into(),
                reason: Some("code expired".into()),
            },
        ] {
            let enc = encode_control(&m).unwrap();
            assert_eq!(enc[0], KIND_CONTROL);
            assert_eq!(decode(&enc).unwrap(), Frame::Control(m));
        }
    }

    /// The quick-text variants (M7.3, gated on `TRANSFER_V2`) round-trip through
    /// the control frame, and `TextSend.to_clipboard` is additive: a payload
    /// written without the key (a peer that never sets it) decodes to `None`, so
    /// the field is bidirectionally safe like `FileMeta.sha256`/resume offsets.
    #[test]
    fn text_send_and_ack_roundtrip_with_additive_flag() {
        for m in [
            AppMessage::TextSend {
                text: "ship it 🚀".into(),
                to_clipboard: Some(true),
            },
            AppMessage::TextSend {
                text: "no clip".into(),
                to_clipboard: None,
            },
            AppMessage::Ack { of: "text".into() },
        ] {
            let enc = encode_control(&m).unwrap();
            assert_eq!(enc[0], KIND_CONTROL);
            assert_eq!(decode(&enc).unwrap(), Frame::Control(m));
        }

        // A sender that omits `to_clipboard` entirely (older/other build) still
        // decodes: the missing key defaults to None, never an error.
        let legacy = br#"{"t":"text_send","text":"hi"}"#;
        let mut frame = vec![KIND_CONTROL];
        frame.extend_from_slice(legacy);
        match decode(&frame).unwrap() {
            Frame::Control(AppMessage::TextSend { text, to_clipboard }) => {
                assert_eq!(text, "hi");
                assert_eq!(
                    to_clipboard, None,
                    "a missing to_clipboard key must default to None"
                );
            }
            other => panic!("expected TextSend, got {other:?}"),
        }
    }

    /// A v2 control variant carrying a large-but-legal body still encodes to a
    /// SINGLE transport plaintext under `MAX_PLAINTEXT` (the Noise message cap),
    /// so pairing/quick-text ride one control frame and never need the paging the
    /// file manifest uses.
    #[test]
    fn v2_control_variant_fits_one_frame() {
        let text = "x".repeat(60_000);
        let msg = AppMessage::TextSend {
            text,
            to_clipboard: Some(true),
        };
        let enc = encode_control(&msg).unwrap();
        assert_eq!(enc[0], KIND_CONTROL);
        assert!(
            enc.len() <= crate::consts::MAX_PLAINTEXT,
            "a v2 control frame must fit one Noise message ({} > {})",
            enc.len(),
            crate::consts::MAX_PLAINTEXT
        );
        assert_eq!(decode(&enc).unwrap(), Frame::Control(msg));
    }
}
