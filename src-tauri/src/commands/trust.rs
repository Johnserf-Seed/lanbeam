//! Trust store (M4.4): the trusted-peer list and the commands that change it.

use tauri::{AppHandle, Emitter, State};

use super::devices::list_devices_snapshot;
use crate::state::AppState;
use crate::transport;
use crate::trust::{self, TrustedPeerDto};

/// The trusted-peer list, sorted for display.
#[tauri::command]
pub fn list_trusted(state: State<'_, AppState>) -> Vec<TrustedPeerDto> {
    // Poisoned lock: an empty list beats panicking the whole invoke — the UI
    // refreshes on the next `trust_updated` anyway.
    state.trusted.read().map(|t| t.list()).unwrap_or_default()
}

/// Add or update a trusted peer. Persists, then emits `trust_updated` with
/// the full list. Malformed device ids (not a base64url 32-byte key) are
/// ignored — same setters-ignore-invalid contract as `set_log_level`.
/// `paired_at`/`last_seen` (unix seconds, optional — additive for the
/// localStorage migration) seed a NEW entry's timestamps so migrated records
/// keep their original dates; updates ignore them (see `TrustStore::set`).
#[tauri::command]
pub fn set_trusted(
    app: AppHandle,
    state: State<'_, AppState>,
    device_id: String,
    name: String,
    auto_accept: bool,
    paired_at: Option<u64>,
    last_seen: Option<u64>,
) {
    if transport::decode_device_id(&device_id).is_none() {
        return;
    }
    // Never trust yourself. Not a philosophical position — a self-record shows up
    // in your own trust circle as a peer you trust and can never reach (discovery
    // drops its own announces, so it is permanently "offline"), and it grants
    // nothing: you do not need permission to receive from yourself. Startup evicts
    // any that an older build let in.
    if device_id == state.identity.device_id() {
        log::debug!("trust: refusing a self-record");
        return;
    }
    // Mutate + snapshot under the write guard; the file write and the emit
    // happen AFTER it drops, so a slow disk (AV scan, sleeping HDD) never
    // stalls readers — or this main-thread command — on the trust lock (M4.6).
    let (snapshot, list) = {
        let Ok(mut store) = state.trusted.write() else {
            return;
        };
        store.set(device_id, name, auto_accept, paired_at, last_seen);
        (store.snapshot(), store.list())
    };
    trust::persist_async(snapshot);
    trust::emit_updated(&app, list);
}

/// Delete a device: drop BOTH its trust record and the manually-added address
/// that keeps it in the device list. Emits `trust_updated` and `devices_updated`
/// for whatever actually changed.
///
/// A device that is still announcing itself on the LAN will come back — as a
/// stranger. This erases LanBeam's memory of the device, not the machine.
///
/// Untrusting (`remove_trusted`) is deliberately NOT this: a device you stop
/// trusting is still one you want to be able to reach. Deleting is the stronger
/// act, and until now it did not exist. `remove_trusted` was all the UI had, so
/// "delete" cleared the trust row while the peer sat untouched in the manual
/// address table — reappearing on the very next list, with nothing anywhere able
/// to take it out. (Dial your own address once and it was permanent: your own id
/// can't be discovered away, because discovery drops its own announces.)
#[tauri::command]
pub fn forget_device(app: AppHandle, state: State<'_, AppState>, device_id: String) {
    // Trust row: same shape as `remove_trusted` — snapshot under the guard, then
    // I/O + emit after it drops.
    let trust_change = match state.trusted.write() {
        // A pattern guard can't mutate, so the remove has to happen in the body.
        Ok(mut store) => store
            .remove(&device_id)
            .then(|| (store.snapshot(), store.list())),
        Err(_) => None,
    };
    if let Some((snapshot, list)) = trust_change {
        trust::persist_async(snapshot);
        trust::emit_updated(&app, list);
    }
    // Manual address. This is the half that was missing.
    if forget_manual_peer(&state, &device_id) {
        let _ = app.emit("devices_updated", list_devices_snapshot(&state));
    }
}

/// Drop a device's hand-added address, from the table and from its file — the
/// file matters as much: left there, the device is back after a restart.
/// `true` when there was one to drop.
fn forget_manual_peer(state: &AppState, device_id: &str) -> bool {
    let dropped = match state.manual_peers.lock() {
        Ok(mut m) => m.remove(device_id).then(|| m.snapshot()),
        Err(_) => None,
    };
    let Some(snapshot) = dropped else {
        return false;
    };
    crate::manual_peers::persist_async(snapshot);
    true
}

/// Remove a peer from the trust store. Persists + emits `trust_updated` only
/// when something was actually deleted.
#[tauri::command]
pub fn remove_trusted(app: AppHandle, state: State<'_, AppState>, device_id: String) {
    // Same shape as `set_trusted`: snapshot under the guard, I/O + emit after.
    let (snapshot, list) = {
        let Ok(mut store) = state.trusted.write() else {
            return;
        };
        if !store.remove(&device_id) {
            return; // nothing deleted — no save, no event
        }
        (store.snapshot(), store.list())
    };
    trust::persist_async(snapshot);
    trust::emit_updated(&app, list);
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::commands::test_support::test_app_state;
    use crate::manual_peers::{ManualPeer, ManualPeers};

    /// 删除设备 sticks: the address leaves the file as well as the table, so the
    /// device does not come back on the next start.
    #[test]
    fn forgotten_address_leaves_the_file() {
        let dir = std::env::temp_dir().join(format!("lanbeam-forget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("manual_peers.json");
        let mut table = ManualPeers::load(file.clone());
        table.insert(
            "nas".into(),
            ManualPeer {
                name: "NAS".into(),
                addr: Ipv4Addr::new(10, 2, 0, 9),
                port: 52000,
                last_used: Instant::now(),
            },
        );
        table.snapshot().unwrap().persist();
        let state = test_app_state();
        *state.manual_peers.lock().unwrap() = table;

        assert!(forget_manual_peer(&state, "nas"));
        assert!(!forget_manual_peer(&state, "nas"), "nothing left to drop");
        assert!(state.manual_peers.lock().unwrap().get("nas").is_none());
        // Written on the blocking pool — give it a moment.
        let gone = (0..200).any(|_| {
            let on_disk = ManualPeers::load(file.clone()).get("nas").is_some();
            if on_disk {
                std::thread::sleep(Duration::from_millis(10));
            }
            !on_disk
        });
        assert!(gone, "the forgotten address must not survive a restart");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
