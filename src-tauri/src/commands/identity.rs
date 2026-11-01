//! This device's public identity, and its factory reset (M5.7).

use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::error::LanBeamError;
use crate::state::AppState;
use crate::{discovery, identity, settings};

/// Public identity shown to the user. Never includes the private key.
#[derive(Serialize)]
pub struct MyIdentity {
    #[serde(rename = "deviceId")]
    pub device_id: String,
    #[serde(rename = "shortId")]
    pub short_id: String,
    pub name: String,
}

#[tauri::command]
pub fn get_my_identity(state: State<'_, AppState>) -> MyIdentity {
    let device_id = state.identity.device_id();
    let short_id = device_id.chars().take(8).collect();
    // device_id/short_id come from the immutable identity Arc; only the name
    // reads the shared settings lock. A poisoned lock falls back to the default
    // device name rather than panicking the invoke.
    let name = state
        .settings
        .read()
        .map(|s| s.device_name.clone())
        .unwrap_or_else(|_| settings::default_device_name());
    MyIdentity {
        device_id,
        short_id,
        name,
    }
}

// ── identity reset (M5.7) ──────────────────────────────────────────────

/// Factory-reset this device's identity. After it, other devices see us as a
/// brand-new device and MUST re-verify (their pinned Device ID no longer
/// matches), and our own trust list starts empty.
///
/// Order matters: (1) go invisible NOW — discoverable off in-memory plus one
/// best-effort tombstone broadcast — so peers drop the old ID immediately
/// instead of showing a ghost until expiry; (2) delete the stored identity,
/// the only step that can fail, placed before anything irreversible happens;
/// (3) clear the trust store, persisted before the restart; (4) restart —
/// the identity is snapshotted at startup and threaded through the live
/// listener/announcer, so hot-swapping the keypair is not realistic, and a
/// fresh one is generated on the way back up.
#[tauri::command]
pub async fn reset_identity(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), LanBeamError> {
    // (1) In-memory only, deliberately NOT persisted: the next launch (fresh
    // identity) comes up with whatever discoverability the user configured.
    // Capture the PRIOR discoverable so step (2) can roll it back if the
    // identity delete fails — otherwise the box goes silently invisible
    // (tombstone already out, announces stopped) while the UI still shows
    // 可被发现. A poisoned lock degrades to an empty tombstone name — removal is
    // keyed on the id, and a recovery action must not be blocked by a lock.
    let (prev_disc, name) = state
        .settings
        .write()
        .map(|mut s| {
            let prev = s.discoverable;
            s.discoverable = false;
            (prev, s.device_name.clone())
        })
        .unwrap_or((false, String::new()));
    discovery::send_tombstone(
        state.identity.device_id(),
        name,
        state.tcp_port.load(Ordering::Relaxed),
    )
    .await;

    // (2) The point of commitment — but failing here must not strand the box
    // invisible: restore the discoverable flag flipped in (1) before returning,
    // so the announce loop re-announces within one 2s tick and peers that
    // consumed the best-effort tombstone re-add the device. The identity itself
    // is intact, so the UI can simply show the error and stop.
    if let Err(e) = identity::delete_persisted(
        crate::instance_id().as_deref(),
        &crate::paths::data_dir(&app),
    ) {
        if let Ok(mut s) = state.settings.write() {
            s.discoverable = prev_disc;
        }
        return Err(e);
    }

    // (3) Forget every pairing decision, and WAIT for the file write — the
    // restart below must not race the persist. A poisoned trust lock is only
    // logged: the identity is already gone, so aborting now would strand the
    // user half-reset with no way forward.
    let snapshot = match state.trusted.write() {
        Ok(mut t) => {
            if t.clear() {
                t.snapshot()
            } else {
                None // already empty — nothing to persist
            }
        }
        Err(_) => {
            log::warn!("trust lock poisoned; trust file left as-is during reset");
            None
        }
    };
    if let Some(snap) = snapshot {
        let _ = tauri::async_runtime::spawn_blocking(move || snap.persist()).await;
    }

    log::info!("identity reset: stored identity deleted, trust cleared; restarting");
    // (4) Mark quit intent so close-to-tray can never intercept teardown.
    state.quitting.store(true, Ordering::Relaxed);
    app.restart()
}
