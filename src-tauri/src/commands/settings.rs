//! Settings: the snapshot the UI reads, and one setter per preference — each
//! validating its input and persisting it, several also applying it live.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::LanBeamError;
use crate::settings;
use crate::state::AppState;

#[derive(Serialize)]
pub struct SettingsDto {
    #[serde(rename = "deviceName")]
    pub device_name: String,
    pub discoverable: bool,
    #[serde(rename = "autoOpenFolder")]
    pub auto_open_folder: bool,
    #[serde(rename = "logLevel")]
    pub log_level: String,
    #[serde(rename = "recvPolicy")]
    pub recv_policy: String,
    /// Omitted (not `null`) when unset — `downloadDirOverride?` on the TS
    /// side, additive for frontends that predate it (M5.2).
    #[serde(
        rename = "downloadDirOverride",
        skip_serializing_if = "Option::is_none"
    )]
    pub download_dir_override: Option<String>,
    /// `0` = LanBeam's default port.
    pub port: u16,
    /// Whether closing the window hides to the tray instead of quitting (M5.3).
    #[serde(rename = "trayClose")]
    pub tray_close: bool,
    /// OS notifications for incoming prompts / finished receives (M5.4).
    #[serde(rename = "notifSystem")]
    pub notif_system: bool,
    /// Launch at login (M5.5); the setter mirrors it into the OS entry.
    pub autostart: bool,
    /// Discovery interface filter (M5.6). Omitted when unset — same additive
    /// contract as `downloadDirOverride`.
    #[serde(rename = "ifaceFilter", skip_serializing_if = "Option::is_none")]
    pub iface_filter: Option<String>,
    /// Whether the global quick-summon hotkey is registered (M5.5). Additive
    /// camelCase; defaults false so the app never claims Alt+Space unasked.
    #[serde(rename = "hotkeyEnabled")]
    pub hotkey_enabled: bool,
    /// The accelerator the quick-summon hotkey binds (M5.5 rebind), in the
    /// `MOD+KEY` form (e.g. `Alt+Space`). Additive camelCase; the UI formats it
    /// for display but stores this canonical string.
    pub hotkey: String,
    /// Per-file SHA-256 integrity verification (M6.3). Additive camelCase;
    /// defaults true so a fresh install verifies out of the box.
    #[serde(rename = "verifyHash")]
    pub verify_hash: bool,
    /// Name-collision policy (M6.5): `"rename" | "overwrite" | "ask"`. Additive
    /// camelCase; the `"ask"` default drives the ConflictModal.
    pub conflict: String,
    /// Auto-organize mode (M6.6): `"none" | "device" | "date"`. Additive
    /// camelCase; defaults `"none"` (write straight under the download root).
    pub organize: String,
    /// Concurrency cap (M6.7): how many transfers stream at once. Additive
    /// camelCase; defaults 3, range 1..=8.
    #[serde(rename = "maxConcurrent")]
    pub max_concurrent: u32,
    // H12: the one field that went out snake_case. The UI reads `uiZoom`, got
    // undefined, and its first Ctrl+= stepped DOWN (stepZoom(undefined) lands on
    // the 0.9 notch); the zoom select showed nothing. A contract test in
    // src/lib/dtoContract.test.ts now holds every field to the UI's type.
    #[serde(rename = "uiZoom")]
    pub ui_zoom: f64,
    /// Per-transfer throughput cap (M6.7): `"unlimited"` or MB/s. Additive
    /// camelCase; defaults `"unlimited"`.
    #[serde(rename = "rateLimit")]
    pub rate_limit: String,
    /// Whether an incoming quick text is also written to this machine's
    /// clipboard (M7.3). Additive camelCase; defaults false (opt-in consent).
    #[serde(rename = "clipShare")]
    pub clip_share: bool,
    /// Strip photo metadata (EXIF/ICC/XMP) from images before sending (M9.1).
    /// Additive camelCase; defaults true so location/camera data does not leak.
    #[serde(rename = "stripExif")]
    pub strip_exif: bool,
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> SettingsDto {
    // Poisoned lock: return a snapshot built from `Settings::default()` rather
    // than panic the invoke — an honest default shape the UI can render, same
    // graceful contract as the other readers. The next explicit read refreshes.
    let s = state.settings.read().map(|g| g.clone()).unwrap_or_default();
    SettingsDto {
        device_name: s.device_name.clone(),
        discoverable: s.discoverable,
        auto_open_folder: s.auto_open_folder,
        log_level: s.log_level.clone(),
        recv_policy: s.recv_policy.clone(),
        download_dir_override: s.download_dir_override.clone(),
        port: s.port,
        tray_close: s.tray_close,
        notif_system: s.notif_system,
        autostart: s.autostart,
        iface_filter: s.iface_filter.clone(),
        hotkey_enabled: s.hotkey_enabled,
        hotkey: s.hotkey.clone(),
        verify_hash: s.verify_hash,
        conflict: s.conflict.clone(),
        organize: s.organize.clone(),
        max_concurrent: s.max_concurrent,
        ui_zoom: s.ui_zoom,
        rate_limit: s.rate_limit.clone(),
        clip_share: s.clip_share,
        strip_exif: s.strip_exif,
    }
}

/// Snapshot-under-guard settings write shared by the simple setters (M5–M9):
/// take the write guard, apply `f`, clone the new state, then persist AFTER the
/// guard drops — never any disk I/O under the settings lock. A poisoned lock is
/// a graceful no-op, the same contract every setter upheld inline before. The
/// bespoke setters (live re-registration, OS reconcile, `Result`-returning)
/// keep their own bodies; this is only for the pure snapshot+mutate+save ones.
fn update_settings(app: &AppHandle, state: &AppState, f: impl FnOnce(&mut settings::Settings)) {
    let snapshot = {
        let Ok(mut s) = state.settings.write() else {
            return;
        };
        f(&mut s);
        s.clone()
    };
    settings::save(app, &snapshot);
}

#[tauri::command]
pub fn set_device_name(app: AppHandle, state: State<'_, AppState>, name: String) {
    let clamped: String = name
        .trim()
        .chars()
        .take(crate::consts::MAX_NAME_CHARS)
        .collect();
    update_settings(&app, &state, |s| {
        s.device_name = if clamped.is_empty() {
            settings::default_device_name()
        } else {
            clamped
        };
    });
}

#[tauri::command]
pub fn set_discoverable(app: AppHandle, state: State<'_, AppState>, discoverable: bool) {
    update_settings(&app, &state, |s| s.discoverable = discoverable);
}

#[tauri::command]
pub fn set_auto_open(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    update_settings(&app, &state, |s| s.auto_open_folder = enabled);
}

/// Set the diagnostics log level (`"errors" | "normal" | "verbose"`).
/// Unknown values are ignored so the stored blob only ever holds a level
/// the logging layer can map.
#[tauri::command]
pub fn set_log_level(app: AppHandle, state: State<'_, AppState>, level: String) {
    if !settings::LOG_LEVELS.contains(&level.as_str()) {
        return;
    }
    update_settings(&app, &state, |s| s.log_level = level);
}

/// Set the inbound transfer policy (`"ask" | "trusted" | "all"`).
/// Unknown values are ignored — same contract as `set_log_level`.
#[tauri::command]
pub fn set_recv_policy(app: AppHandle, state: State<'_, AppState>, policy: String) {
    if !settings::RECV_POLICIES.contains(&policy.as_str()) {
        return;
    }
    update_settings(&app, &state, |s| s.recv_policy = policy);
}

/// Set the folder incoming files are saved to. The path must be an existing
/// directory; it is canonicalized before use. Returns the canonical path as
/// stored and shown by `get_download_dir`.
#[tauri::command]
pub fn set_download_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<String, LanBeamError> {
    // Reject bad input up front (nonexistent path, plain file) — receiving
    // must never be pointed at something writes can't land in (M5.2).
    let canon = settings::canonical_dir(&path)
        .ok_or_else(|| LanBeamError::Io(format!("not an existing directory: {path}")))?;
    // Retarget the LIVE root first, so the very next inbound session lands in
    // the new folder without a restart; sessions already streaming keep the
    // root they snapshotted at start (see `transfer::handle_incoming`).
    match state.download_dir.write() {
        Ok(mut d) => *d = canon.clone(),
        // Poisoned: keep serving the old root rather than panic the command;
        // the persisted override below still applies on the next launch.
        Err(_) => log::warn!("download_dir lock poisoned; live root unchanged"),
    }
    let stored = canon.display().to_string();
    let snapshot = {
        let Ok(mut s) = state.settings.write() else {
            // The live root DID change — report success so the UI reflects it.
            return Ok(stored);
        };
        s.download_dir_override = Some(stored.clone());
        s.clone()
    };
    settings::save(&app, &snapshot);
    Ok(stored)
}

/// Set the TCP listen port (`0` = LanBeam's default). Persists only — it
/// applies on the next launch, while discovery keeps advertising the port
/// actually bound now, so peers stay correct throughout. Privileged values
/// (1..=1023) are ignored — same contract as `set_log_level`.
#[tauri::command]
pub fn set_listen_port(app: AppHandle, state: State<'_, AppState>, port: u16) {
    if !settings::valid_listen_port(port) {
        return;
    }
    let snapshot = {
        let Ok(mut s) = state.settings.write() else {
            return;
        };
        s.port = port;
        s.clone()
    };
    settings::save(&app, &snapshot);
}

/// Toggle "closing the window hides to the tray" (M5.3). Effective on the
/// very next close — the window-event handler reads the live setting each
/// time, so no restart is involved.
#[tauri::command]
pub fn set_tray_close(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    update_settings(&app, &state, |s| s.tray_close = enabled);
}

/// Toggle OS notifications (M5.4). Effective on the very next prompt or
/// completion — the notify path reads the live setting each time.
#[tauri::command]
pub fn set_notif_system(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    update_settings(&app, &state, |s| s.notif_system = enabled);
}

/// Toggle launch-at-login (M5.5). The OS entry changes FIRST and the setting
/// persists only after that succeeds — persisting a toggle the OS refused
/// would show the user a lie that startup "reconciles" back into the failure
/// on every launch.
#[tauri::command]
pub fn set_autostart(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), LanBeamError> {
    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        let autolaunch = app.autolaunch();
        // Skip the OS call when it already agrees: `disable` on some platforms
        // errors when no entry exists, and that must not fail a no-op toggle.
        let already = autolaunch.is_enabled().unwrap_or(!enabled);
        if already != enabled {
            let res = if enabled {
                autolaunch.enable()
            } else {
                autolaunch.disable()
            };
            res.map_err(|e| LanBeamError::Io(e.to_string()))?;
        }
    }
    let snapshot = {
        let Ok(mut s) = state.settings.write() else {
            return Ok(());
        };
        s.autostart = enabled;
        s.clone()
    };
    settings::save(&app, &snapshot);
    Ok(())
}

/// Set the discovery interface filter (M5.6): an interface's IPv4 address as
/// shown by `get_network_info`, empty to clear (= all interfaces). Applied by
/// the announce loop within one 2s tick — no restart. Unparseable input is
/// ignored — same contract as `set_log_level`.
#[tauri::command]
pub fn set_iface_filter(app: AppHandle, state: State<'_, AppState>, ip: String) {
    let Some(value) = settings::parse_iface_filter(&ip) else {
        return;
    };
    update_settings(&app, &state, |s| s.iface_filter = value);
}

/// Toggle the global quick-summon hotkey (M5.5). Registers or unregisters the
/// currently-configured chord immediately, so the change takes effect without a
/// restart. A registration failure (another app already owns the chord) is
/// reported to the log and the preference is still saved — the same
/// warn-and-continue behavior as startup, so the toggle never fails the command.
#[tauri::command]
pub fn set_hotkey_enabled(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    #[cfg(desktop)]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        // Bind/unbind the user's CONFIGURED combo, not the hardcoded default —
        // a prior `set_hotkey` may have rebound it. The const is only the
        // fall-back when the lock is unreadable.
        let combo = state
            .settings
            .read()
            .map(|s| s.hotkey.clone())
            .unwrap_or_else(|_| crate::consts::DEFAULT_HOTKEY.to_string());
        if enabled {
            if let Err(e) = crate::register_summon_hotkey(&app, &combo) {
                log::warn!(
                    "global shortcut {combo} could not be registered (held by another app?): {e}"
                );
            }
        } else if let Err(e) = app.global_shortcut().unregister(combo.as_str()) {
            log::warn!("global shortcut {combo} unregister failed: {e}");
        }
    }
    let snapshot = {
        let Ok(mut s) = state.settings.write() else {
            return;
        };
        s.hotkey_enabled = enabled;
        s.clone()
    };
    settings::save(&app, &snapshot);
}

/// Rebind the global quick-summon accelerator (M5.5). Pass a shortcut in the
/// accelerator form `MOD(+MOD)*+KEY` — one or more of `Ctrl` / `Alt` / `Shift` /
/// `Super` plus one key, e.g. `Alt+Space` or `Ctrl+Shift+K`. A malformed or
/// modifier-less accelerator is refused. While the hotkey is enabled the new
/// chord is bound live and the old one released; if another app already holds it
/// the previous binding is kept and this returns an error to surface. Effective
/// immediately — no restart.
#[tauri::command]
pub fn set_hotkey(
    app: AppHandle,
    state: State<'_, AppState>,
    combo: String,
) -> Result<(), LanBeamError> {
    let combo = combo.trim().to_string();
    if !settings::valid_hotkey(&combo) {
        return Err(LanBeamError::Protocol(format!(
            "not a valid shortcut: {combo}"
        )));
    }
    // Snapshot the previous combo + whether it is live, so a rejected rebind can
    // leave the old binding untouched.
    let (old, enabled) = {
        let Ok(s) = state.settings.read() else {
            return Err(LanBeamError::Io("settings unavailable".into()));
        };
        (s.hotkey.clone(), s.hotkey_enabled)
    };
    if combo == old {
        return Ok(()); // no change — nothing to rebind or persist
    }
    #[cfg(desktop)]
    if enabled {
        // Bind the NEW chord FIRST: if the plugin refuses it (already held by the
        // OS/another app) the OLD chord was never unregistered, so the existing
        // binding stays live and we surface the conflict without persisting.
        crate::register_summon_hotkey(&app, &combo).map_err(|e| {
            LanBeamError::Io(format!("hotkey {combo} could not be registered: {e}"))
        })?;
        // The new chord is live — release the old one. A failure here only leaks
        // one stale registration; log it rather than fail the (successful) rebind.
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Err(e) = app.global_shortcut().unregister(old.as_str()) {
            log::warn!("global shortcut {old} unregister after rebind failed: {e}");
        }
    }
    #[cfg(not(desktop))]
    let _ = enabled; // no global-shortcut plugin off desktop; persist only
    let snapshot = {
        let Ok(mut s) = state.settings.write() else {
            // The live rebind (if any) already applied; report success so the UI
            // adopts the new combo — the next successful setter re-persists.
            return Ok(());
        };
        s.hotkey = combo;
        s.clone()
    };
    settings::save(&app, &snapshot);
    Ok(())
}

/// Toggle per-file SHA-256 integrity verification (M6.3). Read at send time,
/// so it takes effect on the very next transfer without a restart. A plain
/// boolean with no invalid states, so — unlike the string setters — there is
/// nothing to validate.
#[tauri::command]
pub fn set_verify_hash(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    update_settings(&app, &state, |s| s.verify_hash = enabled);
}

/// Set the name-collision policy (`"rename" | "overwrite" | "ask"`, M6.5). Read
/// at receive time, so it takes effect on the very next transfer. Unknown values
/// are ignored — same contract as `set_log_level`.
#[tauri::command]
pub fn set_conflict_policy(app: AppHandle, state: State<'_, AppState>, policy: String) {
    if !settings::CONFLICT_POLICIES.contains(&policy.as_str()) {
        return;
    }
    update_settings(&app, &state, |s| s.conflict = policy);
}

/// Set the auto-organize mode (`"none" | "device" | "date"`, M6.6). Read at
/// receive time; the subfolder is computed when a transfer starts. Unknown
/// values are ignored — same contract as `set_log_level`.
#[tauri::command]
pub fn set_organize(app: AppHandle, state: State<'_, AppState>, mode: String) {
    if !settings::ORGANIZE_MODES.contains(&mode.as_str()) {
        return;
    }
    update_settings(&app, &state, |s| s.organize = mode);
}

/// Set the concurrency cap (M6.7): how many transfers stream at once, clamped to
/// the allowed range. Read live at each transfer's gate, so a lower value throttles
/// only transfers not yet started; those in flight keep going. Applies to the next
/// launch's default plus every subsequent transfer — no restart.
/// Set the interface scale (1.0 = the design size). Applies immediately and
/// persists. Out-of-range values are clamped rather than refused.
#[tauri::command]
pub fn set_ui_zoom(app: AppHandle, state: State<'_, AppState>, zoom: f64) {
    let z = settings::clamp_ui_zoom(zoom);
    update_settings(&app, &state, |s| s.ui_zoom = z);
    #[cfg(desktop)]
    if let Some(win) = app.get_webview_window("main") {
        crate::apply_ui_zoom(&win, z);
    }
}

#[tauri::command]
pub fn set_max_concurrent(app: AppHandle, state: State<'_, AppState>, max: u32) {
    let clamped = settings::clamp_max_concurrent(max);
    update_settings(&app, &state, |s| s.max_concurrent = clamped);
    // Live: a raise admits transfers already queued (M17).
    state.concurrency.set_limit(clamped);
}

/// Set the per-transfer throughput cap (M6.7): `"unlimited"` or a positive
/// integer count of MB/s. Read when a transfer starts streaming, so it takes
/// effect on the very next transfer. Invalid values (`"0"`, junk) are ignored —
/// same contract as `set_log_level`.
#[tauri::command]
pub fn set_rate_limit(app: AppHandle, state: State<'_, AppState>, limit: String) {
    if !settings::valid_rate_limit(&limit) {
        return;
    }
    let normalized = limit.trim().to_string();
    update_settings(&app, &state, |s| s.rate_limit = normalized);
}

/// Toggle whether an incoming quick text is also written to this machine's
/// clipboard (M7.3). Off by default — turning it on lets a sender's text land on
/// your clipboard when they ask for it. Read at receive time, so it takes effect
/// on the very next text without a restart. A plain boolean with no invalid
/// states, so — unlike the string setters — there is nothing to validate.
#[tauri::command]
pub fn set_clip_share(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    update_settings(&app, &state, |s| s.clip_share = enabled);
}

/// Toggle stripping photo metadata (EXIF/ICC/XMP) from images before sending
/// (M9.1). This is the persisted default; the send confirm dialog can still
/// override it per transfer. Read at send time, so it takes effect on the very
/// next transfer without a restart. A plain boolean with no invalid states, so
/// — unlike the string setters — there is nothing to validate.
#[tauri::command]
pub fn set_strip_exif(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    update_settings(&app, &state, |s| s.strip_exif = enabled);
}
