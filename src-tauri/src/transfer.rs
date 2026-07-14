//! File transfer over an authenticated Noise channel. Sender streams a paged manifest
//! then chunked file bytes; receiver sanitizes every name, writes under the download
//! root, and acks. DESIGN §1.4, §3.5. App wiring (events/commands) is in M3-B's lib/commands.
#![allow(dead_code)] // some helpers are wired by commands.rs / transport listener

use std::collections::HashSet;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::json;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::oneshot;

use crate::consts::{
    ACK_DRAIN_RATE, ACK_TIMEOUT, ACK_TIMEOUT_MAX, CHUNK_IDLE_TIMEOUT, MANIFEST_TIMEOUT,
    MAX_FILE_DATA, MAX_MANIFEST_FILES, MAX_MANIFEST_NAME_BYTES, MAX_MANIFEST_NAME_TOTAL,
    MAX_PAUSE_PARK, MAX_PLAINTEXT, MAX_TEXT_BYTES, RECEIVE_SYNC_INTERVAL, REPLY_TIMEOUT,
    TRANSFER_V2,
};
use crate::error::{LanBeamError, Result};
use crate::exif;
use crate::partials::{FileIdentity, PartialRecord};
use crate::protocol::{self, AppMessage, FileMeta, Frame, ResumeOffset};
use crate::sanitize::{self, Decision};
use crate::settings::Settings;
use crate::state::{ConflictAction, PendingMap, ReplyDecision, TransferControl, TransferCtlGuard};
use crate::transport::noise::NoiseSession;
use crate::transport::TransportCtx;
use crate::trust;

/// One file queued for sending: its manifest metadata + the local absolute path.
pub struct SendItem {
    pub meta: FileMeta, // meta.name = relative path recreated on the receiver
    pub abs: PathBuf,
}

/// The received file list (assembled from the paged manifest).
pub struct Manifest {
    pub transfer_id: String,
    pub total_size: u64,
    pub files: Vec<FileMeta>,
}

// ── control-frame helpers ─────────────────────────────────────────────
async fn send_control(session: &mut NoiseSession, msg: &AppMessage) -> Result<()> {
    session.write_msg(&protocol::encode_control(msg)?).await
}

async fn recv_frame(session: &mut NoiseSession) -> Result<Frame> {
    let pt = session
        .read_msg()
        .await?
        .ok_or_else(|| LanBeamError::Protocol("peer closed connection".into()))?;
    protocol::decode(&pt)
}

async fn recv_control(session: &mut NoiseSession) -> Result<AppMessage> {
    match recv_frame(session).await? {
        Frame::Control(m) => Ok(m),
        Frame::FileChunk(..) => Err(LanBeamError::Protocol("expected control, got chunk".into())),
    }
}

// ── opening dialogue (Hello/DeviceInfo exchange, M4.2) ────────────────
/// What the post-handshake exchange established about the peer, kept for the
/// session (the receive path uses `name` for the accept prompt; `version` is
/// the gate every post-v1 capability must check before sending new variants).
#[derive(Debug)]
pub struct PeerHello {
    /// Negotiated protocol version — the highest both sides speak. Legacy
    /// peers that never sent a `Hello` are pinned to 1.
    pub version: u8,
    /// The peer's self-reported friendly name (`None` for legacy-v1 peers).
    pub name: Option<String>,
}

/// The friendly name we introduce ourselves with, read at call time — not
/// snapshotted at startup — so a rename reaches the very next session.
pub fn local_device_name(settings: &RwLock<Settings>) -> String {
    settings
        .read()
        .map(|s| s.device_name.clone())
        // Poisoned lock: introducing ourselves with the OS default beats
        // failing the whole session over a display name.
        .unwrap_or_else(|_| crate::settings::default_device_name())
}

/// Our `DeviceInfo`. Platform and app version are process constants; only the
/// name varies, which keeps callers to a single settings read.
fn device_info_msg(name: String) -> AppMessage {
    AppMessage::DeviceInfo {
        name,
        platform: std::env::consts::OS.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Say goodbye explicitly. WHY a dedicated frame instead of just closing:
/// `connect_device`'s identity check must end in a way the responder can tell
/// apart from a crashed peer — a `Bye` is swallowed silently (no prompt, no
/// events), a bare close is an error.
pub async fn send_bye(session: &mut NoiseSession) -> Result<()> {
    send_control(session, &AppMessage::Bye).await
}

// ── pairing wire helpers (M7.1) ───────────────────────────────────────
/// Send a `PairRequest` presenting the pairing `code` (M7.1). v2-gated — the
/// joiner (`join_by_code`) checks the negotiated version before calling this,
/// so a v1 peer is never handed the variant.
pub async fn send_pair_request(session: &mut NoiseSession, code: &str) -> Result<()> {
    send_control(
        session,
        &AppMessage::PairRequest {
            code: code.to_string(),
        },
    )
    .await
}

/// Await the responder's `PairConfirm` (M7.1), bounded like the manifest reply
/// (the host answers immediately — the code IS the authorization, no human
/// prompt on its side). Returns `(accepted, responder name, decline reason)`;
/// the name is clamped at this trust boundary before it can reach the store.
pub async fn recv_pair_confirm(
    session: &mut NoiseSession,
) -> Result<(bool, String, Option<String>)> {
    let msg = match tokio::time::timeout(MANIFEST_TIMEOUT, recv_control(session)).await {
        Ok(res) => res?,
        Err(_) => return Err(LanBeamError::Timeout),
    };
    match msg {
        AppMessage::PairConfirm {
            accept,
            name,
            reason,
            // Clamp `reason` at this trust boundary the same way `name` is
            // (finding): join_by_code surfaces it as the command error the
            // pairing UI renders, so an unbounded peer string must not reach it.
        } => Ok((
            accept,
            trust::clamp_name(&name),
            reason.map(|r| trust::clamp_peer_text(&r)),
        )),
        other => Err(LanBeamError::Protocol(format!(
            "expected pair confirm, got {}",
            trust::clamp_peer_text(&format!("{other:?}"))
        ))),
    }
}

// ── quick-text wire helpers (M7.3) ────────────────────────────────────
/// The tag the quick-text receiver acks with (`Ack { of }`). One constant so the
/// receiver's reply and any future matcher can never drift apart.
const TEXT_ACK: &str = "text";
/// Coarse reasons a quick text did not reach the user, carried on the ack. Stable
/// tokens the sender maps to a typed error — never shown raw.
const TEXT_DROP_UNTRUSTED: &str = "untrusted";
const TEXT_DROP_THROTTLED: &str = "throttled";

/// Push one quick text to a peer over an already-open session (M7.3). v2-gated —
/// the `send_text` command checks the negotiated version before calling this, so
/// a v1 peer is never handed the variant. `to_clipboard` rides along as the
/// SENDER's request ("also put this on your clipboard"); whether it actually
/// lands there is the receiver's call, gated on ITS `clip_share` consent.
///
/// Bounded end-to-end (M4.5): the write-then-await-ack round-trip is small
/// control traffic, so it runs under one deadline — a peer that takes the bytes
/// but never acks (crash, hung app) cannot pin this task. Refuses up front if the
/// encoded frame would overflow one Noise message, so an over-long text fails
/// cleanly here rather than deep inside `write_msg`.
pub async fn send_text(
    session: &mut NoiseSession,
    text: String,
    to_clipboard: Option<bool>,
) -> Result<()> {
    let frame = protocol::encode_control(&AppMessage::TextSend { text, to_clipboard })?;
    if frame.len() > MAX_PLAINTEXT {
        return Err(LanBeamError::Protocol(
            "quick text is too large to send in one frame".into(),
        ));
    }
    let acked = tokio::time::timeout(MANIFEST_TIMEOUT, async {
        session.write_msg(&frame).await?;
        recv_control(session).await
    })
    .await;
    match acked {
        // An ack that says it did NOT deliver is not a success. This function's
        // whole contract is "resolves only once the peer confirms it received it"
        // — and the peer drops a stranger's text (there is no prompt to park it
        // in). It used to ack that drop anyway, so the sender reported success for
        // a message that was thrown away on arrival.
        Ok(Ok(AppMessage::Ack {
            delivered: Some(false),
            reason,
            ..
        })) => Err(match reason.as_deref() {
            Some(TEXT_DROP_THROTTLED) => LanBeamError::TextThrottled,
            _ => LanBeamError::TextRefused,
        }),
        // Delivered — or a peer too old to tell us either way, where an ack meant
        // exactly this.
        Ok(Ok(AppMessage::Ack { .. })) => Ok(()),
        Ok(Ok(other)) => Err(LanBeamError::Protocol(format!(
            "expected text ack, got {}",
            trust::clamp_peer_text(&format!("{other:?}"))
        ))),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(LanBeamError::Timeout),
    }
}

/// Initiator side of the opening dialogue: introduce ourselves (`Hello` +
/// `DeviceInfo`), then read the responder's introduction back.
///
/// Every send/receive here runs before any prompt, so the whole exchange is
/// bounded like the manifest read (M4.5).
pub async fn initiator_hello(session: &mut NoiseSession, my_name: String) -> Result<PeerHello> {
    match tokio::time::timeout(MANIFEST_TIMEOUT, initiator_hello_inner(session, my_name)).await {
        Ok(res) => res,
        Err(_) => Err(LanBeamError::Timeout),
    }
}

async fn initiator_hello_inner(session: &mut NoiseSession, my_name: String) -> Result<PeerHello> {
    // Send both introduction frames, then wait for the peer's first reply.
    // WHY the error mapping: a v1 responder expects the manifest as the FIRST
    // control message, so our `Hello` makes it error out and drop the link —
    // anywhere in this stretch, a broken/closed channel means "the peer does
    // not speak the version handshake", which the UI must present as "update
    // the other device" (code `peer_too_old`), not as a generic failure.
    let first: Result<AppMessage> = async {
        send_control(
            session,
            &AppMessage::Hello {
                versions: protocol::SUPPORTED_VERSIONS.to_vec(),
            },
        )
        .await?;
        send_control(session, &device_info_msg(my_name)).await?;
        recv_control(session).await
    }
    .await;
    let hello = first.map_err(|e| match e {
        LanBeamError::Io(_) | LanBeamError::Protocol(_) => {
            LanBeamError::PeerTooOld(format!("peer dropped the version handshake ({e})"))
        }
        other => other,
    })?;
    let version = match hello {
        AppMessage::Hello { versions } => {
            protocol::negotiate_version(&versions).ok_or_else(|| {
                LanBeamError::PeerTooOld(format!(
                    "no common protocol version (peer speaks {versions:?})"
                ))
            })?
        }
        other => {
            return Err(LanBeamError::Protocol(format!(
                "expected hello, got {}",
                trust::clamp_peer_text(&format!("{other:?}"))
            )))
        }
    };
    // Past the Hello the peer provably speaks the new dialogue — from here on,
    // an unexpected message is a genuine protocol error, not an old build.
    let name = match recv_control(session).await? {
        AppMessage::DeviceInfo {
            name,
            platform,
            app_version,
        } => {
            // Clamp the peer's self-declared name at the trust boundary (M5.4):
            // the XX handshake accepts any static key, so this runs pre-trust —
            // an unbounded name would otherwise flow into the notification
            // title, event payloads, and logs. Same rule as `trust::clamp_name`.
            let name = trust::clamp_name(&name);
            log::debug!("peer introduced itself: {name:?} ({platform}, app {app_version})");
            Some(name)
        }
        other => {
            return Err(LanBeamError::Protocol(format!(
                "expected device info, got {}",
                trust::clamp_peer_text(&format!("{other:?}"))
            )))
        }
    };
    Ok(PeerHello { version, name })
}

/// Outcome of the responder-side opening dialogue.
pub enum InboundOpen {
    /// The peer is sending files — the manifest is fully read.
    Files { manifest: Manifest, peer: PeerHello },
    /// The peer redeemed a pairing code (M7.1): its `PairRequest.code`, carried
    /// up so `handle_incoming` can match it against the active pairing session,
    /// mutually trust on success, and reply `PairConfirm`. v2-gated — a peer
    /// that negotiated v1 never reaches this arm.
    Pair { code: String, peer: PeerHello },
    /// The peer pushed a quick text (M7.3): its `TextSend` payload, carried up so
    /// `handle_incoming` can emit `text_received`, optionally mirror it to the
    /// clipboard, and ack. v2-gated — a peer that negotiated v1 never reaches
    /// this arm (the dispatch refuses its premature/forged variant).
    Text {
        text: String,
        to_clipboard: Option<bool>,
        peer: PeerHello,
    },
    /// The peer only wanted the identity/SAS round-trip (`connect_device`)
    /// and said `Bye` — close cleanly: no error, no prompt, no events.
    Bye,
}

/// Responder side of the opening dialogue. Tolerates BOTH generations: a
/// legacy v1 peer opens with `FileListBegin` (it never sends `Hello`), a
/// current peer introduces itself first. WHY keep the legacy path: pre-Hello
/// builds already exist on our own devices, and receiving from them must keep
/// working even though this build always initiates with `Hello`.
pub async fn open_inbound(session: &mut NoiseSession, my_name: String) -> Result<InboundOpen> {
    let peer;
    let opening = match recv_control(session).await? {
        // Legacy peer: the first control frame is already the manifest head.
        first @ AppMessage::FileListBegin { .. } => {
            peer = PeerHello {
                version: 1,
                name: None,
            };
            first
        }
        AppMessage::Hello { versions } => {
            let version = protocol::negotiate_version(&versions).ok_or_else(|| {
                LanBeamError::PeerTooOld(format!(
                    "no common protocol version (peer speaks {versions:?})"
                ))
            })?;
            let name = match recv_control(session).await? {
                AppMessage::DeviceInfo {
                    name,
                    platform,
                    app_version,
                } => {
                    // Clamp the peer's self-declared name at the trust boundary
                    // (M5.4): this runs pre-trust (XX accepts any static key), so
                    // bound it here before it reaches the accept prompt, the OS
                    // notification title, the event payload, and the logs. Same
                    // rule as `trust::clamp_name`.
                    let name = trust::clamp_name(&name);
                    log::debug!("peer introduced itself: {name:?} ({platform}, app {app_version})");
                    Some(name)
                }
                other => {
                    return Err(LanBeamError::Protocol(format!(
                        "expected device info, got {}",
                        trust::clamp_peer_text(&format!("{other:?}"))
                    )))
                }
            };
            // Introduce ourselves back, then the dialogue proper begins.
            send_control(
                session,
                &AppMessage::Hello {
                    versions: protocol::SUPPORTED_VERSIONS.to_vec(),
                },
            )
            .await?;
            send_control(session, &device_info_msg(my_name)).await?;
            peer = PeerHello { version, name };
            recv_control(session).await?
        }
        other => {
            return Err(LanBeamError::Protocol(format!(
                "expected hello or manifest, got {}",
                trust::clamp_peer_text(&format!("{other:?}"))
            )))
        }
    };
    // Post-Hello dispatch: the peer's FIRST control message after the opening
    // dialogue selects the session kind. `PairRequest` (M7.1) and `TextSend`
    // (M7.3) are gated on the negotiated `peer.version >= TRANSFER_V2` — a peer
    // pinned to v1 was never offered either capability, so its (forged/premature)
    // variant is refused rather than acted on. For ANY v2 variant arriving from a
    // peer that (wrongly) negotiated v1, the arm returns a clean `Protocol` error:
    // the Debug format never panics, so a malformed or premature first message can
    // only fail this one session, never the listener.
    match opening {
        AppMessage::FileListBegin {
            transfer_id,
            total_size,
            file_count,
        } => {
            let manifest =
                read_manifest_entries(session, transfer_id, total_size, file_count).await?;
            Ok(InboundOpen::Files { manifest, peer })
        }
        AppMessage::PairRequest { code } => {
            if peer.version < TRANSFER_V2 {
                return Err(LanBeamError::Protocol(
                    "pair request from a peer that negotiated v1".into(),
                ));
            }
            Ok(InboundOpen::Pair { code, peer })
        }
        AppMessage::TextSend { text, to_clipboard } => {
            if peer.version < TRANSFER_V2 {
                return Err(LanBeamError::Protocol(
                    "text send from a peer that negotiated v1".into(),
                ));
            }
            Ok(InboundOpen::Text {
                text,
                to_clipboard,
                peer,
            })
        }
        AppMessage::Bye => Ok(InboundOpen::Bye),
        other => Err(LanBeamError::Protocol(format!(
            "expected manifest or bye, got {}",
            trust::clamp_peer_text(&format!("{other:?}"))
        ))),
    }
}

// ── build the send list from user-selected paths ──────────────────────
/// Enumerate the files under `paths`, producing one [`SendItem`] each. When
/// `verify_hash` is on (M6.3), every file is additionally SHA-256'd here (an
/// extra full read) so its digest can ride the manifest that precedes the
/// bytes. WHY the caller must run this on the blocking pool: with hashing on,
/// this reads every byte of every file, which must never occupy an async worker.
///
/// No metadata stripping (M9.1) — this is the stable entry for direct callers
/// (tests, internal). The send command uses [`build_send_list_scoped`], which
/// additionally scrubs image metadata into temp copies.
pub fn build_send_list(paths: &[PathBuf], verify_hash: bool) -> Result<Vec<SendItem>> {
    // Delegate to the strip-aware builder with stripping OFF: with `strip_exif`
    // false, `build_item` skips its whole strip block and falls straight through
    // to `item`, and the scratch base is never created or touched — so every
    // returned item is byte-identical to walking with `item` directly. One
    // enumeration to maintain instead of a parallel copy.
    build_send_list_scoped(paths, verify_hash, false, std::path::PathBuf::new())
        .map(|(items, _)| items)
}

/// The strip-aware send-list builder (M9.1): the same enumeration as
/// [`build_send_list`], but when `strip_exif` is on, every JPEG/PNG/WebP is
/// scrubbed of EXIF/ICC/XMP into a temp copy under `scratch_base`, and that copy
/// — not the original — is what the returned item points at, with `size` and
/// (when `verify_hash`) `sha256` computed over the CLEANED bytes so the manifest
/// matches exactly what streams. Non-images, images we can't parse, files over
/// [`exif::MAX_STRIP_BYTES`], and the whole strip-off path are byte-identical to
/// [`build_send_list`].
///
/// Returns the items plus a [`SendScratch`] guard the caller MUST hold until the
/// send finishes: dropping it deletes every temp copy on success OR failure. WHY
/// the caller runs this on the blocking pool — like [`build_send_list`] it reads
/// every file, and now also writes cleaned copies, which must never occupy an
/// async worker.
pub fn build_send_list_scoped(
    paths: &[PathBuf],
    verify_hash: bool,
    strip_exif: bool,
    scratch_base: PathBuf,
) -> Result<(Vec<SendItem>, SendScratch)> {
    let mut ctx = StripCtx::new(strip_exif, scratch_base);
    let mut out = Vec::new();
    for p in paths {
        let meta = std::fs::metadata(p)?;
        if meta.is_file() {
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            out.push(build_item(name, p.clone(), &meta, verify_hash, &mut ctx)?);
        } else if meta.is_dir() {
            let base = p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "folder".into());
            walk_dir_scoped(p, &base, &mut out, verify_hash, &mut ctx)?;
        }
    }
    Ok((out, ctx.scratch))
}

fn walk_dir_scoped(
    dir: &Path,
    rel_prefix: &str,
    out: &mut Vec<SendItem>,
    verify_hash: bool,
    ctx: &mut StripCtx,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = format!("{rel_prefix}/{name}");
        if meta.is_file() {
            out.push(build_item(rel, entry.path(), &meta, verify_hash, ctx)?);
        } else if meta.is_dir() {
            walk_dir_scoped(&entry.path(), &rel, out, verify_hash, ctx)?;
        }
    }
    Ok(())
}

fn item(
    name: String,
    abs: PathBuf,
    meta: &std::fs::Metadata,
    verify_hash: bool,
) -> Result<SendItem> {
    // Precompute the content hash in this same (blocking) pass when the user
    // asked for integrity verification (M6.3). WHY hash here and not fold it
    // during send: the manifest carrying the digest is sent BEFORE any file
    // bytes, so the hash must already exist — that costs one extra full read of
    // every file on the sender, the price of the guarantee. A file mutated
    // between this read and the send simply fails the receiver's check, which
    // is the correct outcome (its bytes no longer match what was promised).
    let sha256 = if verify_hash {
        Some(hash_file(&abs)?)
    } else {
        None
    };
    Ok(SendItem {
        meta: FileMeta {
            name,
            size: meta.len(),
            mtime: mtime_secs(meta),
            mode: 0,
            sha256,
        },
        abs,
    })
}

/// Seconds-since-epoch mtime for `meta`, `0` when the platform can't report it.
/// The manifest `mtime` field, shared by both send-list builders.
fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── EXIF/metadata stripping on send (M9.1) ────────────────────────────
/// Per-send scratch directory for metadata-stripped temp copies (M9.1). Its
/// whole subtree is deleted on drop, so the cleaned copies of the user's photos
/// are removed on EVERY exit path of the send — success, error, or cancel — and
/// a temp copy of a private photo never lingers on disk. Created LAZILY: the
/// directory only appears the first time a file is actually stripped, so a send
/// with nothing to strip leaves nothing behind.
///
/// Ownership: `send_files` holds this guard for the whole `run_send` and drops
/// it when the command returns (all exit paths). If `build_send_list_scoped`
/// itself errors partway, the partially-populated guard is a local there and
/// drops on the `?`, still cleaning up every temp already written.
pub struct SendScratch {
    /// The scratch directory once created; `None` until the first temp is written.
    dir: Option<PathBuf>,
}

impl SendScratch {
    /// A guard owning no directory — the strip-off / nothing-stripped case.
    fn empty() -> Self {
        Self { dir: None }
    }

    /// The scratch directory, if temp copies were written. Observability for
    /// tests asserting the temps are removed once the guard drops.
    #[doc(hidden)]
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }
}

impl Drop for SendScratch {
    fn drop(&mut self) {
        if let Some(dir) = self.dir.take() {
            // Best-effort: a Drop must not panic. The dir is app-private scratch
            // (never the download root), so a rare leftover from a locked/vanished
            // file is harmless and swept with the app cache later.
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// Threads the strip decision, the lazily-created scratch dir, and a per-file
/// counter through the send-list walk (M9.1).
struct StripCtx {
    strip_exif: bool,
    /// The per-transfer scratch directory to create on first use.
    base: PathBuf,
    /// The RAII guard accumulating whatever directory gets created.
    scratch: SendScratch,
    /// Monotonic index for unique temp file names within the scratch dir.
    counter: u32,
}

impl StripCtx {
    fn new(strip_exif: bool, base: PathBuf) -> Self {
        Self {
            strip_exif,
            base,
            scratch: SendScratch::empty(),
            counter: 0,
        }
    }

    /// Write `bytes` to a fresh temp file in the scratch dir (creating the dir on
    /// first call) and return its path. The name is index-based on purpose: the
    /// receiver names the file from the manifest, never from this path, so the
    /// temp's own name and extension are internal only.
    fn write_temp(&mut self, bytes: &[u8], ext: &str) -> Result<PathBuf> {
        if self.scratch.dir.is_none() {
            std::fs::create_dir_all(&self.base)?;
            // On Unix, lock the scratch dir to the owner (0700). The temp copies
            // are plaintext of the user's private photos, and the app-cache /tmp
            // fallback (commands.rs) can land in a world-traversable directory —
            // 0700 keeps another local user from reading them during the send.
            // No-op on Windows, where %LOCALAPPDATA%/%TEMP% are per-user ACL'd.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&self.base, std::fs::Permissions::from_mode(0o700))?;
            }
            self.scratch.dir = Some(self.base.clone());
        }
        let path = self.base.join(format!("{}.{ext}", self.counter));
        self.counter += 1;
        write_owner_only(&path, bytes)?;
        Ok(path)
    }
}

/// Write `bytes` to `path`, owner-read/write only on Unix (mode 0600) so no other
/// local user can read a stripped copy of a private photo even briefly. On
/// non-Unix this is a plain write — the enclosing scratch dir is already
/// per-user ACL'd there, and Unix mode bits do not apply.
#[cfg(unix)]
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes)?;
    Ok(())
}

/// The lowercased extension of `abs` IF it is a format [`exif::strip_image_metadata`]
/// can attempt — the gate that keeps a non-image (or a HEIC/TIFF/RAW we can't
/// parse) from ever being read into memory just to have the strip decline it.
fn image_strip_ext(abs: &Path) -> Option<String> {
    let ext = abs.extension()?.to_str()?.to_ascii_lowercase();
    exif::is_strippable_ext(&ext).then_some(ext)
}

/// The outcome of attempting a strip on one file (M9.1), carrying the bytes it
/// read so the caller never re-reads the file just to hash it.
enum StripOutcome {
    /// Metadata was actually removed — these CLEANED bytes (distinct from the
    /// file on disk) are what will stream; `size`/`sha256` come from them.
    Cleaned(Vec<u8>),
    /// The file was read but left unchanged (no metadata to remove, or an
    /// unparseable/passthrough image). The ORIGINAL file still streams, but its
    /// already-read bytes ride along so the caller hashes THEM in memory instead
    /// of reading the whole file a second time.
    Unchanged(Vec<u8>),
    /// The file could not be read here at all — defer to the normal [`item`]
    /// path, which reads it (for the hash) and surfaces any error there.
    Unreadable,
}

/// Read `abs` and attempt a metadata strip (M9.1), distinguishing a real strip
/// (needs a cleaned temp copy) from an unchanged passthrough (the original
/// streams) while keeping the bytes it read. A metadata-free image re-encodes
/// byte-identical: returning [`StripOutcome::Unchanged`] with the original buffer
/// lets the caller hash it in memory, avoiding a pointless temp copy AND the
/// second full read the old `Option` shape forced. Stripping is best-effort and
/// never fails the send — a read error defers to [`item`]'s own error handling.
fn try_strip(abs: &Path, ext: &str) -> StripOutcome {
    let Ok(original) = std::fs::read(abs) else {
        return StripOutcome::Unreadable;
    };
    match exif::strip_image_metadata(&original, ext) {
        Some(cleaned) if cleaned != original => StripOutcome::Cleaned(cleaned),
        // No metadata removed (or an unparseable image): the original streams.
        _ => StripOutcome::Unchanged(original),
    }
}

/// Build one [`SendItem`], stripping image metadata into a temp copy first when
/// the strip is on and applies (M9.1). On a real strip the item points at the
/// CLEANED temp file with `size` and (when `verify_hash`) `sha256` recomputed
/// over the cleaned bytes — the consistency contract that keeps the receiver's
/// size/hash checks passing and the chunk stream aligned. Otherwise it is
/// exactly [`item`]'s original-file result.
fn build_item(
    name: String,
    abs: PathBuf,
    meta: &std::fs::Metadata,
    verify_hash: bool,
    ctx: &mut StripCtx,
) -> Result<SendItem> {
    if ctx.strip_exif {
        if let Some(ext) = image_strip_ext(&abs) {
            // Skip absurdly large "images" (a video renamed .jpg) rather than
            // slurp them whole into RAM — send them unstripped instead.
            if meta.len() <= exif::MAX_STRIP_BYTES {
                match try_strip(&abs, &ext) {
                    StripOutcome::Cleaned(cleaned) => {
                        let temp = ctx.write_temp(&cleaned, &ext)?;
                        // Hash the CLEANED bytes in memory (no re-read): they are
                        // the bytes that will stream, so they are what the manifest
                        // must promise.
                        let sha256 = verify_hash.then(|| to_hex(&Sha256::digest(&cleaned)));
                        return Ok(SendItem {
                            meta: FileMeta {
                                name,
                                size: cleaned.len() as u64,
                                mtime: mtime_secs(meta),
                                mode: 0,
                                sha256,
                            },
                            abs: temp,
                        });
                    }
                    StripOutcome::Unchanged(original) => {
                        // Nothing to strip: the ORIGINAL file streams (abs stays
                        // put), but we already hold its bytes — hash THEM here
                        // instead of making [`item`] read the whole file again.
                        // `size` is the buffer we hashed, so size+hash stay
                        // consistent with the bytes `run_send` streams from `abs`.
                        let sha256 = verify_hash.then(|| to_hex(&Sha256::digest(&original)));
                        return Ok(SendItem {
                            meta: FileMeta {
                                name,
                                size: original.len() as u64,
                                mtime: mtime_secs(meta),
                                mode: 0,
                                sha256,
                            },
                            abs,
                        });
                    }
                    // Unreadable here — defer to `item` below, which reads the
                    // file and surfaces the error on the normal path.
                    StripOutcome::Unreadable => {}
                }
            }
        }
    }
    // No strip applied — original path, size and (optional) hash unchanged.
    item(name, abs, meta, verify_hash)
}

/// Stream a file through SHA-256 and return the lowercase-hex digest (M6.3).
/// Synchronous std I/O by design: every caller runs inside `spawn_blocking`,
/// so a multi-gigabyte read never occupies an async worker.
fn hash_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// Lowercase-hex encoding of a byte slice — the wire form for `FileMeta.sha256`.
fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

// ── cancel + pause plumbing (M6.1/6.2) ────────────────────────────────
/// Park the chunk loop while its session is paused (M6.2), waking when
/// `resume_transfer` clears the flag; a cancel while parked still ends the
/// session promptly (`Cancelled`). WHY the arm-then-recheck dance: `Notify`
/// only wakes waiters already registered when `notify_waiters` fires, so a
/// resume landing between the flag read and the await would be lost — arming
/// the waiter (`enable`) BEFORE re-reading `paused` closes that race. The
/// common "not paused" case takes a single relaxed-cost load and none of the
/// `Notify` machinery, so the per-chunk overhead is negligible.
///
/// BOUNDED (finding-4): the park is capped at [`MAX_PAUSE_PARK`] and then
/// AUTO-RESUMES. Pause is session-local with no on-wire signal, so while THIS
/// side parks the peer stays blocked in its per-chunk wire wait bounded by
/// `CHUNK_IDLE_TIMEOUT`; a park that outlived that deadline would make the peer
/// time out and abort the transfer (a >60s pause used to fail silently). By
/// resuming a safe margin before the deadline the next chunk always moves
/// inside the peer's idle window, so neither side ever times out. A keepalive
/// frame cannot lift this limit for both directions: the sender is write-only
/// mid-stream and cannot read one (and a keepalive can't cross a stalled write
/// without splitting the Noise session), so the receiver-pause direction would
/// still abort — a symmetric bound is the honest, framing-safe fix.
///
/// Returns `Ok(true)` when the cap forced an auto-resume — the caller emits
/// `transfer_resumed` so the UI clears its optimistic pause flag; `Ok(false)`
/// on the not-paused fast path or a genuine `resume_transfer`.
async fn wait_while_paused(ctl: &TransferControl) -> Result<bool> {
    wait_while_paused_capped(ctl, MAX_PAUSE_PARK).await
}

/// The bounded park, with `cap` split out so tests exercise the auto-resume in
/// real time with a tiny cap instead of the production [`MAX_PAUSE_PARK`].
async fn wait_while_paused_capped(ctl: &TransferControl, cap: Duration) -> Result<bool> {
    // Fast path: the overwhelmingly common case is "not paused".
    if !ctl.paused.load(Ordering::SeqCst) {
        return Ok(false);
    }
    // One deadline for the WHOLE park (spanning any spurious re-arm below), so
    // the cap bounds the total parked time, not each inner wait.
    let cap = tokio::time::sleep(cap);
    tokio::pin!(cap);
    loop {
        // Arm the waiter first so a concurrent resume (store=false → notify)
        // cannot slip through between the check below and the await.
        let notified = ctl.resume_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !ctl.paused.load(Ordering::SeqCst) {
            return Ok(false);
        }
        tokio::select! {
            biased;
            _ = ctl.cancel.cancelled() => return Err(LanBeamError::Cancelled),
            // A genuine resume is preferred over the cap when both are ready.
            _ = notified => {}
            _ = &mut cap => {
                // The cap fired first: auto-resume so the peer's idle timeout
                // never trips. Clear the flag exactly as `resume_transfer`
                // would (store=false, then wake any other waiter) and tell the
                // caller so it can surface the auto-resume to the UI.
                ctl.paused.store(false, Ordering::SeqCst);
                ctl.resume_notify.notify_waiters();
                return Ok(true);
            }
        }
    }
}

// ── rate limiting + per-file progress (M6.7/6.8) ──────────────────────
/// Per-transfer byte-rate throttle (M6.7). `None` (the `"unlimited"` setting)
/// is a true no-op — `throttle` returns on a single branch, so an unthrottled
/// transfer pays nothing. `Some(bytes_per_sec)` paces the stream with a small
/// token bucket: each chunk spends credit, and when the bucket runs dry the
/// loop sleeps just long enough to refill what it overspent. Banked credit is
/// capped at one second's worth, so a transfer that idles (paused, slow disk)
/// cannot hoard allowance and then blast past the cap on resume.
///
/// Per-transfer, NOT global (documented): two concurrent transfers each get
/// their own bucket, so the aggregate can reach N× the cap. A global limiter
/// would need shared mutable state on every chunk of every transfer; the LAN
/// case this serves (leave headroom for other traffic) is well met per-transfer.
struct RateLimiter {
    /// Bytes per second, or `None` for unlimited.
    rate: Option<u64>,
    /// Spendable byte credit, refilled at `rate` over elapsed time.
    allowance: f64,
    /// When `allowance` was last refilled.
    last: Instant,
}

impl RateLimiter {
    /// Build a limiter from a byte-per-second cap (`None` = unlimited).
    fn new(rate: Option<u64>) -> Self {
        Self {
            rate,
            allowance: 0.0,
            last: Instant::now(),
        }
    }

    /// Account for `n` just-transferred bytes and, if over the cap, sleep to
    /// pace the stream back down to it. A no-op when unlimited. The sleep is
    /// bounded by roughly one chunk's worth of time (chunk / rate), so it adds
    /// at most that to cancel latency — negligible even at low caps.
    async fn throttle(&mut self, n: usize) {
        let Some(rate) = self.rate else { return };
        let rate = rate as f64;
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        // Refill, capping banked credit at one second's worth.
        self.allowance = (self.allowance + elapsed * rate).min(rate);
        self.allowance -= n as f64;
        if self.allowance < 0.0 {
            let deficit = -self.allowance;
            tokio::time::sleep(Duration::from_secs_f64(deficit / rate)).await;
            // The sleep paid the debt; start the next window from zero.
            self.allowance = 0.0;
        }
    }
}

/// Events the send/receive chunk loops hand to their `on_file` sink (M6.8):
/// per-file progress + completion, plus one transfer-level pause signal. The
/// caller turns these into `transfer_file_progress` / `transfer_file_done` /
/// `transfer_resumed` events; tests and the pre-M6.8 internal entry points pass
/// a no-op sink (`&mut |_| {}`). WHY the pause signal rides this same sink: it
/// already carries the session id + app handle to both loops, so reusing it
/// avoids threading a second emit path (and a fourth closure) through
/// `run_send`/`run_receive_core`.
pub enum FileEvent<'a> {
    /// `done` of `total` bytes delivered for manifest file `index`. `name` is
    /// the sender-declared leaf/rel path (the receiver's own on-disk name may
    /// differ after de-dupe; the UI keys the per-file row by `index`).
    Streaming {
        index: usize,
        name: &'a str,
        done: u64,
        total: u64,
    },
    /// File `index` finished. `verified` is the SHA-256 outcome (M6.3): on the
    /// RECEIVE side `true` means a digest was present AND matched (a mismatch
    /// never reaches here — it errors the transfer first); on the SEND side it
    /// means a digest was ATTACHED (the peer will verify it). Both reduce to
    /// "this file carried a verified hash" — exactly `sha256.is_some()` at a
    /// successful completion.
    Done { index: usize, verified: bool },
    /// The session's pause hit its [`MAX_PAUSE_PARK`] cap and AUTO-RESUMED
    /// (finding-4) — pause is bounded so it can never outlive the peer's idle
    /// timeout. NOT per-file: the sink emits `transfer_resumed` so the UI clears
    /// its optimistic `paused` flag (there is no other backend pause event).
    PauseAutoResumed,
}

/// Build the per-file event sink (M6.8) shared by `send_files` and
/// `handle_incoming`: emits `transfer_file_progress` — throttled to a
/// whole-percent change PER FILE, mirroring [`emit_progress`]'s aggregate
/// throttle — and `transfer_file_done`. Returned as a closure for
/// `run_send`/`run_receive_core`'s `on_file` parameter.
pub fn file_event_sink<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> impl FnMut(FileEvent<'_>) {
    // Track the last emitted percent for the CURRENT file; reset when the index
    // advances so each file's bar starts fresh at 0.
    let mut last_index = usize::MAX;
    let mut last_pct = -1i64;
    move |event| match event {
        FileEvent::Streaming {
            index,
            name,
            done,
            total,
        } => {
            if index != last_index {
                last_index = index;
                last_pct = -1;
            }
            let pct = if total == 0 {
                100
            } else {
                ((done as u128 * 100) / total as u128) as i64
            };
            if pct != last_pct {
                last_pct = pct;
                let _ = app.emit(
                    "transfer_file_progress",
                    json!({
                        "sessionId": session_id,
                        "fileIndex": index,
                        "fileName": name,
                        "done": done,
                        "total": total,
                        "percent": pct,
                    }),
                );
            }
        }
        FileEvent::Done { index, verified } => {
            let _ = app.emit(
                "transfer_file_done",
                json!({ "sessionId": session_id, "fileIndex": index, "verified": verified }),
            );
        }
        FileEvent::PauseAutoResumed => {
            // Pause is time-limited (finding-4): the loop resumed on its own so
            // the peer's idle timeout could never fire. Tell the UI so its
            // optimistic pause flag clears instead of lying about a paused row
            // whose bytes are moving again.
            let _ = app.emit(
                "transfer_resumed",
                json!({ "sessionId": session_id, "reason": "pause_timeout" }),
            );
        }
    }
}

// ── send side ─────────────────────────────────────────────────────────
/// Send the manifest, await acceptance, then stream every file chunked.
///
/// `ctl` carries the session's cancel token + pause flag (M6.1/6.2): each
/// chunk parks while paused and a cancel ends the loop promptly with
/// `Cancelled` (the caller then drops the session, so the receiver aborts too).
///
/// `rate_limit` is an optional byte-per-second cap (M6.7, `None` = unlimited),
/// and `on_file` receives a [`FileEvent`] as each file streams and finishes
/// (M6.8) — both no-ops for callers that pass `None`/`&mut |_| {}`.
pub async fn run_send<F: FnMut(u64, u64)>(
    session: &mut NoiseSession,
    transfer_id: &str,
    items: &[SendItem],
    ctl: &TransferControl,
    rate_limit: Option<u64>,
    // `+ Send`: `handle_incoming`'s receive task and the async send command both
    // hold this across `.await`, so the boxed sink must be Send for the future.
    on_file: &mut (dyn FnMut(FileEvent<'_>) + Send),
    mut on_progress: F,
) -> Result<()> {
    let total_size: u64 = items.iter().map(|i| i.meta.size).sum();
    let mut limiter = RateLimiter::new(rate_limit);

    // The manifest preamble is small control traffic; bound it as ONE unit —
    // mirroring the receiver, which reads the whole opening under this same
    // deadline — so a peer that stops reading cannot wedge these writes on
    // TCP backpressure (M4.5).
    let preamble = async {
        send_control(
            session,
            &AppMessage::FileListBegin {
                transfer_id: transfer_id.to_string(),
                total_size,
                file_count: items.len() as u32,
            },
        )
        .await?;
        for it in items {
            send_control(
                session,
                &AppMessage::FileListEntry {
                    file: it.meta.clone(),
                },
            )
            .await?;
        }
        send_control(
            session,
            &AppMessage::FileListEnd {
                transfer_id: transfer_id.to_string(),
            },
        )
        .await
    };
    // Cancel wins over the wait (finding), same as the chunk loop below: a user
    // who cancels while this is parked must free the ConcurrencyGate slot at
    // once, not after the peer finally answers. The cancel arm returns and the
    // caller drops `session`, so nothing reuses the mutable borrow afterwards.
    tokio::select! {
        biased;
        _ = ctl.cancel.cancelled() => return Err(LanBeamError::Cancelled),
        r = tokio::time::timeout(MANIFEST_TIMEOUT, preamble) => match r {
            Ok(res) => res?,
            Err(_) => return Err(LanBeamError::Timeout),
        }
    }

    // Bounded reply wait (M4.5): a live receiver ALWAYS answers — its accept
    // prompt auto-declines at 120s — so only a peer that vanished silently
    // (sleep, power loss, WiFi drop with no RST) leaves this read pending,
    // and idle keepalive-free TCP would never notice on its own. NOT
    // ACK_TIMEOUT: that equals the prompt window exactly, so a user accepting
    // at the deadline would race the sender's timeout.
    // Cancel wins over the reply wait (finding): without this a cancel issued
    // while the receiver's accept prompt is up is not observed until the first
    // chunk after the peer answers — up to REPLY_TIMEOUT (150s) with the slot
    // held. Mirrors the chunk loop's biased select.
    let reply = tokio::select! {
        biased;
        _ = ctl.cancel.cancelled() => return Err(LanBeamError::Cancelled),
        r = tokio::time::timeout(REPLY_TIMEOUT, recv_control(session)) => match r {
            Ok(res) => res?,
            Err(_) => return Err(LanBeamError::Timeout),
        }
    };
    // Per-file resume offsets (M6.4): a receiver that holds a verified prefix of
    // some files asks us to stream only the tail. `None`/absent (legacy receiver
    // or a fresh transfer) means "start every file at 0". The whole-file SHA-256
    // already rode the manifest (build_send_list hashes the WHOLE file), so the
    // receiver verifies old-bytes + tail — nothing to re-hash here.
    let sparse = match reply {
        AppMessage::FileSendReply {
            accept: true,
            offsets,
            ..
        } => offsets.unwrap_or_default(),
        AppMessage::FileSendReply { accept: false, .. } => {
            return Err(LanBeamError::Rejected);
        }
        other => {
            return Err(LanBeamError::Protocol(format!(
                "expected reply, got {}",
                trust::clamp_peer_text(&format!("{other:?}"))
            )))
        }
    };
    // Densify the SPARSE reply into a per-file offset vector. Guard every pair:
    // an out-of-range index (a malformed/oversized reply) is ignored, and each
    // offset is clamped to its file size so a hostile value can only skip within
    // the file, never seek past its end. A duplicate index keeps the last pair.
    let mut offsets = vec![0u64; items.len()];
    for ResumeOffset { index, offset } in sparse {
        if let Some(slot) = offsets.get_mut(index as usize) {
            *slot = offset.min(items[index as usize].meta.size);
        }
    }

    // Seed progress with the bytes the receiver already holds so the bar reflects
    // total delivered, not just what THIS session streams.
    let mut sent: u64 = offsets.iter().sum::<u64>().min(total_size);
    // Per-chunk write deadline. On a RESUMED transfer the receiver re-reads and
    // SHA-256-folds each file's existing prefix from disk BEFORE it reads the
    // tail off the socket, so it stops draining this loop's writes for the
    // fold's duration; on slow media a multi-GB prefix can outlast the fixed
    // CHUNK_IDLE_TIMEOUT (60s), timing out the send on every retry so the file
    // can never complete (finding). Grant extra time scaled by the total
    // resumed bytes at the same drain rate as `ack_deadline`, capped identically
    // so a genuinely dead receiver is still bounded. Because TCP pipelining
    // hides WHICH write races the fold (a small tail may push it onto a later
    // file's chunk), the allowance covers every chunk of the transfer, not just
    // the resumed file's first. For a fresh send or a legacy peer every offset
    // is 0, so this is exactly CHUNK_IDLE_TIMEOUT — no behavior change.
    let resume_grace = Duration::from_secs(offsets.iter().sum::<u64>() / ACK_DRAIN_RATE);
    let chunk_write_deadline = (CHUNK_IDLE_TIMEOUT + resume_grace).min(ACK_TIMEOUT_MAX);
    let mut buf = vec![0u8; MAX_FILE_DATA];
    for (i, it) in items.iter().enumerate() {
        // Already clamped to the file size when densified above.
        let offset = offsets[i];
        // Stage disk reads through a 1 MiB BufReader so the loop pulls ~16 wire
        // chunks per blocking-pool hop instead of one hop per 64 KiB frame — a
        // measurable win on 10 GbE where the per-chunk round-trip no longer
        // hides behind the wire (finding). The seek below discards the (empty)
        // buffer harmlessly; it runs once per file, before any read.
        let mut f =
            tokio::io::BufReader::with_capacity(1 << 20, tokio::fs::File::open(&it.abs).await?);
        if offset > 0 {
            f.seek(SeekFrom::Start(offset)).await?;
        }
        let mut index = 0u32;
        // Per-file byte counter for the M6.8 progress event; seeded at the
        // resume offset so a resumed file's bar reflects total delivered.
        let mut file_sent = offset;
        // Stream EXACTLY the manifest's declared tail (`meta.size - offset`), NOT
        // the file's live byte count (finding-0). The receiver slices the chunk
        // stream by `meta.size` alone with no per-file end marker, so if a file
        // mutated since `build_send_list` changed how many bytes we put on the
        // wire, the drift would bleed across the file boundary: a GROWN file's
        // tail would be consumed as the NEXT file's data, a SHRUNK one would
        // leave the receiver reading on into the next file's frames — corrupting
        // or aborting files that never changed. Bounding here keeps total wire
        // bytes == sum(meta.size) regardless of concurrent mutation, so a changed
        // file fails ONLY its own SHA-256 check, never desyncs its neighbours.
        let mut remaining = it.meta.size.saturating_sub(offset);
        while remaining > 0 {
            // Pause gate (M6.2): park before producing the next chunk so a
            // paused sender simply stops writing → TCP backpressure stalls the
            // peer. A cancel while parked ends the loop promptly. The park is
            // bounded (finding-4): if it auto-resumes at the cap, tell the UI so
            // a >MAX_PAUSE_PARK pause never leaves the receiver idling into an
            // abort — it resumes here first.
            if wait_while_paused(ctl).await? {
                on_file(FileEvent::PauseAutoResumed);
            }
            // Never read past the declared tail: a file that GREW would otherwise
            // leak its extra bytes into the next file's frames.
            let want = remaining.min(buf.len() as u64) as usize;
            let n = f.read(&mut buf[..want]).await?;
            if n == 0 {
                // EOF before `meta.size`: the file SHRANK since the manifest was
                // built. Fail THIS file locally (identified by name) instead of
                // streaming a short count that would pull the next file's frames
                // early and desync every file after it (finding-0).
                return Err(LanBeamError::Io(format!(
                    "{}: file is shorter than its manifest size ({} of {} bytes)",
                    it.meta.name,
                    it.meta.size - remaining,
                    it.meta.size,
                )));
            }
            // Write deadline: a receiver that accepts then stops reading (hung
            // app, zero TCP window) must not wedge this loop on backpressure —
            // the same idle bound the receive side applies between chunks (M4.5).
            // Cancel (M6.1) wins over the write (biased); the caller then drops
            // the session, so the receiver's read fails through its error path.
            let frame = protocol::encode_file_chunk(index, &buf[..n])?;
            tokio::select! {
                biased;
                _ = ctl.cancel.cancelled() => return Err(LanBeamError::Cancelled),
                r = tokio::time::timeout(chunk_write_deadline, session.write_msg(&frame)) => match r {
                    Ok(res) => res?,
                    Err(_) => return Err(LanBeamError::Timeout),
                }
            }
            index += 1;
            sent += n as u64;
            file_sent += n as u64;
            remaining -= n as u64;
            on_progress(sent, total_size);
            on_file(FileEvent::Streaming {
                index: i,
                name: &it.meta.name,
                done: file_sent,
                total: it.meta.size,
            });
            // Pace the write loop after accounting the chunk (M6.7); a no-op
            // when unthrottled.
            limiter.throttle(n).await;
        }
        // This file's bytes are all on the wire (M6.8). `verified` on the send
        // side means an integrity digest was attached — the peer verifies it.
        on_file(FileEvent::Done {
            index: i,
            verified: it.meta.sha256.is_some(),
        });
    }

    // Bounded ack wait: the receiver may still be flushing/syncing large files
    // after the last chunk, so the deadline scales with the transfer size —
    // but a dead peer must not pin this task (and the whole send_files
    // command) forever (M4.5). See `ack_deadline`.
    // Cancel wins over the ack wait too (finding): `ack_deadline` scales to
    // ACK_TIMEOUT_MAX (15 min) for slow media, so a cancel here must not leave
    // the slot pinned that long. Note the semantic edge: every byte is already
    // on the wire, so cancelling now reports Cancelled on this side while the
    // receiver may still verify + save and show success — acceptable, the peer
    // already has the data, and it matches the existing cancel-drops-session
    // behavior.
    let ack = tokio::select! {
        biased;
        _ = ctl.cancel.cancelled() => return Err(LanBeamError::Cancelled),
        r = tokio::time::timeout(ack_deadline(total_size), recv_control(session)) => match r {
            Ok(res) => res?,
            Err(_) => return Err(LanBeamError::Timeout),
        }
    };
    match ack {
        AppMessage::TransferAck { ok: true, .. } => Ok(()),
        AppMessage::TransferAck {
            ok: false, error, ..
        } => Err(LanBeamError::Protocol(format!(
            "receiver error: {}",
            // Clamp the peer-authored reason before it reaches the toast/log
            // (finding): a hostile receiver could otherwise ack with a ~64 KB
            // error string.
            trust::clamp_peer_text(&error.unwrap_or_default())
        ))),
        other => Err(LanBeamError::Protocol(format!(
            "expected ack, got {}",
            trust::clamp_peer_text(&format!("{other:?}"))
        ))),
    }
}

/// The sender's deadline for the final `TransferAck`, scaled with the transfer
/// size. WHY not a fixed 120s: the receiver flushes + syncs every file before
/// acking, and slow media (USB stick, SD card, SMR disk) can take minutes to
/// drain gigabytes of page cache even though the wire was fast — a fixed
/// deadline misreported such transfers as timeouts while the receiver saved
/// everything. Base `ACK_TIMEOUT` plus the drain time at `ACK_DRAIN_RATE`
/// (~20 MB/s), capped at `ACK_TIMEOUT_MAX` (15 min) so a dead peer still
/// cannot pin the send task for long (M4.5).
fn ack_deadline(total_size: u64) -> Duration {
    let drain = Duration::from_secs(total_size / ACK_DRAIN_RATE);
    (ACK_TIMEOUT + drain).min(ACK_TIMEOUT_MAX)
}

// ── receive side ──────────────────────────────────────────────────────
/// Read the paged manifest (FileListBegin → entries → FileListEnd).
pub async fn read_manifest(session: &mut NoiseSession) -> Result<Manifest> {
    match recv_control(session).await? {
        AppMessage::FileListBegin {
            transfer_id,
            total_size,
            file_count,
        } => read_manifest_entries(session, transfer_id, total_size, file_count).await,
        other => Err(LanBeamError::Protocol(format!(
            "expected manifest, got {}",
            trust::clamp_peer_text(&format!("{other:?}"))
        ))),
    }
}

/// The entries half of the manifest read — split out because `open_inbound`
/// has already consumed the `FileListBegin` when it dispatches here (M4.2).
async fn read_manifest_entries(
    session: &mut NoiseSession,
    transfer_id: String,
    total_size: u64,
    file_count: u32,
) -> Result<Manifest> {
    // Bound the peer-controlled transfer_id BEFORE building the manifest, the
    // same way the entry names below are bounded: this runs pre-trust (the XX
    // handshake accepts any static key), and the id is echoed back in the
    // FileSendReply frame, used as registry keys, and carried in every emitted
    // event — an unbounded id would risk overflowing the reply frame and bloat
    // those maps/payloads before any trust decision (M4.5).
    if transfer_id.len() > MAX_MANIFEST_NAME_BYTES {
        return Err(LanBeamError::Protocol(format!(
            "manifest transfer_id is {} bytes (limit {MAX_MANIFEST_NAME_BYTES})",
            transfer_id.len()
        )));
    }
    // Reject a hostile file_count BEFORE any allocation: the XX handshake accepts
    // any static key, so this code runs pre-trust — an attacker-declared count
    // must not size a reservation or flood the accept prompt (M4.5).
    if file_count > MAX_MANIFEST_FILES {
        return Err(LanBeamError::Protocol(format!(
            "manifest declares {file_count} files (limit {MAX_MANIFEST_FILES})"
        )));
    }
    // Capacity is a hint, not a promise: even an in-limit declaration only
    // pre-reserves up to 1024 entries — the Vec grows normally past that.
    let mut files = Vec::with_capacity((file_count as usize).min(1024));
    // The count clamp alone doesn't bound memory: each entry frame can carry a
    // ~65 KB name, so 10k in-limit entries could still materialize ~650 MB
    // pre-trust. Enforce a per-name cap (nothing longer could ever pass the
    // sanitizer anyway) AND a cumulative byte budget across the manifest,
    // both checked BEFORE the entry is kept (M4.5).
    let mut name_bytes = 0usize;
    loop {
        match recv_control(session).await? {
            AppMessage::FileListEntry { file } => {
                if file.name.len() > MAX_MANIFEST_NAME_BYTES {
                    return Err(LanBeamError::Protocol(format!(
                        "manifest entry name is {} bytes (limit {MAX_MANIFEST_NAME_BYTES})",
                        file.name.len()
                    )));
                }
                // Bound the peer-supplied hash too (finding): the name budget
                // caps `file.name` but NOT `file.sha256`, so 10k in-limit entries
                // could each carry a ~64 KB sha256 string and materialize ~640 MB
                // pre-trust — an unauthenticated memory-amplification DoS. A real
                // SHA-256 is exactly 64 lowercase-hex chars, and the receive loop
                // compares against a 64-char computed hex, so anything longer
                // could never verify. A fixed 64-char cap needs no cumulative
                // accounting (M4.5).
                if file.sha256.as_deref().is_some_and(|h| h.len() > 64) {
                    return Err(LanBeamError::Protocol(
                        "manifest entry sha256 exceeds 64 bytes".into(),
                    ));
                }
                name_bytes += file.name.len();
                if name_bytes > MAX_MANIFEST_NAME_TOTAL {
                    return Err(LanBeamError::Protocol(format!(
                        "manifest name bytes exceed budget (limit {MAX_MANIFEST_NAME_TOTAL})"
                    )));
                }
                files.push(file)
            }
            AppMessage::FileListEnd { .. } => break,
            other => {
                return Err(LanBeamError::Protocol(format!(
                    "bad manifest item {}",
                    trust::clamp_peer_text(&format!("{other:?}"))
                )))
            }
        }
        if files.len() > file_count as usize {
            return Err(LanBeamError::Protocol("manifest overran file_count".into()));
        }
    }
    Ok(Manifest {
        transfer_id,
        total_size,
        files,
    })
}

/// Every declared name must pass the pure sanitizer BEFORE we accept the transfer.
pub fn validate_manifest(m: &Manifest) -> Result<()> {
    for f in &m.files {
        if let Decision::Reject(r) = sanitize::validate(&f.name) {
            return Err(LanBeamError::UnsafePath(format!("{}: {:?}", f.name, r)));
        }
    }
    Ok(())
}

/// Send a reply. On reject the transfer ends here. The accept path carries no
/// resume offsets — use [`send_reply_full`] when continuing partials (M6.4).
pub async fn send_reply(
    session: &mut NoiseSession,
    transfer_id: &str,
    accept: bool,
    reason: Option<String>,
) -> Result<()> {
    send_reply_full(session, transfer_id, accept, reason, None).await
}

/// Send a reply with optional SPARSE per-file resume offsets (M6.4): each
/// [`ResumeOffset`] says how many bytes we already hold for one manifest file,
/// so the sender streams only its tail. `None` = start every file at 0 (the
/// send side treats an omitted field identically, keeping legacy peers
/// compatible).
async fn send_reply_full(
    session: &mut NoiseSession,
    transfer_id: &str,
    accept: bool,
    reason: Option<String>,
    offsets: Option<Vec<ResumeOffset>>,
) -> Result<()> {
    send_control(
        session,
        &AppMessage::FileSendReply {
            transfer_id: transfer_id.to_string(),
            accept,
            reason,
            offsets,
        },
    )
    .await
}

/// Per-file receive plan (M6.4/6.5), one entry per manifest file IN ORDER:
/// either continue a persisted partial from an offset, or write a fresh file
/// resolving a name collision with the chosen action.
enum FileDisposition {
    /// Continue the partial at `disk_rel` (relative to the download root) from
    /// `offset` bytes — reopened as-is, its prefix folded back into the hasher.
    Resume { disk_rel: PathBuf, offset: u64 },
    /// A new file at `rel` (auto-organize prefix already applied) — resolve a
    /// collision with `conflict`.
    Fresh {
        rel: PathBuf,
        conflict: ConflictAction,
    },
}

impl FileDisposition {
    /// Bytes the receiver already holds — 0 for a fresh file — which rides
    /// `FileSendReply.offsets` so the sender streams only the tail.
    fn offset(&self) -> u64 {
        match self {
            FileDisposition::Resume { offset, .. } => *offset,
            FileDisposition::Fresh { .. } => 0,
        }
    }
}

/// JSON byte budget for the sparse resume-offset array in a `FileSendReply`
/// (M6.4). Well under the control-frame cap (`MAX_PLAINTEXT` 65519) so the rest
/// of the reply JSON (transfer_id, flags, array punctuation) always fits too.
const RESUME_REPLY_BUDGET: usize = 60_000;

/// Decimal digit count of `n` (`0` → 1) — used to size a resume pair's JSON.
fn dec_digits(mut n: u64) -> usize {
    let mut d = 1;
    while n >= 10 {
        n /= 10;
        d += 1;
    }
    d
}

/// Encode the receiver's per-file resume offsets SPARSELY for the reply (M6.4).
///
/// `eff_offsets[i]` is what file `i` should resume from; only the non-zero
/// entries ride the wire as `(index, offset)` pairs, so a dense O(file_count)
/// vector never overflows the single control frame the reply is (finding: a
/// ~9000-file reply as `Vec<u64>` blew past 65535 and errored the whole
/// transfer, wiping every persisted partial). If even the sparse set would
/// exceed [`RESUME_REPLY_BUDGET`], the overflow files are DROPPED to a fresh
/// start: their `eff_offsets` entry is zeroed IN PLACE — so the receive loop
/// restarts them from 0 in lockstep with what the sender is told — rather than
/// erroring. That costs those files a full re-transfer, never a broken transfer.
fn resume_offsets_for_reply(eff_offsets: &mut [u64]) -> Option<Vec<ResumeOffset>> {
    let mut pairs: Vec<ResumeOffset> = Vec::new();
    let mut used = 0usize;
    for (i, off) in eff_offsets.iter_mut().enumerate() {
        if *off == 0 {
            continue;
        }
        // Worst-case JSON for `{"index":I,"offset":O}` plus a `,` separator:
        // the two keys + braces are 21 fixed bytes, the rest is the digits.
        let cost = 21 + dec_digits(i as u64) + dec_digits(*off);
        if used + cost > RESUME_REPLY_BUDGET {
            // No room — restart this file fresh (mirrored locally by zeroing it).
            *off = 0;
            continue;
        }
        used += cost;
        pairs.push(ResumeOffset {
            index: i as u32,
            offset: *off,
        });
    }
    (!pairs.is_empty()).then_some(pairs)
}

/// One file's write progress through a receive, tracked so the error path can
/// persist a resumable partial (M6.4) or clean up. An entry is pushed when a
/// file is opened; on error the accumulator is PRUNED to exactly the files left
/// on disk, so `handle_incoming` persists precisely those.
struct FileProgress {
    /// Matches a future manifest entry (sanitized rel + size + hash).
    identity: FileIdentity,
    /// On-disk path relative to the download root — reopened verbatim on resume.
    disk_rel: String,
    /// Absolute path, for cleanup of a non-resumable partial.
    path: PathBuf,
    /// Resume is only offered for hashed files; a non-hashed partial is deleted
    /// on interruption exactly as before M6.4.
    hashed: bool,
    bytes_written: u64,
    /// Fully written, synced, and (if hashed) digest-matched. Such a file is
    /// whole, not partial — the error path leaves it on disk instead of deleting
    /// it because a LATER file in the same batch went bad. (Under `overwrite`
    /// "whole" still means "whole, in a temp file": that commit is all-or-nothing
    /// and only happens once the entire transfer succeeds.)
    done: bool,
}

/// Accept + receive with the PRE-M6 semantics: every file fresh, de-dupe on
/// collision, no resume. Kept as the stable entry point for direct callers
/// (unit tests, internal); `handle_incoming` builds a richer plan (auto-organize
/// prefix, conflict policy, resume analysis) and drives [`run_receive_core`].
pub async fn run_receive<F: FnMut(u64, u64)>(
    session: &mut NoiseSession,
    manifest: &Manifest,
    download_root: &Path,
    ctl: &TransferControl,
    on_progress: F,
) -> Result<Vec<PathBuf>> {
    let mut plan = Vec::with_capacity(manifest.files.len());
    for f in &manifest.files {
        match sanitize::validate(&f.name) {
            Decision::Accept(rel) => plan.push(FileDisposition::Fresh {
                rel,
                conflict: ConflictAction::Rename,
            }),
            Decision::Reject(r) => {
                return Err(LanBeamError::UnsafePath(format!("{}: {:?}", f.name, r)))
            }
        }
    }
    let mut progress = Vec::new();
    let mut opened = Vec::new();
    // The pre-M6 entry point: no throttle, no per-file events (production
    // receives go through `run_receive_core` directly from `handle_incoming`).
    run_receive_core(
        session,
        manifest,
        download_root,
        ctl,
        &plan,
        &mut progress,
        &mut opened,
        None,
        &mut |_| {},
        on_progress,
    )
    .await
}

/// The receive engine: reply (with resume `offsets` derived from `plan`), stream
/// each file per its disposition, verify, ack, and return the saved paths.
///
/// `ctl` carries the session's cancel token + pause flag (M6.1/6.2): each chunk
/// parks while paused (no reads → TCP backpressure stalls the sender), and a
/// cancel ends the loop with `Cancelled`.
///
/// `progress` is filled as files are written and, on error, PRUNED to the files
/// still on disk: a hashed file survives an INTERRUPTION (kept for resume, M6.4),
/// while a non-hashed file — or ANY file on a data-integrity failure — is
/// deleted, matching the pre-M6.4 all-or-nothing cleanup. The caller reads the
/// pruned accumulator to persist exactly those partials.
///
/// `opened` is a NON-pruned out-accumulator of every file's identity actually
/// opened this session, in open order. The caller uses it as the `remove` set
/// when clearing stale partial records on the error path — scoped to files this
/// session reached, so a still-valid partial for a file the session never got
/// to is left intact rather than wiped (finding-1).
///
/// `rate_limit` throttles the read loop (M6.7, `None` = unlimited) and `on_file`
/// receives a [`FileEvent`] as each file streams and verifies (M6.8).
#[allow(clippy::too_many_arguments)]
async fn run_receive_core<F: FnMut(u64, u64)>(
    session: &mut NoiseSession,
    manifest: &Manifest,
    download_root: &Path,
    ctl: &TransferControl,
    plan: &[FileDisposition],
    progress: &mut Vec<FileProgress>,
    opened: &mut Vec<FileIdentity>,
    rate_limit: Option<u64>,
    // `+ Send` for the same reason as `run_send`: the receive task is spawned.
    on_file: &mut (dyn FnMut(FileEvent<'_>) + Send),
    mut on_progress: F,
) -> Result<Vec<PathBuf>> {
    // Effective per-file resume offsets, then the SPARSE wire form — which may
    // zero an entry that would overflow the reply frame, restarting that file
    // fresh (see `resume_offsets_for_reply`). The loop below reads `offsets`, so
    // any drop is honored locally in lockstep with what the sender is told.
    let mut offsets: Vec<u64> = plan.iter().map(FileDisposition::offset).collect();
    let sparse = resume_offsets_for_reply(&mut offsets);
    send_reply_full(session, &manifest.transfer_id, true, None, sparse).await?;

    let mut limiter = RateLimiter::new(rate_limit);
    let mut saved = Vec::new();
    // Overwrite-mode files stream into a de-duped temp sibling and are renamed
    // onto the real target only if the WHOLE transfer succeeds (see the Ok arm),
    // so an interruption or integrity failure can only ever delete the temp —
    // never the user's existing file (finding: no up-front unlink).
    let mut overwrite_renames: Vec<(PathBuf, PathBuf)> = Vec::new();
    // Bytes already on disk (resumed prefixes) count as delivered, so the bar
    // resumes where it left off rather than restarting at 0.
    let mut received: u64 = offsets.iter().sum::<u64>().min(manifest.total_size);
    let result: Result<()> = async {
        for (i, f) in manifest.files.iter().enumerate() {
            // The file's stable identity — how a FUTURE manifest matches this
            // partial. Recomputed from the (already-validated) manifest name so
            // it is independent of the auto-organize prefix.
            let identity_rel = match sanitize::validate(&f.name) {
                Decision::Accept(p) => p.to_string_lossy().replace('\\', "/"),
                Decision::Reject(r) => {
                    return Err(LanBeamError::UnsafePath(format!("{}: {:?}", f.name, r)))
                }
            };
            let identity = FileIdentity {
                rel: identity_rel,
                size: f.size,
                sha256: f.sha256.clone(),
            };
            let hashed = f.sha256.is_some();

            // Open per the plan. Resume reopens the exact recorded partial; a
            // fresh file resolves its collision (rename de-dupes, overwrite
            // replaces) — both uphold the sanitizer's containment invariant.
            let (path, disk_rel, std_file, start_offset) = match &plan[i] {
                FileDisposition::Resume {
                    disk_rel,
                    offset: recorded,
                } => {
                    // Use the EFFECTIVE offset (`resume_offsets_for_reply` may have
                    // zeroed it to a fresh restart), not the plan's original, so a
                    // dropped file opens its existing bytes and rewrites from 0 —
                    // matching the 0 the sender was told for it.
                    let offset = offsets[i];
                    let (path, file, actual) =
                        sanitize::resolve_and_open_resumable(download_root, disk_rel, offset)?;
                    // The partial's on-disk length must still equal what analysis
                    // RECORDED. Any drift — a shrink OR a growth — means another
                    // local process touched the file during the accept window: a
                    // shrink can't satisfy the promised offset, and a GROWTH is
                    // worse (finding) — the hasher folds only the first `offset`
                    // bytes and the tail streams into `[offset, size)`, so bytes
                    // past `size` survive UNHASHED yet the whole-file SHA-256 still
                    // passes, silently voiding the M6.3 integrity guarantee. Fail
                    // as an interruption so the kept partial is retried, never
                    // saved oversized-but-"verified". Compare against `recorded`,
                    // NOT the effective `offset`: the zeroed restart above
                    // legitimately reopens a nonzero partial with offset 0, and
                    // there `actual == recorded` still holds.
                    if actual != *recorded {
                        return Err(LanBeamError::Io(format!(
                            "partial {} changed on disk ({actual} bytes) since analysis ({recorded})",
                            path.display()
                        )));
                    }
                    (
                        path,
                        disk_rel.to_string_lossy().replace('\\', "/"),
                        file,
                        offset,
                    )
                }
                FileDisposition::Fresh { rel, conflict } => {
                    let (path, file) = match conflict {
                        ConflictAction::Overwrite => {
                            // Stream into a de-duped temp sibling; the replace of
                            // the real target happens only in the success arm, so
                            // an abort never destroys the original (finding).
                            let (temp, file, target) =
                                sanitize::resolve_and_open_overwrite(rel, download_root)?;
                            overwrite_renames.push((temp.clone(), target));
                            (temp, file)
                        }
                        ConflictAction::Rename => sanitize::resolve_and_open(rel, download_root)?,
                    };
                    (
                        path.clone(),
                        disk_rel_under_root(download_root, &path),
                        file,
                        0u64,
                    )
                }
            };

            let mut out = tokio::fs::File::from_std(std_file);
            // Record the opened identity in the NON-pruned accumulator so the
            // caller's error-path `remove` set is scoped to files this session
            // reached (finding-1), even if this one is later deleted on cleanup.
            opened.push(identity.clone());
            // Track progress for this file BEFORE the first byte, so an
            // interruption right after open still records the partial (M6.4).
            progress.push(FileProgress {
                identity,
                disk_rel,
                path: path.clone(),
                hashed,
                bytes_written: start_offset,
                done: false,
            });
            let pi = progress.len() - 1;

            // Integrity digest (M6.3) covers the WHOLE file. On resume the
            // pre-existing prefix is read back and folded in first, so the final
            // compare is over `old-bytes + tail`.
            let mut hasher = f.sha256.as_ref().map(|_| Sha256::new());
            if start_offset > 0 {
                if let Some(h) = hasher.as_mut() {
                    out.seek(SeekFrom::Start(0)).await?;
                    fold_prefix(&mut out, start_offset, h).await?;
                }
                // Position the write cursor at the resume point regardless of
                // whether we folded (a non-hashed resume never happens, but be
                // explicit): writes must APPEND, never overwrite from 0.
                out.seek(SeekFrom::Start(start_offset)).await?;
            }

            let mut remaining = f.size.saturating_sub(start_offset);
            // Bytes written since the last sync — drained every
            // RECEIVE_SYNC_INTERVAL so the final sync_all covers only the last
            // window, not minutes of dirty page cache on slow media.
            let mut unsynced = 0u64;
            while remaining > 0 {
                // Pause gate (M6.2): stop reading while paused → the socket's
                // receive window fills → the sender stalls on backpressure. The
                // park is bounded (finding-4): an auto-resume at the cap moves a
                // chunk before the sender's blocked `write_msg` idle-times-out,
                // and the UI is told so the row un-pauses.
                if wait_while_paused(ctl).await? {
                    on_file(FileEvent::PauseAutoResumed);
                }
                // Idle-bounded: the sender paces the stream, but a peer that
                // stalls (or over-declared f.size) must not wedge this loop
                // forever — Timeout ends the transfer via the error path (M4.5).
                // Cancel (M6.1) wins over the read (biased).
                let frame = tokio::select! {
                    biased;
                    _ = ctl.cancel.cancelled() => return Err(LanBeamError::Cancelled),
                    r = tokio::time::timeout(CHUNK_IDLE_TIMEOUT, recv_frame(session)) => match r {
                        Ok(res) => res?,
                        Err(_) => return Err(LanBeamError::Timeout),
                    }
                };
                match frame {
                    Frame::FileChunk(_idx, bytes) => {
                        // Defense-in-depth (finding-0): with a correct sender every
                        // file's chunks sum to EXACTLY `f.size - start_offset`, so a
                        // single chunk can never exceed this file's `remaining`. If
                        // one does, the sender over-sent for this file and clamping
                        // it (the old `min(remaining)`) would silently drop the
                        // overflow — bytes that belong to the NEXT file's frames —
                        // desyncing the stream. Reject it as a protocol error so the
                        // drift fails cleanly here instead of corrupting a neighbour.
                        if bytes.len() as u64 > remaining {
                            return Err(LanBeamError::Protocol(format!(
                                "file chunk overruns declared size: {}",
                                f.name
                            )));
                        }
                        let take = bytes.len();
                        out.write_all(&bytes[..take]).await?;
                        if let Some(h) = hasher.as_mut() {
                            h.update(&bytes[..take]);
                        }
                        remaining -= take as u64;
                        received += take as u64;
                        progress[pi].bytes_written += take as u64;
                        unsynced += take as u64;
                        if unsynced >= RECEIVE_SYNC_INTERVAL {
                            out.sync_data().await?;
                            unsynced = 0;
                        }
                        on_progress(received, manifest.total_size);
                        on_file(FileEvent::Streaming {
                            index: i,
                            name: &f.name,
                            done: progress[pi].bytes_written,
                            total: f.size,
                        });
                        // Pace the read loop (M6.7); a no-op when unthrottled.
                        limiter.throttle(take).await;
                    }
                    Frame::Control(m) => {
                        return Err(LanBeamError::Protocol(format!(
                            "chunk expected, got {}",
                            trust::clamp_peer_text(&format!("{m:?}"))
                        )))
                    }
                }
            }
            out.flush().await?;
            // Verify BEFORE the file is synced + published (M6.3): a mismatch
            // returns through the error path, which — because Integrity is
            // non-resumable — deletes the corrupt partial. `hasher` is Some only
            // when `f.sha256` is Some, by construction.
            if let Some(h) = hasher {
                let got = to_hex(&h.finalize());
                let expected = f.sha256.as_deref().unwrap_or_default();
                if !got.eq_ignore_ascii_case(expected) {
                    return Err(LanBeamError::Integrity(format!(
                        "sha256 mismatch: {}",
                        f.name
                    )));
                }
            }
            out.sync_all().await?;
            // File delivered (M6.8). Reaching here for a hashed file means its
            // digest matched — a mismatch returned above — so `verified` is
            // exactly `f.sha256.is_some()`.
            on_file(FileEvent::Done {
                index: i,
                verified: f.sha256.is_some(),
            });
            progress[pi].done = true;
            saved.push(path);
        }
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            // Commit overwrite files: the whole transfer succeeded (and every
            // hashed file verified), so replace each real target with the temp we
            // streamed into. `rename` won't clobber an existing file on Windows,
            // so fall back to remove-then-rename; a crash in that tiny window
            // leaves the fully-synced new bytes at the temp (a "(1)" sibling) —
            // never nothing, and the original was never touched until here.
            // `remove_file` never follows a symlink, so containment holds.
            for (temp, target) in &overwrite_renames {
                let committed = std::fs::rename(temp, target).or_else(|_| {
                    let _ = std::fs::remove_file(target);
                    std::fs::rename(temp, target)
                });
                match committed {
                    Ok(()) => {
                        for s in saved.iter_mut() {
                            if *s == *temp {
                                *s = target.clone();
                            }
                        }
                    }
                    Err(e) => log::warn!(
                        "overwrite commit {} -> {} failed ({e}); kept the received copy at {}",
                        temp.display(),
                        target.display(),
                        temp.display()
                    ),
                }
            }
            // Best-effort ack: every byte is already flushed + synced, so a
            // failed ack write (the sender may have hit ITS deadline and
            // closed first) must not turn a fully-saved transfer into an
            // error on this side too — log it and keep the success.
            if let Err(e) = send_control(
                session,
                &AppMessage::TransferAck {
                    transfer_id: manifest.transfer_id.clone(),
                    ok: true,
                    error: None,
                },
            )
            .await
            {
                log::warn!(
                    "transfer {}: final ack send failed ({e}); files are saved",
                    manifest.transfer_id
                );
            }
            Ok(saved)
        }
        Err(e) => {
            // Best-effort: tell the sender. Then decide each written file's fate
            // (M6.4): keep a hashed partial through an interruption for a later
            // resume, delete a non-hashed one, and — on a data-integrity failure —
            // delete the file whose digest did NOT match while LEAVING its verified
            // siblings alone. (This comment used to say "delete EVERYTHING on a
            // data-integrity failure", which was true, and was the bug: one bad
            // hash took the whole batch with it. See the retain below.) The
            // accumulator is pruned to exactly the files left on disk so the
            // caller persists precisely those.
            let _ = send_control(
                session,
                &AppMessage::TransferAck {
                    transfer_id: manifest.transfer_id.clone(),
                    ok: false,
                    // Send only the COARSE ui_code over the wire, never the
                    // detailed error (finding): receive-path failures embed
                    // absolute local paths ("create C:\\Users\\<name>\\...:
                    // Access is denied"), which would otherwise cross to a
                    // possibly-untrusted peer and land in ITS toast, disclosing
                    // this machine's username and disk layout. The full detail is
                    // still surfaced LOCALLY — the returned `Err(e)` drives this
                    // side's own transfer_error event and log.
                    error: Some(e.ui_code().to_string()),
                },
            )
            .await;
            let keep_resumable = e.keeps_partial();
            progress.retain(|w| {
                // A file that already finished — written, synced, digest matched —
                // is FINISHED. A later file in the batch failing its checksum says
                // nothing about this one (a hash mismatch means corruption in
                // transit; a peer out to lie would simply have sent a matching
                // hash). Deleting good, verified data to punish a neighbour is
                // just data loss, and it used to happen to the whole batch. Leave
                // it on disk, and drop it from the partial accumulator — it is
                // whole, not partial.
                //
                // `overwrite` is the exception, and only looks like one: there
                // "finished" means the bytes are in a TEMP file while the real
                // target still holds the old content. That commit is deliberately
                // all-or-nothing and only runs once the entire transfer succeeds,
                // so an uncommitted temp falls through to the normal rule below —
                // deleted outright, or kept as a resumable partial.
                let uncommitted_temp = overwrite_renames.iter().any(|(t, _)| *t == w.path);
                if w.done && !uncommitted_temp {
                    return false;
                }
                let stays = keep_resumable && w.hashed;
                if !stays {
                    // The file handle unwound with the async block above, so
                    // Windows allows the delete.
                    let _ = std::fs::remove_file(&w.path);
                }
                stays
            });
            Err(e)
        }
    }
}

/// Read exactly `n` bytes from the current position of `file`, folding them into
/// `hasher` — the resume prefix fold-back (M6.4), so the final digest spans the
/// whole file. WHY read it back rather than trust it: the digest must cover
/// EVERY byte we deliver, including the ones a prior session already wrote.
async fn fold_prefix(file: &mut tokio::fs::File, n: u64, hasher: &mut Sha256) -> Result<()> {
    let mut remaining = n;
    let mut buf = vec![0u8; 128 * 1024];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        file.read_exact(&mut buf[..want]).await?;
        hasher.update(&buf[..want]);
        remaining -= want as u64;
    }
    Ok(())
}

/// The on-disk path of `path` RELATIVE to `download_root`, forward-slashed —
/// the `disk_rel` a partial records so a resume reopens the exact same file.
/// `path` is always `download_root.join(...)`, so the strip cannot fail; the
/// fallback keeps the full path rather than panic.
fn disk_rel_under_root(download_root: &Path, path: &Path) -> String {
    path.strip_prefix(download_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

// ── app orchestration (inbound) ───────────────────────────────────────
/// Monotonic stamp for parked prompts — see [`park_pending`]. Process-wide so
/// two entries can never share a generation, whatever their ids.
static PENDING_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Park the accept/decline sender under `transfer_id` — unless one is already
/// parked there. WHY refuse instead of replace: `pending` is keyed by the
/// SENDER-chosen transfer_id, so honoring a duplicate would drop the victim's
/// oneshot sender and force-decline someone else's live prompt (M4.5).
/// Returns the entry's generation on success — the token the parking session
/// must present to [`remove_pending_if_owner`] later — or `None` (caller must
/// reject the session) on duplicate or when the lock is poisoned: a panicked
/// bookkeeping task means the map is unusable, and declining beats cascading
/// the panic.
fn park_pending(
    pending: &Mutex<PendingMap>,
    transfer_id: &str,
    tx: oneshot::Sender<ReplyDecision>,
) -> Option<u64> {
    let Ok(mut map) = pending.lock() else {
        return None;
    };
    match map.entry(transfer_id.to_string()) {
        std::collections::hash_map::Entry::Occupied(_) => None,
        std::collections::hash_map::Entry::Vacant(slot) => {
            let generation = PENDING_GENERATION.fetch_add(1, Ordering::Relaxed);
            slot.insert((generation, tx));
            Some(generation)
        }
    }
}

/// Owner-only removal of a parked prompt: the entry goes away only when its
/// generation matches the one this session parked. WHY not remove by id:
/// `reply_file_request` vacates the slot the moment it answers, so a
/// successor session reusing the same sender-chosen id may have re-parked in
/// the interim — an id-only removal would drop the successor's live oneshot
/// and force-decline its prompt without any user input (M4.5).
fn remove_pending_if_owner(pending: &Mutex<PendingMap>, transfer_id: &str, generation: u64) {
    if let Ok(mut map) = pending.lock() {
        if map.get(transfer_id).is_some_and(|(g, _)| *g == generation) {
            map.remove(transfer_id);
        }
    }
}

/// RAII claim on an inbound session id, held for the WHOLE life of
/// `handle_incoming`. WHY `pending` isn't enough: it only covers the prompt
/// window — the auto-accept path never parks at all, and even a prompted
/// session vacates its entry before `run_receive` starts — so without this
/// set, two sessions reusing one sender-chosen transfer_id would run
/// concurrently and conflate every event and completed entry keyed by it.
/// Drop releases the id on every exit path (decline, error, success).
struct InFlightGuard<'a> {
    set: &'a Mutex<HashSet<String>>,
    id: String,
}

impl<'a> InFlightGuard<'a> {
    /// Atomically claim `id`. `None` when it is already in flight — or when
    /// the lock is poisoned: an unusable registry must reject, same contract
    /// as [`park_pending`].
    fn claim(set: &'a Mutex<HashSet<String>>, id: &str) -> Option<Self> {
        let Ok(mut s) = set.lock() else {
            return None;
        };
        if !s.insert(id.to_string()) {
            return None;
        }
        Some(Self {
            set,
            id: id.to_string(),
        })
    }
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut s) = self.set.lock() {
            s.remove(&self.id);
        }
    }
}

/// Refresh a trusted peer's bookkeeping after an ACCEPTED transfer (M4.4):
/// bump `last_seen`, adopt a fresher DeviceInfo name, persist, and tell the
/// UI. A no-op for peers not in the store — see `TrustStore::touch_seen`.
/// Mutation + snapshot happen under the write guard; the file write runs on
/// the blocking pool and the emit fires after the guard drops — this runs on
/// a tokio worker inside `handle_incoming`, and disk I/O under the trust lock
/// would stall every concurrent session's policy check and the UI's sync
/// trust commands (M4.6).
fn touch_trusted<R: Runtime>(ctx: &TransportCtx<R>, device_id: &str, name: Option<&str>) {
    let (snapshot, list) = {
        // Poisoned lock: skip the bookkeeping rather than cascade the panic —
        // the transfer itself must proceed regardless.
        let Ok(mut store) = ctx.trusted.write() else {
            return;
        };
        if !store.touch_seen(device_id, name) {
            return;
        }
        (store.snapshot(), store.list())
    };
    trust::persist_async(snapshot);
    trust::emit_updated(&ctx.app, list);
}

/// Fire an OS notification if the `notif_system` setting (read at fire time)
/// allows it (M5.4). Content is deliberately LOCALE-NEUTRAL: the backend has
/// no i18n layer, so instead of hardcoding sentences in any one language the
/// title is the peer's display name and the body is symbols + numbers only
/// ("↓ 3 · 1.2 MB") — digits, arrows and unit symbols read the same in every
/// locale, and the direction/meaning lives in the glyph, not in words.
#[cfg(desktop)]
fn notify_system<R: Runtime>(ctx: &TransportCtx<R>, title: &str, body: &str) {
    use tauri::Manager;
    let enabled = ctx.settings.read().map(|s| s.notif_system).unwrap_or(true);
    if !enabled {
        return;
    }
    // try_state, NOT NotificationExt::notification(): the ext accessor panics
    // when the plugin is absent, and the integration tests drive the real
    // `handle_incoming` on a mock app with no plugins — a missing notifier
    // must degrade to silence, never take the session down with it.
    let Some(n) = ctx
        .app
        .try_state::<tauri_plugin_notification::Notification<R>>()
    else {
        return;
    };
    if let Err(e) = n.builder().title(title).body(body).show() {
        log::warn!("system notification failed: {e}");
    }
}

/// Show the just-received files in the file manager, if the user asked for it
/// (`auto_open_folder`, read at fire time).
///
/// That setting had a switch, a command, a persisted field and its own line in
/// the diagnostics export — and nothing anywhere that acted on it. Rust read it
/// only to hand it back to the UI so the switch could render its own state. It
/// also defaults ON, so every install has been promising this and never doing it.
#[cfg(desktop)]
fn reveal_if_asked<R: Runtime>(ctx: &TransportCtx<R>, path: Option<&std::path::Path>) {
    use tauri::Manager;
    let enabled = ctx
        .settings
        .read()
        .map(|s| s.auto_open_folder)
        .unwrap_or(false);
    if !enabled {
        return;
    }
    let Some(path) = path else { return };
    // try_state, NOT OpenerExt::opener(): the ext accessor panics when the plugin
    // is absent, and the integration tests drive the real `handle_incoming` on a
    // mock app with no plugins — a missing opener must degrade to silence, never
    // take the session down with it. (Same reason `notify_system` does it.)
    let Some(opener) = ctx.app.try_state::<tauri_plugin_opener::Opener<R>>() else {
        return;
    };
    if let Err(e) = opener.reveal_item_in_dir(path) {
        log::warn!("auto-open: reveal {} failed: {e}", path.display());
    }
}

/// Mobile stub — see `notify_system`.
#[cfg(not(desktop))]
fn reveal_if_asked<R: Runtime>(_ctx: &TransportCtx<R>, _path: Option<&std::path::Path>) {}

/// Mobile stub — the notification plugin is a desktop-only dependency here,
/// and a no-op keeps every call site free of cfg noise.
#[cfg(not(desktop))]
fn notify_system<R: Runtime>(_ctx: &TransportCtx<R>, _title: &str, _body: &str) {}

/// Human-readable byte size ("824 B", "1.2 MB", "40 GB"), 1024-based. Feeds
/// the OS notification body, so it stays symbols + numbers — see
/// [`notify_system`] for the locale-neutrality contract.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit]) // one decimal while it matters
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// Handle one inbound connection: opening dialogue (hello exchange, or a
/// legacy manifest) → validate → decide via the receive policy (auto-accept
/// or ask the UI) → receive, emitting lifecycle events. Runs in the
/// listener's per-connection task.
pub async fn handle_incoming<R: Runtime>(
    mut session: NoiseSession,
    ctx: &TransportCtx<R>,
) -> Result<()> {
    // Bounded: the whole opening dialogue (hello exchange + manifest) is small
    // control traffic, so a peer that opens a session and trickles (or stalls)
    // must not hold this task open (M4.5).
    let my_name = local_device_name(&ctx.settings);
    let opened =
        match tokio::time::timeout(MANIFEST_TIMEOUT, open_inbound(&mut session, my_name)).await {
            Ok(res) => res?,
            Err(_) => return Err(LanBeamError::Timeout),
        };
    let device_id = URL_SAFE_NO_PAD.encode(session.remote_static());
    let (manifest, peer) = match opened {
        // `connect_device`'s identity/SAS round-trip ends here by design:
        // the peer introduced itself and said Bye — nothing user-visible
        // happened, so the UI sees no prompt, no events, no state.
        InboundOpen::Bye => {
            log::debug!("clean bye from {device_id} (identity round-trip)");
            return Ok(());
        }
        // A pairing redemption (M7.1) — handled and CLOSED here, BEFORE any
        // incoming_file_request path: match the code, mutually trust on success,
        // reply PairConfirm, and return. Never falls through to the file flow.
        InboundOpen::Pair { code, peer } => {
            return handle_pair_request(&mut session, ctx, &device_id, code, peer).await;
        }
        // A quick text (M7.3) — handled and CLOSED here, BEFORE any file path:
        // emit `text_received`, optionally mirror to the clipboard (receiver
        // consent), ack, and return. Never touches disk, never falls through.
        InboundOpen::Text {
            text,
            to_clipboard,
            peer,
        } => {
            return handle_text_received(&mut session, ctx, &device_id, text, to_clipboard, peer)
                .await;
        }
        InboundOpen::Files { manifest, peer } => (manifest, peer),
    };
    let session_id = manifest.transfer_id.clone();
    // When the sender's reply clock (REPLY_TIMEOUT) began: it started waiting the
    // moment it finished sending this manifest — i.e. now. Both the accept prompt
    // and any concurrency-gate queue below spend against this same budget, so the
    // gate wait is bounded by what's LEFT of it (finding), not a fresh 150s.
    let received_at = Instant::now();
    // Friendly sender name for the accept prompt: the peer's self-declared
    // DeviceInfo name is best (it exists even for peers absent from the
    // discovery table), the table name is next, and the shortened device id
    // always exists. The negotiated version is what later capabilities gate on.
    let sender_name = peer
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(String::from)
        .or_else(|| {
            ctx.peers
                .lock()
                .ok()
                .and_then(|t| t.get(&device_id).map(|p| p.name.clone()))
        })
        .unwrap_or_else(|| device_id.chars().take(8).collect());
    log::info!(
        "inbound session {session_id} from {sender_name:?} ({device_id}, proto v{})",
        peer.version
    );

    // Reject unsafe manifests before involving the user.
    if let Err(e) = validate_manifest(&manifest) {
        let _ = send_reply(
            &mut session,
            &session_id,
            false,
            Some("unsafe filename".into()),
        )
        .await;
        let _ = ctx.app.emit(
            "transfer_error",
            json!({ "sessionId": session_id, "error": e.to_string(), "code": e.ui_code() }),
        );
        return Err(e);
    }

    // Whole-lifetime duplicate guard: claimed BEFORE any event fires and held
    // until this function returns, so a reused sender-chosen id is politely
    // refused whether the live session is auto-accepted, prompting, or already
    // mid-transfer — see `InFlightGuard`.
    let Some(_in_flight) = InFlightGuard::claim(&ctx.in_flight, &session_id) else {
        log::warn!(
            "rejecting inbound transfer from {device_id}: id {session_id} is already in flight"
        );
        let _ = send_reply(
            &mut session,
            &session_id,
            false,
            Some("duplicate transfer id".into()),
        )
        .await;
        // Deliberately NO transfer_error emit here: the UI keys sessions by this
        // id, and an event would clobber the live session's still-valid state.
        return Err(LanBeamError::Protocol(format!(
            "duplicate transfer id {session_id}"
        )));
    };

    // Receive policy (M4.4): decide BEFORE parking a prompt whether this peer
    // skips it — "all" auto-accepts anyone on the LAN, "trusted" only peers
    // the user explicitly marked auto-accept, "ask" always prompts. A poisoned
    // lock (settings or trust) also prompts: when in doubt, ask the human.
    let policy = ctx
        .settings
        .read()
        .map(|s| s.recv_policy.clone())
        .unwrap_or_else(|_| "ask".to_string());
    let trusted_entry = ctx
        .trusted
        .read()
        .ok()
        .and_then(|t| t.get(&device_id).cloned());
    let auto_accepted = trust::should_auto_accept(&policy, trusted_entry.as_ref());

    // Snapshot the download root NOW (M5.2): a concurrent `set_download_dir`
    // must not split this session's files across two folders (and the resume /
    // collision analysis below must agree with the eventual write). The lock is
    // never held across an await — the receive path gets a plain path.
    let download_root = match ctx.download_dir.read() {
        Ok(d) => d.clone(),
        // Poisoned: the last-written value is still the user's choice.
        Err(e) => e.into_inner().clone(),
    };

    // Auto-organize prefix (M6.6): an optional subfolder every file in this
    // transfer is nested under — the sanitized sender name, or today's date.
    // Computed once so a transfer spanning midnight all lands in one folder.
    let (conflict_policy, organize) = ctx
        .settings
        .read()
        .map(|s| (s.conflict.clone(), s.organize.clone()))
        .unwrap_or_else(|_| ("ask".to_string(), "none".to_string()));
    let prefix: Option<PathBuf> = match organize.as_str() {
        "device" => Some(PathBuf::from(sanitize::sanitize_component(
            &sender_name,
            "device",
        ))),
        "date" => Some(PathBuf::from(today_ymd())),
        _ => None, // "none" + forward-compat fallback
    };

    // Per-file resume + collision analysis (M6.4/6.5): where each file would
    // land, whether a persisted partial lets it continue, and whether a fresh
    // copy would collide with an existing file. Run on the blocking pool
    // (finding): it does 1-2 synchronous std::fs stats PER FILE, and a 10k-file
    // manifest on a network share or spun-down disk would otherwise pin an async
    // worker for tens of seconds, stalling other sessions before the prompt even
    // fires. `manifest` moves in and back out; the roots/id are cloned since the
    // receive path still needs them, and `ctx.partials` is an `Arc` clone.
    let partials = ctx.partials.clone();
    let analysis_root = download_root.clone();
    let analysis_dev = device_id.clone();
    let (analyses, manifest) = tauri::async_runtime::spawn_blocking(move || {
        let a = analyze_files(
            &manifest,
            &analysis_root,
            prefix.as_deref(),
            &analysis_dev,
            &partials,
        );
        (a, manifest)
    })
    .await
    .map_err(|e| LanBeamError::Io(format!("file analysis task failed: {e}")))?;

    let files: Vec<_> = manifest
        .files
        .iter()
        .map(|f| json!({ "name": f.name, "size": f.size }))
        .collect();
    // Files a fresh write would collide with — the ConflictModal's input (M6.5).
    let conflicts: Vec<_> = analyses
        .iter()
        .filter(|a| a.collides)
        .map(|a| a.name.clone())
        .collect();
    // Files continuing a partial (M6.4) — informational for the UI.
    let resuming: Vec<_> = analyses
        .iter()
        .filter_map(|a| {
            a.resume
                .as_ref()
                .map(|(_, off)| json!({ "name": a.name, "offset": off }))
        })
        .collect();
    let request_payload = json!({
        "sessionId": session_id,
        "deviceId": device_id,
        "sas": session.sas(),
        "totalSize": manifest.total_size,
        "fileCount": manifest.files.len(),
        "files": files,
        // Additive (M4.2): older frontends simply ignore it.
        "senderName": sender_name,
        // Additive (M4.4): `true` means the policy already accepted — the UI
        // should toast/log instead of prompting (a reply would go nowhere).
        "autoAccepted": auto_accepted,
        // Additive (M6.5): names that collide with existing files, plus the
        // active policy — when it is "ask" and this list is non-empty the UI
        // shows the ConflictModal and echoes the choice via `reply_file_request`.
        "conflicts": conflicts,
        "conflictPolicy": conflict_policy,
        // Additive (M6.4): files that will continue from a partial (offset > 0).
        "resuming": resuming,
    });

    // Auto-accept answers the ACCEPT question ("do I want anything from this
    // device") — it does not answer the COLLISION question ("what do I do with my
    // existing file"). Those are two settings and two decisions. A user who set
    // 冲突策略 = 每次询问 asked to be asked, and silently answering "rename" on
    // their behalf was worst precisely where it is most likely to bite: a trusted
    // device with auto-accept on is the MAIN path, so under the default settings
    // (recv_policy "trusted" + conflict "ask") the modal could never appear at all.
    // So: still no accept prompt, but do stop and ask about the collision.
    let ask_collision = auto_accepted && conflict_policy == "ask" && !conflicts.is_empty();

    let (accept, conflict_choice) = if auto_accepted && !ask_collision {
        // The prompt machinery is skipped entirely: nothing parked in
        // `pending`, no oneshot, no 120s window. The event still fires so
        // the UI can surface WHAT was auto-accepted and from whom.
        log::info!("auto-accepting {session_id} from {device_id} (recv_policy \"{policy}\")");
        let _ = ctx.app.emit("incoming_file_request", request_payload);
        (true, None)
    } else {
        // Ask the UI and wait for `reply_file_request` (timeout = decline).
        // The in-flight guard above already rejected duplicates, so a `None`
        // here means the pending lock is poisoned — decline defensively.
        let (tx, rx) = oneshot::channel();
        let Some(generation) = park_pending(&ctx.pending, &session_id, tx) else {
            log::warn!(
                "rejecting inbound transfer from {device_id}: cannot park prompt for {session_id}"
            );
            let _ = send_reply(
                &mut session,
                &session_id,
                false,
                Some("duplicate transfer id".into()),
            )
            .await;
            // Deliberately NO transfer_error emit here: the UI keys sessions by this
            // id, and an event would clobber the victim's still-valid prompt state.
            return Err(LanBeamError::Protocol(format!(
                "duplicate transfer id {session_id}"
            )));
        };
        let _ = ctx.app.emit("incoming_file_request", request_payload);
        // OS toast beside the in-app prompt (M5.4): with close-to-tray on by
        // default the window is often hidden, and an unanswered prompt
        // auto-declines at 120s — the toast is what makes that window real.
        // Only the PROMPT path notifies; auto-accepted sessions surface via
        // the completion toast below instead of pinging twice.
        notify_system(
            ctx,
            &sender_name,
            &format!(
                "↓ {} · {}",
                manifest.files.len(),
                human_size(manifest.total_size)
            ),
        );

        let decision = match tokio::time::timeout(Duration::from_secs(120), rx).await {
            Ok(Ok(d)) => d,
            // Nobody answered. What that MEANS depends on what was asked. An
            // accept prompt unanswered is a "no" — silence must never become
            // consent. But an auto-accepted transfer already has its yes; only
            // the collision choice was outstanding, so throwing the whole
            // transfer away over an unanswered follow-up would punish the user
            // for stepping out. Fall back to the safe, non-destructive rename —
            // the same thing this path did before it learned to ask at all.
            _ if ask_collision => ReplyDecision {
                accept: true,
                conflict: Some(ConflictAction::Rename),
            },
            _ => ReplyDecision::plain(false),
        };
        // Owner-only cleanup: on the answered path `reply_file_request` already
        // vacated the slot, and the generation check keeps a racing successor's
        // re-parked entry (same sender-chosen id) alive.
        remove_pending_if_owner(&ctx.pending, &session_id, generation);
        (decision.accept, decision.conflict)
    };

    if !accept {
        let _ = send_reply(&mut session, &session_id, false, Some("declined".into())).await;
        // `code` lets the UI show "you declined" instead of a failure state
        // without string-matching `error` (M4.5). The string stays unchanged.
        let _ = ctx.app.emit(
            "transfer_error",
            json!({ "sessionId": session_id, "error": "declined", "code": "declined" }),
        );
        return Ok(());
    }

    // Accepted — by policy or by hand: refresh the peer's trust bookkeeping.
    // `last_seen` feeds the UI's trusted list, and the DeviceInfo name follows
    // renames without re-pairing. Peers absent from the store are untouched.
    touch_trusted(ctx, &device_id, peer.name.as_deref());

    let _ = ctx.app.emit(
        "transfer_started",
        json!({ "sessionId": session_id, "direction": "receive", "totalSize": manifest.total_size }),
    );

    // Resolve the collision action for THIS transfer (M6.5): "rename"/"overwrite"
    // apply directly; "ask" (and any unknown value) honors the modal choice, or
    // falls back to the safe rename when none was made — the auto-accept path,
    // which never prompts, always lands here as Rename, so nothing is ever
    // overwritten without an explicit instruction. Resume takes precedence over
    // this per file (a resumed file is the same file continuing, not a new
    // collision), which the plan below already encodes.
    let action = match conflict_policy.as_str() {
        "overwrite" => ConflictAction::Overwrite,
        "rename" => ConflictAction::Rename,
        _ => conflict_choice.unwrap_or(ConflictAction::Rename),
    };
    let plan: Vec<FileDisposition> = analyses
        .iter()
        .map(|a| match &a.resume {
            Some((disk_rel, offset)) => FileDisposition::Resume {
                disk_rel: disk_rel.clone(),
                offset: *offset,
            },
            None => FileDisposition::Fresh {
                rel: a.organized_rel.clone(),
                conflict: action,
            },
        })
        .collect();
    let all_identities: Vec<FileIdentity> = analyses.iter().map(|a| a.identity.clone()).collect();

    let app = ctx.app.clone();
    let sid = session_id.clone();
    let mut last_pct = -1i64;
    // Register live cancel/pause controls for the receive chunk phase
    // (M6.1/6.2); the guard removes them on every exit path below, so a
    // cancel/pause of this id after it finishes is a graceful no-op.
    let (_ctl_guard, ctl) = TransferCtlGuard::register(&ctx.transfers_ctl, &session_id);

    // Concurrency gate (M6.7): bound how many transfers stream at once, across
    // both directions (the send side draws from the same gate). Acquired AFTER
    // the accept decision — so the prompt still showed and trust was refreshed —
    // and held for the whole receive; a lowered cap applies only to transfers
    // not yet past this gate. While the gate is full the transfer QUEUES here;
    // its accept reply is sent inside `run_receive_core`, so the queue is bounded
    // by what remains of the sender's `REPLY_TIMEOUT` (finding): if the gate
    // can't drain before that deadline, sending an accept would land on a socket
    // the sender already closed, so we decline "busy" instead and let both peers
    // resolve deterministically. The rate cap (M6.7) is read at the same point,
    // both from one settings read.
    let (limit, rate_limit) = ctx
        .settings
        .read()
        .map(|s| {
            (
                crate::settings::clamp_max_concurrent(s.max_concurrent),
                crate::settings::rate_limit_bytes_per_sec(&s.rate_limit),
            )
        })
        .unwrap_or((crate::settings::clamp_max_concurrent(0), None));
    let _slot = match ctx.concurrency.try_acquire(limit) {
        Some(guard) => guard,
        None => {
            let _ = ctx.app.emit(
                "transfer_queued",
                json!({ "sessionId": session_id, "direction": "receive" }),
            );
            // A cancel issued while QUEUED must win over the wait (finding-2):
            // without this select the acquire ignores the already-registered
            // cancel token, so the transfer stays parked and only aborts once a
            // slot frees and it reaches the chunk loop — after sending an accept
            // reply. Emit the error so the UI row resolves, then bail.
            //
            // The acquire is also bounded by what's LEFT of the sender's reply
            // clock (finding): the prompt already spent part of REPLY_TIMEOUT, so
            // waiting a fresh 150s here would overshoot the sender's deadline and
            // send our accept into a dead socket. `saturating_sub` keeps the
            // deadline non-negative (an accept at the wire) and leaves a small
            // margin so the reply itself still fits. On expiry we decline "busy":
            // the sender then resolves to a deterministic Rejected instead of a
            // confusing Timeout/io pair, and the queued UI row settles on a local
            // Timeout error.
            let gate_deadline = REPLY_TIMEOUT
                .saturating_sub(received_at.elapsed())
                .saturating_sub(Duration::from_secs(5));
            tokio::select! {
                biased;
                _ = ctl.cancel.cancelled() => {
                    let _ = ctx.app.emit(
                        "transfer_error",
                        json!({ "sessionId": session_id, "error": LanBeamError::Cancelled.to_string(), "code": LanBeamError::Cancelled.ui_code() }),
                    );
                    return Err(LanBeamError::Cancelled);
                }
                r = tokio::time::timeout(gate_deadline, ctx.concurrency.acquire(limit)) => match r {
                    Ok(g) => g,
                    Err(_) => {
                        let _ = send_reply(&mut session, &session_id, false, Some("busy".into())).await;
                        let _ = ctx.app.emit(
                            "transfer_error",
                            json!({ "sessionId": session_id, "error": LanBeamError::Timeout.to_string(), "code": LanBeamError::Timeout.ui_code() }),
                        );
                        return Err(LanBeamError::Timeout);
                    }
                },
            }
        }
    };

    // Per-file progress/completion events (M6.8).
    let mut on_file = file_event_sink(ctx.app.clone(), session_id.clone());
    // Filled as files are written; on error, pruned to the partials still on
    // disk so we persist exactly those for a later resume (M6.4).
    let mut progress: Vec<FileProgress> = Vec::new();
    // Every file this session actually OPENED (non-pruned), scoping the error
    // path's stale-record removal to files we reached (finding-1).
    let mut opened: Vec<FileIdentity> = Vec::new();
    let saved = run_receive_core(
        &mut session,
        &manifest,
        &download_root,
        &ctl,
        &plan,
        &mut progress,
        &mut opened,
        rate_limit,
        &mut on_file,
        move |recv, total| emit_progress(&app, &sid, "receive", recv, total, &mut last_pct),
    )
    .await;

    match saved {
        Ok(paths) => {
            // Delivered: clear any persisted partials for these files (a resumed
            // transfer that just completed, or a no-op when there were none).
            persist_partials(ctx, &device_id, &all_identities, Vec::new());
            let names: Vec<_> = paths
                .iter()
                .map(|p| {
                    p.file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
                .collect();
            let file_count = paths.len();
            // Kept before `paths` moves into the completed log below.
            let first_saved = paths.first().cloned();
            // Poisoned lock = graceful no-op, not a cascade panic (the invariant
            // every other registry access in this file upholds): a panicked
            // holder of `completed` must degrade to a skipped history entry, not
            // crash the per-connection receive task. The transfer is already
            // saved; the completed-log insert is only bookkeeping for `reveal`.
            if let Ok(mut log) = ctx.completed.lock() {
                log.insert(session_id.clone(), paths);
            }
            let _ = ctx.app.emit(
                "transfer_done",
                json!({ "sessionId": session_id, "direction": "receive", "savedNames": names }),
            );
            // Completion toast for receives (M5.4) — the transfer may have run
            // for minutes with the window hidden in the tray. Send completions
            // stay quiet: the user just drove that flow in the foreground.
            notify_system(
                ctx,
                &sender_name,
                &format!("✓ {} · {}", file_count, human_size(manifest.total_size)),
            );
            reveal_if_asked(ctx, first_saved.as_deref());
            Ok(())
        }
        Err(e) => {
            // A failed receive is at least as worth interrupting for as a
            // successful one — and it used to be the only one that said nothing
            // at all, so a transfer that ran for ten minutes and then threw its
            // bytes away did it in total silence. Locale-neutral like the rest
            // (a cross, a count, a size); the WHY is the in-app toast's job,
            // where there is an i18n layer to say it in the user's language.
            notify_system(
                ctx,
                &sender_name,
                &format!(
                    "✕ {} · {}",
                    manifest.files.len(),
                    human_size(manifest.total_size)
                ),
            );
            // Record resumable partials (M6.4): `progress` was pruned by
            // `run_receive_core` to the files still on disk (hashed files kept
            // through an interruption; a corrupt file deleted, its verified
            // siblings left alone), so persist exactly those and drop the rest.
            // `set_files` replaces this manifest's identities, so a corrupt or
            // deleted file's stale partial is cleared even when nothing is kept.
            let kept: Vec<PartialRecord> = progress
                .iter()
                .map(|w| PartialRecord {
                    rel: w.identity.rel.clone(),
                    size: w.identity.size,
                    sha256: w.identity.sha256.clone(),
                    disk_rel: w.disk_rel.clone(),
                    bytes_written: w.bytes_written,
                })
                .collect();
            // Scope removal to files this session OPENED (finding-1), NOT every
            // manifest identity: an interruption that never reached a later file
            // must leave that file's still-valid persisted partial intact instead
            // of wiping it (which would force a full re-download and orphan its
            // on-disk bytes). Files opened-then-deleted (non-hashed / integrity
            // failure) are in `opened` too, so their stale records still clear.
            persist_partials(ctx, &device_id, &opened, kept);
            let _ = ctx.app.emit(
                "transfer_error",
                json!({ "sessionId": session_id, "error": e.to_string(), "code": e.ui_code() }),
            );
            Err(e)
        }
    }
}

// ── pairing host side (M7.1) ──────────────────────────────────────────
/// Handle an inbound pairing redemption, gated on `TRANSFER_V2` upstream. Runs
/// the whole exchange and CLOSES the session — it never falls through to the
/// file-receive path.
///
/// Order: (1) per-source rate gate — a source over the failure cap is DROPPED
/// without even checking the code, so the throttle, not the 6 digits, is the
/// real brute-force defense; (2) match the presented code against the active
/// invitation (constant-time-ish, consumed on success); (3a) on a match,
/// mutually trust the joiner (store it under its handshake identity + DeviceInfo
/// name, `auto_accept` off — pairing establishes identity, not a standing
/// prompt-free grant), reply `PairConfirm{accept:true}`, and emit `pair_joined`;
/// (3b) on a miss, record the failure, reply `PairConfirm{accept:false}` with a
/// reason, and change nothing. A reject is a clean `Ok(())` for the listener
/// (like a declined file), not an error.
async fn handle_pair_request<R: Runtime>(
    session: &mut NoiseSession,
    ctx: &TransportCtx<R>,
    device_id: &str,
    code: String,
    peer: PeerHello,
) -> Result<()> {
    let peer_ip = session.peer_addr().map(|a| a.ip());
    let now = Instant::now();

    // (1) Rate gate — atomic check-and-RESERVE under one lock hold. A limited
    // source is DROPPED (no reply): answering would hand a brute-forcer a fast,
    // cheap accept/reject oracle. The join side surfaces the dropped connection as
    // a plain failure. Reserving the attempt here (rather than a read-only check
    // now and a `record_failure` later) is what closes the concurrent-burst race:
    // N simultaneous PairRequests from one IP each see the reservations of the
    // ones before them, so at most MAX_PAIR_FAILURES pass within a window instead
    // of all N reading a stale count-0. A poisoned lock fails open, as before. The
    // reservation is provisional — a miss against a live code keeps it (the real
    // failure), a successful pair clears it, and a request at an IDLE host rolls
    // it back below, so spamming an idle host still can't grow the map or drain
    // the per-source budget.
    if let Some(ip) = peer_ip {
        let allowed = ctx
            .pairing
            .rate
            .lock()
            .map(|mut r| r.reserve_attempt(ip, now))
            .unwrap_or(true);
        if !allowed {
            log::warn!("pairing: rate-limited PairRequest from {ip}; dropping");
            return Err(LanBeamError::Protocol("pairing attempts throttled".into()));
        }
    }

    // (2) Match against the active invitation. A poisoned lock fails toward
    // reject. Decide first (borrowing the guard), THEN mutate — a match consumes
    // the code (single-use) and an expired invitation is cleared in passing;
    // computing the `(clear, ok, had_session)` tuple up front releases the read
    // borrow before the write. `had_session` gates failure recording below: a
    // PairRequest arriving while NO invitation is active has nothing to
    // brute-force, so it must not be recorded (that is the unbounded-growth
    // amplification vector — spam PairRequests at an idle host to grow the map).
    let (matched, had_session) = match ctx.pairing.session.lock() {
        Ok(mut guard) => {
            let (clear, ok, present) = match guard.as_ref() {
                Some(sess) if now >= sess.expires => (true, false, true),
                Some(sess) if code_matches(&sess.code, &code) => (true, true, true),
                Some(_) => (false, false, true),
                None => (false, false, false),
            };
            if clear {
                *guard = None;
            }
            (ok, present)
        }
        Err(_) => (false, false),
    };

    let my_name = local_device_name(&ctx.settings);
    if matched {
        // (3a) The code was right, so the protocol accepts — but NO trust is
        // granted here. Accepting a code and trusting a device are two different
        // decisions, and only the second one needs a human: the SAS below is the
        // sole defence against a machine-in-the-middle, and a code nobody read is
        // not a check. `pair_joined` hands the UI the SAS; the UI records trust
        // (`set_trusted`) once the user confirms it matches the other screen.
        let joiner_name = peer
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(String::from)
            .unwrap_or_else(|| device_id.chars().take(8).collect());
        if let Some(ip) = peer_ip {
            if let Ok(mut r) = ctx.pairing.rate.lock() {
                r.clear(&ip);
            }
        }
        send_control(
            session,
            &AppMessage::PairConfirm {
                accept: true,
                name: my_name,
                reason: None,
            },
        )
        .await?;
        // Hand the UI who redeemed the code and the SAS to compare. This is the
        // ONLY route by which a pairing becomes trust on this side — if the user
        // never confirms, nothing was trusted.
        let _ = ctx.app.emit(
            "pair_joined",
            json!({ "deviceId": device_id, "name": joiner_name, "sas": session.sas() }),
        );
        log::info!(
            "pairing: {device_id} ({joiner_name:?}) redeemed the code; awaiting SAS confirm"
        );
        Ok(())
    } else {
        // (3b) No trust change. The failure was already recorded at the gate
        // (the reservation) so a guesser stays throttled — but that only belongs
        // when an invitation was actually active: a wrong code against a live code
        // is a real guess, whereas a PairRequest at an idle host is nothing to
        // brute-force, so keeping its reservation would let a stranger grow the
        // per-source failure map without ever touching a real code. Roll the
        // reservation back there to remove that amplification vector; the map is
        // also hard-capped.
        if !had_session {
            if let Some(ip) = peer_ip {
                if let Ok(mut r) = ctx.pairing.rate.lock() {
                    r.unreserve(&ip);
                }
            }
        }
        send_control(
            session,
            &AppMessage::PairConfirm {
                accept: false,
                name: my_name,
                reason: Some("pairing code rejected".into()),
            },
        )
        .await?;
        log::info!("pairing: rejected PairRequest from {device_id} (bad or expired code)");
        Ok(())
    }
}

/// Length-checked, non-short-circuiting equality for a pairing code (M7.1):
/// compares every byte so the time taken does not leak how many leading
/// characters matched — a brute-forcer must not learn "the first N were right"
/// from a faster reject. Not the primary defense (the per-source throttle is),
/// just no free timing oracle. The length check leaks only the code's length,
/// which is fixed and public (6 digits).
fn code_matches(expected: &str, got: &str) -> bool {
    let a = expected.as_bytes();
    let b = got.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

// ── quick-text receive side (M7.3) ─────────────────────────────────────
/// Handle an inbound quick text, gated on `TRANSFER_V2` upstream. Emits
/// `text_received` for the inbox, optionally mirrors the text to THIS machine's
/// clipboard, acks, and CLOSES — it never touches disk and never falls through
/// to the file-receive path.
///
/// Clipboard policy (privacy): the mirror happens only when BOTH the receiver's
/// own `clip_share` consent setting is on AND the sender asked for it
/// (`to_clipboard == Some(true)`, the UI's 同时写入对方剪贴板 toggle). The
/// receiver's setting is the master switch — no peer can place text on this
/// machine's clipboard unless the user opted into clipboard sharing — so a
/// sender's request alone is never enough. The write is best-effort: a failure
/// (or a build without the clipboard plugin) is logged, never fatal, and never
/// blocks the ack.
async fn handle_text_received<R: Runtime>(
    session: &mut NoiseSession,
    ctx: &TransportCtx<R>,
    device_id: &str,
    text: String,
    to_clipboard: Option<bool>,
    peer: PeerHello,
) -> Result<()> {
    // Bound memory before anything downstream sees the text (M4.5): a single
    // frame already caps the size, but clamp again so the inbox event, the
    // clipboard, and the logs never handle an over-long string.
    let text = clamp_text(text);

    // (1) Flood control (M7.3 hardening): cap how many texts one source may
    // deliver per window. Quick text has NO accept-prompt, so an unthrottled peer
    // could spam the inbox / notifications — this is the text analog of the
    // pairing throttle. A throttled text is DROPPED (not surfaced) but still
    // acked, so the sender resolves rather than hangs and gets no spam oracle.
    if let Some(ip) = session.peer_addr().map(|a| a.ip()) {
        let admitted = ctx
            .text_rate
            .lock()
            .map(|mut r| r.admit(ip, Instant::now()))
            .unwrap_or(true);
        if !admitted {
            log::warn!("quick text from {ip} throttled; dropping");
            ack_text(session, Some(TEXT_DROP_THROTTLED)).await;
            return Ok(());
        }
    }

    // (2) Delivery gate (M7.3 hardening): mirror the file path's recv_policy /
    // trust decision. The file path PARKS an untrusted sender for a prompt; quick
    // text has no prompt, so instead it is DROPPED unless the sender is trusted OR
    // the policy is "all" — text obeys the same "strangers don't get through under
    // ask/trusted" contract files do, and a stranger's note/link is never
    // surfaced unconditionally. This deliberately differs from `should_auto_accept`
    // (which returns false for EVERYONE under "ask", where files PROMPT): text
    // cannot prompt, so "ask"/"trusted" here mean "trusted senders only" rather
    // than "drop everything". A dropped text is still acked (no trust/policy
    // oracle to a stranger); a poisoned lock fails toward the safe side (drop for
    // any non-"all" policy).
    let policy = ctx
        .settings
        .read()
        .map(|s| s.recv_policy.clone())
        .unwrap_or_else(|_| "ask".to_string());
    let is_trusted = ctx
        .trusted
        .read()
        .ok()
        .is_some_and(|t| t.get(device_id).is_some());
    if !is_trusted && policy != "all" {
        log::info!("quick text from {device_id} dropped (untrusted, recv_policy \"{policy}\")");
        // Tell the sender it was dropped. Their `send_text` now fails instead of
        // reporting a delivery that never happened.
        ack_text(session, Some(TEXT_DROP_UNTRUSTED)).await;
        return Ok(());
    }

    // Friendly sender name: the peer's DeviceInfo name (present even for peers
    // absent from discovery), then the discovery table, then the short device
    // id — the same precedence the file-accept prompt uses.
    let sender_name = peer
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(String::from)
        .or_else(|| {
            ctx.peers
                .lock()
                .ok()
                .and_then(|t| t.get(device_id).map(|p| p.name.clone()))
        })
        .unwrap_or_else(|| device_id.chars().take(8).collect());

    // Receiver consent: mirror to the clipboard only when the user enabled
    // clip_share AND the sender requested it. Read the setting at fire time so a
    // just-toggled preference takes effect on the very next text.
    let clip_share = ctx.settings.read().map(|s| s.clip_share).unwrap_or(false);
    if clip_share && to_clipboard == Some(true) {
        write_clipboard(ctx, &text);
    }

    // Millisecond unix stamp so the inbox can render a real receive time.
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    log::info!(
        "quick text from {sender_name:?} ({device_id}, {} bytes)",
        text.len()
    );
    let _ = ctx.app.emit(
        "text_received",
        json!({ "deviceId": device_id, "senderName": sender_name, "text": text, "at": at }),
    );

    // Ack so the sender's `send_text` resolves; a failed ack write is only
    // logged (best-effort, like the file path's final ack) — the text already
    // reached the UI, so a lost ack must not turn a delivered text into an error.
    ack_text(session, None).await;
    Ok(())
}

/// Best-effort quick-text ack. The sender's `send_text` resolves on ANY ack, so
/// send one and only LOG a write failure — a text that was delivered (or was
/// deliberately dropped by the trust/flood gates) must never surface to the peer
/// as an error, and a dropped text must not hand the sender a delivery oracle.
/// Ack a quick text. `dropped` carries the reason when it did NOT reach the user,
/// so the sender can fail instead of reporting a delivery that never happened.
async fn ack_text(session: &mut NoiseSession, dropped: Option<&str>) {
    if let Err(e) = send_control(
        session,
        &AppMessage::Ack {
            of: TEXT_ACK.to_string(),
            delivered: Some(dropped.is_none()),
            reason: dropped.map(str::to_string),
        },
    )
    .await
    {
        log::warn!("quick-text ack send failed: {e}");
    }
}

/// Truncate a quick text to [`MAX_TEXT_BYTES`] on a char boundary (M7.3). A
/// single control frame already bounds the incoming size, so this only ever
/// trims a deliberately-oversized push — and never splits a multi-byte char, so
/// the emitted string is always valid UTF-8.
fn clamp_text(mut text: String) -> String {
    if text.len() <= MAX_TEXT_BYTES {
        return text;
    }
    let mut end = MAX_TEXT_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

/// Best-effort write to THIS machine's clipboard (M7.3). Uses `try_state`, NOT
/// the `ClipboardExt` accessor, for the same reason as `notify_system`: the ext
/// panics when the plugin is absent, and the integration tests drive the real
/// `handle_incoming` on a mock app with no plugins — a missing clipboard must
/// degrade to a no-op, never take the session down with it.
fn write_clipboard<R: Runtime>(ctx: &TransportCtx<R>, text: &str) {
    use tauri::Manager;
    let Some(clip) = ctx
        .app
        .try_state::<tauri_plugin_clipboard_manager::Clipboard<R>>()
    else {
        log::debug!("clipboard plugin unavailable; skipping clipboard mirror");
        return;
    };
    if let Err(e) = clip.write_text(text.to_string()) {
        log::warn!("clipboard write failed: {e}");
    }
}

/// One file's resume + collision analysis, computed before the accept prompt so
/// the reply can carry offsets (M6.4) and the event can list conflicts (M6.5).
struct FileAnalysis {
    /// Stable identity (sanitized rel + size + hash) — the partials-store key.
    identity: FileIdentity,
    /// Where a FRESH copy lands (auto-organize prefix applied, pre-de-dupe).
    organized_rel: PathBuf,
    /// `Some((disk_rel, offset))` when a valid persisted partial lets this file
    /// continue instead of starting over.
    resume: Option<(PathBuf, u64)>,
    /// A fresh write would collide with an existing file (feeds the conflict UI).
    collides: bool,
    /// The sender's declared name, for event display.
    name: String,
}

/// Analyze every manifest file for resume (M6.4) and collision (M6.5). Resume is
/// offered ONLY for a hashed file whose stored partial still matches the bytes
/// on disk (length == recorded, not a symlink) — the whole-file hash is what
/// makes continuing `old-bytes + tail` verifiable and safe against a sender that
/// ignores offsets, so a non-hashed file always starts fresh. Names are assumed
/// already validated by `validate_manifest`.
fn analyze_files(
    manifest: &Manifest,
    download_root: &Path,
    prefix: Option<&Path>,
    device_id: &str,
    partials: &RwLock<crate::partials::PartialsStore>,
) -> Vec<FileAnalysis> {
    manifest
        .files
        .iter()
        .map(|f| {
            let rel = match sanitize::validate(&f.name) {
                Decision::Accept(p) => p,
                // validate_manifest already passed; keep a harmless empty rel so
                // one odd entry cannot panic the analysis (the write path rejects).
                Decision::Reject(_) => PathBuf::new(),
            };
            let identity = FileIdentity {
                rel: rel.to_string_lossy().replace('\\', "/"),
                size: f.size,
                sha256: f.sha256.clone(),
            };
            let organized_rel = match prefix {
                Some(p) => p.join(&rel),
                None => rel.clone(),
            };
            // Resume only a hashed file with a matching, intact on-disk partial.
            let resume = if f.sha256.is_some() {
                partials
                    .read()
                    .ok()
                    .and_then(|s| s.lookup(device_id, &identity))
                    .filter(|rec| {
                        let disk = download_root.join(&rec.disk_rel);
                        match std::fs::symlink_metadata(&disk) {
                            Ok(m) if !m.file_type().is_symlink() => m.len() == rec.bytes_written,
                            _ => false,
                        }
                    })
                    .map(|rec| (PathBuf::from(&rec.disk_rel), rec.bytes_written))
            } else {
                None
            };
            // A fresh (non-resumed) write collides when its target already exists.
            let collides = resume.is_none()
                && std::fs::symlink_metadata(download_root.join(&organized_rel)).is_ok();
            FileAnalysis {
                identity,
                organized_rel,
                resume,
                collides,
                name: f.name.clone(),
            }
        })
        .collect()
}

/// Persist the resume-state change for `device_id` after a receive (M6.4):
/// `remove` = this manifest's identities, `keep` = the partials still on disk
/// (empty on success). Mutates under the write guard, snapshots, and writes on
/// the blocking pool — no disk I/O under the lock (the trust-store pattern).
fn persist_partials<R: Runtime>(
    ctx: &TransportCtx<R>,
    device_id: &str,
    remove: &[FileIdentity],
    keep: Vec<PartialRecord>,
) {
    let snapshot = {
        // Poisoned lock: skip — losing resume state costs at most a re-download,
        // never a cascade of the panic.
        let Ok(mut store) = ctx.partials.write() else {
            return;
        };
        if !store.set_files(device_id, remove, keep) {
            return; // nothing changed — no needless write
        }
        store.snapshot()
    };
    crate::partials::persist_async(snapshot);
}

/// Today's date as `YYYY-MM-DD` (UTC) for the auto-organize date folder (M6.6).
/// Hand-rolled via [`civil_from_days`] to avoid a date-crate dependency for one
/// call site.
fn today_ymd() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian (Howard
/// Hinnant's `civil_from_days`). Duplicated from `commands.rs` deliberately:
/// both are one-call-site date helpers with no shared home, and a `pub` export
/// would leak a diagnostics-only detail across module boundaries.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Throttled progress emit — only fires on a whole-percent change.
pub fn emit_progress<R: Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    direction: &str,
    done: u64,
    total: u64,
    last_pct: &mut i64,
) {
    let pct = if total == 0 {
        100
    } else {
        ((done as u128 * 100) / total as u128) as i64
    };
    if pct != *last_pct {
        *last_pct = pct;
        let _ = app.emit(
            "transfer_progress",
            json!({ "sessionId": session_id, "direction": direction, "done": done, "total": total, "percent": pct }),
        );
    }
}

#[cfg(test)]
mod tests {
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

        let gate = Arc::new(crate::state::ConcurrencyGate::new());
        let _held = gate.try_acquire(1).expect("first slot is free");
        assert!(
            gate.try_acquire(1).is_none(),
            "the gate is now full — the next transfer queues"
        );

        let ctl = TransferControl::neutral();
        ctl.cancel.cancel(); // cancelled while parked on the gate

        let outcome: Result<()> = tokio::select! {
            biased;
            _ = ctl.cancel.cancelled() => Err(LanBeamError::Cancelled),
            _g = gate.acquire(1) => Ok(()),
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
                FileEvent::PauseAutoResumed => {}
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
}
