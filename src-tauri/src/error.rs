//! Crate-wide error type. Serializes with a `{kind, message}` shape so a failed
//! `#[tauri::command]` surfaces a structured, readable error on the JS side.
//! `message`s are internal diagnostics for logs/UI — never a user-facing contract.

use serde::Serialize;

#[derive(thiserror::Error, Debug, Serialize)]
#[serde(tag = "kind", content = "message")]
pub enum LanBeamError {
    #[error("io: {0}")]
    Io(String),
    #[error("bind failed: {0}")]
    Bind(String),
    #[error("handshake failed: {0}")]
    Handshake(String),
    #[error("peer identity mismatch")]
    IdentityMismatch,
    #[error("peer not found")]
    PeerNotFound,
    /// The peer received the text frame but did not surface it: this device is not
    /// on their trusted list, and their receive policy only lets trusted senders
    /// through. Quick text has no accept prompt, so there is nowhere to park it.
    #[error("the peer's receive policy dropped the text")]
    TextRefused,
    /// The peer is rate-limiting quick text from this source.
    #[error("the peer is throttling quick text")]
    TextThrottled,
    /// A quick text is over the size cap — raw bytes past `MAX_TEXT_BYTES`, or a
    /// JSON-escaped frame past one Noise message. Raised BEFORE any byte goes out
    /// (usually before the socket is even opened), so it is kept apart from
    /// `Protocol`: the UI used to show "connection failed, retry" for it, and no
    /// retry of the same text can ever succeed.
    #[error("quick text is too long")]
    TextTooLong,
    /// The address dialed answered with THIS device's own key. You cannot pair
    /// with, trust, or send to yourself — and an entry for yourself in the peer
    /// list is uniquely poisonous: discovery drops its own announces (so it can
    /// never be discovered away), which leaves it stuck in the device list with
    /// nothing able to remove it.
    #[error("that address is this device")]
    SelfPeer,
    #[error("rejected by peer")]
    Rejected,
    #[error("unsafe filename: {0}")]
    UnsafePath(String),
    #[error("keyring: {0}")]
    Keyring(String),
    #[error("crypto: {0}")]
    Crypto(String),
    #[error("protocol: {0}")]
    Protocol(String),
    /// The peer dropped (or failed) the post-handshake version exchange —
    /// almost always a build predating `Hello`/`DeviceInfo` (M4.2). Kept apart
    /// from `Protocol` so the UI can say "update the other device" instead of
    /// showing a generic failure.
    #[error("peer version too old: {0}")]
    PeerTooOld(String),
    /// The pairing host checked the presented code and said no: wrong digits,
    /// an expired invitation, or no invitation active there at all. Kept apart
    /// from `Protocol` so the UI can say "wrong or expired code" instead of
    /// "the connection failed" (the message is the host's reason token).
    #[error("pairing rejected: {0}")]
    PairRejected(String),
    /// A dial target the user typed (or a link carried) is not an `ip[:port]`.
    /// User input, not a wire fault — its own kind so the UI can point at the
    /// address field rather than blaming the connection.
    #[error("invalid address: {0}")]
    InvalidAddress(String),
    /// A join was attempted with no pairing code at all (neither typed nor
    /// carried by the link).
    #[error("no pairing code provided")]
    PairCodeMissing,
    /// The TCP dial itself failed — connection refused, no route, host down.
    /// Distinct from `Io` (which the UI renders as "a file or disk error") so
    /// an unreachable peer reads as exactly that.
    #[error("unreachable: {0}")]
    Unreachable(String),
    /// The connection dropped in the middle of a conversation: the peer closed
    /// it (cancelled, quit, crashed) or the network went away. A TRANSPORT
    /// failure — a read or write on the peer's socket — kept apart from `Io`,
    /// which the UI words as a file or disk error, and from `Protocol`, which
    /// means the peer said something that made no sense.
    #[error("connection lost: {0}")]
    ConnectionLost(String),
    /// The receiver declined on its own because it was already running as many
    /// transfers as it allows. Nobody said no, and later can work.
    #[error("the peer is busy")]
    PeerBusy,
    /// The receiver reported that the transfer was cancelled on its side.
    #[error("the peer cancelled the transfer")]
    PeerCancelled,
    /// The receiver's checksum of a file did not match the one we sent: the data
    /// was corrupted on the way, and the receiver deleted that file. Its own kind
    /// on the SENDING side, because `Integrity`'s words are the receiver's ("the
    /// file was deleted; the rest are in your download folder").
    #[error("the peer's checksum of a file failed")]
    PeerIntegrity,
    /// The receiver could not write what it received (a full disk, no write
    /// permission). The failure is on THEIR machine — `Io` would send the user
    /// looking at their own disk.
    #[error("the peer could not save the files")]
    PeerIo,
    /// The selection is more than one transfer may carry (file count, or the
    /// total bytes of the file names), caught before anything is hashed or
    /// dialed.
    #[error("too many files: {0}")]
    TooManyFiles(String),
    /// The selection holds no files at all — only empty folders, say.
    #[error("no files to send")]
    NoFiles,
    #[error("cancelled")]
    Cancelled,
    /// A received file's SHA-256 did not match the sender-declared digest
    /// (M6.3) — the bytes were corrupted in flight (or the source changed
    /// between hashing and sending). Kept apart from `Protocol`/`Io` so the UI
    /// can say "integrity check failed, resend" rather than showing a generic
    /// failure; the partial is deleted through the same error path.
    #[error("integrity: {0}")]
    Integrity(String),
    #[error("timeout")]
    Timeout,
    /// A path the UI asked us to open (a received file, the download folder) is
    /// not on disk any more — moved, deleted, or the record predates a
    /// download-dir change. Kept APART from `Io` so the UI can say "that file is
    /// gone" instead of a generic failure, and — crucially — so it stops
    /// blaming a stale record for what might really be an open failure.
    #[error("not found: {0}")]
    NotFound(String),
}

impl LanBeamError {
    /// Coarse machine-readable code for the `transfer_error` event's optional
    /// `code` field: `"declined" | "timeout" | "cancelled" | "peer_too_old" |
    /// "integrity" | "text_dropped" | "unreachable" | "connection_lost" |
    /// "peer_busy" | "peer_cancelled" | "peer_integrity" | "peer_io" |
    /// "unsafe_path" | "io" | "protocol"`. The frontend's contract test reads
    /// this match and fails on a code it has no words for.
    ///
    /// The receiver also sends its own code to the sender as the
    /// `TransferAck.error` token; `transfer::receive::peer_ack_error` turns it back into
    /// the sender-side kind.
    /// WHY: the UI must tell "the peer said no" apart from a genuine failure
    /// without string-matching the human-oriented `error` text (M4.5), and the
    /// error strings themselves are NOT a stable contract.
    pub fn ui_code(&self) -> &'static str {
        match self {
            LanBeamError::Rejected => "declined",
            LanBeamError::Timeout => "timeout",
            LanBeamError::Cancelled => "cancelled",
            LanBeamError::PeerTooOld(_) => "peer_too_old",
            // A corrupted-in-flight file is its own UI state (offer a resend),
            // distinct from the "conversation broke" protocol bucket (M6.3).
            LanBeamError::Integrity(_) => "integrity",
            // Not a broken conversation — the peer heard it and declined to
            // surface it. Its own bucket so the UI can say something actionable
            // ("ask them to trust you") instead of "the connection failed".
            LanBeamError::TextRefused | LanBeamError::TextThrottled => "text_dropped",
            // The peer could not be dialed at all — "can't reach them", not a
            // disk error and not a broken conversation.
            LanBeamError::Unreachable(_) => "unreachable",
            // The peer's socket went away mid-conversation — not this machine's
            // disk, and not a peer talking nonsense.
            LanBeamError::ConnectionLost(_) => "connection_lost",
            // What the receiver told us about its side. Each needs its own words:
            // "busy" invites a retry, "cancelled" was a person, and an integrity
            // or disk failure happened on THEIR machine.
            LanBeamError::PeerBusy => "peer_busy",
            LanBeamError::PeerCancelled => "peer_cancelled",
            LanBeamError::PeerIntegrity => "peer_integrity",
            LanBeamError::PeerIo => "peer_io",
            // A name the receiver could not place safely — "the connection
            // failed" sent people retrying something that can never work.
            LanBeamError::UnsafePath(_) => "unsafe_path",
            LanBeamError::Io(_)
            | LanBeamError::Bind(_)
            | LanBeamError::Keyring(_)
            | LanBeamError::NotFound(_) => "io",
            // Handshake / identity / sanitizer / crypto / protocol failures all
            // mean "the conversation itself broke" as far as the UI cares.
            _ => "protocol",
        }
    }

    /// Whether a receive that ended with this error should KEEP the in-flight
    /// partial on disk for a later resume (M6.4), rather than delete it.
    ///
    /// An INTERRUPTION — a cancel, a timeout, a dropped/closed connection, an
    /// I/O blip, a torn conversation — leaves valid bytes on disk that a retry
    /// can continue from, so we keep them. A DATA-INTEGRITY failure
    /// ([`LanBeamError::Integrity`]) means the reassembled bytes are provably
    /// wrong, so the partial is worthless: it is discarded and the file
    /// re-fetched from scratch. Only integrity is non-resumable; every other
    /// error is treated as a survivable interruption.
    ///
    /// NOTE the receive path additionally gates on a per-file content hash: a
    /// partial is only actually kept when the manifest carried a SHA-256 for
    /// that file (the hash is what lets the resume be verified end to end), so
    /// a non-hashed transfer still cleans up on interruption exactly as before.
    pub fn keeps_partial(&self) -> bool {
        !matches!(self, LanBeamError::Integrity(_))
    }
}

impl From<std::io::Error> for LanBeamError {
    fn from(e: std::io::Error) -> Self {
        LanBeamError::Io(e.to_string())
    }
}
impl From<keyring::Error> for LanBeamError {
    fn from(e: keyring::Error) -> Self {
        LanBeamError::Keyring(e.to_string())
    }
}
impl From<snow::Error> for LanBeamError {
    fn from(e: snow::Error) -> Self {
        LanBeamError::Crypto(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, LanBeamError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_text_matches_thiserror_attributes() {
        assert_eq!(LanBeamError::Io("disk".into()).to_string(), "io: disk");
        assert_eq!(
            LanBeamError::Bind("addr in use".into()).to_string(),
            "bind failed: addr in use"
        );
        assert_eq!(
            LanBeamError::Handshake("nope".into()).to_string(),
            "handshake failed: nope"
        );
        assert_eq!(
            LanBeamError::IdentityMismatch.to_string(),
            "peer identity mismatch"
        );
        assert_eq!(LanBeamError::PeerNotFound.to_string(), "peer not found");
        assert_eq!(LanBeamError::Rejected.to_string(), "rejected by peer");
        assert_eq!(
            LanBeamError::UnsafePath("../etc".into()).to_string(),
            "unsafe filename: ../etc"
        );
        assert_eq!(
            LanBeamError::Keyring("locked".into()).to_string(),
            "keyring: locked"
        );
        assert_eq!(
            LanBeamError::Crypto("bad key".into()).to_string(),
            "crypto: bad key"
        );
        assert_eq!(
            LanBeamError::Protocol("oops".into()).to_string(),
            "protocol: oops"
        );
        assert_eq!(
            LanBeamError::PeerTooOld("v0".into()).to_string(),
            "peer version too old: v0"
        );
        assert_eq!(LanBeamError::Cancelled.to_string(), "cancelled");
        assert_eq!(
            LanBeamError::Integrity("hash".into()).to_string(),
            "integrity: hash"
        );
        assert_eq!(LanBeamError::Timeout.to_string(), "timeout");
        assert_eq!(
            LanBeamError::TextTooLong.to_string(),
            "quick text is too long"
        );
        assert_eq!(
            LanBeamError::PairRejected("code".into()).to_string(),
            "pairing rejected: code"
        );
        assert_eq!(
            LanBeamError::InvalidAddress("x".into()).to_string(),
            "invalid address: x"
        );
        assert_eq!(
            LanBeamError::PairCodeMissing.to_string(),
            "no pairing code provided"
        );
        assert_eq!(
            LanBeamError::Unreachable("refused".into()).to_string(),
            "unreachable: refused"
        );
        assert_eq!(
            LanBeamError::ConnectionLost("reset".into()).to_string(),
            "connection lost: reset"
        );
        assert_eq!(LanBeamError::PeerBusy.to_string(), "the peer is busy");
        assert_eq!(
            LanBeamError::TooManyFiles("12000".into()).to_string(),
            "too many files: 12000"
        );
        assert_eq!(LanBeamError::NoFiles.to_string(), "no files to send");
    }

    #[test]
    fn ui_code_covers_every_variant() {
        assert_eq!(LanBeamError::Rejected.ui_code(), "declined");
        assert_eq!(LanBeamError::Timeout.ui_code(), "timeout");
        assert_eq!(LanBeamError::Cancelled.ui_code(), "cancelled");
        assert_eq!(
            LanBeamError::PeerTooOld("x".into()).ui_code(),
            "peer_too_old"
        );
        assert_eq!(LanBeamError::Integrity("x".into()).ui_code(), "integrity");
        assert_eq!(LanBeamError::Io("x".into()).ui_code(), "io");
        assert_eq!(LanBeamError::Bind("x".into()).ui_code(), "io");
        assert_eq!(LanBeamError::Keyring("x".into()).ui_code(), "io");
        assert_eq!(LanBeamError::Handshake("x".into()).ui_code(), "protocol");
        assert_eq!(LanBeamError::IdentityMismatch.ui_code(), "protocol");
        assert_eq!(LanBeamError::PeerNotFound.ui_code(), "protocol");
        assert_eq!(LanBeamError::Crypto("x".into()).ui_code(), "protocol");
        assert_eq!(LanBeamError::Protocol("x".into()).ui_code(), "protocol");
        assert_eq!(
            LanBeamError::Unreachable("x".into()).ui_code(),
            "unreachable"
        );
        // Pairing-only kinds never ride a transfer_error event; they fall into
        // the generic bucket like every other "conversation broke" kind.
        assert_eq!(LanBeamError::PairRejected("x".into()).ui_code(), "protocol");
        assert_eq!(
            LanBeamError::InvalidAddress("x".into()).ui_code(),
            "protocol"
        );
        assert_eq!(LanBeamError::PairCodeMissing.ui_code(), "protocol");
        assert_eq!(LanBeamError::TextRefused.ui_code(), "text_dropped");
        assert_eq!(
            LanBeamError::ConnectionLost("x".into()).ui_code(),
            "connection_lost"
        );
        assert_eq!(LanBeamError::PeerBusy.ui_code(), "peer_busy");
        assert_eq!(LanBeamError::PeerCancelled.ui_code(), "peer_cancelled");
        assert_eq!(LanBeamError::PeerIntegrity.ui_code(), "peer_integrity");
        assert_eq!(LanBeamError::PeerIo.ui_code(), "peer_io");
        assert_eq!(
            LanBeamError::UnsafePath("x".into()).ui_code(),
            "unsafe_path"
        );
        // Caught before a transfer exists, so they never ride transfer_error;
        // the command error's kind carries them to the UI.
        assert_eq!(LanBeamError::TooManyFiles("x".into()).ui_code(), "protocol");
        assert_eq!(LanBeamError::NoFiles.ui_code(), "protocol");
    }

    #[test]
    fn keeps_partial_only_false_for_integrity() {
        assert!(!LanBeamError::Integrity("bad".into()).keeps_partial());
        assert!(LanBeamError::Timeout.keeps_partial());
        assert!(LanBeamError::Cancelled.keeps_partial());
        assert!(LanBeamError::Io("blip".into()).keeps_partial());
        assert!(LanBeamError::Rejected.keeps_partial());
        assert!(LanBeamError::Protocol("torn".into()).keeps_partial());
        assert!(LanBeamError::PeerTooOld("v0".into()).keeps_partial());
    }

    #[test]
    fn from_io_error_maps_to_io_variant() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: LanBeamError = io.into();
        match &err {
            LanBeamError::Io(msg) => assert!(msg.contains("missing")),
            other => panic!("expected Io, got {other:?}"),
        }
        assert_eq!(err.ui_code(), "io");
    }

    #[test]
    fn serialize_uses_kind_and_message_tags() {
        let json = serde_json::to_value(LanBeamError::Protocol("boom".into())).unwrap();
        assert_eq!(json["kind"], "Protocol");
        assert_eq!(json["message"], "boom");

        let unit = serde_json::to_value(LanBeamError::Timeout).unwrap();
        assert_eq!(unit["kind"], "Timeout");

        // The UI keys its "text too long" message off this exact tag (sendops.ts).
        let long = serde_json::to_value(LanBeamError::TextTooLong).unwrap();
        assert_eq!(long["kind"], "TextTooLong");
    }
}
