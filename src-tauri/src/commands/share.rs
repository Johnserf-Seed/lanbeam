//! Browser share (M8.1b): files published for a browser on the LAN to
//! download — the fallback for a recipient who doesn't have LanBeam.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use super::pairing::first_lan_ip;
use crate::error::LanBeamError;
use crate::share;
use crate::state::AppState;

/// The result of starting a browser share: the access token, the LAN URL a
/// browser opens, and when the link expires (unix seconds).
#[derive(Serialize)]
pub struct ShareStarted {
    pub token: String,
    pub url: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
}

/// The new expiry (unix seconds) after a share's lifetime is changed.
#[derive(Serialize)]
pub struct ShareUpdated {
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
}

/// One live browser share as the shares list shows it.
#[derive(Clone, Serialize)]
pub struct ShareEntry {
    pub token: String,
    pub url: String,
    #[serde(rename = "fileCount")]
    pub file_count: usize,
    #[serde(rename = "totalSize")]
    pub total_size: u64,
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
    pub downloads: u32,
    #[serde(rename = "maxDownloads")]
    pub max_downloads: Option<u32>,
}

/// Publish a set of files for a browser to download over the local network — the
/// fallback for a recipient who doesn't have LanBeam. Pass the absolute file
/// paths, how long the link should stay live (`ttlSecs`), and an optional
/// download limit (`maxDownloads`, omit for unlimited). Returns the link, its
/// access token, and when it expires. Every path must be an existing file; the
/// link serves only those files and stops working when it expires, reaches its
/// download limit, or you stop it.
#[tauri::command]
pub fn start_share(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
    ttl_secs: u64,
    max_downloads: Option<u32>,
) -> Result<ShareStarted, LanBeamError> {
    let started = do_start_share(&state, paths, ttl_secs, max_downloads)?;
    emit_shares(&app, &state);
    Ok(started)
}

/// Change a live share's lifetime and download limit — the ShareModal's duration
/// and count controls reconfiguring a share in place. Pass the share's token and
/// the new `ttlSecs` / `maxDownloads`; the new lifetime starts now. Returns the
/// new expiry, or nothing if the token is unknown or the share was already
/// stopped.
#[tauri::command]
pub fn update_share(
    app: AppHandle,
    state: State<'_, AppState>,
    token: String,
    ttl_secs: u64,
    max_downloads: Option<u32>,
) -> Option<ShareUpdated> {
    let updated = do_update_share(&state, &token, ttl_secs, max_downloads);
    if updated.is_some() {
        emit_shares(&app, &state);
    }
    updated
}

/// Stop a browser share now: its link stops working immediately. Safe to call
/// with an unknown token.
#[tauri::command]
pub fn stop_share(app: AppHandle, state: State<'_, AppState>, token: String) {
    if share::stop_share(&state.shares, &token) {
        emit_shares(&app, &state);
    }
}

/// The browser shares that are currently live — each with its link, file count,
/// total size, expiry, and how many times it has been downloaded.
#[tauri::command]
pub fn list_shares(state: State<'_, AppState>) -> Vec<ShareEntry> {
    list_shares_dto(&state)
}

/// Broadcast the live-share list to the UI.
///
/// A live share is FILES BEING SERVED over HTTP on the LAN. The only place that
/// fact ever appeared was inside the modal that created it — and closing that
/// modal does not stop the share (deliberately: a link you handed someone should
/// survive you closing the panel you copied it from). So a share you'd forgotten
/// went on serving, invisibly, and unstoppably: reopening the modal minted a NEW
/// share rather than adopting the live one, so the only 停止分享 button in the app
/// could never reach it again.
///
/// Emitted on every mutation (start / update / stop) and by the sweeper when a
/// share expires on its own — the UI's indicator has to disappear by itself, not
/// the next time somebody happens to open a modal.
pub(crate) fn emit_shares(app: &AppHandle, state: &AppState) {
    let _ = app.emit("shares_updated", list_shares_dto(state));
}

/// Core of [`start_share`], split off so it takes `&AppState` and is unit-testable
/// without a Tauri `State` (the `list_discovered_devices` → `list_devices_snapshot`
/// pattern). Builds the base URL FIRST so a server that never bound (`:0`) fails
/// before an unreachable share is ever registered.
fn do_start_share(
    state: &AppState,
    paths: Vec<String>,
    ttl_secs: u64,
    max_downloads: Option<u32>,
) -> Result<ShareStarted, LanBeamError> {
    // Each file's display name is its OWN base name — never a caller-influenced
    // string aimed elsewhere, and the share layer addresses bytes by index
    // regardless. `create_share` does the real gate (absolute + existing regular
    // file) and rejects an empty set.
    let files: Vec<(String, PathBuf)> = paths
        .into_iter()
        .map(|p| {
            let path = PathBuf::from(&p);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "download".to_string());
            (name, path)
        })
        .collect();
    let ttl = share::clamp_ttl(ttl_secs);
    let max = share::clamp_max_downloads(max_downloads);
    let base = share_base_url(state)?;
    let token = share::create_share(&state.shares, files, ttl, max)?;
    // Read the stored expiry back rather than recompute `now` — one source of
    // truth with what `list_shares` will later surface.
    let expires_at = share::share_expiry_secs(&state.shares, &token).unwrap_or(0);
    Ok(ShareStarted {
        url: format!("{base}/s/{token}"),
        token,
        expires_at,
    })
}

/// Core of [`update_share`]: clamp the requested TTL/cap the same way creation
/// does, then reconfigure. `None` for an unknown or already-stopped token.
fn do_update_share(
    state: &AppState,
    token: &str,
    ttl_secs: u64,
    max_downloads: Option<u32>,
) -> Option<ShareUpdated> {
    let ttl = share::clamp_ttl(ttl_secs);
    let max = share::clamp_max_downloads(max_downloads);
    share::update_share(&state.shares, token, ttl, max)
        .map(|expires_at| ShareUpdated { expires_at })
}

/// Core of [`list_shares`]: map each live share into its DTO, prefixing the
/// index links with the shared base URL. With no bound port the base URL is
/// unavailable, so entries carry an empty URL rather than a `:0` link (the list
/// itself is still honest about what exists).
fn list_shares_dto(state: &AppState) -> Vec<ShareEntry> {
    let base = share_base_url(state).ok();
    share::list_shares(&state.shares, Instant::now())
        .into_iter()
        .map(|s| ShareEntry {
            url: base
                .as_deref()
                .map(|b| format!("{b}/s/{}", s.token))
                .unwrap_or_default(),
            token: s.token,
            file_count: s.file_count,
            total_size: s.total_size,
            expires_at: s.expires_at_secs,
            downloads: s.downloads,
            max_downloads: s.max_downloads,
        })
        .collect()
}

/// Build the `http://<lan-ip>:<share_port>` prefix for a share URL. The host is
/// the first LAN IPv4 (respecting the interface filter when it is set and live),
/// falling back to loopback with a logged note when the machine has no LAN
/// address — the share still works on this box. Errors when the share server has
/// no bound port yet (browser-receive disabled or still starting): a `:0` URL is
/// useless, so registering an unreachable share is refused up front.
fn share_base_url(state: &AppState) -> Result<String, LanBeamError> {
    let port = state.share_port.load(Ordering::Relaxed);
    if port == 0 {
        return Err(LanBeamError::Io(
            "browser share server is not available".into(),
        ));
    }
    let host = match share_lan_ip(state) {
        Some(ip) => ip.to_string(),
        None => {
            log::warn!("no LAN address for a browser share; the link is loopback-only");
            Ipv4Addr::LOCALHOST.to_string()
        }
    };
    Ok(format!("http://{host}:{port}"))
}

/// The LAN IPv4 a share URL should point at. Uses the same ranked picker as the
/// pairing link ([`first_lan_ip`]): the interface-filter address when that filter
/// is set AND still matches a live interface (mirroring the discovery announcer's
/// choice), else the best-ranked address (see [`lan_ip_rank`] — a real NIC beats a
/// Docker/WSL/APIPA adapter; ties break on the numeric IP for stability). `None`
/// when the machine has no non-loopback IPv4.
///
/// [`lan_ip_rank`]: super::pairing::lan_ip_rank
fn share_lan_ip(state: &AppState) -> Option<Ipv4Addr> {
    let filter = state.settings.read().ok().and_then(|s| {
        s.iface_filter
            .as_deref()
            .and_then(|v| v.parse::<Ipv4Addr>().ok())
    });
    // Delegate to the pairing link's picker so a share URL never drifts from the
    // pairing hint — sharing a raw numeric sort here could advertise a virtual
    // 172.x/APIPA address a LAN peer can't reach. select()'s stale-filter fallback
    // matches the old fall-through ("never strand on an empty selection").
    first_lan_ip(filter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::test_app_state;

    /// A temp file returning its ABSOLUTE path — a share requires absolute paths.
    fn share_temp_file(tag: &str, bytes: &[u8]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lanbeam-cmdshare-{}-{}", std::process::id(), tag));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(tag);
        std::fs::write(&path, bytes).unwrap();
        path.canonicalize().unwrap()
    }

    /// `start_share` registers a share whose URL carries the bound share port and
    /// the returned token; `list_shares` then surfaces it with the right counts,
    /// and both agree on the URL.
    #[test]
    fn start_share_lists_with_url_and_port() {
        let state = test_app_state();
        state.share_port.store(52200, Ordering::Relaxed);
        let f = share_temp_file("doc.bin", b"hello share"); // 11 bytes

        let started = do_start_share(&state, vec![f.to_string_lossy().into_owned()], 600, Some(3))
            .expect("an existing absolute file starts a share");
        assert!(
            started.url.contains(":52200/s/"),
            "the URL carries the bound share port: {}",
            started.url
        );
        assert!(
            started.url.starts_with("http://"),
            "a plain-HTTP LAN URL: {}",
            started.url
        );
        assert!(
            started.url.ends_with(&started.token),
            "the URL ends with the token: {}",
            started.url
        );
        assert!(
            started.expires_at > 0,
            "a live share has a wall-clock expiry"
        );

        let listed = list_shares_dto(&state);
        assert_eq!(listed.len(), 1, "the new share is listed");
        let e = &listed[0];
        assert_eq!(e.token, started.token);
        assert_eq!(e.url, started.url, "list and start agree on the URL");
        assert_eq!(e.file_count, 1);
        assert_eq!(e.total_size, 11, "\"hello share\" is 11 bytes");
        assert_eq!(e.max_downloads, Some(3));
        assert_eq!(e.downloads, 0);
        assert_eq!(e.expires_at, started.expires_at);
    }

    /// With no bound share port (server disabled / still starting), `start_share`
    /// refuses to mint a `:0` link that no browser could reach.
    #[test]
    fn start_share_refused_without_a_bound_port() {
        let state = test_app_state(); // share_port defaults to 0
        let f = share_temp_file("np.bin", b"x");
        assert!(
            do_start_share(&state, vec![f.to_string_lossy().into_owned()], 600, None).is_err(),
            "no bound share port → start_share errors instead of minting a dead link"
        );
        assert!(
            list_shares_dto(&state).is_empty(),
            "a refused start registers nothing"
        );
    }

    /// `update_share` restarts the lifetime (pushing the expiry out) and lifts the
    /// cap; the change is reflected in `list_shares`, and an unknown token no-ops.
    #[test]
    fn update_share_changes_expiry_and_cap() {
        let state = test_app_state();
        state.share_port.store(52200, Ordering::Relaxed);
        let f = share_temp_file("u.bin", b"data");
        let started =
            do_start_share(&state, vec![f.to_string_lossy().into_owned()], 60, Some(1)).unwrap();

        let updated = do_update_share(&state, &started.token, 86_400, None)
            .expect("an active share reconfigures");
        assert!(
            updated.expires_at >= started.expires_at,
            "the longer TTL moves the expiry out: {} < {}",
            updated.expires_at,
            started.expires_at
        );

        let e = list_shares_dto(&state)
            .into_iter()
            .find(|e| e.token == started.token)
            .expect("the reconfigured share is still listed");
        assert_eq!(
            e.max_downloads, None,
            "the download cap was lifted to unlimited"
        );
        assert_eq!(
            e.expires_at, updated.expires_at,
            "the list shows the new expiry"
        );

        assert!(
            do_update_share(&state, "ghost-token", 60, None).is_none(),
            "an unknown token is a no-op"
        );
    }

    /// `stop_share` kills a share immediately: it drops off the list and can no
    /// longer serve — the registry gate would answer 410 for its link.
    #[test]
    fn stop_share_kills_the_link() {
        let state = test_app_state();
        state.share_port.store(52200, Ordering::Relaxed);
        let f = share_temp_file("s.bin", b"bye");
        let started =
            do_start_share(&state, vec![f.to_string_lossy().into_owned()], 600, None).unwrap();
        assert_eq!(
            list_shares_dto(&state).len(),
            1,
            "the share is live before stop"
        );

        share::stop_share(&state.shares, &started.token);
        assert!(
            list_shares_dto(&state).is_empty(),
            "a stopped share drops off the list"
        );
        assert!(
            !share::has_active_share(&state.shares, Instant::now()),
            "a stopped share can no longer serve — its link is dead"
        );
    }
}
