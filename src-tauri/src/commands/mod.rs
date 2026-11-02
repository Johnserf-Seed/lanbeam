//! Tauri commands (UI → Rust). M1: identity + settings.
//! `camelCase` on the JS side is auto-converted to `snake_case` params here.
//!
//! One module per area. Every command `lib.rs` registers is re-exported
//! below, so `generate_handler!` keeps naming each one by its bare name.

mod devices;
mod diag;
mod identity;
mod network;
mod pairing;
mod settings;
mod share;
mod transfer;
mod trust;

pub use devices::list_discovered_devices;
pub(crate) use devices::merge_devices;
pub use diag::{export_diagnostics, get_log_dir};
pub use identity::{get_my_identity, reset_identity};
pub use network::{get_listen_port, get_net_status, get_network_info};
pub use pairing::{
    cancel_pairing, connect_by_addr, join_by_code, start_pairing, take_pending_deep_link,
};
pub use settings::{
    get_settings, set_auto_open, set_autostart, set_clip_share, set_conflict_policy,
    set_device_name, set_discoverable, set_download_dir, set_hotkey, set_hotkey_enabled,
    set_iface_filter, set_listen_port, set_log_level, set_max_concurrent, set_notif_system,
    set_organize, set_rate_limit, set_recv_policy, set_strip_exif, set_tray_close, set_ui_zoom,
    set_verify_hash,
};
pub(crate) use share::emit_shares;
pub use share::{list_shares, start_share, stop_share, update_share};
pub use transfer::{
    cancel_transfer, discard_partials, get_download_dir, list_partials, open_local_path,
    pause_transfer, pending_requests, reply_file_request, resume_transfer, reveal_received,
    send_files, send_text,
};
pub(crate) use transfer::{send_scratch_root, sweep_send_scratch};
pub use trust::{forget_device, list_trusted, remove_trusted, set_trusted};

#[cfg(test)]
mod test_support {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use crate::state::AppState;
    use crate::{discovery, identity, settings};

    /// A minimal `AppState` with every table empty — the base both the manual-peer
    /// tests and the browser-share tests build on, so the big literal lives once.
    pub(super) fn test_app_state() -> AppState {
        use std::sync::atomic::{AtomicBool, AtomicU16};
        use std::sync::{Arc, Mutex, RwLock};

        let tmp = std::env::temp_dir().join(format!("lanbeam-cmd-{}", std::process::id()));
        AppState {
            identity: Arc::new(identity::generate_identity().unwrap()),
            settings: Arc::new(RwLock::new(settings::Settings::default())),
            peers: Arc::new(Mutex::new(discovery::PeerTable::new())),
            tcp_port: Arc::new(AtomicU16::new(0)),
            download_dir: Arc::new(RwLock::new(PathBuf::from("."))),
            pending: Arc::new(Mutex::new(HashMap::new())),
            trusted: Arc::new(RwLock::new(crate::trust::TrustStore::load(
                tmp.join("trusted.json"),
            ))),
            completed: Arc::new(Mutex::new(crate::state::CompletedLog::new())),
            degraded: Arc::new(Mutex::new(Vec::new())),
            quitting: Arc::new(AtomicBool::new(false)),
            transfers_ctl: Arc::new(Mutex::new(HashMap::new())),
            partials: Arc::new(RwLock::new(crate::partials::PartialsStore::load(
                tmp.join("partials.json"),
            ))),
            concurrency: Arc::new(crate::state::ConcurrencyGate::new(3)),
            pairing: Arc::new(crate::state::PairingState::default()),
            manual_peers: Default::default(),
            shares: crate::share::new_registry(),
            share_port: Arc::new(AtomicU16::new(0)),
            pending_deep_link: Arc::new(Mutex::new(None)),
        }
    }
}
