//! File transfer over an authenticated Noise channel. Sender streams a paged manifest
//! then chunked file bytes; receiver sanitizes every name, writes under the download
//! root, and acks. DESIGN §1.4, §3.5. The commands that drive it are in `commands/`.
//!
//! One module per stage: the wire helpers, the send list, pacing, progress,
//! the send side, the receive side, inbound orchestration, auto-organize, the
//! pairing host and the quick-text receiver. Every public item is re-exported below, so the
//! `transfer::…` paths callers use are unchanged.

mod inbound;
mod organize;
mod pacing;
mod pairing_host;
mod progress;
mod receive;
mod send;
mod send_list;
mod text;
mod wire;

pub use inbound::handle_incoming;
pub(crate) use inbound::{human_size, peer_display_name};
pub use pairing_host::normalize_pair_code;
pub use progress::{emit_progress, file_event_sink, FileEvent};
pub use receive::{read_manifest, run_receive, send_reply, validate_manifest, PendingState};
pub use send::run_send;
pub use send_list::{build_send_list, build_send_list_scoped, check_send_selection, SendScratch};
pub use wire::{
    initiator_hello, local_device_name, open_inbound, recv_pair_confirm, send_bye,
    send_pair_request, send_text, InboundOpen, PeerHello,
};

use std::path::PathBuf;

use crate::protocol::FileMeta;

// `tests.rs` predates the split: the helpers it tests, and the names its
// fixtures use, reach it through `use super::*`.
#[cfg(test)]
use {
    crate::consts::{
        ACK_TIMEOUT, ACK_TIMEOUT_MAX, CHUNK_IDLE_TIMEOUT, MAX_MANIFEST_FILES,
        MAX_MANIFEST_NAME_BYTES, MAX_MANIFEST_NAME_TOTAL, MAX_PAUSE_PARK, MAX_TEXT_BYTES,
    },
    crate::error::{LanBeamError, Result},
    crate::exif,
    crate::partials::{FileIdentity, PartialRecord},
    crate::protocol::{self, AppMessage, Frame, ResumeOffset},
    crate::sanitize::{self, Decision},
    crate::state::{ConflictAction, PendingMap, ReplyDecision, TransferControl},
    crate::transport::noise::NoiseSession,
    inbound::{
        analyze_files, park_pending, remove_pending_if_owner, today_ymd, ymd_in, InFlightGuard,
    },
    pacing::wait_while_paused_capped,
    pairing_host::code_matches,
    receive::{
        is_single_clean_name, peer_ack_error, resume_offsets_for_reply, run_receive_core,
        send_pending, FileDisposition, FileProgress, REPLY_BUSY, REPLY_CANCELLED, REPLY_DECLINED,
        REPLY_UNSAFE_NAME,
    },
    send::ack_deadline,
    send_list::{check_selection_within, to_hex},
    sha2::{Digest, Sha256},
    std::collections::HashSet,
    std::sync::atomic::Ordering,
    std::sync::{Mutex, RwLock},
    std::time::{Duration, Instant},
    text::clamp_text,
    tokio::sync::oneshot,
    wire::{device_info_msg, recv_control, recv_frame, send_control},
};

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

#[cfg(test)]
mod tests;
