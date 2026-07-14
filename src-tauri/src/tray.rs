//! System tray (M5.3, extended into a full remote control).
//!
//! DESIGN — the tray is a REMOTE, not a second brain. Rust owns only what the
//! webview cannot do from a hidden window: showing the window and quitting the
//! process. Everything else (open a modal, navigate, reveal the download folder,
//! flip discoverability) is emitted as a `tray:<id>` event and performed by the
//! frontend, which already implements those actions and owns the app state. That
//! keeps ONE writer per piece of state — no dual-write race between a tray click
//! and the UI — and means the tray needs no i18n layer of its own: the frontend
//! pushes every label through [`sync_tray`], so the menu follows the app's
//! language and live state (device name, LAN IP, discoverable) for free.
//!
//! The webview stays alive while the window is hidden in the tray, so those
//! event-driven items keep working even when the window is closed to tray.
//!
//! `tauri::tray`/`menu` are desktop-only, so everything but the command and its
//! payload is `cfg(desktop)`-gated — the command itself must exist on every
//! target because `generate_handler!` cannot cfg an individual entry.

use serde::Deserialize;
use tauri::AppHandle;

#[cfg(desktop)]
use std::sync::atomic::Ordering;
#[cfg(desktop)]
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
#[cfg(desktop)]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
#[cfg(desktop)]
use tauri::{Emitter, Manager, Wry};

#[cfg(desktop)]
use crate::state::AppState;

/// The entire user-facing surface of the tray, pushed from the frontend.
///
/// Deliberately a WHOLE snapshot rather than per-field setters: the UI re-sends
/// it whenever anything changes (language, device name, LAN IP, discoverability),
/// so the call is idempotent and the tray can never drift out of sync.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraySync {
    /// The disabled header line — e.g. `"书房 · 192.168.1.20"`.
    pub status: String,
    /// Hover tooltip on the tray icon itself.
    pub tooltip: String,
    pub show: String,
    pub send: String,
    pub quick_text: String,
    pub share: String,
    pub pair: String,
    pub discoverable: String,
    pub open_dir: String,
    pub inbox: String,
    pub transfers: String,
    pub settings: String,
    pub quit: String,
    /// Whether the「可被发现」item shows its tick.
    pub is_discoverable: bool,
}

/// Log-and-swallow a tray mutation. Swallow, because a cosmetic relabel must
/// never fail a command the user actually asked for — but LOG, because a failed
/// `set_checked` is precisely the case where the tray starts lying about whether
/// the device is discoverable, and a silent drop leaves nothing in the exported
/// diagnostics to explain it. (Linux appindicator, e.g., has no tooltip at all.)
#[cfg(desktop)]
fn swallow(what: &str, r: tauri::Result<()>) {
    if let Err(e) = r {
        log::debug!("tray: {what} failed: {e}");
    }
}

/// Apply a label + state snapshot to the tray.
#[tauri::command]
pub fn sync_tray(app: AppHandle, sync: TraySync) {
    #[cfg(desktop)]
    apply(&app, &sync);
    // Mobile has no tray — consume the params so the signature still type-checks.
    #[cfg(not(desktop))]
    {
        let _ = (app, sync);
    }
}

/// Live handles to every retextable menu item, kept in managed state so
/// [`sync_tray`] can relabel + recheck them after the frontend loads (or the
/// user switches language) without rebuilding the whole tray.
#[cfg(desktop)]
pub struct TrayHandles {
    status: MenuItem<Wry>,
    show: MenuItem<Wry>,
    send: MenuItem<Wry>,
    quick_text: MenuItem<Wry>,
    share: MenuItem<Wry>,
    pair: MenuItem<Wry>,
    discoverable: CheckMenuItem<Wry>,
    open_dir: MenuItem<Wry>,
    inbox: MenuItem<Wry>,
    transfers: MenuItem<Wry>,
    settings: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

#[cfg(desktop)]
fn apply(app: &AppHandle, sync: &TraySync) {
    let Some(h) = app.try_state::<TrayHandles>() else {
        // The tray failed to build (or never did) — nothing to sync.
        return;
    };
    swallow("status", h.status.set_text(&sync.status));
    swallow("show", h.show.set_text(&sync.show));
    swallow("send", h.send.set_text(&sync.send));
    swallow("quick_text", h.quick_text.set_text(&sync.quick_text));
    swallow("share", h.share.set_text(&sync.share));
    swallow("pair", h.pair.set_text(&sync.pair));
    swallow(
        "discoverable label",
        h.discoverable.set_text(&sync.discoverable),
    );
    // The one that matters most: a dropped tick makes the tray lie about whether
    // this device is broadcasting.
    swallow(
        "discoverable tick",
        h.discoverable.set_checked(sync.is_discoverable),
    );
    swallow("open_dir", h.open_dir.set_text(&sync.open_dir));
    swallow("inbox", h.inbox.set_text(&sync.inbox));
    swallow("transfers", h.transfers.set_text(&sync.transfers));
    swallow("settings", h.settings.set_text(&sync.settings));
    swallow("quit", h.quit.set_text(&sync.quit));
    if let Some(tray) = app.tray_by_id("main") {
        swallow("tooltip", tray.set_tooltip(Some(&sync.tooltip)));
    }
}

/// What clicking a tray id should do.
#[cfg(desktop)]
#[derive(Debug, PartialEq, Eq)]
enum Action {
    /// Only the backend can do these two.
    Show,
    Quit,
    /// Hand to the webview, but surface the window first — the action puts
    /// something ON SCREEN (a modal, a page) and would otherwise happen behind
    /// a still-hidden window.
    EmitWithWindow,
    /// Hand to the webview as-is; it does this fine while hidden.
    Emit,
    /// Disabled (`status`) or not one of ours.
    Ignore,
}

/// Route a menu id. Pure, so the whole dispatch table is unit-testable — the
/// click handler is otherwise unreachable from a test.
#[cfg(desktop)]
fn route(id: &str) -> Action {
    match id {
        "show" => Action::Show,
        "quit" => Action::Quit,
        "send" | "quick_text" | "share" | "pair" | "inbox" | "transfers" | "settings" => {
            Action::EmitWithWindow
        }
        "open_dir" | "discoverable" => Action::Emit,
        _ => Action::Ignore,
    }
}

/// Build the tray + its menu and register the handlers. Labels start in English
/// and are replaced by the frontend's first [`sync_tray`] — a tray opened in the
/// split second before that is still readable, never blank.
#[cfg(desktop)]
pub fn build(app: &tauri::App) -> tauri::Result<()> {
    // Seed the STATEFUL items from the real settings, which are already managed
    // by the time the tray is built. A stale *label* is merely untranslated (the
    // frontend fixes it in ms), but a stale *tick* is a false claim about whether
    // this device is broadcasting on the LAN — and it would persist forever if
    // the webview never loaded. Never assert state we can simply read.
    let (checked, name) = match app.try_state::<AppState>() {
        Some(s) => (
            s.settings.read().map(|st| st.discoverable).unwrap_or(true),
            crate::transfer::local_device_name(&s.settings),
        ),
        None => (true, "LanBeam".to_string()),
    };

    let status = MenuItem::with_id(app, "status", &name, false, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Show LanBeam", true, None::<&str>)?;
    let send = MenuItem::with_id(app, "send", "Send files…", true, None::<&str>)?;
    let quick_text = MenuItem::with_id(app, "quick_text", "Quick text…", true, None::<&str>)?;
    let share = MenuItem::with_id(app, "share", "Receive in a browser…", true, None::<&str>)?;
    let pair = MenuItem::with_id(app, "pair", "Pair a device…", true, None::<&str>)?;
    let discoverable = CheckMenuItem::with_id(
        app,
        "discoverable",
        "Discoverable",
        true,
        checked,
        None::<&str>,
    )?;
    let open_dir = MenuItem::with_id(app, "open_dir", "Open download folder", true, None::<&str>)?;
    let inbox = MenuItem::with_id(app, "inbox", "Inbox", true, None::<&str>)?;
    let transfers = MenuItem::with_id(app, "transfers", "Transfers", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &show,
            &PredefinedMenuItem::separator(app)?,
            &send,
            &quick_text,
            &share,
            &pair,
            &PredefinedMenuItem::separator(app)?,
            &discoverable,
            &PredefinedMenuItem::separator(app)?,
            &open_dir,
            &inbox,
            &transfers,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("LanBeam")
        .menu(&menu)
        // Left click restores the window instead of popping the menu (macOS
        // would otherwise open the menu); the menu stays on right click.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            match route(id) {
                Action::Show => crate::show_main_window(app),
                Action::Quit => quit_app(app),
                Action::EmitWithWindow => {
                    crate::show_main_window(app);
                    let _ = app.emit(&format!("tray:{id}"), ());
                }
                Action::Emit => {
                    let _ = app.emit(&format!("tray:{id}"), ());
                }
                Action::Ignore => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::show_main_window(tray.app_handle());
            }
        });
    // macOS wants a TEMPLATE icon in the menu bar: alpha-only, so the system
    // paints it black on a light bar and white on a dark one and it follows the
    // theme like every other menu-bar item. Handing it the app icon instead — a
    // 32×32 colour PNG, which is what `default_window_icon()` resolves to on
    // non-Windows targets (the first `.png` in `bundle.icon`) — gave a smudged
    // little square that stayed dark on a dark bar.
    //
    // Everywhere else, the app icon is the right icon.
    #[cfg(target_os = "macos")]
    {
        match tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png")) {
            Ok(icon) => {
                tray = tray.icon(icon).icon_as_template(true);
            }
            Err(e) => {
                log::warn!("tray: template icon failed to decode ({e}); using the app icon");
                if let Some(icon) = app.default_window_icon() {
                    tray = tray.icon(icon.clone());
                }
            }
        }
    }
    // A missing icon degrades to a blank-but-clickable tray — not worth failing
    // startup over.
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;

    app.manage(TrayHandles {
        status,
        show,
        send,
        quick_text,
        share,
        pair,
        discoverable,
        open_dir,
        inbox,
        transfers,
        settings,
        quit,
    });
    Ok(())
}

/// Quit for real. Flags BEFORE exiting: any close request fired during teardown
/// must not be intercepted into a hide, or the process would linger headless.
#[cfg(desktop)]
fn quit_app(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.quitting.store(true, Ordering::Relaxed);
        // Drain fire-and-forget trust/partials writes before app.exit(0), which
        // does not wait on the blocking pool — a just-made pairing/trust or
        // discard decision must not be lost on quit.
        crate::flush_persistence(&state);
    }
    app.exit(0);
}

#[cfg(all(test, desktop))]
mod tests {
    use super::*;

    /// Mirrors the ids [`build`] creates. Test-only: its whole job is to keep
    /// [`route`] honest — an item added to the menu (and listed here) but left
    /// unrouted would otherwise ship as an entry that silently does nothing.
    const MENU_IDS: [&str; 12] = [
        "status",
        "show",
        "send",
        "quick_text",
        "share",
        "pair",
        "discoverable",
        "open_dir",
        "inbox",
        "transfers",
        "settings",
        "quit",
    ];

    /// The two actions only the backend can perform must NOT be delegated to the
    /// webview (a hidden window has no one to service `tray:show`/`tray:quit`).
    #[test]
    fn show_and_quit_are_handled_natively() {
        assert_eq!(route("show"), Action::Show);
        assert_eq!(route("quit"), Action::Quit);
    }

    /// Anything that puts UI on screen must surface the window FIRST, or it
    /// would open behind a window still hidden in the tray.
    #[test]
    fn ui_items_surface_the_window_before_emitting() {
        for id in [
            "send",
            "quick_text",
            "share",
            "pair",
            "inbox",
            "transfers",
            "settings",
        ] {
            assert_eq!(
                route(id),
                Action::EmitWithWindow,
                "{id} must show the window"
            );
        }
    }

    /// Revealing a folder / flipping discoverability needs no window — surfacing
    /// one would yank the user out of whatever they were doing.
    #[test]
    fn headless_items_emit_without_surfacing_the_window() {
        assert_eq!(route("open_dir"), Action::Emit);
        assert_eq!(route("discoverable"), Action::Emit);
    }

    /// The disabled header line, and anything that isn't ours, do nothing.
    #[test]
    fn status_and_unknown_ids_are_ignored() {
        assert_eq!(route("status"), Action::Ignore);
        assert_eq!(route("nope"), Action::Ignore);
        assert_eq!(route(""), Action::Ignore);
    }

    /// Every id the menu is BUILT with must have a route — otherwise it ships as
    /// a visible menu entry that silently does nothing when clicked. `status` is
    /// the one deliberate exception (it is a disabled label, not a button).
    #[test]
    fn every_built_menu_id_is_routed() {
        for id in MENU_IDS {
            let action = route(id);
            if id == "status" {
                assert_eq!(action, Action::Ignore, "status is a disabled label");
            } else {
                assert_ne!(action, Action::Ignore, "menu item {id} has no handler");
            }
        }
    }
}
