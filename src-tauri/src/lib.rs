//! LanBeam — Tauri v2 backend entry point.
//! M1: real device identity (X25519 in the OS keychain) + persisted settings,
//! exposed to the UI through commands. M4.6 wires the `log` facade to
//! `tauri-plugin-log` (stdout + a file in the app log dir) and removes the
//! M0 greet/hello_tick self-test scaffolding. M5.3 adds the system tray and
//! close-to-tray (the `quitting` flag on `AppState` is the escape hatch that
//! keeps 退出 and `reset_identity`'s restart able to really exit).

// The core modules are `pub` but `#[doc(hidden)]`: NOT a public API. The
// integration tests (tests/) must reach the real transfer stack, and they
// cannot live in lib unit tests — a mock-runtime Tauri app links comctl32-v6
// imports, and cargo can attach the required Windows manifest linker args
// only to integration-test targets (see build.rs).
mod commands;
#[doc(hidden)]
pub mod consts;
#[doc(hidden)]
pub mod discovery;
#[doc(hidden)]
pub mod error;
#[doc(hidden)]
pub mod exif;
#[doc(hidden)]
pub mod identity;
#[doc(hidden)]
pub mod partials;
#[doc(hidden)]
pub mod protocol;
mod sanitize;
#[doc(hidden)]
pub mod settings;
#[doc(hidden)]
pub mod share;
#[doc(hidden)]
pub mod state;
#[doc(hidden)]
pub mod transfer;
#[doc(hidden)]
pub mod transport;
#[doc(hidden)]
pub mod trust;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, RwLock};

#[cfg(target_os = "macos")]
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
// Desktop, not just macOS: the global-shortcut handler (M5.5) emits too.
#[cfg(desktop)]
use tauri::Emitter;
use tauri::Manager;

use commands::{
    cancel_pairing, cancel_transfer, connect_by_addr, connect_device, discard_partials,
    export_diagnostics, get_download_dir, get_listen_port, get_log_dir, get_my_identity,
    get_net_status, get_network_info, get_settings, join_by_code, list_discovered_devices,
    list_shares, list_trusted, pause_transfer, remove_trusted, reply_file_request, reset_identity,
    resume_transfer, reveal_received, self_test_secure_channel, send_files, send_text,
    set_auto_open, set_autostart, set_clip_share, set_conflict_policy, set_device_name,
    set_discoverable, set_download_dir, set_hotkey, set_hotkey_enabled, set_iface_filter,
    set_listen_port, set_log_level, set_max_concurrent, set_notif_system, set_organize,
    set_rate_limit, set_recv_policy, set_strip_exif, set_tray_close, set_trusted, set_verify_hash,
    start_pairing, start_share, stop_share, take_pending_pair_link, update_share,
};
use discovery::{DiscoveryCtx, PeerTable};
use identity::Identity;
use state::{AppState, CompletedLog, NetDegraded};
use transport::TransportCtx;

/// Build the native application menu (macOS global menu bar only).
#[cfg(target_os = "macos")]
fn build_menu<R: tauri::Runtime>(handle: &tauri::AppHandle<R>) -> tauri::Result<Menu<R>> {
    let about = MenuItem::with_id(handle, "about", "About LanBeam", true, None::<&str>)?;
    let quit = PredefinedMenuItem::quit(handle, Some("Quit LanBeam"))?;
    let app_menu = Submenu::with_items(
        handle,
        "LanBeam",
        true,
        &[&about, &PredefinedMenuItem::separator(handle)?, &quit],
    )?;

    let reload = MenuItem::with_id(handle, "reload", "Reload", true, Some("CmdOrCtrl+R"))?;
    let view_menu = Submenu::with_items(handle, "View", true, &[&reload])?;

    Menu::with_items(handle, &[&app_menu, &view_menu])
}

/// Map the persisted `log_level` setting onto the `log` facade.
/// `"errors"`→Error, `"normal"`→Info, `"verbose"`→Trace; anything else (a level
/// written by a NEWER build) degrades to Info rather than failing startup.
/// Applied once when the plugin is registered — a changed setting takes effect
/// on the next launch.
fn level_filter(level: &str) -> log::LevelFilter {
    match level {
        "errors" => log::LevelFilter::Error,
        "verbose" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info, // "normal" + forward-compat fallback
    }
}

/// A test-instance id from `LANBEAM_INSTANCE` (e.g. "b"). When set, this process runs
/// alongside the primary with a distinct identity/name/download dir — for same-machine
/// testing. `pub(crate)`: `reset_identity` must delete THIS instance's keychain entry.
pub(crate) fn instance_id() -> Option<String> {
    std::env::var("LANBEAM_INSTANCE")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Bring the main window back from the tray/taskbar: show + unminimize +
/// focus. One helper because three paths need the exact same sequence — the
/// tray's left click, its 显示 menu item, and a second launch caught by
/// single-instance — and a partial sequence (focus without show) looks like
/// the app ignoring the user when the window is hidden in the tray (M5.3).
/// `pub(crate)` so the `set_hotkey_enabled` command's live-registered handler
/// reuses the exact same restore sequence as the startup one.
pub(crate) fn show_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Route an incoming `lanbeam://` deep link (currently only `…//pair?…`) to the
/// UI. A deep link is UNTRUSTED input — anyone can hand one to the OS — so this
/// NEVER acts on it: it brings the window forward and forwards the raw link to
/// the webview, which pre-fills the pairing form and waits for the user to
/// confirm. A non-`lanbeam://` URL is ignored. Shared by the cold-start,
/// macOS `on_open_url`, and Windows/Linux single-instance paths so all three
/// behave identically.
fn handle_pair_link<R: tauri::Runtime>(app: &tauri::AppHandle<R>, url: &str) {
    use tauri::Emitter;
    let url = url.trim();
    if !url.starts_with("lanbeam://") {
        return;
    }
    show_main_window(app);
    let _ = app.emit("pair_link", url);
}

/// Register the global quick-summon shortcut `combo` with the standard press
/// handler: show/focus the main window and tell the webview to open quick-text.
/// Shared by startup, `set_hotkey_enabled`, and `set_hotkey` so the three bind
/// sites can never drift (M5.5). `combo` is a tauri global-shortcut accelerator
/// (`Alt+Space`, `Ctrl+Shift+K`); the plugin's error is returned when the chord
/// can't be claimed (held by the OS / another app) so callers can surface it and
/// keep the previous binding.
#[cfg(desktop)]
pub(crate) fn register_summon_hotkey<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    combo: &str,
) -> Result<(), tauri_plugin_global_shortcut::Error> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    app.global_shortcut()
        .on_shortcut(combo, |app, _shortcut, event| {
            // Press only — firing on release too would double-run every summon.
            if event.state() == ShortcutState::Pressed {
                show_main_window(app);
                let _ = app.emit("hotkey:quick-text", ());
            }
        })
}

/// Whether a window-close request should be intercepted and turned into
/// hide-to-tray. Pure so the decision matrix is unit-testable: the user's
/// `tray_close` setting keeps the app running in the background, EXCEPT when
/// a quit is explicitly in progress (tray 退出, `reset_identity`'s restart) —
/// intercepting those would make the app unquittable (M5.3).
fn should_hide_on_close(tray_close: bool, quitting: bool) -> bool {
    tray_close && !quitting
}

/// Synchronously flush the trust and partials stores before a deliberate exit.
/// WHY: both stores normally persist fire-and-forget on the blocking pool
/// (`persist_async`), which `app.exit(0)` does NOT drain — so a pairing/trust or
/// discard-partials decision made moments before quitting could be lost inside
/// the fsync+rename window. Snapshot under each read guard, drop the guard, then
/// persist on this thread. Safe against any in-flight async persist: the store's
/// sequence gate skips a stale image, and this snapshot carries the current
/// (highest) seq, so it wins. A poisoned lock degrades to a skipped flush rather
/// than blocking the quit. `reset_identity` already awaits its own trust persist
/// for the same reason.
fn flush_persistence(state: &AppState) {
    if let Some(snap) = state.trusted.read().ok().and_then(|t| t.snapshot()) {
        snap.persist();
    }
    if let Some(snap) = state.partials.read().ok().and_then(|p| p.snapshot()) {
        snap.persist();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();
    // single-instance guard — skipped for test instances so two can run on one machine.
    if instance_id().is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // A second launch means "bring LanBeam back" — the window may be
            // hidden in the tray (M5.3), so focusing alone is not enough.
            show_main_window(app);
            // On Windows/Linux, clicking a lanbeam:// link while we are already
            // running relaunches us with the URL as an argument. Route it to the
            // same handler the cold-start path uses instead of ignoring it.
            for arg in &args {
                if arg.starts_with("lanbeam://") {
                    handle_pair_link(app, arg);
                }
            }
        }));
    }
    let builder = builder
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // Quick text (M7.3): mirror an incoming text to the local clipboard when
        // the receiver opted in. All-platform, so it registers here rather than
        // in the desktop-only block below.
        .plugin(tauri_plugin_clipboard_manager::init());
    // System-integration plugins (M5.4/M5.5) are desktop-only dependencies —
    // login items and global hotkeys have no mobile equivalent, and the
    // notification call sites are cfg-gated to match. No launch args for
    // autostart: a tray-enabled app restored at login should come up exactly
    // as a normal launch does.
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // Deep links (lanbeam://pair). Registered AFTER single-instance so a
        // link clicked while LanBeam runs reaches the existing instance.
        .plugin(tauri_plugin_deep_link::init());
    // The window chrome is frontend-drawn (decorations: false); only macOS
    // keeps a native menu, where it lives in the global menu bar.
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(build_menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "about" => {
                let _ = app.emit("menu:about", ());
            }
            "reload" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.eval("window.location.reload()");
                }
            }
            _ => {}
        });
    builder
        .setup(|app| {
            let instance = instance_id();

            // Load (or generate) the device identity (distinct per test instance) + settings.
            let identity = Arc::new(
                Identity::load_or_create(instance.as_deref())
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?,
            );
            let (mut loaded, settings_diag) = settings::load(app.handle());

            // Logging comes up as early as possible, but AFTER settings — the level
            // maps from `log_level` ("errors"→Error, "normal"→Info, "verbose"→Trace)
            // and therefore applies on the NEXT launch when the setting changes.
            // Targets: stdout (dev console) + a file in the app log dir (field
            // diagnostics / export_diagnostics).
            app.handle().plugin(
                tauri_plugin_log::Builder::new()
                    .targets([
                        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                            file_name: None,
                        }),
                    ])
                    // Big enough that export_diagnostics' 500 KB tail is meaningful;
                    // the plugin rotates when the file exceeds this.
                    .max_file_size(512_000)
                    .level(level_filter(&loaded.log_level))
                    .build(),
            )?;
            log::info!("logging initialized (level setting: {})", loaded.log_level);
            // Deferred from settings::load, which runs before the logger exists.
            if let Some(e) = settings_diag {
                log::warn!("settings blob corrupted, fell back to defaults: {e}");
            }

            if let Some(id) = &instance {
                // distinguish the two windows in the UI
                loaded.device_name = format!(
                    "{} ({})",
                    settings::default_device_name(),
                    id.to_uppercase()
                );
            }
            // Startup resolutions (M5.2), read before the blob moves into
            // shared state: the listener port (0 sentinel → default) and the
            // download root (a valid persisted override wins, else OS default).
            let listen_port = settings::effective_listen_port(loaded.port);
            let dir_override = loaded.download_dir_override.clone();
            // Read here too (M5.5): the reconcile below runs after the blob
            // has moved into the shared RwLock.
            #[cfg(desktop)]
            let want_autostart = loaded.autostart;
            // The hotkey opt-in + the configured combo, read before the blob
            // moves — the startup registration below applies them (default OFF:
            // no chord grab; default combo Alt+Space until the user rebinds).
            #[cfg(desktop)]
            let want_hotkey = loaded.hotkey_enabled;
            #[cfg(desktop)]
            let want_hotkey_combo = loaded.hotkey.clone();
            let settings = Arc::new(RwLock::new(loaded));
            let peers: Arc<Mutex<PeerTable>> = Arc::new(Mutex::new(HashMap::new()));
            // Pre-bind placeholder only — spawn_listener stores the port it
            // actually bound; announcing the intended port until then beats
            // announcing a default the user overrode.
            let tcp_port = Arc::new(AtomicU16::new(listen_port));
            let download_dir = {
                let base = app
                    .path()
                    .download_dir()
                    .unwrap_or_else(|_| std::env::temp_dir());
                let mut dir = base.join("LanBeam");
                if let Some(id) = &instance {
                    dir = dir.join(format!("instance-{id}")); // separate downloads per instance
                }
                let _ = std::fs::create_dir_all(&dir);
                // dunce (not std) so the resolved default download path shown in
                // the UI is not a Windows `\\?\` verbatim path (M5.2).
                let os_default = dunce::canonicalize(&dir).unwrap_or(dir);
                // An override whose folder vanished since it was set (deleted,
                // unplugged drive) falls back to the OS default — receiving
                // must keep working rather than fail every write (M5.2).
                let resolved = dir_override
                    .as_deref()
                    .and_then(|o| {
                        let valid = settings::canonical_dir(o);
                        if valid.is_none() {
                            log::warn!(
                                "download dir override {o:?} is not an existing directory; \
                                 using the OS default"
                            );
                        }
                        valid
                    })
                    .unwrap_or(os_default);
                // RwLock (M5.2): set_download_dir retargets NEW sessions
                // immediately; in-flight ones keep their snapshotted root.
                Arc::new(RwLock::new(resolved))
            };
            let pending = Arc::new(Mutex::new(HashMap::new()));
            // Bounded history — see CompletedLog for the eviction rationale.
            let completed = Arc::new(Mutex::new(CompletedLog::new()));
            // Bind-time degradations, recorded because their events fire before
            // the webview loads — served to the UI via get_net_status (M4.6).
            let degraded: Arc<Mutex<Vec<NetDegraded>>> = Arc::new(Mutex::new(Vec::new()));
            // Trust store (M4.4): its own JSON file in the app data dir (the
            // same directory the store plugin resolves settings.json into).
            // Loaded AFTER the logger so a corrupted file's warning is
            // captured; per-instance file (like the keychain identity) so two
            // test processes on one machine don't share trust decisions.
            let trusted = {
                let dir = app
                    .path()
                    .app_data_dir()
                    .unwrap_or_else(|_| std::env::temp_dir());
                let file = match &instance {
                    Some(id) => format!("trusted-{id}.json"),
                    None => "trusted.json".to_string(),
                };
                Arc::new(RwLock::new(trust::TrustStore::load(dir.join(file))))
            };

            // Resume state for interrupted receives (M6.4): its own JSON file in
            // the app data dir, per-instance like the trust store so two test
            // processes never resume onto each other's partials.
            let partials = {
                let dir = app
                    .path()
                    .app_data_dir()
                    .unwrap_or_else(|_| std::env::temp_dir());
                let file = match &instance {
                    Some(id) => format!("partials-{id}.json"),
                    None => "partials.json".to_string(),
                };
                Arc::new(RwLock::new(partials::PartialsStore::load(dir.join(file))))
            };

            // False until a deliberate exit path flips it — see AppState::quitting.
            let quitting = Arc::new(AtomicBool::new(false));

            // Live per-session transfer controls (M6.1/6.2). Shared between
            // AppState (the cancel/pause/resume commands) and the listener's
            // TransportCtx (receive registration) — unlike in_flight, which
            // only the listener touches.
            let transfers_ctl: state::TransfersCtl = Arc::new(Mutex::new(HashMap::new()));

            // Global concurrency cap (M6.7): ONE gate shared by AppState (the
            // send command) and the listener's TransportCtx (receive), so the
            // cap bounds all in-flight transfers, both directions, together.
            let concurrency = Arc::new(state::ConcurrencyGate::new());

            // Pairing state (M7.1): the active code + per-source failure throttle,
            // shared by AppState (start/cancel commands) and the listener's
            // TransportCtx (an inbound PairRequest matches against it).
            let pairing = Arc::new(state::PairingState::default());
            // Manually-added peers (M7.2): kept out of the discovery table so the
            // expiry loop never evicts an IP-dialed device. Shared by AppState
            // (the connect_by_addr/join_by_code commands record here, and sends
            // resolve against it) and DiscoveryCtx (every devices_updated emit
            // merges it in, so a discovery change never drops a manual peer). The
            // listener's TransportCtx still never reads it.
            let manual_peers = Arc::new(Mutex::new(HashMap::new()));

            // Browser-share state (M8.1a): the live share registry + the port its
            // HTTP server binds. Shared by AppState (the share commands mint/stop
            // entries) and the share server task (which serves them). The port is
            // filled in by the task once it binds; 0 is the pre-bind sentinel.
            let shares = share::new_registry();
            let share_port = Arc::new(AtomicU16::new(0));

            // Cold-start deep link (lanbeam://pair): filled below if this process
            // was launched by a link, drained by the webview on mount. See the
            // AppState field for why a stash (not an event) is needed.
            let pending_pair_link: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

            app.manage(AppState {
                identity: identity.clone(),
                settings: settings.clone(),
                peers: peers.clone(),
                tcp_port: tcp_port.clone(),
                download_dir: download_dir.clone(),
                pending: pending.clone(),
                completed: completed.clone(),
                trusted: trusted.clone(),
                degraded: degraded.clone(),
                quitting: quitting.clone(),
                transfers_ctl: transfers_ctl.clone(),
                partials: partials.clone(),
                concurrency: concurrency.clone(),
                pairing: pairing.clone(),
                manual_peers: manual_peers.clone(),
                shares: shares.clone(),
                share_port: share_port.clone(),
                pending_pair_link: pending_pair_link.clone(),
            });

            // Start the TCP transfer listener (answers Noise handshakes; sets the real port).
            transport::spawn_listener(
                TransportCtx {
                    app: app.handle().clone(),
                    identity: identity.clone(),
                    tcp_port: tcp_port.clone(),
                    download_dir: download_dir.clone(),
                    settings: settings.clone(),
                    peers: peers.clone(),
                    pending: pending.clone(),
                    // Whole-lifetime inbound-session id registry (duplicate guard);
                    // only the listener path uses it, so no AppState mirror.
                    in_flight: Arc::new(Mutex::new(HashSet::new())),
                    completed: completed.clone(),
                    trusted: trusted.clone(),
                    // Shared with AppState so cancel/pause/resume reach a
                    // running receive (M6.1/6.2).
                    transfers_ctl: transfers_ctl.clone(),
                    // Shared with AppState so the receive path records/clears
                    // resume state and `discard_partials` can reach it (M6.4).
                    partials: partials.clone(),
                    // Shared with AppState so send + receive draw slots from the
                    // same concurrency gate (M6.7).
                    concurrency: concurrency.clone(),
                    // Shared with AppState so an inbound PairRequest matches the
                    // code the start_pairing command minted (M7.1).
                    pairing: pairing.clone(),
                    // Listener-only per-source quick-text throttle (M7.3): quick
                    // text has no accept-prompt, so this bounds inbound text_received
                    // spam. Like in_flight, no AppState mirror.
                    text_rate: Arc::new(Mutex::new(state::TextRateLimiter::default())),
                },
                degraded.clone(),
                listen_port,
            );

            // Start the browser-share HTTP server (M8.1a): a one-shot LAN file
            // server for peers without LanBeam. Binds its own ephemeral port
            // (stored in share_port for the URL/discovery) and serves only the
            // files a share explicitly registers, by index. See `share`.
            // Surface every browser download: emit `share_download` for the UI
            // (toast / history / live count) and — when the notify setting allows
            // — an OS notification, so a tray'd/backgrounded app still tells you
            // your shared file was pulled, and by whom (the IP).
            let dl_app = app.handle().clone();
            let on_download: share::DownloadHook =
                Some(Arc::new(move |ev: share::ShareDownloadEvent| {
                    let _ = dl_app.emit("share_download", &ev);
                    #[cfg(desktop)]
                    {
                        let notify = dl_app
                            .try_state::<AppState>()
                            .and_then(|st| st.settings.read().ok().map(|s| s.notif_system))
                            .unwrap_or(true);
                        if notify {
                            // Offload the OS notification: `.show()` can make a
                            // blocking OS call and this hook runs on the share
                            // server's request path — keep a download's response
                            // off it. try_state (not the ext accessor) so a missing
                            // plugin degrades to silence; content is locale-neutral
                            // (the backend has no i18n) — the file name + "↓ <ip>".
                            let app2 = dl_app.clone();
                            let title = ev.name;
                            let ip = ev.peer_ip;
                            tauri::async_runtime::spawn_blocking(move || {
                                if let Some(n) = app2.try_state::<
                                    tauri_plugin_notification::Notification<tauri::Wry>,
                                >() {
                                    let _ = n
                                        .builder()
                                        .title(title)
                                        .body(format!("↓ {ip}"))
                                        .show();
                                }
                            });
                        }
                    }
                }));
            share::spawn_share_server(share::ShareServerCtx {
                registry: shares.clone(),
                share_port: share_port.clone(),
                on_download,
            });

            // Start LAN discovery (announce + listen + expiry tasks).
            discovery::spawn(
                DiscoveryCtx {
                    app: app.handle().clone(),
                    my_id: identity.device_id(),
                    settings,
                    peers,
                    tcp_port,
                    // Merged behind the discovery snapshot in every devices_updated
                    // emit so a discovery change never evicts a manual peer (M7.2).
                    manual_peers,
                    // Read each announce tick to advertise the browser-share port
                    // (M8.3) only while a share is live. See `share_http_advert`.
                    shares,
                    share_port,
                },
                degraded,
            );

            // System tray (M5.3, desktop only — `tauri::tray` does not exist on
            // mobile). With close-to-tray on by default, the tray is the way
            // back into a hidden window AND the only reliable way out: 退出
            // flips the quitting flag the close interception honors.
            #[cfg(desktop)]
            {
                use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
                use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

                let show = MenuItem::with_id(app, "show", "显示 LanBeam", true, None::<&str>)?;
                let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
                let menu =
                    Menu::with_items(app, &[&show, &PredefinedMenuItem::separator(app)?, &quit])?;
                let mut tray = TrayIconBuilder::with_id("main")
                    .tooltip("LanBeam")
                    .menu(&menu)
                    // Left click restores the window instead of popping the
                    // menu (macOS would otherwise open the menu); the menu
                    // stays reachable via right click everywhere.
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app, event| match event.id().as_ref() {
                        "show" => show_main_window(app),
                        "quit" => {
                            // Flag BEFORE exiting: any close request fired
                            // during teardown must not be intercepted into a
                            // hide, or the process would linger headless.
                            if let Some(state) = app.try_state::<AppState>() {
                                state.quitting.store(true, Ordering::Relaxed);
                                // Drain fire-and-forget trust/partials writes
                                // before app.exit(0), which does not wait on the
                                // blocking pool — a just-made pairing/trust or
                                // discard decision must not be lost on quit.
                                flush_persistence(&state);
                            }
                            app.exit(0);
                        }
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            show_main_window(tray.app_handle());
                        }
                    });
                // A missing icon degrades to a blank-but-clickable tray — not
                // worth failing startup over.
                if let Some(icon) = app.default_window_icon() {
                    tray = tray.icon(icon.clone());
                }
                tray.build(app)?;
            }

            // Autostart reconcile (M5.5): the persisted setting is the source
            // of truth. The OS entry can drift behind our back (removed via
            // Task Manager, rewritten by a reinstall), so every launch reads
            // the real state and re-applies the setting, logging the drift
            // instead of silently adopting it. Test instances skip this: they
            // share ONE OS login item with the primary, and their default-off
            // setting must not un-register the primary's choice.
            #[cfg(desktop)]
            if instance.is_none() {
                use tauri_plugin_autostart::ManagerExt;
                let autolaunch = app.autolaunch();
                match autolaunch.is_enabled() {
                    Ok(actual) if actual != want_autostart => {
                        log::warn!(
                            "autostart drift: OS entry enabled={actual}, setting wants \
                             {want_autostart}; reconciling to the setting"
                        );
                        let res = if want_autostart {
                            autolaunch.enable()
                        } else {
                            autolaunch.disable()
                        };
                        if let Err(e) = res {
                            log::warn!("autostart reconcile failed: {e}");
                        }
                    }
                    Ok(_) => {}
                    Err(e) => log::warn!("autostart state unreadable (reconcile skipped): {e}"),
                }
            }

            // Global shortcut (M5.5, desktop only): the chord brings LanBeam up
            // from anywhere and tells the webview to open quick-text. OPT-IN —
            // Alt+Space is the Windows system-menu chord, so we claim the chord
            // OS-wide only when the user turned it on (hotkey_enabled), and we bind
            // the CONFIGURED combo (a prior `set_hotkey` may have rebound it), not
            // the const; `set_hotkey_enabled` / `set_hotkey` register/unregister it
            // live thereafter. Test instances skip this so two processes never
            // contend for the one chord (like autostart). Registration can still
            // fail — another app may own the chord — and a missing hotkey must not
            // fail startup, so a conflict only logs.
            #[cfg(desktop)]
            if instance.is_none() && want_hotkey {
                match register_summon_hotkey(app.handle(), &want_hotkey_combo) {
                    Ok(()) => log::info!("global shortcut {want_hotkey_combo} registered"),
                    Err(e) => log::warn!(
                        "global shortcut {want_hotkey_combo} unavailable (held by another app?): {e}"
                    ),
                }
            }

            // Deep links (lanbeam://pair): a pairing link from another device can
            // launch or focus LanBeam and pre-fill the join form. See
            // handle_pair_link — the webview still requires the user to confirm.
            #[cfg(desktop)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                // Register the lanbeam:// scheme against THIS exe on every startup
                // (Windows/Linux). WHY not just rely on the installer: a PORTABLE
                // build — a "green" exe run WITHOUT the NSIS installer — has nobody
                // to register the scheme, so without this a downloaded exe could
                // never open a link. Doing it each launch also RE-POINTS the
                // association after a portable exe is moved to a new folder. For an
                // INSTALLED build this only rewrites the same per-user (HKCU) key
                // the NSIS installer already set — a harmless refresh, not a
                // conflict. Skipped for test instances so two processes never fight
                // over the one machine-wide association. Best-effort: a failure only
                // means link opening is unavailable, never a startup failure. (macOS
                // resolves schemes from the .app bundle's Info.plist, so there is
                // nothing to register at runtime there.)
                #[cfg(any(target_os = "windows", target_os = "linux"))]
                if instance.is_none() {
                    match app.deep_link().register("lanbeam") {
                        Ok(()) => log::info!("lanbeam:// scheme registered to this executable"),
                        Err(e) => log::warn!(
                            "lanbeam:// scheme registration failed (link opening unavailable): {e}"
                        ),
                    }
                }
                // Cold start: LanBeam may have been LAUNCHED by a link. The
                // webview is not listening yet (events have no replay), so STASH
                // the launch link for it to pull on mount (take_pending_pair_link)
                // instead of emitting into the void. Warm links (below / single-
                // instance) emit normally because the webview is already up.
                match app.deep_link().get_current() {
                    Ok(Some(urls)) => {
                        if let Some(link) = urls
                            .iter()
                            .map(|u| u.as_str().trim())
                            .find(|u| u.starts_with("lanbeam://"))
                        {
                            if let Ok(mut slot) = pending_pair_link.lock() {
                                *slot = Some(link.to_string());
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => log::warn!("reading the launch deep link failed: {e}"),
                }
                // Warm delivery on platforms that hand the URL to the running
                // process (macOS). Windows/Linux route through single-instance.
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        handle_pair_link(&handle, url.as_str());
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Read the LIVE setting + quit intent on every close (M5.3).
                // Every failure mode degrades to a plain close: a window the
                // user cannot close is strictly worse than a missed hide.
                let hide = window
                    .try_state::<AppState>()
                    .map(|state| {
                        let tray_close =
                            state.settings.read().map(|s| s.tray_close).unwrap_or(false);
                        should_hide_on_close(tray_close, state.quitting.load(Ordering::Relaxed))
                    })
                    .unwrap_or(false);
                if hide {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_my_identity,
            get_settings,
            set_device_name,
            set_discoverable,
            set_auto_open,
            set_log_level,
            set_recv_policy,
            set_download_dir,
            set_listen_port,
            set_tray_close,
            set_notif_system,
            set_autostart,
            set_iface_filter,
            set_hotkey_enabled,
            set_hotkey,
            set_verify_hash,
            set_conflict_policy,
            set_organize,
            set_max_concurrent,
            set_rate_limit,
            set_clip_share,
            set_strip_exif,
            discard_partials,
            reset_identity,
            get_network_info,
            get_listen_port,
            list_discovered_devices,
            connect_device,
            connect_by_addr,
            start_pairing,
            cancel_pairing,
            join_by_code,
            take_pending_pair_link,
            self_test_secure_channel,
            send_text,
            send_files,
            start_share,
            update_share,
            stop_share,
            list_shares,
            reply_file_request,
            cancel_transfer,
            pause_transfer,
            resume_transfer,
            get_download_dir,
            reveal_received,
            list_trusted,
            set_trusted,
            remove_trusted,
            get_log_dir,
            export_diagnostics,
            get_net_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The close-interception matrix (M5.3): hide-to-tray ONLY when the user
    /// wants it AND no quit is in progress. The quitting override is what
    /// keeps tray-退出 (and `reset_identity`'s restart) able to actually end
    /// the process instead of hiding its own teardown.
    #[test]
    fn close_hides_only_when_enabled_and_not_quitting() {
        assert!(should_hide_on_close(true, false));
        assert!(
            !should_hide_on_close(true, true),
            "quit intent must beat tray_close"
        );
        assert!(!should_hide_on_close(false, false));
        assert!(!should_hide_on_close(false, true));
    }
}
