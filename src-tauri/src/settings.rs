//! User settings, persisted as one JSON blob (the `{"settings": {...}}` shape
//! tauri-plugin-store originally wrote) via an atomic temp+fsync+rename write.
//! Loaded into `AppState` at startup; commands mutate + re-save.
//!
//! Schema-evolution contract (M4.1): every field carries a per-field
//! `#[serde(default = "...")]`, so a blob written by an OLDER build (missing
//! newer fields) merges field-by-field instead of resetting the whole blob.
//! Only hard corruption falls back wholesale to `Default` — value-level
//! (wrong types inside a parseable file, see [`parse_or_default`]) and
//! file-level (malformed JSON the store plugin silently swallows, see
//! [`corruption_diag`]) — and BOTH classes surface a diagnostic the caller
//! logs after logger init; a wiped settings file is never silent.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::consts::{DEFAULT_HOTKEY, DEFAULT_TCP_PORT};

const STORE_FILE: &str = "settings.json";
const KEY: &str = "settings";

/// Store file name for a given test-instance id (`LANBEAM_INSTANCE`): the
/// primary keeps `settings.json`, an instance gets its own
/// `settings-{id}.json` — the same isolation as `trusted-{id}.json` and the
/// keychain identity, so a setting changed in a test instance (its decorated
/// device name, a `recv_policy` experiment) can never clobber the primary's
/// persisted settings.
fn store_file_for(instance: Option<&str>) -> String {
    match instance.filter(|s| !s.is_empty()) {
        Some(id) => format!("settings-{id}.json"),
        None => STORE_FILE.to_string(),
    }
}

/// The store file for THIS process — re-reads `LANBEAM_INSTANCE` with the
/// same emptiness filter as `lib.rs`'s `instance_id`.
fn store_file() -> String {
    let instance = std::env::var("LANBEAM_INSTANCE").ok();
    store_file_for(instance.as_deref())
}

/// Allowed `log_level` values; setters ignore anything else so the persisted
/// blob never contains a level the logging layer can't map.
pub const LOG_LEVELS: [&str; 3] = ["errors", "normal", "verbose"];

/// Allowed `recv_policy` values; same ignore-invalid contract as [`LOG_LEVELS`].
pub const RECV_POLICIES: [&str; 3] = ["ask", "trusted", "all"];

/// Allowed `conflict` values (M6.5); same ignore-invalid contract as
/// [`LOG_LEVELS`]. `"rename"` de-duplicates (`report (1).pdf`), `"overwrite"`
/// replaces the existing file, `"ask"` prompts the user per transfer.
pub const CONFLICT_POLICIES: [&str; 3] = ["rename", "overwrite", "ask"];

/// Allowed `organize` values (M6.6); same ignore-invalid contract as
/// [`LOG_LEVELS`]. `"none"` writes straight under the download root, `"device"`
/// files each transfer under a `<sender name>/` folder, `"date"` under a
/// `<YYYY-MM-DD>/` folder.
pub const ORGANIZE_MODES: [&str; 3] = ["none", "device", "date"];

/// Inclusive range the `max_concurrent` setter clamps into (M6.7): at least one
/// transfer streams at a time, at most eight — beyond which more parallelism
/// just thrashes the disk/NIC on a LAN without going faster.
pub const MAX_CONCURRENT_RANGE: (u32, u32) = (1, 8);

/// Inclusive range the `ui_zoom` setter clamps into. The floor is where the app's
/// 10px mono labels stop being readable; the ceiling is where a 920pt-wide window
/// (the layout's own minimum) can no longer hold the layout even after the window
/// minimum scales up with it — past 1.5 the app would be demanding a window most
/// laptops can't give it.
pub const UI_ZOOM_RANGE: (f64, f64) = (0.8, 1.5);

/// The layout's minimum in CSS pixels — the same numbers as `tauri.conf.json`'s
/// `minWidth`/`minHeight`, and they have to stay in step.
///
/// WHY they live here at all: a webview zoom SHRINKS the CSS viewport. A 920pt-wide
/// window at 150% has 613 CSS px of layout space, which is far under what this UI
/// is built for — so a zoom that ignored the window minimum would let the user
/// scale the interface straight off the edge of its own window (clipped sidebar,
/// clipped radar). The window floor scales with the zoom instead; see
/// `lib::apply_ui_zoom`.
pub const MIN_LAYOUT_SIZE: (f64, f64) = (920.0, 600.0);

/// Upper bound (in MB/s) the `rate_limit` setter accepts (M6.7). 1 TB/s is
/// already far beyond any LAN hardware, so the cap only exists to reject absurd
/// input — and, crucially, to keep `mb * 1024 * 1024` from overflowing u64 (a
/// value near 2^44 would wrap to a 0-byte/s cap that hard-stalls transfers).
pub const MAX_RATE_LIMIT_MB: u64 = 1_000_000;

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_device_name")]
    pub device_name: String,
    #[serde(default = "default_discoverable")]
    pub discoverable: bool,
    #[serde(default = "default_auto_open_folder")]
    pub auto_open_folder: bool,
    /// Diagnostics verbosity: `"errors" | "normal" | "verbose"`.
    /// Stored as a string (not an enum) so future levels stay parseable by old builds.
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Inbound transfer policy: `"ask"` (always prompt), `"trusted"`
    /// (auto-accept peers in the trust store), `"all"` (auto-accept everyone).
    #[serde(default = "default_recv_policy")]
    pub recv_policy: String,
    /// Absolute path overriding the OS download folder; `None` = OS default.
    /// Stored canonicalized by the setter, but re-validated at every startup
    /// via [`canonical_dir`] — the folder can vanish between runs (unplugged
    /// drive, deleted directory), and receiving must fall back rather than
    /// fail every write.
    #[serde(default = "default_download_dir_override")]
    pub download_dir_override: Option<String>,
    /// TCP listen port; `0` means "LanBeam's default". WHY a sentinel instead
    /// of persisting 51704: a blob that never pinned a port keeps following
    /// `DEFAULT_TCP_PORT` if it ever changes across upgrades. Applied on the
    /// NEXT launch — the running listener keeps its bound port, which
    /// discovery advertises, so peers adapt either way.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Closing the window hides to the system tray instead of quitting
    /// (desktop, M5.3). Read live on every `CloseRequested`, so toggling it
    /// needs no restart.
    #[serde(default = "default_tray_close")]
    pub tray_close: bool,
    /// OS notifications for incoming prompts / finished receives (M5.4).
    /// Read at fire time, so toggling it needs no restart.
    #[serde(default = "default_notif_system")]
    pub notif_system: bool,
    /// Launch LanBeam at login (M5.5). This field is the source of truth: the
    /// setter mirrors it into the OS launch entry, and startup reconciles the
    /// OS state back to it (the entry can drift — removed via Task Manager,
    /// changed by a reinstall).
    #[serde(default = "default_autostart")]
    pub autostart: bool,
    /// Restrict discovery to the interface carrying this IPv4 address;
    /// `None` = announce/listen on all interfaces (M5.6). Stored as a string
    /// (like the discovery packet's addresses) and re-matched against the live
    /// enumeration every announce tick — a filter whose interface vanished
    /// falls back to ALL interfaces rather than stranding discovery.
    #[serde(default = "default_iface_filter")]
    pub iface_filter: Option<String>,
    /// Opt-in for the global quick-summon hotkey (M5.5). Default OFF: the
    /// chord (`DEFAULT_HOTKEY`, Alt+Space) is the Windows system-menu shortcut,
    /// so LanBeam must not claim it OS-wide until the user asks. Registered
    /// live by `set_hotkey_enabled` and (re)applied from this value at startup.
    #[serde(default = "default_hotkey_enabled")]
    pub hotkey_enabled: bool,
    /// The accelerator the global quick-summon hotkey binds (M5.5 rebind). Format
    /// is the tauri global-shortcut form `MOD(+MOD)*+KEY` — one or more modifiers
    /// (`Ctrl` / `Alt` / `Shift` / `Super`) plus one key (`Space`, `K`, `F5`,
    /// `ArrowUp`, …), e.g. `Alt+Space`, `Ctrl+Shift+K`. Defaults to
    /// [`DEFAULT_HOTKEY`]; validated by [`valid_hotkey`] before it can persist, so
    /// the stored value is always registerable. Read (not the const) by the
    /// startup registration and by `set_hotkey_enabled` when it (re)binds.
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    /// Per-file SHA-256 integrity verification (M6.3). When on, the SENDER
    /// hashes each file (one extra read pass) and puts the digest in the
    /// manifest; the RECEIVER re-hashes the received bytes and rejects — then
    /// deletes — any file whose hash differs. Read at send time, so toggling it
    /// needs no restart.
    #[serde(default = "default_verify_hash")]
    pub verify_hash: bool,
    /// Interface scale. 1.0 = the design size; the webview is zoomed to this and
    /// the window's minimum size scales with it (see `MIN_LAYOUT_SIZE`).
    #[serde(default = "default_ui_zoom")]
    pub ui_zoom: f64,
    /// How a received name that collides with an existing file is resolved
    /// (M6.5): `"rename"` (de-dupe), `"overwrite"` (replace), or `"ask"` (prompt
    /// per transfer via the ConflictModal). Read at receive time, so toggling it
    /// needs no restart. A resumable partial (M6.4) takes precedence over this —
    /// continuing the same file is not a new collision.
    #[serde(default = "default_conflict")]
    pub conflict: String,
    /// Auto-organize received files into subfolders (M6.6): `"none"`, `"device"`
    /// (a `<sender name>/` folder), or `"date"` (a `<YYYY-MM-DD>/` folder).
    /// Read at receive time; the subfolder is computed when a transfer starts.
    #[serde(default = "default_organize")]
    pub organize: String,
    /// How many transfers may stream bytes at once (M6.7), clamped to
    /// [`MAX_CONCURRENT_RANGE`] by the setter. Read LIVE each time a transfer
    /// reaches the concurrency gate, so lowering it throttles transfers that
    /// have not started streaming yet — ones already in flight keep their slot
    /// until they finish. Applies across BOTH directions (send + receive share
    /// one gate).
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: u32,
    /// Per-transfer throughput cap (M6.7): `"unlimited"`, or a positive integer
    /// count of MB/s (the UI offers "50" / "10"). Applied to both the send write
    /// loop and the receive read loop, PER TRANSFER — so with N transfers active
    /// each may reach the cap. Read once when a transfer starts streaming.
    #[serde(default = "default_rate_limit")]
    pub rate_limit: String,
    /// Whether an incoming quick text is ALSO written to this machine's
    /// clipboard (M7.3). This is the receiver's CONSENT switch: it must be on
    /// (and the sender must have asked) before any peer's text touches the local
    /// clipboard, so it defaults OFF — a fresh install never lets foreign text
    /// onto the clipboard unasked. Read at receive time, so toggling it needs no
    /// restart.
    #[serde(default = "default_clip_share")]
    pub clip_share: bool,
    /// Strip EXIF / ICC / XMP metadata from JPEG/PNG/WebP images before sending
    /// them (M9.1). The metadata (GPS location, camera model, capture time) is
    /// removed at the container level from a temporary copy — pixels are never
    /// recompressed — and only that copy is sent; formats we can't parse pass
    /// through untouched. Read at send time (and overridable per send from the
    /// confirm dialog), so toggling it needs no restart. Defaults ON so a fresh
    /// install does not leak photo location by default.
    #[serde(default = "default_strip_exif")]
    pub strip_exif: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device_name: default_device_name(),
            discoverable: default_discoverable(),
            auto_open_folder: default_auto_open_folder(),
            log_level: default_log_level(),
            recv_policy: default_recv_policy(),
            download_dir_override: default_download_dir_override(),
            port: default_port(),
            tray_close: default_tray_close(),
            notif_system: default_notif_system(),
            autostart: default_autostart(),
            iface_filter: default_iface_filter(),
            hotkey_enabled: default_hotkey_enabled(),
            hotkey: default_hotkey(),
            verify_hash: default_verify_hash(),
            ui_zoom: default_ui_zoom(),
            conflict: default_conflict(),
            organize: default_organize(),
            max_concurrent: default_max_concurrent(),
            rate_limit: default_rate_limit(),
            clip_share: default_clip_share(),
            strip_exif: default_strip_exif(),
        }
    }
}

/// Best-effort OS device/host name for the default `device_name`.
pub fn default_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "LanBeam device".to_string())
}

fn default_discoverable() -> bool {
    true
}

fn default_auto_open_folder() -> bool {
    true
}

fn default_log_level() -> String {
    "normal".to_string()
}

fn default_recv_policy() -> String {
    // Prompt-free receiving only from peers the user already trusts —
    // the safe middle ground between "ask" (noisy) and "all" (open door).
    "trusted".to_string()
}

fn default_download_dir_override() -> Option<String> {
    None // no override — the startup path resolves the OS download folder
}

fn default_port() -> u16 {
    0 // sentinel: follow DEFAULT_TCP_PORT — see the field's doc for why
}

fn default_tray_close() -> bool {
    // A LAN receiver is most useful when it keeps listening in the background;
    // quitting outright is the surprising default for this kind of app.
    true
}

fn default_notif_system() -> bool {
    // With close-to-tray on by default, the window is often hidden when a
    // prompt or completion lands — the OS toast is how the user finds out.
    true
}

fn default_autostart() -> bool {
    // Installing an app must not silently claim a login item; the user opts in.
    false
}

fn default_iface_filter() -> Option<String> {
    None // all interfaces — the right answer for almost every machine
}

fn default_hotkey_enabled() -> bool {
    // Alt+Space is the Windows system-menu chord; do not hijack it OS-wide
    // until the user opts in.
    false
}

fn default_hotkey() -> String {
    // The const stays the single source of the default chord; the field only
    // exists so the user can rebind it (M5.5). Kept in sync automatically.
    DEFAULT_HOTKEY.to_string()
}

fn default_verify_hash() -> bool {
    // Integrity verification is on out of the box: silent corruption of a
    // received file is worse than one extra read pass on the sender, and the
    // UI prototype ships this defaulted on too.
    true
}

fn default_conflict() -> String {
    // Prompt on collision by default: silently overwriting a file the user
    // already has is the one irreversible receive outcome, and always renaming
    // buries duplicates. When no one is at the keyboard (auto-accept) the
    // receive path falls back to the safe rename — see `handle_incoming`.
    "ask".to_string()
}

fn default_organize() -> String {
    // Write straight under the download root, preserving the pre-M6.6 layout;
    // sub-foldering is opt-in.
    "none".to_string()
}

fn default_ui_zoom() -> f64 {
    1.0
}

fn default_max_concurrent() -> u32 {
    // Three at once saturates a typical LAN link without thrashing the disk;
    // the user can raise it up to the range ceiling or drop to serial (1).
    3
}

fn default_rate_limit() -> String {
    // No throttle out of the box — the fast path costs nothing (see
    // `rate_limit_bytes_per_sec`).
    "unlimited".to_string()
}

fn default_clip_share() -> bool {
    // Off by default: writing an incoming text to the local clipboard is a
    // privacy-sensitive action, so the user must opt in before any peer's text
    // can land on this machine's clipboard (M7.3).
    false
}

pub fn default_strip_exif() -> bool {
    // On by default: sending a photo with its GPS/camera/time EXIF intact leaks
    // the sender's location, so scrub it out of the box (M9.1). Matches the UI
    // prototype, which ships this defaulted on too. The strip is lossless and
    // best-effort — unsupported formats simply pass through — so defaulting on
    // costs nothing beyond the strippable images actually being cleaned.
    true
}

/// Clamp a requested `max_concurrent` into [`MAX_CONCURRENT_RANGE`] (M6.7). Used
/// by the setter to reject nonsense up front AND at every gate read, so even a
/// hand-edited blob (`max_concurrent: 999`) can never uncap parallelism.
pub fn clamp_max_concurrent(n: u32) -> u32 {
    n.clamp(MAX_CONCURRENT_RANGE.0, MAX_CONCURRENT_RANGE.1)
}

/// Clamp a UI scale into the usable range. A NaN — which a hand-edited settings.json
/// or a bad float could produce — falls back to 1.0 rather than propagating into a
/// window size, where it would resolve to nothing at all.
pub fn clamp_ui_zoom(z: f64) -> f64 {
    if !z.is_finite() {
        return default_ui_zoom();
    }
    z.clamp(UI_ZOOM_RANGE.0, UI_ZOOM_RANGE.1)
}

/// Whether a `rate_limit` string is one the setter may persist (M6.7):
/// `"unlimited"` or a positive integer count of MB/s. Same ignore-invalid
/// contract as [`LOG_LEVELS`] — a `0` or non-numeric value is rejected so a
/// throttle that would wedge every transfer never reaches the blob.
pub fn valid_rate_limit(rate_limit: &str) -> bool {
    let s = rate_limit.trim();
    s == "unlimited"
        || s.parse::<u64>()
            .is_ok_and(|mb| mb > 0 && mb <= MAX_RATE_LIMIT_MB)
}

/// Resolve a stored `rate_limit` string to a byte-per-second cap (M6.7).
/// `"unlimited"` — and, defensively, any unrecognized or non-positive value
/// (a hand-edited blob) — yields `None`, the zero-overhead unthrottled path.
/// A positive integer is MB/s (matching the UI) converted to bytes/sec. WHY
/// interpret unknowns as unlimited rather than error: a bad throttle value must
/// never silently stall receiving; the worst case is "no limit", not "no data".
pub fn rate_limit_bytes_per_sec(rate_limit: &str) -> Option<u64> {
    match rate_limit.trim() {
        "unlimited" | "" => None,
        s => s
            .parse::<u64>()
            .ok()
            .filter(|&mb| mb > 0)
            // Clamp to the sane ceiling THEN saturating-multiply: a hand-edited
            // blob with an absurd MB value (this runs on the raw stored string,
            // which bypasses `valid_rate_limit`) can neither overflow u64 — a
            // value near 2^44 would wrap `mb * 1024 * 1024` to 0, a hard stall —
            // nor become a garbage cap. The worst case is a finite, very high
            // limit, never "no data".
            .map(|mb| mb.min(MAX_RATE_LIMIT_MB).saturating_mul(1024 * 1024)),
    }
}

/// Normalize a `set_iface_filter` input. Outer `None` = invalid, ignore (the
/// same setters-ignore-invalid contract as [`LOG_LEVELS`]); `Some(None)` =
/// clear the filter (empty/whitespace input); `Some(Some(ip))` = store the
/// canonical IPv4 text. Parsing through `Ipv4Addr` is what makes "canonical"
/// real — the stored string must compare equal to `Iface::ip.to_string()`
/// at match time, so hand-typed forms like `" 192.168.001.5 "` cannot persist.
pub fn parse_iface_filter(input: &str) -> Option<Option<String>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Some(None);
    }
    trimmed
        .parse::<std::net::Ipv4Addr>()
        .ok()
        .map(|ip| Some(ip.to_string()))
}

/// The canonicalized form of `path` IF it currently exists and is a
/// directory. The single validity gate shared by `set_download_dir` (reject
/// bad input up front) and startup resolution (a persisted override whose
/// folder vanished must fall back, not break receiving). Canonicalizing also
/// resolves `..`/symlinks, so the stored override and the live root name the
/// same real directory.
pub fn canonical_dir(path: &str) -> Option<PathBuf> {
    // `dunce::canonicalize` mirrors `std::fs::canonicalize` but strips the
    // Windows `\\?\` verbatim prefix when the simplified form is unambiguous,
    // so the stored + displayed download path stays copy-pasteable (falling
    // back to the verbatim form only for paths that genuinely need it).
    let canon = dunce::canonicalize(path).ok()?;
    canon.is_dir().then_some(canon)
}

/// `port` values a setter may persist: `0` ("use the default") or a
/// non-privileged port. WHY exclude 1..=1023: binding those needs elevation
/// on most systems, so persisting one would silently kill listening on every
/// launch until the user rediscovers the setting.
pub fn valid_listen_port(port: u16) -> bool {
    port == 0 || port >= 1024
}

/// The port the listener actually binds for a stored `port` value: the `0`
/// sentinel — and any out-of-range value from a hand-edited blob — resolves
/// to [`DEFAULT_TCP_PORT`].
pub fn effective_listen_port(port: u16) -> u16 {
    if port != 0 && valid_listen_port(port) {
        port
    } else {
        DEFAULT_TCP_PORT
    }
}

/// Whether `combo` is a shortcut accelerator the quick-summon hotkey may bind
/// (M5.5 rebind). It must PARSE as a tauri global-shortcut accelerator AND carry
/// at least one modifier plus a key (`Alt+Space`, `Ctrl+Shift+K`): a bare key
/// (`K`) or a lone modifier is refused, since a modifier-less global chord would
/// swallow that key across the whole OS. Reuses the plugin's own parser — the
/// same one `on_shortcut`/`unregister` run — so a value this accepts is always
/// registerable, and the `set_hotkey` command re-checks with it before persisting.
#[cfg(desktop)]
pub fn valid_hotkey(combo: &str) -> bool {
    use std::str::FromStr;
    let s = combo.trim();
    !s.is_empty()
        && tauri_plugin_global_shortcut::Shortcut::from_str(s).is_ok_and(|sc| !sc.mods.is_empty())
}

/// Mobile builds pull in no global-shortcut plugin (the dependency is
/// desktop-only), so validate structurally here — a non-empty accelerator with a
/// `+` separating at least one modifier token from a key token. Present only so
/// the command compiles on every target; the hotkey never fires on mobile.
#[cfg(not(desktop))]
pub fn valid_hotkey(combo: &str) -> bool {
    combo
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .count()
        >= 2
}

/// Parse a stored blob. Missing fields are absorbed by the per-field serde
/// defaults above, so reaching the error arm means the blob itself is broken
/// (not JSON, wrong types, …) — only then do we fall back wholesale.
///
/// The error is RETURNED, not logged: `load` runs before the log plugin is
/// registered (the plugin's level comes from these very settings), so a
/// `log::warn!` here would be dropped. The caller logs it after logger init.
fn parse_or_default(v: serde_json::Value) -> (Settings, Option<String>) {
    match serde_json::from_value::<Settings>(v) {
        Ok(s) => (s, None),
        Err(e) => (Settings::default(), Some(e.to_string())),
    }
}

/// Read the settings blob straight from disk (NOT via tauri-plugin-store).
/// `Some` only when the file exists, parses as JSON, and carries the `KEY`
/// object — mirroring the plugin's old `store.get(KEY)`: a missing, unreadable,
/// unparseable, or keyless file yields `None`, so `load` routes it through
/// [`corruption_diag`] for the first-run-vs-corrupt distinction.
fn read_settings_blob(path: &std::path::Path) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    let doc: serde_json::Value = serde_json::from_str(&raw).ok()?;
    doc.get(KEY).cloned()
}

/// Load settings plus an optional corruption diagnostic (`Some` means the
/// stored blob was unreadable and defaults were used) for deferred logging.
pub fn load(app: &AppHandle) -> (Settings, Option<String>) {
    let file = store_file();
    // Resolve the blob's path ourselves (relative store paths live under the
    // app data dir) and read it directly — NOT through tauri-plugin-store. The
    // plugin persists non-atomically (truncate-then-write, plus a debounced
    // auto-save and an exit-time re-save of every REGISTERED store), which tears
    // settings.json on a crash. `save` now writes atomically; never touching the
    // plugin here keeps the store from re-registering, which would both
    // resurrect the exit-save tear window AND, at shutdown, overwrite an atomic
    // save with the plugin's stale in-memory copy.
    let path = Some(crate::paths::data_dir(app).join(&file));
    if let Some(v) = path.as_deref().and_then(read_settings_blob) {
        return parse_or_default(v);
    }
    // No blob came back. Either a genuine first run (no file yet) or FILE-level
    // corruption: a torn or hand-edited file would otherwise silently reset
    // every setting with no diagnostic — the M4.1 corruption contract must fire
    // for this class too. `corruption_diag` re-reads to tell first-run from a
    // broken file and carries the concrete parse error for the latter.
    let diag = path.as_deref().and_then(corruption_diag);
    (Settings::default(), diag)
}

/// Distinguish a true first run from a store file that exists on disk but
/// yielded no settings blob. `None` = no file = first run; `Some` carries
/// the deferred-log diagnostic (with the concrete parse error when the file
/// is malformed JSON — the torn-write / hand-edit case).
fn corruption_diag(path: &std::path::Path) -> Option<String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return Some(format!("store file unreadable ({}): {e}", path.display())),
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Err(e) => Some(format!(
            "store file is not valid JSON ({}): {e}",
            path.display()
        )),
        // Parseable but the plugin still produced no blob (keyless file, or
        // the store failed to open) — the app never writes that shape, so it
        // still means the user's settings were lost.
        Ok(_) => Some(format!(
            "store file exists but no settings blob was loaded ({})",
            path.display()
        )),
    }
}

pub fn save(app: &AppHandle, s: &Settings) {
    // Serialize concurrent saves within this process. tauri-plugin-store used to
    // provide this via its internal store mutex; writing the file ourselves, two
    // command setters racing would otherwise share one temp path and could
    // rename a half-written file. A poisoned lock (a writer panicked mid-save)
    // degrades to a no-op — skipping one save beats racing an unknown on-disk
    // state; the next setter re-persists the freshest snapshot anyway.
    static SAVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let Ok(_guard) = SAVE_LOCK.lock() else {
        return;
    };

    // Every degraded branch is logged (M4.6): a silently failed save means a
    // security-relevant setting (recv_policy!) quietly reverts next launch
    // while the user believes it changed. Logging is safe here — save only
    // runs from commands, after the log plugin is registered (the pre-logger
    // constraint applies to `load` alone).
    let path = crate::paths::data_dir(app).join(store_file());
    // Serialize the same `{"settings": {...}}` shape the plugin wrote, so `load`
    // (and any older build still on the plugin) reads it back unchanged.
    let json = match serde_json::to_value(s).map(|v| serde_json::json!({ KEY: v })) {
        Ok(doc) => match serde_json::to_string_pretty(&doc) {
            Ok(j) => j,
            Err(e) => {
                log::warn!("settings serialize failed: {e}");
                return;
            }
        },
        Err(e) => {
            log::warn!("settings serialize failed: {e}");
            return;
        }
    };

    // Atomic replace mirroring `TrustSnapshot::persist`: write a sibling temp,
    // fsync it, then rename over the target. A crash or power loss mid-write
    // leaves the OLD complete file or the NEW one, never the torn/zero-length
    // blob the plugin's truncate-then-write could leave behind — which `load`
    // would treat as wholesale corruption and reset every setting (recv_policy!).
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    let written = (|| -> std::io::Result<()> {
        use std::io::Write as _;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        // fsync BEFORE the rename: the rename must never publish a file whose
        // bytes are still only in the page cache.
        f.sync_all()
    })();
    if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, &path)) {
        log::warn!("settings save failed ({}): {e}", path.display());
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
// clippy: reassign is clearer than a many-field struct literal in test setup
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A blob written by an M1 build (only the 3 original fields) must parse,
    /// keep its stored values, and fill new fields with defaults — upgrading
    /// must never wipe the user's settings.
    /// The scale is clamped, and a NaN — which a hand-edited settings.json or a bad
    /// float can produce — falls back to 1.0. It must never reach `set_min_size`: a
    /// NaN window size resolves to no window at all.
    #[test]
    fn ui_zoom_is_clamped_and_nan_falls_back() {
        let (lo, hi) = UI_ZOOM_RANGE;
        assert_eq!(clamp_ui_zoom(1.0), 1.0);
        assert_eq!(clamp_ui_zoom(1.25), 1.25);
        assert_eq!(clamp_ui_zoom(0.1), lo, "below the floor");
        assert_eq!(clamp_ui_zoom(9.0), hi, "above the ceiling");
        assert_eq!(clamp_ui_zoom(f64::NAN), 1.0, "NaN is not a window size");
        assert_eq!(clamp_ui_zoom(f64::INFINITY), 1.0);
        assert_eq!(clamp_ui_zoom(f64::NEG_INFINITY), 1.0);
    }

    /// A fresh install is at the design size.
    #[test]
    fn ui_zoom_defaults_to_one() {
        assert_eq!(Settings::default().ui_zoom, 1.0);
    }

    #[test]
    fn old_blob_yields_defaults_for_new_fields() {
        let old = json!({
            "device_name": "My PC",
            "discoverable": false,
            "auto_open_folder": false
        });
        let s: Settings = serde_json::from_value(old).unwrap();
        assert_eq!(s.device_name, "My PC");
        assert!(!s.discoverable);
        assert!(!s.auto_open_folder);
        assert_eq!(s.log_level, "normal");
        assert_eq!(s.recv_policy, "trusted");
        assert_eq!(s.download_dir_override, None);
        assert_eq!(s.port, 0);
        assert!(s.tray_close);
        assert!(s.notif_system);
        assert!(!s.autostart);
        assert_eq!(s.iface_filter, None);
        assert!(!s.hotkey_enabled, "the hotkey is opt-in — default OFF");
        assert_eq!(
            s.hotkey, DEFAULT_HOTKEY,
            "hotkey combo defaults to the const for old blobs"
        );
        assert!(
            s.verify_hash,
            "integrity verification defaults ON for old blobs too"
        );
        assert_eq!(
            s.conflict, "ask",
            "conflict policy defaults to ask for old blobs"
        );
        assert_eq!(
            s.organize, "none",
            "auto-organize defaults to none for old blobs"
        );
        assert_eq!(
            s.max_concurrent, 3,
            "concurrency cap defaults to 3 for old blobs"
        );
        assert_eq!(
            s.rate_limit, "unlimited",
            "rate limit defaults to unlimited for old blobs"
        );
        assert!(
            !s.clip_share,
            "clipboard sharing defaults OFF for old blobs (opt-in)"
        );
        assert!(
            s.strip_exif,
            "EXIF stripping defaults ON for old blobs (privacy)"
        );
    }

    /// A blob written by a FUTURE build (fields we don't know yet) must still
    /// parse — serde_json ignores unknown fields, so downgrade is safe too.
    #[test]
    fn unknown_fields_are_ignored() {
        let future = json!({
            "device_name": "X",
            "discoverable": true,
            "auto_open_folder": true,
            "log_level": "verbose",
            "recv_policy": "ask",
            "some_future_field": 123,
            "nested_future": { "a": [1, 2, 3] }
        });
        let s: Settings = serde_json::from_value(future).unwrap();
        assert_eq!(s.device_name, "X");
        assert_eq!(s.log_level, "verbose");
        assert_eq!(s.recv_policy, "ask");
    }

    /// An empty object is the degenerate "all fields missing" case: every
    /// field must come back as its documented default.
    #[test]
    fn empty_object_yields_full_defaults() {
        let s: Settings = serde_json::from_value(json!({})).unwrap();
        let d = Settings::default();
        assert_eq!(s.device_name, d.device_name);
        assert!(s.discoverable);
        assert!(s.auto_open_folder);
        assert_eq!(s.log_level, "normal");
        assert_eq!(s.recv_policy, "trusted");
        assert_eq!(s.download_dir_override, None);
        assert_eq!(s.port, 0);
        assert!(s.tray_close);
        assert!(s.notif_system);
        assert!(!s.autostart);
        assert_eq!(s.iface_filter, None);
        assert!(!s.hotkey_enabled);
        assert_eq!(s.hotkey, DEFAULT_HOTKEY);
        assert!(s.verify_hash);
        assert_eq!(s.conflict, "ask");
        assert_eq!(s.organize, "none");
        assert_eq!(s.max_concurrent, 3);
        assert_eq!(s.rate_limit, "unlimited");
        assert!(!s.clip_share);
        assert!(s.strip_exif);
    }

    /// Hard corruption (wrong types) is the ONLY case that resets wholesale —
    /// and it must come with a diagnostic for the caller to log.
    #[test]
    fn corrupted_blob_falls_back_to_defaults() {
        let corrupted = json!({ "discoverable": "yes", "device_name": 42 });
        let (s, diag) = parse_or_default(corrupted);
        let d = Settings::default();
        assert!(diag.is_some(), "corruption must surface a diagnostic");
        assert_eq!(s.device_name, d.device_name);
        assert!(s.discoverable);
        assert!(s.auto_open_folder);
        assert_eq!(s.log_level, "normal");
        assert_eq!(s.recv_policy, "trusted");

        // A non-object blob is corruption too.
        let (s, diag) = parse_or_default(json!("not an object"));
        assert!(diag.is_some());
        assert_eq!(s.log_level, "normal");
    }

    /// First run (no store file) must NOT report corruption; an existing file
    /// that yielded no blob MUST — including the concrete parse error for
    /// malformed JSON (torn write / hand edit), and the zero-length file an
    /// interrupted truncate-then-write classically leaves behind.
    #[test]
    fn corruption_diag_distinguishes_first_run_from_corrupt_file() {
        let dir = std::env::temp_dir().join(format!("lanbeam-setdiag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        assert!(
            corruption_diag(&path).is_none(),
            "missing file = first run, no warning"
        );

        std::fs::write(&path, "{\"settings\": {tor").unwrap();
        let diag = corruption_diag(&path).expect("malformed file must be diagnosed");
        assert!(diag.contains("not valid JSON"), "got: {diag}");

        std::fs::write(&path, "").unwrap();
        assert!(
            corruption_diag(&path).is_some(),
            "zero-length file is corruption too"
        );

        // Parseable-but-keyless still warns: the app never writes that shape.
        std::fs::write(&path, "{}").unwrap();
        let diag = corruption_diag(&path).expect("keyless store file must be diagnosed");
        assert!(diag.contains("no settings blob"), "got: {diag}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Per-instance isolation: a `LANBEAM_INSTANCE` process must resolve its
    /// OWN settings file, never the primary's — a policy experiment in a test
    /// instance must not become the primary's policy on its next launch.
    #[test]
    fn store_file_is_isolated_per_instance() {
        assert_eq!(store_file_for(None), "settings.json");
        assert_eq!(store_file_for(Some("b")), "settings-b.json");
        // A blank id is "no instance" — same filter as lib.rs's instance_id.
        assert_eq!(store_file_for(Some("")), "settings.json");
    }

    /// Round-trip: what save() serializes, load() parses back unchanged.
    #[test]
    fn roundtrip_preserves_all_fields() {
        let mut orig = Settings::default();
        orig.device_name = "Round Trip".into();
        orig.discoverable = false;
        orig.log_level = "errors".into();
        orig.recv_policy = "all".into();
        orig.download_dir_override = Some("D:\\Drop".into());
        orig.port = 51999;
        orig.tray_close = false;
        orig.notif_system = false;
        orig.autostart = true;
        orig.iface_filter = Some("192.168.1.5".into());
        orig.hotkey_enabled = true;
        orig.hotkey = "Ctrl+Shift+K".into(); // non-default (default is "Alt+Space")
        orig.verify_hash = false; // non-default (default is ON) must survive the trip
        orig.conflict = "overwrite".into(); // non-default (default is "ask")
        orig.organize = "device".into(); // non-default (default is "none")
        orig.max_concurrent = 6; // non-default (default is 3)
        orig.rate_limit = "50".into(); // non-default (default is "unlimited")
        orig.clip_share = true; // non-default (default is OFF)
        orig.strip_exif = false; // non-default (default is ON)
        let v = serde_json::to_value(&orig).unwrap();
        let (back, diag) = parse_or_default(v);
        assert!(
            diag.is_none(),
            "a clean round-trip must not report corruption"
        );
        assert_eq!(back.device_name, "Round Trip");
        assert!(!back.discoverable);
        assert!(back.auto_open_folder);
        assert_eq!(back.log_level, "errors");
        assert_eq!(back.recv_policy, "all");
        assert_eq!(back.download_dir_override.as_deref(), Some("D:\\Drop"));
        assert_eq!(back.port, 51999);
        assert!(
            !back.tray_close,
            "a non-default tray_close must survive the trip"
        );
        assert!(
            !back.notif_system,
            "a non-default notif_system must survive the trip"
        );
        assert!(
            back.autostart,
            "a non-default autostart must survive the trip"
        );
        assert_eq!(back.iface_filter.as_deref(), Some("192.168.1.5"));
        assert!(
            back.hotkey_enabled,
            "a non-default hotkey_enabled must survive the trip"
        );
        assert_eq!(
            back.hotkey, "Ctrl+Shift+K",
            "a non-default hotkey combo must survive the trip"
        );
        assert!(
            !back.verify_hash,
            "a non-default verify_hash must survive the trip"
        );
        assert_eq!(
            back.conflict, "overwrite",
            "a non-default conflict must survive the trip"
        );
        assert_eq!(
            back.organize, "device",
            "a non-default organize must survive the trip"
        );
        assert_eq!(
            back.max_concurrent, 6,
            "a non-default max_concurrent must survive the trip"
        );
        assert_eq!(
            back.rate_limit, "50",
            "a non-default rate_limit must survive the trip"
        );
        assert!(
            back.clip_share,
            "a non-default clip_share must survive the trip"
        );
        assert!(
            !back.strip_exif,
            "a non-default strip_exif must survive the trip"
        );
    }

    /// The concurrency cap (M6.7) clamps into `MAX_CONCURRENT_RANGE` on both
    /// ends: `0` snaps up to the floor (never a deadlock), anything huge snaps
    /// down to the ceiling, and in-range values pass through.
    #[test]
    fn max_concurrent_clamps_into_range() {
        let (lo, hi) = MAX_CONCURRENT_RANGE;
        assert_eq!(clamp_max_concurrent(0), lo, "0 must snap up to the floor");
        assert_eq!(clamp_max_concurrent(1), 1);
        assert_eq!(clamp_max_concurrent(3), 3);
        assert_eq!(clamp_max_concurrent(hi), hi);
        assert_eq!(
            clamp_max_concurrent(999),
            hi,
            "over-range snaps to the ceiling"
        );
    }

    /// The rate-limit gate (M6.7): `"unlimited"` and positive MB/s pass; `0`,
    /// blanks and junk are rejected by the setter, and every rejected/blank
    /// value resolves to the unthrottled path so a bad value never stalls a
    /// transfer.
    #[test]
    fn rate_limit_validation_and_byte_conversion() {
        assert!(valid_rate_limit("unlimited"));
        assert!(valid_rate_limit(" 50 "), "trimmed numeric MB/s is valid");
        assert!(valid_rate_limit("10"));
        assert!(!valid_rate_limit("0"), "a 0 MB/s cap would wedge transfers");
        assert!(!valid_rate_limit(""), "blank is not a valid stored value");
        assert!(!valid_rate_limit("fast"), "junk is rejected");
        assert!(
            !valid_rate_limit(&(MAX_RATE_LIMIT_MB + 1).to_string()),
            "a value above the ceiling is rejected"
        );

        assert_eq!(rate_limit_bytes_per_sec("unlimited"), None);
        assert_eq!(rate_limit_bytes_per_sec("50"), Some(50 * 1024 * 1024));
        assert_eq!(rate_limit_bytes_per_sec(" 10 "), Some(10 * 1024 * 1024));
        // Defensive: a hand-edited nonsense value resolves to unlimited, not a stall.
        assert_eq!(rate_limit_bytes_per_sec("0"), None);
        assert_eq!(rate_limit_bytes_per_sec("garbage"), None);
        // Overflow guard (finding-3): mb = 2^44 would wrap `mb * 1024 * 1024` to
        // exactly 0 (a hard-stall cap) and panic in debug — instead it clamps to
        // a finite ceiling, never 0, never a panic.
        let two_pow_44 = (1u64 << 44).to_string();
        let clamped = rate_limit_bytes_per_sec(&two_pow_44);
        assert_eq!(clamped, Some(MAX_RATE_LIMIT_MB * 1024 * 1024));
        assert_ne!(
            clamped,
            Some(0),
            "an overflowing MB value must never cap at 0 B/s"
        );
    }

    /// The hotkey opt-in defaults OFF (never claim Alt+Space out of the box)
    /// and round-trips both states through the settings blob.
    #[test]
    fn hotkey_enabled_defaults_off_and_roundtrips() {
        assert!(!Settings::default().hotkey_enabled, "opt-in: default OFF");
        // A pre-M5.5 blob (field absent) loads as OFF, not ON.
        let (s, _) = parse_or_default(json!({ "device_name": "X" }));
        assert!(!s.hotkey_enabled, "a blob without the field defaults OFF");
        // An explicit true survives the round-trip.
        let mut on = Settings::default();
        on.hotkey_enabled = true;
        let (back, diag) = parse_or_default(serde_json::to_value(&on).unwrap());
        assert!(diag.is_none());
        assert!(back.hotkey_enabled);
    }

    /// The rebindable hotkey combo (M5.5): defaults to the const, a pre-rebind
    /// blob (field absent) still loads the default, and a custom chord survives
    /// the round-trip.
    #[test]
    fn hotkey_combo_defaults_and_roundtrips() {
        assert_eq!(
            Settings::default().hotkey,
            DEFAULT_HOTKEY,
            "the default combo is the const"
        );
        // A blob written before the rebind field existed loads the default.
        let (s, _) = parse_or_default(json!({ "device_name": "X" }));
        assert_eq!(
            s.hotkey, DEFAULT_HOTKEY,
            "a blob without the field defaults"
        );
        // A custom combo survives the round-trip.
        let mut custom = Settings::default();
        custom.hotkey = "Ctrl+Alt+L".into();
        let (back, diag) = parse_or_default(serde_json::to_value(&custom).unwrap());
        assert!(diag.is_none());
        assert_eq!(back.hotkey, "Ctrl+Alt+L");
    }

    /// The `set_hotkey` accelerator gate: a combo must parse AND carry at least
    /// one modifier plus a key. A bare key, a lone modifier, blanks and junk are
    /// all rejected so a modifier-less global chord can never persist. Desktop
    /// only — the mobile fallback has no plugin parser to exercise.
    #[cfg(desktop)]
    #[test]
    fn hotkey_accelerator_validation() {
        assert!(valid_hotkey(DEFAULT_HOTKEY), "the default combo is valid");
        assert!(valid_hotkey("Alt+Space"));
        assert!(valid_hotkey("Ctrl+Shift+K"));
        assert!(valid_hotkey(" Ctrl+Alt+Delete "), "trimmed input is valid");
        assert!(valid_hotkey("Super+F5"));
        assert!(valid_hotkey("Ctrl+ArrowUp"), "code-style keys parse");

        assert!(!valid_hotkey(""), "blank is rejected");
        assert!(!valid_hotkey("   "), "whitespace is rejected");
        assert!(!valid_hotkey("K"), "a bare key has no modifier");
        assert!(!valid_hotkey("Space"), "a bare key has no modifier");
        assert!(
            !valid_hotkey("Ctrl"),
            "a lone modifier has no key and does not parse"
        );
        assert!(!valid_hotkey("Ctrl+"), "a trailing empty token is rejected");
        assert!(!valid_hotkey("Ctrl+NotAKey"), "an unknown key is rejected");
    }

    /// The `set_iface_filter` input gate: empty clears, a valid IPv4 literal
    /// is canonicalized, junk is rejected (outer None → the setter ignores it).
    #[test]
    fn parse_iface_filter_clears_canonicalizes_and_rejects() {
        assert_eq!(
            parse_iface_filter(""),
            Some(None),
            "empty clears the filter"
        );
        assert_eq!(
            parse_iface_filter("   "),
            Some(None),
            "whitespace clears too"
        );
        assert_eq!(
            parse_iface_filter(" 192.168.1.5 "),
            Some(Some("192.168.1.5".into())),
            "valid input is trimmed + stored canonically"
        );
        assert_eq!(parse_iface_filter("not-an-ip"), None, "junk is ignored");
        assert_eq!(
            parse_iface_filter("fe80::1"),
            None,
            "IPv6 is not a discovery interface"
        );
        assert_eq!(
            parse_iface_filter("300.1.1.1"),
            None,
            "out-of-range octet rejected"
        );
    }

    /// The shared download-dir gate (M5.2): only an EXISTING directory passes
    /// — a plain file or a dead path must be rejected, both by the setter and
    /// by the startup override resolution.
    #[test]
    fn canonical_dir_accepts_only_existing_directories() {
        let dir = std::env::temp_dir().join(format!("lanbeam-canon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("plain.txt");
        std::fs::write(&file, "x").unwrap();

        let got = canonical_dir(dir.to_str().unwrap()).expect("an existing dir must pass");
        assert!(got.is_dir());
        assert!(got.is_absolute(), "the stored override must be absolute");

        assert!(
            canonical_dir(file.to_str().unwrap()).is_none(),
            "a FILE must be rejected"
        );
        assert!(
            canonical_dir(dir.join("nope").to_str().unwrap()).is_none(),
            "a nonexistent path must be rejected"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Canonicalization must not emit a Windows verbatim (`\\?\`) prefix, so
    /// the stored/displayed download path stays copy-pasteable and shell-opener
    /// flows never receive a verbatim path (M5.2). No-op assertion off Windows,
    /// where canonical paths never carry the prefix.
    #[test]
    fn canonical_dir_strips_verbatim_prefix() {
        let dir = std::env::temp_dir().join(format!("lanbeam-dunce-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let got = canonical_dir(dir.to_str().unwrap()).expect("an existing dir must pass");
        let shown = got.to_string_lossy();
        assert!(
            !shown.starts_with(r"\\?\"),
            "the verbatim prefix must be stripped: {shown}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Port validation (setter gate) and startup resolution: `0` and the
    /// privileged range resolve to the default; a pinned in-range port wins.
    #[test]
    fn listen_port_validation_and_resolution() {
        assert!(valid_listen_port(0), "0 = use the default");
        assert!(!valid_listen_port(1));
        assert!(!valid_listen_port(1023), "privileged ports are refused");
        assert!(valid_listen_port(1024));
        assert!(valid_listen_port(65535));

        assert_eq!(effective_listen_port(0), DEFAULT_TCP_PORT);
        assert_eq!(effective_listen_port(51999), 51999);
        // A hand-edited blob with a privileged port must not brick listening.
        assert_eq!(effective_listen_port(80), DEFAULT_TCP_PORT);
        // The privileged/default boundary: 1023 falls back, 1024 pins.
        assert_eq!(
            effective_listen_port(1023),
            DEFAULT_TCP_PORT,
            "a privileged port resolves to the default"
        );
        assert_eq!(
            effective_listen_port(1024),
            1024,
            "the lowest non-privileged port pins as-is"
        );
        assert_eq!(effective_listen_port(65535), 65535);
    }

    /// `read_settings_blob` mirrors the plugin's old `store.get(KEY)`: `Some`
    /// only for an existing file that parses as JSON AND carries the `settings`
    /// object; a missing, non-JSON, or keyless file yields `None` so `load`
    /// routes it through the corruption diagnostic.
    #[test]
    fn read_settings_blob_requires_keyed_json() {
        let dir = std::env::temp_dir().join(format!("lanbeam-blob-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        assert!(
            read_settings_blob(&path).is_none(),
            "a missing file yields None"
        );

        std::fs::write(&path, "{not json").unwrap();
        assert!(
            read_settings_blob(&path).is_none(),
            "unparseable JSON yields None"
        );

        std::fs::write(&path, "{\"other\": 1}").unwrap();
        assert!(
            read_settings_blob(&path).is_none(),
            "a keyless file yields None"
        );

        std::fs::write(&path, "{\"settings\": {\"device_name\": \"Blob PC\"}}").unwrap();
        let blob = read_settings_blob(&path).expect("a keyed blob must be returned");
        assert_eq!(
            blob.get("device_name").and_then(|v| v.as_str()),
            Some("Blob PC")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `store_file` re-reads `LANBEAM_INSTANCE` with the same emptiness filter
    /// as `store_file_for`: unset/blank resolves to the primary's file, a
    /// non-empty id gets its own isolated file.
    #[test]
    fn store_file_follows_instance_env() {
        let prev = std::env::var("LANBEAM_INSTANCE").ok();

        std::env::remove_var("LANBEAM_INSTANCE");
        assert_eq!(store_file(), "settings.json", "unset = primary file");

        std::env::set_var("LANBEAM_INSTANCE", "");
        assert_eq!(store_file(), "settings.json", "blank = primary file");

        std::env::set_var("LANBEAM_INSTANCE", "z");
        assert_eq!(store_file(), "settings-z.json", "an id gets its own file");

        match prev {
            Some(v) => std::env::set_var("LANBEAM_INSTANCE", v),
            None => std::env::remove_var("LANBEAM_INSTANCE"),
        }
    }
}
