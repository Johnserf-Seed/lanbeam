//! ⭐ Security-sensitive: the browser-receive HTTP fallback (M8.1a).
//!
//! When the other side has no LanBeam, the sender can publish an explicit set of
//! files over plain HTTP for a browser to download. This module is the whole
//! server: a [`ShareRegistry`] of live [`Share`]s plus an axum task that serves
//! them.
//!
//! LOAD-BEARING INVARIANTS (HARD RULE 1):
//! - A share only ever serves the files it was CREATED with. The HTTP layer
//!   addresses a file by its NUMERIC INDEX into [`Share::files`] — a client can
//!   never supply a path or a name, so there is no traversal surface and the
//!   download directory is never exposed. Any path a browser sends that is not
//!   `/s/<token>` or `/s/<token>/<u32>` simply does not route.
//! - The `token` is a 24-char base64url draw from the OS CSPRNG (144 bits) — the
//!   only credential. Combined with the server-enforced TTL + max-downloads +
//!   stopped checks re-run on EVERY request, an unguessed link is the whole
//!   access-control story; we deliberately add no other auth (this is a LAN
//!   convenience tool, not an internet share).
//!
//! LAN EXPOSURE (documented per HARD RULE 1): the listener binds `0.0.0.0:0`
//! (an ephemeral port), so the share is reachable from every interface this
//! machine is on — the same reach as the transfer listener. That is intentional:
//! the browser on the peer's phone/laptop must connect over the LAN. The
//! unguessable token plus the per-request limits are what keep it safe; nothing
//! is served without a valid token.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::{ConnectInfo, Path as UrlPath, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Extension, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use tokio::net::TcpListener;
use tokio_util::io::ReaderStream;

use crate::error::{LanBeamError, Result};

/// Random bytes behind a share token. 18 bytes → 24 base64url chars (no
/// padding), a 144-bit unguessable space — comfortably past the 22-char floor,
/// and divisible by 3 so the encoding never carries `=` padding.
const TOKEN_BYTES: usize = 18;

/// TTL clamp range for a share, in SECONDS (M8.1b): at least a minute (a link
/// too short-lived to walk over and open is pointless) and at most a day (a LAN
/// convenience share is not a permanent host). `start_share`/`update_share`
/// clamp into this before minting, so a hand-crafted command can never register
/// an absurdly long-lived or already-dead share.
pub const SHARE_TTL_RANGE_SECS: (u64, u64) = (60, 86_400);

/// Upper bound on a share's download cap (M8.1b): `None` stays unlimited,
/// `Some(n)` is clamped into `1..=this`. The floor of 1 keeps a `Some(0)` — a
/// share nobody could ever download — from being stored.
pub const MAX_SHARE_DOWNLOADS: u32 = 1000;

/// Read-ahead buffer for streaming a shared file to the browser. `ReaderStream`
/// defaults to 4 KiB, which turns a multi-GB download into hundreds of thousands
/// of `spawn_blocking` read hops and tiny body frames, capping throughput well
/// below LAN line rate. 64 KiB matches the Noise transfer path's chunk
/// granularity and stays under `tokio::fs::File`'s 2 MiB max-buffer, so each poll
/// is still a single blocking read.
const SHARE_STREAM_BUF: usize = 64 * 1024;

/// Clamp a requested TTL (seconds) into [`SHARE_TTL_RANGE_SECS`] and return it
/// as the [`Duration`] [`create_share`]/[`update_share`] take.
pub fn clamp_ttl(secs: u64) -> Duration {
    Duration::from_secs(secs.clamp(SHARE_TTL_RANGE_SECS.0, SHARE_TTL_RANGE_SECS.1))
}

/// Clamp a requested download cap: `None` stays unlimited; `Some(n)` is pinned
/// into `1..=`[`MAX_SHARE_DOWNLOADS`], so neither a `0` (undownloadable) nor an
/// absurd cap can be stored.
pub fn clamp_max_downloads(max: Option<u32>) -> Option<u32> {
    max.map(|n| n.clamp(1, MAX_SHARE_DOWNLOADS))
}

/// One file registered in a share: the display name shown to the browser and
/// the ABSOLUTE on-disk path the bytes stream from. Captured at share creation;
/// the HTTP layer only ever reaches this by its numeric index in
/// [`Share::files`], never by name or path.
pub struct ShareFile {
    /// Label shown on the landing page and sent as the download filename.
    pub name: String,
    /// Absolute path validated to exist + be a regular file at creation time.
    pub path: PathBuf,
    /// Byte length captured at creation (the landing page's human size, and the
    /// `Content-Length` fallback if the file is unreadable at serve time).
    pub size: u64,
}

/// A live browser share. Created with a fixed file set, a TTL and an optional
/// download cap; every field the HTTP layer checks (`active`, `created`+`ttl`,
/// `downloads` vs `max_downloads`) is re-evaluated on EVERY request, so a share
/// cannot outlive its limits even if a link leaks.
pub struct Share {
    /// The access token — also the registry key. Kept on the struct so a future
    /// "list my active shares" command can surface the token/URL (M8.2).
    pub token: String,
    /// The exact files this share serves, addressed by index. Never mutated
    /// after creation.
    pub files: Vec<ShareFile>,
    /// Monotonic mint time; the share expires at `created + ttl`. An `Instant`
    /// (never displayed or persisted) so a wall-clock change cannot extend or
    /// cut short a share.
    created: Instant,
    /// How long past `created` the share stays serveable.
    ttl: Duration,
    /// The PER-FILE download cap, or `None` for unlimited. [`check`] scales it
    /// by the file count: "N 次" means each shared file may be fetched N times,
    /// so a share's total budget is `N × files.len()` — a single-file share is
    /// therefore unchanged (`N × 1`).
    max_downloads: Option<u32>,
    /// Raw count of individual file fetches so far — every served file bumps it
    /// by one, under the registry lock at the START of the serve (see
    /// [`download`]). Compared against the scaled budget in [`check`]; a HEAD
    /// probe never bumps it (it delivers no bytes).
    downloads: u32,
    /// Cleared by [`stop_share`]; a stopped share answers 410 on every request
    /// until the sweep reclaims it.
    active: bool,
    /// Wall-clock expiry (unix SECONDS) for DISPLAY ONLY (M8.1b) — surfaced by
    /// [`list_shares`]/[`share_expiry_secs`] so a ShareModal card can show a live
    /// countdown. Deliberately SEPARATE from the monotonic `created + ttl` gate
    /// in [`check`]: that gate stays immune to a wall-clock jump, while the UI
    /// needs a real timestamp to subtract `Date.now()` from. Recomputed by
    /// [`update_share`] whenever the TTL is reconfigured.
    expires_at_secs: u64,
}

/// Why a request was refused, mapped to the HTTP status the browser sees.
enum Denied {
    /// Unknown token, or an in-range route with an out-of-range file index.
    NotFound,
    /// The token existed but is no longer serveable: stopped, expired, or its
    /// download budget is spent.
    Gone,
}

impl Share {
    /// Whether the share may serve RIGHT NOW, given the caller's `now`. Passing
    /// `now` in (rather than reading the clock here) keeps the gate a pure
    /// function the tests pin down deterministically. Order of refusal does not
    /// matter — any failing check means "not serveable".
    fn check(&self, now: Instant) -> std::result::Result<(), Denied> {
        if !self.active {
            return Err(Denied::Gone);
        }
        // `duration_since` saturates to zero when `now` precedes `created`, so
        // this never panics on a clock quirk — it just reads as "not expired".
        if now.duration_since(self.created) >= self.ttl {
            return Err(Denied::Gone);
        }
        if let Some(per_file) = self.max_downloads {
            // The cap is PER FILE: "N 次" means each shared file may be fetched
            // N times, so the share's total budget scales with the file count.
            // `downloads` counts every served file, so a K-file share with cap N
            // retires only after N×K fetches — a single-file share is unchanged
            // (N×1 = N). `.max(1)` guards the impossible empty set (create_share
            // refuses one) so the budget is never a degenerate zero.
            let budget = per_file.saturating_mul(self.files.len().max(1) as u32);
            if self.downloads >= budget {
                return Err(Denied::Gone);
            }
        }
        Ok(())
    }
}

/// The shared registry of live shares: token → [`Share`]. Cloned (Arc) into both
/// [`AppState`](crate::state::AppState) — where the share commands mint/stop
/// entries — and the HTTP server task, so a share created by a command is
/// instantly serveable.
pub type ShareRegistry = Arc<Mutex<HashMap<String, Share>>>;

/// A fresh empty registry.
pub fn new_registry() -> ShareRegistry {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Emitted once for every file a browser actually fetches over a live share, so
/// the app can surface the download (toast / history / notification / live
/// count). Fired on a real, successful GET only — never a HEAD probe or a fetch
/// whose file vanished. Serialized camelCase to match the frontend event shape.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareDownloadEvent {
    /// The share the file belongs to (lets an open ShareModal match its own link).
    pub token: String,
    /// Index of the fetched file within the share.
    pub index: u32,
    /// The file's display name.
    pub name: String,
    /// The file's size in bytes.
    pub size: u64,
    /// Whole-SET downloads so far (raw per-file fetches ÷ file count), matching
    /// the count [`ShareInfo`] reports — so an open panel can display it verbatim.
    pub downloads: u32,
    /// The per-file download cap the sender set, or `null` for unlimited.
    pub max_downloads: Option<u32>,
    /// How many files the share serves (the divisor behind `downloads`).
    pub file_count: usize,
    /// The downloader's IP, best-effort (`"?"` if the peer address was unreadable).
    pub peer_ip: String,
}

/// An optional sink the download handler calls once per successful file fetch.
/// `None` in tests (and anywhere without a Tauri app); the real app wires it to
/// emit the [`ShareDownloadEvent`] + fire an OS notification. Cloneable + thread
/// safe so it can ride an axum `Extension` layer into the handler.
pub type DownloadHook = Option<Arc<dyn Fn(ShareDownloadEvent) + Send + Sync>>;

/// Mint a token and register a share over `files` (each `(display name,
/// absolute path)`), serveable for `ttl` with an optional `max_downloads` cap.
/// Returns the token.
///
/// Validation is done UP FRONT, at creation, where an error can be reported to
/// the user: every path must be ABSOLUTE and resolve to an existing regular
/// file (its size is captured then). Capturing absolute paths — never a
/// client-supplied string — is what makes the index-addressed HTTP layer safe.
/// An empty file set is refused (a share with nothing to serve is a bug).
pub fn create_share(
    registry: &ShareRegistry,
    files: Vec<(String, PathBuf)>,
    ttl: Duration,
    max_downloads: Option<u32>,
) -> Result<String> {
    if files.is_empty() {
        return Err(LanBeamError::UnsafePath(
            "a share must contain at least one file".into(),
        ));
    }
    let mut share_files = Vec::with_capacity(files.len());
    for (name, path) in files {
        // Reject relative paths outright: a share must pin the exact file, never
        // one resolved against whatever the process cwd happens to be.
        if !path.is_absolute() {
            return Err(LanBeamError::UnsafePath(format!(
                "share path is not absolute: {}",
                path.display()
            )));
        }
        let meta = std::fs::metadata(&path)
            .map_err(|e| LanBeamError::Io(format!("share file {}: {e}", path.display())))?;
        if !meta.is_file() {
            return Err(LanBeamError::UnsafePath(format!(
                "share path is not a regular file: {}",
                path.display()
            )));
        }
        share_files.push(ShareFile {
            name,
            path,
            size: meta.len(),
        });
    }

    let token = generate_token()?;
    let share = Share {
        token: token.clone(),
        files: share_files,
        created: Instant::now(),
        ttl,
        max_downloads,
        downloads: 0,
        active: true,
        expires_at_secs: now_unix_secs().saturating_add(ttl.as_secs()),
    };
    if let Ok(mut map) = registry.lock() {
        map.insert(token.clone(), share);
    }
    Ok(token)
}

/// Stop a share by token: it immediately answers 410 on every later request.
/// Marks it inactive rather than removing it, so any request already past the
/// lock (holding its captured file handle) still finishes, and the periodic
/// [`sweep`] reclaims the memory. Returns whether a share was found.
pub fn stop_share(registry: &ShareRegistry, token: &str) -> bool {
    match registry.lock() {
        Ok(mut map) => match map.get_mut(token) {
            Some(s) => {
                s.active = false;
                true
            }
            None => false,
        },
        Err(_) => false,
    }
}

/// Drop shares that can never serve again — stopped or past their TTL — so a
/// long-running server does not retain every share for its whole uptime. Safe
/// to call on a timer; a still-live share is left untouched. A budget-exhausted
/// share is kept until it also expires, so its link keeps giving an honest 410
/// rather than a bare 404.
pub fn sweep(registry: &ShareRegistry, now: Instant) {
    if let Ok(mut map) = registry.lock() {
        map.retain(|_, s| s.active && now.duration_since(s.created) < s.ttl);
    }
}

/// Reconfigure an ACTIVE share's TTL and download cap in place (M8.1b): the
/// ShareModal's lifetime/count dropdowns adjust a live share. The new TTL starts
/// NOW — `created` is reset — so "10 minutes" always means ten minutes from the
/// change, never from the original mint (which would surprise the user
/// reconfiguring an old share). The download COUNTER is left as-is: slots
/// already spent stay spent, so lowering the cap below the current count simply
/// retires the share (its next request 410s), the honest outcome. Returns the
/// new display expiry (unix seconds), or `None` for an unknown, already-stopped,
/// or already-expired token — a no-op-safe call either way.
pub fn update_share(
    registry: &ShareRegistry,
    token: &str,
    ttl: Duration,
    max_downloads: Option<u32>,
) -> Option<u64> {
    let mut map = registry.lock().ok()?;
    let now = Instant::now();
    let share = map.get_mut(token)?;
    // A stopped share stays dead — reconfiguring must not resurrect a link the
    // user already killed.
    if !share.active {
        return None;
    }
    // An EXPIRED-but-unswept share is likewise dead: `active` stays true until
    // the ~60s sweep reclaims it, but its TTL has elapsed. Reconfiguring here
    // would silently revive a link the user believes gone with a fresh TTL, so
    // refuse it — an expired share can only be re-created via `create_share`.
    // TTL-only guard (mirrors sweep()/check()); NOT check(now), which would also
    // refuse a budget-exhausted-but-active share that update should still be
    // allowed to retire or extend (see the doc above).
    if now.duration_since(share.created) >= share.ttl {
        return None;
    }
    share.created = Instant::now();
    share.ttl = ttl;
    share.max_downloads = max_downloads;
    share.expires_at_secs = now_unix_secs().saturating_add(ttl.as_secs());
    Some(share.expires_at_secs)
}

/// The display expiry (unix seconds) stored for `token`, or `None` if unknown.
/// Lets `start_share` return the SAME wall-clock expiry a share was minted with,
/// from one source of truth (the stored field) rather than recomputing `now`.
pub fn share_expiry_secs(registry: &ShareRegistry, token: &str) -> Option<u64> {
    registry.lock().ok()?.get(token).map(|s| s.expires_at_secs)
}

/// One live share as [`list_shares`] surfaces it (M8.1b) — everything the shares
/// list / ShareModal needs EXCEPT the URL, which the command builds from the LAN
/// address + bound port. `total_size` sums the files' captured sizes.
pub struct ShareInfo {
    pub token: String,
    pub file_count: usize,
    pub total_size: u64,
    pub expires_at_secs: u64,
    /// How many times the WHOLE shared set has been downloaded — the raw
    /// per-file fetch tally divided by the file count. Reported this way (rather
    /// than raw fetches) so it pairs honestly with the PER-FILE `max_downloads`:
    /// a value in `0..=max_downloads` that reads as "the set was downloaded this
    /// many of N times". A single-file share divides by one, so it is the raw
    /// fetch count, unchanged.
    pub downloads: u32,
    /// The PER-FILE download cap the user chose (the "N 次" dropdown value), or
    /// `None` for unlimited — matches what `update_share` takes back. The
    /// share's real budget is this × [`file_count`](Self::file_count).
    pub max_downloads: Option<u32>,
}

/// Snapshot every still-listable share for the shares view: active and
/// un-expired. A budget-EXHAUSTED share is still listed — its `downloads` /
/// `max_downloads` let the UI show "limit reached" — but a stopped or expired one
/// is omitted (it is effectively gone, and the sweep will reclaim it). Ordered
/// newest-expiry-first (ties broken by token) so the list is stable across calls
/// rather than following `HashMap`'s arbitrary iteration order.
pub fn list_shares(registry: &ShareRegistry, now: Instant) -> Vec<ShareInfo> {
    let Ok(map) = registry.lock() else {
        return Vec::new();
    };
    let mut out: Vec<ShareInfo> = map
        .values()
        .filter(|s| s.active && now.duration_since(s.created) < s.ttl)
        .map(|s| ShareInfo {
            token: s.token.clone(),
            file_count: s.files.len(),
            total_size: s.files.iter().map(|f| f.size).sum(),
            expires_at_secs: s.expires_at_secs,
            // Raw per-file fetches → completed whole-set downloads, so it stays
            // in `0..=max_downloads` (see `ShareInfo::downloads`).
            downloads: s.downloads / (s.files.len().max(1) as u32),
            max_downloads: s.max_downloads,
        })
        .collect();
    out.sort_by(|a, b| {
        b.expires_at_secs
            .cmp(&a.expires_at_secs)
            .then_with(|| a.token.cmp(&b.token))
    });
    out
}

/// Whether at least one share can serve RIGHT NOW (active, un-expired, budget
/// left) — the signal the discovery announcer uses to decide whether to
/// advertise the browser-share HTTP port (M8.3). A stopped or budget-exhausted
/// share does NOT count: advertising a link that only 410s would be misleading.
pub fn has_active_share(registry: &ShareRegistry, now: Instant) -> bool {
    registry
        .lock()
        .map(|m| m.values().any(|s| s.check(now).is_ok()))
        .unwrap_or(false)
}

/// Current wall-clock time as unix SECONDS (display-only expiries). Saturates to
/// `0` if the clock is somehow before the epoch — an impossible-in-practice
/// value that still yields an already-expired display timestamp rather than a
/// panic.
fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Refund one download slot for `token` after a reserved serve failed to start
/// (the file vanished between registration and the request). Best-effort: a
/// missing share or poisoned lock is a no-op — the worst case is one download
/// counted that did not deliver, which errs toward serving fewer, never more.
fn refund(registry: &ShareRegistry, token: &str) {
    if let Ok(mut map) = registry.lock() {
        if let Some(s) = map.get_mut(token) {
            s.downloads = s.downloads.saturating_sub(1);
        }
    }
}

/// Draw a fresh unguessable token from the OS CSPRNG. Fails closed: a getrandom
/// error (an OS-level catastrophe the whole crypto stack would already be dead
/// from) propagates rather than ever handing out a predictable token.
fn generate_token() -> Result<String> {
    let mut buf = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut buf).map_err(|e| LanBeamError::Crypto(format!("share token rng: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// Everything the share server task needs: the registry it serves and the
/// atomic to publish the port it binds.
pub struct ShareServerCtx {
    pub registry: ShareRegistry,
    /// Set to the actually-bound ephemeral port once the listener is up (0 until
    /// then) — read to build a share URL / advertise the port (M8.2/M8.3).
    pub share_port: Arc<AtomicU16>,
    /// Called once per successful file download so the app can surface it
    /// (toast / history / notification / live count). `None` disables that.
    pub on_download: DownloadHook,
}

/// Spawn the browser-share HTTP server on the Tauri runtime, plus a periodic
/// sweeper. Binds `0.0.0.0:0` (see the module LAN-exposure note) and publishes
/// the bound port into `share_port`. A bind failure disables browser-receive but
/// must not take down the app — it is logged, and the rest of LanBeam runs on.
pub fn spawn_share_server(ctx: ShareServerCtx) {
    tauri::async_runtime::spawn(async move {
        let listener = match TcpListener::bind(("0.0.0.0", 0)).await {
            Ok(l) => l,
            Err(e) => {
                log::error!("share server bind failed (browser receive disabled): {e}");
                return;
            }
        };
        match listener.local_addr() {
            Ok(addr) => {
                ctx.share_port.store(addr.port(), Ordering::Relaxed);
                log::info!("browser-share server on {addr}");
            }
            // Bound but the address is unreadable — serve anyway; only the
            // advertised URL is affected, and the port stays at its 0 sentinel.
            Err(e) => log::warn!("share server bound but local_addr failed: {e}"),
        }

        // Reclaim expired/stopped shares roughly every minute — cheap, and keeps
        // a desk machine's registry from carrying every share of a long session.
        {
            let reg = ctx.registry.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(60));
                loop {
                    ticker.tick().await;
                    sweep(&reg, Instant::now());
                }
            });
        }

        // A vanished LAN peer would otherwise pin its connection and fd forever:
        // axum::serve sets no idle bound, so a client that connects and never
        // sends a request line, or a phone that drops off WiFi with a keep-alive
        // connection idle (no RST/FIN), leaves the task ESTABLISHED indefinitely.
        // Enable TCP keepalive on the LISTENING socket so the OS reaps dead peers;
        // accepted sockets inherit SO_KEEPALIVE on Windows/Linux/macOS. Best-effort
        // — a keepalive failure must not take down the share server.
        let _ = socket2::SockRef::from(&listener)
            .set_tcp_keepalive(&socket2::TcpKeepalive::new().with_time(Duration::from_secs(60)));

        let app = router(ctx.registry.clone(), ctx.on_download.clone());
        // `into_make_service_with_connect_info` is what makes the peer address
        // available to the download handler (for the ShareDownloadEvent's IP).
        let svc = app.into_make_service_with_connect_info::<SocketAddr>();
        if let Err(e) = axum::serve(listener, svc).await {
            log::error!("share server exited: {e}");
        }
    });
}

/// Build the axum router. ONLY two routes exist — the token landing page and
/// the index-addressed download — so no request can reach anything but an
/// explicitly registered file. `:index` is typed `u32`, so a non-numeric
/// segment fails path parsing (400) before any handler runs.
fn router(registry: ShareRegistry, on_download: DownloadHook) -> Router {
    Router::new()
        .route("/s/:token", get(landing))
        .route("/s/:token/:index", get(download))
        // The download hook rides an Extension layer so only the download handler
        // pulls it (the landing page never counts a download).
        .layer(Extension(on_download))
        .with_state(registry)
}

/// `GET /s/:token` — the entry point a shared link points at. Validates the
/// token, then: a single-file share redirects straight to the download; a
/// multi-file share renders a minimal listing (one download link per index).
/// This route never counts against the download budget — only fetching a file
/// does.
async fn landing(State(reg): State<ShareRegistry>, UrlPath(token): UrlPath<String>) -> Response {
    // Snapshot everything the response needs under the lock, then release it —
    // the render/redirect below touches no share state and never awaits while
    // holding a std lock.
    let listing: Vec<(String, u64)> = {
        let map = match reg.lock() {
            Ok(m) => m,
            Err(_) => return internal_error(),
        };
        let share = match map.get(&token) {
            Some(s) => s,
            None => return denied(Denied::NotFound),
        };
        if let Err(d) = share.check(Instant::now()) {
            return denied(d);
        }
        if share.files.len() == 1 {
            // One file → hand the browser the download directly (302).
            return redirect_to(&format!("/s/{token}/0"));
        }
        share
            .files
            .iter()
            .map(|f| (f.name.clone(), f.size))
            .collect()
    };
    Html(render_landing(&token, &listing)).into_response()
}

/// `GET /s/:token/:index` — stream one registered file as an attachment (a HEAD
/// on the same route returns those headers only, spending no slot). Validates
/// the token, bounds-checks the index and reserves a download slot ALL under the
/// registry lock, so N concurrent requests can never collectively exceed the
/// share's budget (the per-file cap × its file count). The file is then opened
/// and streamed with the lock released (never held across `.await`); a file that
/// vanished since creation refunds its slot and 404s.
async fn download(
    State(reg): State<ShareRegistry>,
    Extension(on_download): Extension<DownloadHook>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: Method,
    UrlPath((token, index)): UrlPath<(String, u32)>,
) -> Response {
    // axum 0.7 routes a HEAD with no explicit HEAD route to this GET handler. A
    // HEAD is a metadata probe — link unfurlers, download managers, AV/proxy
    // pre-flights and `curl -I` all send one — so it must NOT spend a download
    // slot or open the file; it only advertises the headers a GET would carry.
    let is_head = method == Method::HEAD;

    let (path, name, size, ev) = {
        let mut map = match reg.lock() {
            Ok(m) => m,
            Err(_) => return internal_error(),
        };
        let share = match map.get_mut(&token) {
            Some(s) => s,
            None => return denied(Denied::NotFound),
        };
        if let Err(d) = share.check(Instant::now()) {
            return denied(d);
        }
        let (path, name, size) = match share.files.get(index as usize) {
            Some(f) => (f.path.clone(), f.name.clone(), f.size),
            // A valid token but no such file index — a fabricated or stale link.
            None => return denied(Denied::NotFound),
        };
        // Reserve the slot atomically with the checks above — but only for a
        // real GET. A HEAD delivers no bytes, so it reserves nothing. The event
        // is assembled here (counts consistent under the lock) but only FIRED
        // later, once the file actually opens — a vanished file refunds the slot
        // and surfaces nothing.
        let ev = if is_head {
            None
        } else {
            share.downloads += 1;
            let file_count = share.files.len().max(1);
            Some(ShareDownloadEvent {
                token: token.clone(),
                index,
                name: name.clone(),
                size,
                // Whole-SET count, matching ShareInfo.downloads (raw ÷ files).
                downloads: share.downloads / file_count as u32,
                max_downloads: share.max_downloads,
                file_count,
                peer_ip: peer.ip().to_string(),
            })
        };
        (path, name, size, ev)
    };

    // HEAD: advertise the captured metadata and stop — no file open, no body,
    // no slot spent. Content-Length is the size captured at creation (the same
    // value a GET falls back to when the live metadata is unreadable).
    if is_head {
        let mut resp = Response::new(Body::empty());
        apply_file_headers(resp.headers_mut(), size, &name);
        return resp;
    }

    match tokio::fs::File::open(&path).await {
        Ok(file) => {
            // Prefer the live length so Content-Length matches the bytes we
            // actually stream; fall back to the size captured at creation.
            let len = match file.metadata().await {
                Ok(m) => m.len(),
                Err(_) => size,
            };
            let body = Body::from_stream(ReaderStream::with_capacity(file, SHARE_STREAM_BUF));
            let mut resp = Response::new(body);
            apply_file_headers(resp.headers_mut(), len, &name);
            // The file opened and is about to stream — a real download. Surface
            // it to the app. (`ev` is None for a HEAD, and this arm is never
            // reached when the file vanished, so no false positives.)
            if let (Some(ev), Some(hook)) = (ev, &on_download) {
                hook(ev);
            }
            resp
        }
        Err(e) => {
            // The file is gone since it was registered — give the slot back and
            // report it as missing rather than leaking the OS error.
            refund(&reg, &token);
            log::warn!("share file {} unavailable: {e}", path.display());
            denied(Denied::NotFound)
        }
    }
}

/// Set the download response headers a share serves for a file of `len` bytes
/// named `name`: octet-stream type, content length, and an `attachment`
/// disposition. Shared by the GET stream and the HEAD probe so both advertise
/// identical metadata (a HEAD must report what a GET would send).
fn apply_file_headers(h: &mut HeaderMap, len: u64, name: &str) {
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(v) = HeaderValue::from_str(&len.to_string()) {
        h.insert(header::CONTENT_LENGTH, v);
    }
    if let Ok(v) = HeaderValue::from_str(&content_disposition(name)) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
}

/// A 302 redirect to `location` — a bare, browser-followed hop (used to send a
/// single-file share straight to its download).
fn redirect_to(location: &str) -> Response {
    let mut resp = Response::new(Body::empty());
    *resp.status_mut() = StatusCode::FOUND;
    if let Ok(v) = HeaderValue::from_str(location) {
        resp.headers_mut().insert(header::LOCATION, v);
    }
    resp
}

/// The tiny HTML error page for a refused request, with the right status.
fn denied(d: Denied) -> Response {
    let (status, msg) = match d {
        Denied::NotFound => (
            StatusCode::NOT_FOUND,
            "This share link is invalid or the file no longer exists.",
        ),
        Denied::Gone => (
            StatusCode::GONE,
            "This share is no longer available — it was stopped, expired, or hit its download limit.",
        ),
    };
    (status, Html(message_page(msg))).into_response()
}

/// A poisoned lock (a panicked holder) — surface a 500 rather than unwrap-panic
/// the server task.
fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html(message_page("Something went wrong serving this share.")),
    )
        .into_response()
}

/// Build the `Content-Disposition` header value for a download. Carries BOTH an
/// ASCII `filename=` fallback (older clients) and a UTF-8 `filename*=`
/// (RFC 5987 / 6266, honored by modern browsers), so a non-ASCII name survives
/// the trip and the value is always a safe single token.
fn content_disposition(name: &str) -> String {
    let ascii = ascii_fallback(name);
    let encoded = rfc5987_encode(name);
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

/// A quoted-string-safe ASCII rendering of `name`: non-ASCII, control, quote and
/// backslash bytes become `_`. Falls back to `download` if nothing survives, so
/// the `filename=` fallback is never empty.
fn ascii_fallback(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.trim().is_empty() {
        "download".to_string()
    } else {
        s
    }
}

/// Percent-encode `name` per RFC 5987 `value-chars`: attr-chars pass through,
/// everything else becomes `%HH`. The result is a valid `filename*` token.
fn rfc5987_encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for &b in name.as_bytes() {
        let attr_char = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
            );
        if attr_char {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

/// Render the multi-file landing page: honest LanBeam branding, one download
/// link per file (by index), each with its human size. Self-contained — inline
/// CSS only, no external assets — and every file name is HTML-escaped.
fn render_landing(token: &str, files: &[(String, u64)]) -> String {
    let mut items = String::new();
    for (i, (name, size)) in files.iter().enumerate() {
        // The href is built only from the validated token (base64url) and a
        // numeric index — both intrinsically URL-safe.
        items.push_str(&format!(
            "<li><a href=\"/s/{token}/{i}\">{}</a><span class=\"sz\">{}</span></li>\n",
            html_escape(name),
            human_size(*size),
        ));
    }
    let count = files.len();
    let noun = if count == 1 { "file" } else { "files" };
    page_shell(&format!(
        "<h1>LanBeam</h1>\
         <p class=\"lead\">{count} {noun} shared with you.</p>\
         <ul class=\"files\">{items}</ul>\
         <p class=\"foot\">Served directly from the sender's device over your local network — no account, no cloud, no third-party server.</p>"
    ))
}

/// A one-line message page (errors, refusals) in the same shell as the listing.
fn message_page(msg: &str) -> String {
    page_shell(&format!(
        "<h1>LanBeam</h1><p class=\"lead\">{}</p>",
        html_escape(msg)
    ))
}

/// The shared HTML document shell: inline styles, no external assets, so the
/// page renders identically with no network access beyond this response.
fn page_shell(body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>LanBeam</title><style>\
         body{{font-family:system-ui,-apple-system,Segoe UI,Roboto,sans-serif;\
         max-width:34rem;margin:3rem auto;padding:0 1.25rem;color:#1a1a1a;\
         background:#fafafa;line-height:1.5}}\
         h1{{font-size:1.4rem;margin:0 0 .25rem}}\
         .lead{{color:#444;margin:.25rem 0 1.5rem}}\
         ul.files{{list-style:none;padding:0;margin:0}}\
         ul.files li{{display:flex;justify-content:space-between;align-items:center;\
         gap:1rem;padding:.6rem .8rem;border:1px solid #e3e3e3;border-radius:.5rem;\
         margin-bottom:.5rem;background:#fff}}\
         ul.files a{{color:#2563eb;text-decoration:none;word-break:break-all}}\
         ul.files a:hover{{text-decoration:underline}}\
         .sz{{color:#777;font-size:.85rem;white-space:nowrap}}\
         .foot{{color:#999;font-size:.8rem;margin-top:2rem}}\
         @media(prefers-color-scheme:dark){{body{{background:#111;color:#eee}}\
         .lead{{color:#bbb}}ul.files li{{background:#1b1b1b;border-color:#333}}\
         ul.files a{{color:#6ea8fe}}.sz{{color:#888}}.foot{{color:#666}}}}\
         </style></head><body>{body}</body></html>"
    )
}

/// Minimal HTML-entity escaping for untrusted text placed in element content or
/// a quoted attribute.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// A compact human-readable byte size (`1.5 MB`, `812 B`). Binary units, one
/// decimal above bytes.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    /// Write `content` into a fresh temp file and return its ABSOLUTE path.
    fn temp_file(tag: &str, content: &[u8]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lanbeam-share-{}-{}", std::process::id(), unique()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(tag);
        std::fs::write(&path, content).unwrap();
        path.canonicalize().unwrap()
    }

    /// A process-unique counter so parallel test cases never collide on a path.
    fn unique() -> u64 {
        use std::sync::atomic::AtomicU64;
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Spawn the router on an ephemeral loopback port; return the bound address.
    async fn serve(registry: ShareRegistry) -> SocketAddr {
        serve_with_hook(registry, None).await
    }

    /// Serve a registry with an explicit download hook, so a test can assert the
    /// [`ShareDownloadEvent`] a real fetch fires (`serve` passes `None`).
    async fn serve_with_hook(registry: ShareRegistry, on_download: DownloadHook) -> SocketAddr {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(registry, on_download);
        tokio::spawn(async move {
            let svc = app.into_make_service_with_connect_info::<SocketAddr>();
            let _ = axum::serve(listener, svc).await;
        });
        addr
    }

    /// A one-shot HTTP/1.1 GET (Connection: close) returning
    /// `(status, header_block, body)`. Reading to EOF is safe because the close
    /// header makes the server drop the connection after the response.
    async fn http_get(addr: SocketAddr, path: &str) -> (u16, String, Vec<u8>) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let sep = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("response has a header terminator");
        let head = String::from_utf8_lossy(&buf[..sep]).to_string();
        let body = buf[sep + 4..].to_vec();
        let status: u16 = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .expect("a numeric status on the status line");
        (status, head, body)
    }

    /// A one-shot HTTP/1.1 HEAD (Connection: close) returning `(status,
    /// header_block)`. A HEAD carries no body, so only the head is read.
    async fn http_head(addr: SocketAddr, path: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let req = format!("HEAD {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let head = String::from_utf8_lossy(&buf).to_string();
        let status: u16 = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .expect("a numeric status on the status line");
        (status, head)
    }

    // ── registry validation (create_share) ───────────────────────────────

    /// Creation captures existing regular files and rejects the rest: empty
    /// sets, relative paths, missing paths and directories all fail up front.
    #[test]
    fn create_share_validates_files() {
        let reg = new_registry();
        let f = temp_file("ok.bin", b"hello");

        let token = create_share(
            &reg,
            vec![("ok.bin".into(), f.clone())],
            Duration::from_secs(60),
            None,
        )
        .expect("an existing absolute file is accepted");
        assert!(
            token.len() >= 22,
            "token must be >= 22 chars, got {}",
            token.len()
        );

        assert!(
            create_share(&reg, vec![], Duration::from_secs(60), None).is_err(),
            "empty set rejected"
        );
        assert!(
            create_share(
                &reg,
                vec![("x".into(), PathBuf::from("relative/x"))],
                Duration::from_secs(60),
                None
            )
            .is_err(),
            "relative path rejected"
        );
        let gone = f.parent().unwrap().join("nope.bin");
        assert!(
            create_share(
                &reg,
                vec![("g".into(), gone)],
                Duration::from_secs(60),
                None
            )
            .is_err(),
            "missing path rejected"
        );
        assert!(
            create_share(
                &reg,
                vec![("d".into(), f.parent().unwrap().to_path_buf())],
                Duration::from_secs(60),
                None
            )
            .is_err(),
            "a directory is not a regular file"
        );
    }

    /// Tokens are unguessable and distinct: two shares never collide, and each
    /// token is URL-safe base64 (no padding, no separators).
    #[test]
    fn tokens_are_unique_and_url_safe() {
        let reg = new_registry();
        let f = temp_file("t.bin", b"x");
        let a = create_share(
            &reg,
            vec![("t".into(), f.clone())],
            Duration::from_secs(60),
            None,
        )
        .unwrap();
        let b = create_share(&reg, vec![("t".into(), f)], Duration::from_secs(60), None).unwrap();
        assert_ne!(a, b, "each share gets a fresh token");
        for t in [&a, &b] {
            assert!(
                t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "token must be URL-safe base64url: {t}"
            );
        }
    }

    /// The pure serveability gate (M8.1a): inactive, expired and budget-spent
    /// shares are all refused; a fresh in-budget share passes.
    #[test]
    fn check_gates_active_ttl_and_budget() {
        let now = Instant::now();
        let base = Share {
            token: "t".into(),
            files: vec![],
            created: now,
            ttl: Duration::from_secs(60),
            max_downloads: Some(2),
            downloads: 0,
            active: true,
            expires_at_secs: 0,
        };
        assert!(base.check(now).is_ok(), "a fresh in-budget share serves");

        let stopped = Share {
            active: false,
            ..clone_share(&base)
        };
        assert!(
            matches!(stopped.check(now), Err(Denied::Gone)),
            "stopped → Gone"
        );

        // Expired: query a moment past created + ttl.
        assert!(
            matches!(base.check(now + Duration::from_secs(61)), Err(Denied::Gone)),
            "past TTL → Gone"
        );

        let spent = Share {
            downloads: 2,
            ..clone_share(&base)
        };
        assert!(
            matches!(spent.check(now), Err(Denied::Gone)),
            "budget spent → Gone"
        );
    }

    /// A cheap field copy for the check test (Share isn't Clone by design — it
    /// owns file handles' worth of paths — so spell out the copy here).
    fn clone_share(s: &Share) -> Share {
        Share {
            token: s.token.clone(),
            files: vec![],
            created: s.created,
            ttl: s.ttl,
            max_downloads: s.max_downloads,
            downloads: s.downloads,
            active: s.active,
            expires_at_secs: s.expires_at_secs,
        }
    }

    // ── HTTP surface ──────────────────────────────────────────────────────

    /// A token-addressed fetch returns exactly the shared file's bytes, as an
    /// attachment with the right length.
    #[tokio::test]
    async fn fetch_returns_the_right_bytes() {
        let reg = new_registry();
        let body = b"the quick brown fox".repeat(100);
        let f = temp_file("doc.bin", &body);
        let token = create_share(
            &reg,
            vec![("doc.bin".into(), f)],
            Duration::from_secs(60),
            None,
        )
        .unwrap();
        let addr = serve(reg).await;

        let (status, head, got) = http_get(addr, &format!("/s/{token}/0")).await;
        assert_eq!(status, 200);
        assert_eq!(got, body, "the served bytes are the shared file");
        assert!(
            head.to_lowercase()
                .contains("content-disposition: attachment"),
            "served as attachment: {head}"
        );
        assert!(
            head.to_lowercase()
                .contains(&format!("content-length: {}", body.len())),
            "content-length matches the file: {head}"
        );
    }

    #[tokio::test]
    async fn a_real_get_fires_the_download_hook_but_a_head_does_not() {
        let reg = new_registry();
        let f = temp_file("hooked.bin", b"payload-123");
        let token = create_share(
            &reg,
            vec![("hooked.bin".into(), f)],
            Duration::from_secs(60),
            Some(3),
        )
        .unwrap();

        let seen: Arc<Mutex<Vec<ShareDownloadEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let hook: DownloadHook = Some(Arc::new(move |ev| sink.lock().unwrap().push(ev)));
        let addr = serve_with_hook(reg, hook).await;

        // A HEAD probe serves no bytes → it must NOT fire the hook.
        let _ = http_head(addr, &format!("/s/{token}/0")).await;
        assert!(
            seen.lock().unwrap().is_empty(),
            "a HEAD must not fire the download hook"
        );

        // A real GET streams the file → exactly one event, fields faithful.
        let (status, _, body) = http_get(addr, &format!("/s/{token}/0")).await;
        assert_eq!(status, 200);
        assert_eq!(body, b"payload-123");

        let evs = seen.lock().unwrap();
        assert_eq!(evs.len(), 1, "one successful GET → one download event");
        let ev = &evs[0];
        assert_eq!(ev.token, token);
        assert_eq!(ev.index, 0);
        assert_eq!(ev.name, "hooked.bin");
        assert_eq!(ev.size, 11);
        assert_eq!(ev.downloads, 1, "whole-set count after one fetch");
        assert_eq!(ev.max_downloads, Some(3));
        assert_eq!(ev.file_count, 1);
        assert!(!ev.peer_ip.is_empty(), "a loopback fetch carries a peer IP");
    }

    /// A single-file share's landing route 302-redirects straight to the file.
    #[tokio::test]
    async fn single_file_landing_redirects_to_index_zero() {
        let reg = new_registry();
        let f = temp_file("only.bin", b"solo");
        let token = create_share(
            &reg,
            vec![("only.bin".into(), f)],
            Duration::from_secs(60),
            None,
        )
        .unwrap();
        let addr = serve(reg).await;

        let (status, head, _) = http_get(addr, &format!("/s/{token}")).await;
        assert_eq!(status, 302, "single file redirects");
        // The header name is emitted lowercase; the token keeps its case, so
        // match against the raw head (don't lowercase — that would mangle the
        // mixed-case base64url token).
        assert!(
            head.contains(&format!("location: /s/{token}/0")),
            "redirects to index 0: {head}"
        );
    }

    /// The multi-file landing page lists EXACTLY the shared files (one link per
    /// index) and nothing else.
    #[tokio::test]
    async fn landing_lists_exactly_the_shared_files() {
        let reg = new_registry();
        let f1 = temp_file("alpha.txt", b"aaaa");
        let f2 = temp_file("beta.txt", b"bbbbbbbb");
        let token = create_share(
            &reg,
            vec![("alpha.txt".into(), f1), ("beta.txt".into(), f2)],
            Duration::from_secs(60),
            None,
        )
        .unwrap();
        let addr = serve(reg).await;

        let (status, _, body) = http_get(addr, &format!("/s/{token}")).await;
        let html = String::from_utf8_lossy(&body);
        assert_eq!(status, 200);
        assert!(html.contains("alpha.txt"), "lists the first file: {html}");
        assert!(html.contains("beta.txt"), "lists the second file");
        assert!(
            html.contains(&format!("/s/{token}/0")),
            "download link for index 0"
        );
        assert!(
            html.contains(&format!("/s/{token}/1")),
            "download link for index 1"
        );
        assert!(
            !html.contains(&format!("/s/{token}/2")),
            "no phantom third file"
        );
    }

    /// Unknown, stopped and expired tokens are all refused (404 / 410) — no
    /// bytes ever served.
    #[tokio::test]
    async fn unknown_stopped_and_expired_are_refused() {
        let reg = new_registry();
        let f = temp_file("s.bin", b"data");
        let stopped = create_share(
            &reg,
            vec![("s.bin".into(), f.clone())],
            Duration::from_secs(60),
            None,
        )
        .unwrap();
        let expired = create_share(
            &reg,
            vec![("s.bin".into(), f)],
            Duration::from_millis(40),
            None,
        )
        .unwrap();
        stop_share(&reg, &stopped);
        let addr = serve(reg).await;

        let (unknown, _, _) = http_get(addr, "/s/deadbeefdeadbeefdeadbe/0").await;
        assert_eq!(unknown, 404, "unknown token → 404");

        let (stop_code, _, _) = http_get(addr, &format!("/s/{stopped}/0")).await;
        assert_eq!(stop_code, 410, "stopped token → 410");

        tokio::time::sleep(Duration::from_millis(80)).await;
        let (exp_code, _, _) = http_get(addr, &format!("/s/{expired}/0")).await;
        assert_eq!(exp_code, 410, "expired token → 410");
    }

    /// An out-of-range index on a valid token is 404; a non-numeric index does
    /// not even route (400 from path parsing).
    #[tokio::test]
    async fn index_out_of_range_404_and_non_numeric_rejected() {
        let reg = new_registry();
        let f = temp_file("one.bin", b"only one file");
        let token = create_share(
            &reg,
            vec![("one.bin".into(), f)],
            Duration::from_secs(60),
            None,
        )
        .unwrap();
        let addr = serve(reg).await;

        let (oob, _, _) = http_get(addr, &format!("/s/{token}/5")).await;
        assert_eq!(oob, 404, "index past the file set → 404");

        let (bad, _, _) = http_get(addr, &format!("/s/{token}/notanumber")).await;
        assert_eq!(bad, 400, "a non-numeric index fails path parsing → 400");
    }

    /// max_downloads is enforced server-side: the first N succeed, the (N+1)th
    /// is 410 Gone.
    #[tokio::test]
    async fn max_downloads_enforced() {
        let reg = new_registry();
        let f = temp_file("cap.bin", b"capped");
        let token = create_share(
            &reg,
            vec![("cap.bin".into(), f)],
            Duration::from_secs(60),
            Some(2),
        )
        .unwrap();
        let addr = serve(reg).await;

        let (a, _, _) = http_get(addr, &format!("/s/{token}/0")).await;
        let (b, _, _) = http_get(addr, &format!("/s/{token}/0")).await;
        assert_eq!((a, b), (200, 200), "the first two downloads succeed");

        let (c, _, _) = http_get(addr, &format!("/s/{token}/0")).await;
        assert_eq!(c, 410, "the download over the cap → 410 Gone");
    }

    /// A HEAD probe returns the download headers but spends NO slot: on a
    /// Some(1) share the count stays zero and the recipient's real GET still
    /// serves the bytes. (axum routes HEAD to the GET handler, so the handler
    /// must special-case it — otherwise a pre-flight HEAD burns the one slot.)
    #[tokio::test]
    async fn head_probe_does_not_consume_a_download_slot() {
        let reg = new_registry();
        let body = b"headers only, please".repeat(16);
        let f = temp_file("head.bin", &body);
        let token = create_share(
            &reg,
            vec![("head.bin".into(), f)],
            Duration::from_secs(60),
            Some(1),
        )
        .unwrap();
        let addr = serve(reg.clone()).await;

        let (hstatus, hhead) = http_head(addr, &format!("/s/{token}/0")).await;
        assert_eq!(hstatus, 200, "a HEAD is answered with headers");
        let lower = hhead.to_lowercase();
        assert!(
            lower.contains("content-disposition: attachment"),
            "HEAD advertises the attachment disposition: {hhead}"
        );
        assert!(
            lower.contains(&format!("content-length: {}", body.len())),
            "HEAD advertises the file length: {hhead}"
        );

        // The HEAD spent no slot: list_shares still reports zero downloads...
        let listed = list_shares(&reg, Instant::now());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].downloads, 0, "a HEAD counts as no download");

        // ...and the recipient's real GET on this Some(1) share still succeeds.
        let (gstatus, _, got) = http_get(addr, &format!("/s/{token}/0")).await;
        assert_eq!(gstatus, 200, "the real GET still serves after a HEAD");
        assert_eq!(got, body, "and delivers the file bytes");
    }

    /// The download cap is PER FILE: "N 次" means each shared file may be fetched
    /// N times (effective budget = N × file count). A single-file Some(1) share
    /// is unchanged — one fetch, then 410 — while a K-file Some(1) share serves
    /// every file exactly once before any further fetch 410s.
    #[tokio::test]
    async fn cap_is_per_file_not_per_share() {
        // Single file, cap 1: unchanged — one download, the second 410s.
        {
            let reg = new_registry();
            let f = temp_file("solo.bin", b"solo");
            let token = create_share(
                &reg,
                vec![("solo.bin".into(), f)],
                Duration::from_secs(60),
                Some(1),
            )
            .unwrap();
            let addr = serve(reg).await;
            let (a, _, _) = http_get(addr, &format!("/s/{token}/0")).await;
            let (b, _, _) = http_get(addr, &format!("/s/{token}/0")).await;
            assert_eq!(
                (a, b),
                (200, 410),
                "single-file cap=1: one download then Gone"
            );
        }

        // Three files, cap 1: each of the three downloads once, then any repeat
        // (even of a different index) 410s once the N×K budget is spent.
        {
            let reg = new_registry();
            let f0 = temp_file("m0.bin", b"zero");
            let f1 = temp_file("m1.bin", b"one!");
            let f2 = temp_file("m2.bin", b"twoo");
            let token = create_share(
                &reg,
                vec![
                    ("m0.bin".into(), f0),
                    ("m1.bin".into(), f1),
                    ("m2.bin".into(), f2),
                ],
                Duration::from_secs(60),
                Some(1),
            )
            .unwrap();
            let addr = serve(reg).await;
            for i in 0..3 {
                let (s, _, _) = http_get(addr, &format!("/s/{token}/{i}")).await;
                assert_eq!(s, 200, "file {i} downloads once under a per-file cap");
            }
            let (repeat, _, _) = http_get(addr, &format!("/s/{token}/0")).await;
            assert_eq!(repeat, 410, "every file spent once → repeats are Gone");
        }
    }

    /// The sweep reclaims stopped/expired shares while leaving a live one.
    #[test]
    fn sweep_drops_dead_shares_only() {
        let reg = new_registry();
        let f = temp_file("sw.bin", b"x");
        let live = create_share(
            &reg,
            vec![("sw".into(), f.clone())],
            Duration::from_secs(60),
            None,
        )
        .unwrap();
        let stopped = create_share(
            &reg,
            vec![("sw".into(), f.clone())],
            Duration::from_secs(60),
            None,
        )
        .unwrap();
        let expired =
            create_share(&reg, vec![("sw".into(), f)], Duration::from_millis(1), None).unwrap();
        stop_share(&reg, &stopped);

        // Sweep at a time past the short TTL.
        sweep(&reg, Instant::now() + Duration::from_secs(1));
        let map = reg.lock().unwrap();
        assert!(map.contains_key(&live), "the live share survives");
        assert!(
            !map.contains_key(&stopped),
            "the stopped share is reclaimed"
        );
        assert!(
            !map.contains_key(&expired),
            "the expired share is reclaimed"
        );
    }

    // ── registry reconfigure / list (M8.1b) ───────────────────────────────

    /// TTL + download-cap clamps: the TTL floors/ceils into the range, and the
    /// download cap keeps `None` unlimited while pinning `Some` into `1..=max`.
    #[test]
    fn clamps_pin_ttl_and_download_cap() {
        let (lo, hi) = SHARE_TTL_RANGE_SECS;
        assert_eq!(clamp_ttl(0).as_secs(), lo, "below the floor snaps up");
        assert_eq!(
            clamp_ttl(600).as_secs(),
            600,
            "an in-range TTL passes through"
        );
        assert_eq!(
            clamp_ttl(u64::MAX).as_secs(),
            hi,
            "above the ceiling snaps down"
        );

        assert_eq!(clamp_max_downloads(None), None, "unlimited stays unlimited");
        assert_eq!(clamp_max_downloads(Some(0)), Some(1), "0 snaps up to 1");
        assert_eq!(
            clamp_max_downloads(Some(5)),
            Some(5),
            "in-range passes through"
        );
        assert_eq!(
            clamp_max_downloads(Some(MAX_SHARE_DOWNLOADS + 50)),
            Some(MAX_SHARE_DOWNLOADS),
            "over the cap snaps down"
        );
    }

    /// `update_share` reconfigures an ACTIVE share's TTL/cap and returns the new
    /// display expiry; a stopped or unknown token is a no-op (`None`).
    #[test]
    fn update_share_reconfigures_active_only() {
        let reg = new_registry();
        let f = temp_file("u.bin", b"x");
        let token = create_share(
            &reg,
            vec![("u".into(), f.clone())],
            Duration::from_secs(60),
            Some(1),
        )
        .unwrap();
        let before = share_expiry_secs(&reg, &token).expect("a fresh share has an expiry");

        // Extend the TTL and lift the cap: the returned expiry reflects the new,
        // longer TTL (>= the old one — the new TTL starts now).
        let updated = update_share(&reg, &token, Duration::from_secs(3600), None)
            .expect("an active share reconfigures");
        assert!(
            updated >= before,
            "the extended TTL pushes the expiry out: {updated} < {before}"
        );
        {
            let map = reg.lock().unwrap();
            let s = map.get(&token).unwrap();
            assert_eq!(s.max_downloads, None, "the cap was lifted to unlimited");
        }

        // Unknown token → no-op.
        assert!(update_share(&reg, "nope", Duration::from_secs(60), None).is_none());

        // A stopped share cannot be reconfigured back to life.
        stop_share(&reg, &token);
        assert!(
            update_share(&reg, &token, Duration::from_secs(60), None).is_none(),
            "a stopped share stays dead"
        );
    }

    /// An EXPIRED-but-unswept share cannot be revived by `update_share`: its
    /// `active` flag is still set until the sweep runs, but the TTL guard refuses
    /// the reconfigure, so a link the user believes dead never gets a fresh TTL.
    #[test]
    fn update_share_refuses_an_expired_share() {
        let reg = new_registry();
        let f = temp_file("exp.bin", b"x");
        let token = create_share(
            &reg,
            vec![("exp".into(), f)],
            Duration::from_secs(60),
            Some(1),
        )
        .unwrap();

        // Force expiry without waiting: zero the TTL so `now - created >= ttl`
        // holds immediately, while `active` stays true (the sweep hasn't run).
        {
            let mut map = reg.lock().unwrap();
            map.get_mut(&token).unwrap().ttl = Duration::ZERO;
        }

        assert!(
            update_share(&reg, &token, Duration::from_secs(600), None).is_none(),
            "an expired (but still-active) share is not resurrected by a reconfigure"
        );
        // And it truly stays dead: the gate refuses it.
        let map = reg.lock().unwrap();
        assert!(
            matches!(
                map.get(&token).unwrap().check(Instant::now()),
                Err(Denied::Gone)
            ),
            "the expired share still 410s — no fresh TTL was granted"
        );
    }

    /// `list_shares` surfaces active + un-expired shares (including a
    /// budget-exhausted one) with the right counts, and omits stopped/expired
    /// ones; `has_active_share` reports whether any share is still serveable.
    #[test]
    fn list_shares_reflects_live_shares() {
        let reg = new_registry();
        let f1 = temp_file("a.bin", b"aaaa"); // 4 bytes
        let f2 = temp_file("b.bin", b"bbbbbbbb"); // 8 bytes
        let live = create_share(
            &reg,
            vec![("a.bin".into(), f1.clone()), ("b.bin".into(), f2)],
            Duration::from_secs(60),
            Some(3),
        )
        .unwrap();
        let stopped = create_share(
            &reg,
            vec![("a.bin".into(), f1)],
            Duration::from_secs(60),
            None,
        )
        .unwrap();
        stop_share(&reg, &stopped);

        let now = Instant::now();
        let listed = list_shares(&reg, now);
        assert_eq!(listed.len(), 1, "only the live share is listed");
        let info = &listed[0];
        assert_eq!(info.token, live);
        assert_eq!(info.file_count, 2, "both registered files are counted");
        assert_eq!(info.total_size, 12, "sizes sum (4 + 8)");
        assert_eq!(info.downloads, 0);
        assert_eq!(info.max_downloads, Some(3));
        assert!(
            has_active_share(&reg, now),
            "a serveable share is advertised"
        );

        // Stop the last live share → nothing to list or advertise.
        stop_share(&reg, &live);
        assert!(
            list_shares(&reg, now).is_empty(),
            "a stopped share drops off the list"
        );
        assert!(
            !has_active_share(&reg, now),
            "no serveable share → nothing to advertise"
        );
    }

    /// A budget-exhausted share is STILL listed (so the UI can show the count)
    /// but is NOT advertised for discovery (its link only 410s).
    #[test]
    fn exhausted_share_lists_but_is_not_advertised() {
        let reg = new_registry();
        let f = temp_file("cap.bin", b"x");
        let token = create_share(
            &reg,
            vec![("cap".into(), f)],
            Duration::from_secs(60),
            Some(1),
        )
        .unwrap();
        // Spend the single slot directly on the registry (no HTTP roundtrip).
        {
            let mut map = reg.lock().unwrap();
            map.get_mut(&token).unwrap().downloads = 1;
        }
        let now = Instant::now();
        assert_eq!(
            list_shares(&reg, now).len(),
            1,
            "an exhausted share stays visible in the list"
        );
        assert!(
            !has_active_share(&reg, now),
            "an exhausted share is not advertised — its link only 410s"
        );
    }

    /// Byte-size formatting and RFC 5987 disposition encoding are stable.
    #[test]
    fn helpers_format_size_and_disposition() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");

        let cd = content_disposition("naïve rapport.pdf");
        assert!(
            cd.contains("filename*=UTF-8''"),
            "carries an RFC 5987 filename*: {cd}"
        );
        assert!(
            cd.contains("filename=\""),
            "carries an ASCII fallback: {cd}"
        );
        // The non-ASCII 'ï' is percent-encoded in the extended form and replaced
        // in the ASCII fallback — the header value stays a safe token either way.
        assert!(cd.contains("na%C3%AFve"), "utf-8 percent-encoded: {cd}");
    }

    /// A name that leaves no printable ASCII (only whitespace survives) falls the
    /// `filename=` back to the literal `download`, never an empty token, while the
    /// extended `filename*` still percent-encodes the original bytes.
    #[test]
    fn disposition_ascii_fallback_defaults_to_download() {
        let cd = content_disposition("   ");
        assert!(
            cd.contains("filename=\"download\""),
            "an all-whitespace name falls back to `download`: {cd}"
        );
        assert!(
            cd.contains("filename*=UTF-8''%20%20%20"),
            "the extended form still encodes the original bytes: {cd}"
        );
    }

    /// The refusal helper maps each [`Denied`] variant to the HTTP status the
    /// browser sees — an unknown/out-of-range route is 404, a dead share is 410.
    #[test]
    fn denied_maps_variants_to_status() {
        assert_eq!(
            denied(Denied::NotFound).status(),
            StatusCode::NOT_FOUND,
            "NotFound → 404"
        );
        assert_eq!(
            denied(Denied::Gone).status(),
            StatusCode::GONE,
            "Gone → 410"
        );
    }

    /// `stop_share` reports whether it found a share: a real token flips it
    /// inactive and returns true; an unknown token is a no-op returning false.
    #[test]
    fn stop_share_reports_whether_found() {
        let reg = new_registry();
        let f = temp_file("st.bin", b"x");
        let token =
            create_share(&reg, vec![("st".into(), f)], Duration::from_secs(60), None).unwrap();
        assert!(stop_share(&reg, &token), "an existing share is stopped");
        assert!(
            !stop_share(&reg, "no-such-token"),
            "an unknown token stops nothing → false"
        );
    }

    /// `share_expiry_secs` returns the stored display expiry for a known token and
    /// `None` for one that was never registered.
    #[test]
    fn share_expiry_secs_known_and_unknown() {
        let reg = new_registry();
        let f = temp_file("ex.bin", b"x");
        let token =
            create_share(&reg, vec![("ex".into(), f)], Duration::from_secs(120), None).unwrap();
        assert!(
            share_expiry_secs(&reg, &token).is_some(),
            "a fresh share has a display expiry"
        );
        assert_eq!(
            share_expiry_secs(&reg, "nope"),
            None,
            "an unknown token has no expiry"
        );
    }

    /// `list_shares` reports COMPLETED whole-set downloads: the raw per-file fetch
    /// tally divided by the file count. A 2-file share with 4 raw fetches reads as
    /// two completed downloads.
    #[test]
    fn list_shares_divides_downloads_by_file_count() {
        let reg = new_registry();
        let f1 = temp_file("d1.bin", b"a");
        let f2 = temp_file("d2.bin", b"b");
        let token = create_share(
            &reg,
            vec![("d1".into(), f1), ("d2".into(), f2)],
            Duration::from_secs(60),
            Some(3),
        )
        .unwrap();
        // Four raw per-file fetches = two completed whole-set downloads.
        {
            let mut map = reg.lock().unwrap();
            map.get_mut(&token).unwrap().downloads = 4;
        }
        let listed = list_shares(&reg, Instant::now());
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].downloads, 2,
            "4 raw fetches / 2 files = 2 whole-set downloads"
        );
    }

    /// `list_shares` omits an EXPIRED-but-unswept share: its `active` flag is still
    /// set, but the TTL filter drops it (mirrors the sweep/check gate).
    #[test]
    fn list_shares_omits_an_expired_share() {
        let reg = new_registry();
        let f = temp_file("le.bin", b"x");
        let token =
            create_share(&reg, vec![("le".into(), f)], Duration::from_secs(60), None).unwrap();
        // Force expiry without waiting: zero the TTL while `active` stays true.
        {
            let mut map = reg.lock().unwrap();
            map.get_mut(&token).unwrap().ttl = Duration::ZERO;
        }
        assert!(
            list_shares(&reg, Instant::now()).is_empty(),
            "an expired share is not listed even before the sweep reclaims it"
        );
    }

    /// The sweep keeps a budget-EXHAUSTED but still-active, un-expired share: its
    /// link keeps giving an honest 410 until it also expires, rather than a bare
    /// 404. Only stopped or past-TTL shares are reclaimed.
    #[test]
    fn sweep_keeps_an_exhausted_but_unexpired_share() {
        let reg = new_registry();
        let f = temp_file("sk.bin", b"x");
        let token = create_share(
            &reg,
            vec![("sk".into(), f)],
            Duration::from_secs(60),
            Some(1),
        )
        .unwrap();
        // Spend the whole budget, but leave it active and un-expired.
        {
            let mut map = reg.lock().unwrap();
            map.get_mut(&token).unwrap().downloads = 5;
        }
        sweep(&reg, Instant::now());
        assert!(
            reg.lock().unwrap().contains_key(&token),
            "an exhausted-but-unexpired share survives the sweep"
        );
    }

    /// `has_active_share` is false on an empty registry — nothing to advertise.
    #[test]
    fn has_active_share_false_on_empty_registry() {
        let reg = new_registry();
        assert!(
            !has_active_share(&reg, Instant::now()),
            "an empty registry advertises no share"
        );
    }
}
