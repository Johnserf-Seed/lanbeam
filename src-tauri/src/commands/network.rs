//! This machine's network endpoints (M5.1), and the degradations recorded
//! when the listeners bound (M4.6).

use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::State;

use super::pairing::lan_ip_rank;
use crate::discovery;
use crate::state::{AppState, NetDegraded};

// ── network info (M5.1) ────────────────────────────────────────────────

/// One local IPv4 endpoint as the sidebar/settings page shows it.
#[derive(Serialize)]
pub struct NetworkInfo {
    pub ip: String,
    /// `null` for interfaces without a broadcast address (e.g. P2P links).
    pub broadcast: Option<String>,
}

/// The machine's non-loopback IPv4 addresses — the same enumeration
/// discovery announces on. Sorted numerically because OS enumeration order
/// can reshuffle between calls, and the UI list must not jump around.
#[tauri::command]
pub fn get_network_info() -> Vec<NetworkInfo> {
    let mut ifaces = discovery::interfaces::enumerate();
    // Same order `first_lan_ip` picks from, so the address the sidebar shows
    // first is the one the pair link / invite carries (real LAN ranges before
    // virtual-adapter ranges; by IP within a rank, for a stable list).
    ifaces.sort_by_key(|i| (lan_ip_rank(i.ip), i.ip));
    ifaces
        .into_iter()
        .map(|i| NetworkInfo {
            ip: i.ip.to_string(),
            broadcast: i.broadcast.map(|b| b.to_string()),
        })
        .collect()
}

/// The TCP port the transfer listener is bound to right now — the port peers
/// dial and firewall rules should target. It can differ from the `port`
/// setting, which only takes effect on the next launch.
#[tauri::command]
pub fn get_listen_port(state: State<'_, AppState>) -> u16 {
    state.tcp_port.load(Ordering::Relaxed)
}

// ── network status (M4.6) ──────────────────────────────────────────────

/// Degradations recorded at bind time (UDP receive fallback, TCP ephemeral
/// port). The UI queries this once at startup: the matching `net_degraded`
/// events fire during `setup()`, before the webview has any listener, and
/// Tauri events have no replay — without this pull path the toasts for the
/// most likely trigger (a port already held at launch) can never appear.
#[tauri::command]
pub fn get_net_status(state: State<'_, AppState>) -> Vec<NetDegraded> {
    state.degraded.lock().map(|v| v.clone()).unwrap_or_default()
}
