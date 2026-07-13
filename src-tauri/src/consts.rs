//! Global constants (single source of truth) — see DESIGN §0.

use std::net::Ipv4Addr;
use std::time::Duration;

pub const PROTO_VERSION: u8 = 1;

// Transfer-layer feature versions (M4.2 `Hello`) — a version space ENTIRELY
// SEPARATE from the discovery `PROTO_VERSION` above. Both are 1-based and happen
// to start at the same value, but they move INDEPENDENTLY: `PROTO_VERSION` rides
// the discovery UDP packet and is dropped on strict inequality (bumping it makes
// every existing peer stop discovering this device), whereas these ride the
// post-handshake `Hello` and are NEGOTIATED to the highest common — an older
// peer degrades to its level instead of dropping. Never let one track the other.

/// Transfer v1 — the M4–M6 baseline: paged manifest, SHA-256 (M6.3), resume
/// (M6.4). Every peer that speaks `Hello` advertises at least this.
pub const TRANSFER_V1: u8 = 1;
/// Transfer v2 — M7: in-band pairing (`PairRequest`/`PairConfirm`) and quick
/// text (`TextSend`). Every v2-only message is GATED on the negotiated version:
/// only ever sent to a peer whose `Hello` advertised 2, so a v1 (M4–M6) peer —
/// which negotiates 1 — never receives one.
pub const TRANSFER_V2: u8 = 2;

// Discovery (UDP)
pub const DISCOVERY_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 61, 17); // admin/local scope
pub const DISCOVERY_PORT: u16 = 51703;
pub const MULTICAST_TTL: u32 = 1; // link-local; do not route
pub const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(2);
pub const PEER_EXPIRY: Duration = Duration::from_secs(8); // ~4 missed announces

// Transfer (TCP)
pub const DEFAULT_TCP_PORT: u16 = 51704;

// App shell (M5.5)
/// The global quick-summon chord: show + focus the main window and open the
/// quick-text panel. One constant so registration and any future settings-page
/// display can never drift apart.
pub const DEFAULT_HOTKEY: &str = "Alt+Space";

// Crypto (Noise)
pub const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s"; // never negotiated
pub const NOISE_PROLOGUE: &[u8] = b"LanBeam/noise/v1"; // bound into the handshake hash / SAS
pub const NOISE_TAG: usize = 16;
pub const MAX_PLAINTEXT: usize = 65519; // 65535 max Noise message − 16 tag

// Pairing (M7.1) — in-band code exchange gated on TRANSFER_V2.

/// How long a pairing code minted by `start_pairing` stays redeemable. Long
/// enough to walk to the other device and type it, short enough that a leaked
/// code (shoulder-surfed, screenshared) stops working on its own.
pub const PAIRING_CODE_TTL: Duration = Duration::from_secs(600); // 10 minutes
/// How many failed `PairRequest`s one source IP may make inside
/// [`PAIR_FAILURE_WINDOW`] before further attempts are dropped WITHOUT even
/// checking the code. Pairing is TOFU, authenticated only by a 6-digit code
/// (10^6 space) plus the out-of-band SAS, so the per-source throttle — not the
/// code's length — is what makes an online brute force infeasible.
pub const MAX_PAIR_FAILURES: u32 = 5;
/// Sliding window the per-source `PairRequest` failure count is measured over.
pub const PAIR_FAILURE_WINDOW: Duration = Duration::from_secs(60);
/// Hard cap on how many source IPs the `PairRequest` failure throttle tracks at
/// once. WHY a cap at all: an entry is normally pruned only when the SAME IP is
/// queried again, so a source that fails once and never returns would leave a
/// permanent entry — a peer cycling through many source addresses could grow the
/// map without bound. Past this cap the least-recently-active sources are evicted
/// (a throttled attacker loses at worst its own stale entries; a real in-progress
/// pairing clears its source on success, so it is never the victim).
pub const MAX_PAIR_TRACKED_IPS: usize = 1024;

// Application framing
pub const KIND_CONTROL: u8 = 0x01; // body = UTF-8 JSON AppMessage
pub const KIND_FILE_CHUNK: u8 = 0x02; // body = index:u32 BE || file bytes
pub const MAX_FILE_DATA: usize = 65514; // 65519 − 1 kind − 4 index

// Quick text (M7.3) — in-band `TextSend` gated on TRANSFER_V2.

/// Upper bound on a quick-text payload, in bytes. WHY 60 KiB and not the raw
/// frame limit: a `TextSend` rides ONE Noise control frame (JSON, capped at
/// [`MAX_PLAINTEXT`]), and the JSON wrapper plus any character escaping inflate
/// the wire size past the bare text length — 60 KiB leaves comfortable headroom
/// so ordinary text always fits a single frame, while still being far more than
/// any "quick note / link" the feature is for. The SENDER rejects longer input
/// up front and the RECEIVER clamps to this to bound memory before emitting.
pub const MAX_TEXT_BYTES: usize = 60 * 1024;

/// How many quick texts one source IP may deliver inside [`TEXT_RATE_WINDOW`]
/// before further texts are dropped. Quick text has NO accept-prompt (unlike the
/// file path), so this per-source cap — not a dialog — is what stops a LAN peer
/// from flooding the inbox / notifications. Generous for the real "send a few
/// notes or links" use the feature is for.
pub const MAX_TEXT_PER_WINDOW: u32 = 10;
/// Sliding window the per-source quick-text rate is measured over.
pub const TEXT_RATE_WINDOW: Duration = Duration::from_secs(60);
/// Hard cap on how many source IPs the quick-text throttle tracks at once — the
/// same bound-the-map rationale as [`MAX_PAIR_TRACKED_IPS`], so a peer cycling
/// source addresses cannot grow the throttle's memory without limit.
pub const MAX_TEXT_TRACKED_IPS: usize = 1024;

// Transfer hardening (M4.5) — every wire wait is bounded so a stalled, dead or
// malicious peer can never pin a socket/task forever, and attacker-declared
// sizes can never force large allocations before any trust decision.

/// Upper bound on manifest entries. The Noise XX handshake accepts ANY static
/// key, so `read_manifest` runs pre-trust: a hostile `file_count` must be
/// rejected before it can size an allocation or flood the accept prompt.
pub const MAX_MANIFEST_FILES: u32 = 10_000;
/// Per-entry byte cap on a manifest name. Matches the sanitizer's whole-path
/// limit (4096), so no name this rejects could ever be accepted later — but it
/// is enforced during the manifest READ, because each control frame can carry
/// a ~65 KB name and entries arrive pre-trust, before `validate_manifest` runs.
pub const MAX_MANIFEST_NAME_BYTES: usize = 4096;
/// Cumulative budget for name bytes across ONE manifest. Bounds the total
/// allocation a stranger can force before any prompt: without it, 10k entries
/// with frame-limit names would materialize ~650 MB in the files Vec.
pub const MAX_MANIFEST_NAME_TOTAL: usize = 4 * 1024 * 1024;
/// Outbound TCP dial deadline. Without it a black-holed peer (stale discovery
/// entry, firewall drop) hangs `send_files`/`connect_device` for the OS TCP
/// timeout, which can be minutes.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Per-frame deadline during the Noise handshake, both roles. A stranger can
/// open a TCP connection to the listener at will; this caps how long each
/// half-finished handshake may occupy its task.
pub const HANDSHAKE_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// Deadline for the prompt-free control preamble: the initiator's hello
/// exchange (M4.2) and the responder's whole opening dialogue (hello +
/// manifest, begin → entries → end). Small control traffic either way;
/// 30s tolerates a slow LAN, not a stall.
pub const MANIFEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Receiver's idle deadline between file chunks. Generous because the SENDER
/// paces the stream (its disk may be slow), but bounded so an over-declared
/// manifest size cannot wedge the `remaining > 0` loop forever.
pub const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Upper bound on how long a `pause` (M6.2) parks the chunk loop before it
/// AUTO-RESUMES (finding-4). WHY a cap at all: pause is session-local with NO
/// on-wire signal, so while one side parks the OTHER stays blocked in its
/// per-chunk wire wait — `recv_frame` (receiver) or `write_msg` (sender), each
/// bounded by [`CHUNK_IDLE_TIMEOUT`]. A park that outlived that deadline would
/// make the peer time out and ABORT the whole transfer, so a >60s pause used to
/// fail SILENTLY (and, on a non-hashed transfer, delete the partial). Capping
/// the park comfortably under `CHUNK_IDLE_TIMEOUT` (a 10s margin absorbs
/// scheduling jitter and the time TCP takes to drain a full send buffer once
/// the peer resumes reading) guarantees the next chunk moves inside the peer's
/// idle window, so neither side ever times out. The cost, documented: pause is
/// time-limited — after this it resumes on its own and the loop emits
/// `transfer_resumed`. A keepalive frame cannot lift the limit for BOTH
/// directions, because the sender is write-only mid-stream and cannot read one
/// (a keepalive can't cross a stalled write without splitting the Noise
/// session) — so a symmetric bound is the framing-safe fix. See
/// `transfer::wait_while_paused`.
pub const MAX_PAUSE_PARK: Duration = Duration::from_secs(50);
/// Sender's wait for the receiver's accept/decline `FileSendReply`. Must
/// exceed the receiver's 120s accept-prompt window so a human answering at
/// the deadline is never misreported as a timeout — only a peer that vanished
/// silently (sleep, power loss, WiFi drop with no RST) hits this, and idle
/// keepalive-free TCP would otherwise never notice.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(150);
/// BASE of the sender's wait for the final `TransferAck` — the receiver may
/// still be flushing/syncing (later: hashing) large files after the last
/// chunk lands. The actual deadline scales with the transfer size at
/// [`ACK_DRAIN_RATE`], capped by [`ACK_TIMEOUT_MAX`] — see
/// `transfer::ack_deadline`.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(120);
/// Ceiling of the size-scaled ack deadline: even a multi-terabyte transfer to
/// glacial media must not pin the send task longer than this (M4.5).
pub const ACK_TIMEOUT_MAX: Duration = Duration::from_secs(900);
/// Assumed worst-case rate (bytes/s) at which the receiver's storage drains
/// buffered writes — a slow USB stick / SD card — used to scale the ack
/// deadline with the transfer size (~20 MB/s).
pub const ACK_DRAIN_RATE: u64 = 20 * 1024 * 1024;
/// Receiver-side periodic `sync_data` interval while writing a file. WHY: on
/// slow media a fast LAN piles up gigabytes of dirty page cache, and a single
/// end-of-file `sync_all` taking minutes outlives the sender's ack deadline —
/// a split-brain where the sender reports timeout for a transfer the receiver
/// completed. Syncing every window keeps the final sync short.
pub const RECEIVE_SYNC_INTERVAL: u64 = 128 * 1024 * 1024;
