//! Persistent peer trust (M4.4): Device ID → [`TrustedPeer`], backing the
//! receive policy's auto-accept decision and the UI's trusted-devices list.
//!
//! WHY its own JSON file (`trusted.json`) instead of a key inside the settings
//! blob: trust entries mutate on a different cadence (every accepted transfer
//! bumps `last_seen`), and a corrupted trust file must never take the user's
//! settings down with it — or vice versa.
//!
//! Robustness contract mirrors `settings.rs`, one level deeper: unknown JSON
//! fields are ignored and missing fields default per-field (so old and future
//! builds keep exchanging the file), an unreadable ENTRY is skipped with a
//! warning while the rest of the store survives, and only a wholly corrupted
//! file falls back to an empty store — logged, never silent. Writes are
//! atomic (temp file + fsync + rename), so a crash mid-save can tear at most
//! the abandoned temp file, never the store itself.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};

/// Same clamp as `set_device_name`: names come from the UI *or from a peer's
/// self-declared `DeviceInfo`*, and a hostile peer must not grow the persisted
/// file (or the UI list) with an unbounded string.
const MAX_NAME_CHARS: usize = 63;

/// One trusted peer, keyed by its Device ID in [`TrustStore`].
///
/// Every field carries `#[serde(default)]` so an entry written by an older
/// build (fewer fields) still parses — the settings-schema lesson of M4.1
/// applied from day one instead of retrofitted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrustedPeer {
    /// Friendly display name; refreshed from the peer's `DeviceInfo` on every
    /// accepted transfer so renames propagate without re-pairing.
    #[serde(default)]
    pub name: String,
    /// Whether inbound transfers from this peer skip the accept prompt when
    /// the receive policy is `"trusted"`.
    #[serde(default)]
    pub auto_accept: bool,
    /// Unix seconds when the user first trusted this peer.
    #[serde(default)]
    pub paired_at: u64,
    /// Unix seconds of the last accepted transfer from this peer.
    #[serde(default)]
    pub last_seen: u64,
}

/// What the UI sees for each trusted peer (camelCase over the bridge), both
/// as `list_trusted`'s return value and as the `trust_updated` event payload.
#[derive(Clone, Serialize)]
pub struct TrustedPeerDto {
    #[serde(rename = "deviceId")]
    pub device_id: String,
    pub name: String,
    #[serde(rename = "autoAccept")]
    pub auto_accept: bool,
    #[serde(rename = "pairedAt")]
    pub paired_at: u64,
    #[serde(rename = "lastSeen")]
    pub last_seen: u64,
}

/// The persistent trust table. Lives behind `Arc<RwLock<…>>` on `AppState`
/// (and `TransportCtx`); every mutation is followed by a persist at the call
/// site — [`TrustStore::snapshot`] under the write guard, then
/// [`TrustSnapshot::persist`] after it drops — so the file never lags the
/// in-memory truth and the lock never waits on a disk.
pub struct TrustStore {
    /// Where the store persists — captured at load so save sites don't need
    /// the app handle (the receive path only has a `TransportCtx`).
    path: PathBuf,
    peers: HashMap<String, TrustedPeer>,
    /// Bumped on every mutation and stamped into snapshots, so an image
    /// persisted out of order can never roll the file back — see
    /// [`TrustSnapshot::persist`].
    seq: u64,
    /// Write-out gate shared by this store's snapshots: serializes the file
    /// I/O and remembers the newest sequence already on disk.
    gate: Arc<Mutex<u64>>,
}

/// A point-in-time serialized image of a [`TrustStore`], taken under the
/// store's write guard (cheap, memory-only) and persisted OUTSIDE it — file
/// I/O under the trust lock would stall every concurrent policy check and
/// freeze the UI's sync trust commands behind the writer (M4.6).
pub struct TrustSnapshot {
    /// The store's mutation sequence when this image was taken.
    seq: u64,
    json: String,
    path: PathBuf,
    gate: Arc<Mutex<u64>>,
}

impl TrustSnapshot {
    /// Atomically replace the store file with this image: write a sibling
    /// temp file, fsync it, then rename over the target — a crash or power
    /// loss mid-write leaves either the OLD complete file or the NEW one,
    /// never the torn blob that `load` would treat as wholesale corruption
    /// and silently discard every pairing decision (M4.6). Stale images are
    /// skipped: the gate remembers the newest sequence written, so writers
    /// racing outside the store lock cannot roll the file back. Best-effort
    /// like before — every failure is logged, and a leftover temp file is
    /// cleaned up.
    pub fn persist(self) {
        // Poisoned gate: another writer panicked mid-persist — skipping beats
        // racing an unknown on-disk state.
        let Ok(mut last_written) = self.gate.lock() else {
            return;
        };
        if self.seq < *last_written {
            return; // a newer image already reached the disk
        }
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = self.path.with_extension("json.tmp");
        let written = (|| -> std::io::Result<()> {
            use std::io::Write as _;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(self.json.as_bytes())?;
            // fsync BEFORE the rename: the rename must never publish a file
            // whose bytes are still only in the page cache.
            f.sync_all()
        })();
        match written.and_then(|()| std::fs::rename(&tmp, &self.path)) {
            Ok(()) => *last_written = self.seq,
            Err(e) => {
                log::warn!("trust store save failed ({}): {e}", self.path.display());
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
}

/// Persist a snapshot on the blocking pool — for call sites running on an
/// async worker (or the main thread), where even a post-guard `File::sync_all`
/// on a slow disk would stall the caller. Ordering across tasks is the
/// snapshot gate's job, not the pool's.
pub fn persist_async(snapshot: Option<TrustSnapshot>) {
    if let Some(snap) = snapshot {
        tauri::async_runtime::spawn_blocking(move || snap.persist());
    }
}

impl TrustStore {
    /// Load the store from `path`. Missing file = first run = empty store.
    /// A broken entry is skipped (the rest survives); a broken FILE yields an
    /// empty store — both logged, so the caller must run after logger init.
    pub fn load(path: PathBuf) -> TrustStore {
        let mut store = TrustStore {
            path,
            peers: HashMap::new(),
            seq: 0,
            gate: Arc::new(Mutex::new(0)),
        };
        let raw = match std::fs::read_to_string(&store.path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return store,
            Err(e) => {
                log::warn!(
                    "trust store unreadable, starting empty ({}): {e}",
                    store.path.display()
                );
                return store;
            }
        };
        let map = match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(serde_json::Value::Object(map)) => map,
            Ok(_) => {
                log::warn!(
                    "trust store is not a JSON object, starting empty ({})",
                    store.path.display()
                );
                return store;
            }
            Err(e) => {
                log::warn!(
                    "trust store corrupted, starting empty ({}): {e}",
                    store.path.display()
                );
                return store;
            }
        };
        for (device_id, entry) in map {
            // Per-entry tolerance: one bad record must not cost the user every
            // OTHER pairing decision they ever made.
            match serde_json::from_value::<TrustedPeer>(entry) {
                Ok(peer) => {
                    store.peers.insert(device_id, peer);
                }
                Err(e) => log::warn!("trust entry {device_id} unreadable, skipped: {e}"),
            }
        }
        store
    }

    /// Serialize the current table into a persistable image. Called UNDER the
    /// caller's write guard; the returned snapshot's [`TrustSnapshot::persist`]
    /// does the actual file I/O and must run AFTER the guard is dropped.
    /// `None` means serialization failed — logged here, nothing to persist.
    pub fn snapshot(&self) -> Option<TrustSnapshot> {
        match serde_json::to_string_pretty(&self.peers) {
            Ok(json) => Some(TrustSnapshot {
                seq: self.seq,
                json,
                path: self.path.clone(),
                gate: self.gate.clone(),
            }),
            Err(e) => {
                log::warn!("trust store serialize failed: {e}");
                None
            }
        }
    }

    /// Persist the current table synchronously (snapshot + write in one call).
    /// Best-effort — failing a live transfer (or a UI toggle) over bookkeeping
    /// I/O would be worse than a stale file — but every failure is logged.
    /// The concurrent paths snapshot under the guard and persist outside it
    /// instead; this stays for tests and simple single-threaded callers.
    pub fn save(&self) {
        if let Some(snap) = self.snapshot() {
            snap.persist();
        }
    }

    pub fn get(&self, device_id: &str) -> Option<&TrustedPeer> {
        self.peers.get(device_id)
    }

    /// Snapshot for the UI, sorted like the discovery list (case-insensitive
    /// name, Device ID as tie-break so the order is fully deterministic).
    pub fn list(&self) -> Vec<TrustedPeerDto> {
        let mut list: Vec<TrustedPeerDto> = self
            .peers
            .iter()
            .map(|(id, p)| TrustedPeerDto {
                device_id: id.clone(),
                name: p.name.clone(),
                auto_accept: p.auto_accept,
                paired_at: p.paired_at,
                last_seen: p.last_seen,
            })
            .collect();
        list.sort_by(|a, b| {
            (a.name.to_lowercase(), &a.device_id).cmp(&(b.name.to_lowercase(), &b.device_id))
        });
        list
    }

    /// Add or update a trusted peer. A NEW entry stamps `paired_at`/`last_seen`
    /// with now — or with the caller-supplied unix seconds (clamped to now, so
    /// nothing on disk ever claims to be from the future), which is how the
    /// one-time localStorage migration carries the ORIGINAL pairing dates over
    /// instead of the upgrade date. An UPDATE deliberately keeps both stamps
    /// and ignores the parameters — editing a name or toggling auto-accept in
    /// the UI is not "seeing the peer on the network".
    pub fn set(
        &mut self,
        device_id: String,
        name: String,
        auto_accept: bool,
        paired_at: Option<u64>,
        last_seen: Option<u64>,
    ) {
        let name = clamp_name(&name);
        let now = now_unix();
        self.seq += 1;
        self.peers
            .entry(device_id)
            .and_modify(|p| {
                p.name = name.clone();
                p.auto_accept = auto_accept;
            })
            .or_insert(TrustedPeer {
                name,
                auto_accept,
                paired_at: paired_at.map_or(now, |t| t.min(now)),
                last_seen: last_seen.map_or(now, |t| t.min(now)),
            });
    }

    /// Remove EVERY entry (M5.7 identity reset); returns whether anything was
    /// deleted, same skip-the-save contract as [`TrustStore::remove`]. WHY the
    /// reset wipes trust wholesale: a reset means "make this device a stranger
    /// again, in both directions" — peers must re-verify us because our Device
    /// ID changed, and every auto-accept grant we handed out belonged to the
    /// old identity's pairing decisions, so none may outlive it.
    pub fn clear(&mut self) -> bool {
        if self.peers.is_empty() {
            return false;
        }
        self.seq += 1;
        self.peers.clear();
        true
    }

    /// Remove a peer; returns whether anything was actually deleted (callers
    /// skip the save + event when nothing changed).
    pub fn remove(&mut self, device_id: &str) -> bool {
        let removed = self.peers.remove(device_id).is_some();
        if removed {
            self.seq += 1;
        }
        removed
    }

    /// Refresh a KNOWN peer's liveness: bump `last_seen`, adopt a fresher
    /// `DeviceInfo` name (empty/whitespace names are ignored — a peer that
    /// introduces itself namelessly must not blank the stored one). Returns
    /// whether the peer exists; unknown peers are left alone, because merely
    /// receiving files must never create trust — that stays an explicit user
    /// action via `set_trusted`.
    pub fn touch_seen(&mut self, device_id: &str, name: Option<&str>) -> bool {
        let Some(p) = self.peers.get_mut(device_id) else {
            return false;
        };
        self.seq += 1;
        p.last_seen = now_unix();
        if let Some(n) = name {
            let n = clamp_name(n);
            if !n.is_empty() {
                p.name = n;
            }
        }
        true
    }
}

/// The receive-policy decision, pure so the whole matrix is unit-testable:
/// `"all"` auto-accepts anyone, `"trusted"` only a stored peer with
/// `auto_accept`, `"ask"` — and ANY unknown future value — always prompts.
/// Failing toward the prompt is the safe direction: the worst outcome is one
/// unnecessary question, never an unwanted file on disk.
pub fn should_auto_accept(policy: &str, entry: Option<&TrustedPeer>) -> bool {
    match policy {
        "all" => true,
        "trusted" => entry.is_some_and(|p| p.auto_accept),
        _ => false, // "ask" + forward-compat fallback
    }
}

/// Emit `trust_updated` with the full list — the one event shape the UI needs
/// to keep its trusted-devices view current, fired after every mutation.
/// Takes a pre-built list (not the store) so callers snapshot it under the
/// write guard and emit only after the guard is dropped — no IPC while
/// holding the trust lock (M4.6).
pub fn emit_updated<R: Runtime>(app: &AppHandle<R>, list: Vec<TrustedPeerDto>) {
    let _ = app.emit("trust_updated", list);
}

// NOTE: there is deliberately no `grant()` helper here any more.
//
// It existed so the pairing handshake could trust a peer on its own, and that is
// exactly the thing that must not be easy: a handshake proves a code was right,
// not that the device on the far end is the one you meant. Trust is now recorded
// through ONE door — the `set_trusted` command — which the UI opens only after a
// human has compared the SAS on both screens. Re-adding a backend-side grant
// would quietly restore a second, unconfirmed path into the trust store.

/// Trim + clamp a display name to [`MAX_NAME_CHARS`]. `pub(crate)` so the
/// transfer trust boundary can bound a peer's self-declared `DeviceInfo` name
/// the moment it enters the process, keeping `MAX_NAME_CHARS` the one source
/// of truth for every name-length limit.
pub(crate) fn clamp_name(name: &str) -> String {
    name.trim().chars().take(MAX_NAME_CHARS).collect()
}

/// Cap on any free-form peer-authored string that reaches a log line or a UI
/// toast — an ack `error`, a decline `reason`, or a `Debug`-rendered peer
/// message. Generous next to a name (these are short human sentences) but far
/// below the ~64 KB a single control frame can carry.
const MAX_PEER_TEXT_CHARS: usize = 256;

/// Clamp an arbitrary peer-authored string to [`MAX_PEER_TEXT_CHARS`] before it
/// is embedded in an error, event payload, or log. The codebase already clamps
/// every peer NAME here; these free-form fields were the gap — a hostile peer
/// could push a multi-kilobyte `error`/`reason` straight into a toast. Char-
/// truncates like [`clamp_name`] so a multi-byte boundary is never split.
pub(crate) fn clamp_peer_text(s: &str) -> String {
    s.chars().take(MAX_PEER_TEXT_CHARS).collect()
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use serde_json::json;

    fn tmp_file(tag: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("lanbeam-trust-{tag}-{}", std::process::id()))
            .join("trusted.json")
    }

    fn cleanup(path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    fn peer(auto_accept: bool) -> TrustedPeer {
        TrustedPeer {
            name: "P".into(),
            auto_accept,
            paired_at: 1,
            last_seen: 1,
        }
    }

    /// The full policy matrix: {ask, trusted, all, unknown} × {trusted with
    /// auto-accept, trusted WITHOUT auto-accept, untrusted}. Only two cells
    /// may skip the prompt.
    #[test]
    fn policy_matrix() {
        let auto = peer(true);
        let manual = peer(false);

        // "ask" prompts no matter what the store says.
        assert!(!should_auto_accept("ask", Some(&auto)));
        assert!(!should_auto_accept("ask", Some(&manual)));
        assert!(!should_auto_accept("ask", None));

        // "trusted" skips the prompt ONLY for a stored auto-accept peer.
        assert!(should_auto_accept("trusted", Some(&auto)));
        assert!(!should_auto_accept("trusted", Some(&manual)));
        assert!(!should_auto_accept("trusted", None));

        // "all" is the open door, store or no store.
        assert!(should_auto_accept("all", Some(&auto)));
        assert!(should_auto_accept("all", Some(&manual)));
        assert!(should_auto_accept("all", None));

        // A policy value from a FUTURE build must fail toward prompting.
        assert!(!should_auto_accept("holograms_only", Some(&auto)));
    }

    /// What save() writes, load() reads back unchanged — including timestamps.
    #[test]
    fn roundtrip_preserves_entries() {
        let path = tmp_file("roundtrip");
        cleanup(&path);
        let mut store = TrustStore::load(path.clone());
        assert!(
            store.list().is_empty(),
            "missing file must mean empty store"
        );
        store.set("id-a".into(), "Alice's Laptop".into(), true, None, None);
        store.set("id-b".into(), "bob".into(), false, None, None);
        store.save();

        let back = TrustStore::load(path.clone());
        assert_eq!(back.list().len(), 2);
        let a = back.get("id-a").expect("id-a survives the round-trip");
        assert_eq!(a.name, "Alice's Laptop");
        assert!(a.auto_accept);
        assert!(a.paired_at > 0, "insert must stamp paired_at");
        assert_eq!(
            back.get("id-a"),
            store.get("id-a"),
            "timestamps survive too"
        );
        assert!(!back.get("id-b").unwrap().auto_accept);
        // list order: case-insensitive by name → Alice before bob.
        assert_eq!(back.list()[0].device_id, "id-a");
        cleanup(&path);
    }

    /// Per-entry tolerance: one corrupted entry is skipped, every other entry
    /// (including one missing newer fields) still loads.
    #[test]
    fn corrupted_entry_is_skipped_others_survive() {
        let path = tmp_file("tolerant");
        cleanup(&path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let blob = json!({
            "good-id": { "name": "Good", "auto_accept": true, "paired_at": 5, "last_seen": 9 },
            "bad-id": 42,                       // not even an object
            "worse-id": { "name": 17 },         // wrong type inside
            "old-build-id": { "name": "Old" }   // missing fields → per-field defaults
        });
        std::fs::write(&path, serde_json::to_string(&blob).unwrap()).unwrap();

        let store = TrustStore::load(path.clone());
        assert_eq!(
            store.list().len(),
            2,
            "exactly the parseable entries survive"
        );
        let good = store.get("good-id").expect("intact entry survives");
        assert!(good.auto_accept);
        assert_eq!(good.last_seen, 9);
        let old = store
            .get("old-build-id")
            .expect("old-schema entry survives");
        assert!(!old.auto_accept, "missing fields take their defaults");
        assert!(store.get("bad-id").is_none());
        assert!(store.get("worse-id").is_none());
        cleanup(&path);
    }

    /// A wholly corrupted file (or a non-object) yields an EMPTY store — and
    /// the store must stay usable: the next save writes a clean file again.
    #[test]
    fn corrupted_file_falls_back_to_empty_and_recovers() {
        let path = tmp_file("corrupt");
        cleanup(&path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json {{{").unwrap();
        let mut store = TrustStore::load(path.clone());
        assert!(store.list().is_empty());

        // Non-object JSON is corruption too.
        std::fs::write(&path, "[1, 2, 3]").unwrap();
        assert!(TrustStore::load(path.clone()).list().is_empty());

        // Recovery: mutate + save over the wreck, then load cleanly.
        store.set("id-new".into(), "Phoenix".into(), true, None, None);
        store.save();
        let back = TrustStore::load(path.clone());
        assert_eq!(back.get("id-new").unwrap().name, "Phoenix");
        cleanup(&path);
    }

    /// `set` on an existing entry updates name/auto_accept but must NOT move
    /// `paired_at`/`last_seen` — a UI edit is not a network sighting.
    #[test]
    fn set_update_keeps_timestamps() {
        let path = tmp_file("set-update");
        cleanup(&path);
        let mut store = TrustStore::load(path.clone());
        store.set("id".into(), "First".into(), false, None, None);
        let (paired, seen) = {
            let p = store.get("id").unwrap();
            (p.paired_at, p.last_seen)
        };
        store.set("id".into(), "Renamed".into(), true, Some(1), Some(1));
        let p = store.get("id").unwrap();
        assert_eq!(p.name, "Renamed");
        assert!(p.auto_accept);
        assert_eq!(p.paired_at, paired);
        assert_eq!(p.last_seen, seen);
        cleanup(&path);
    }

    /// `touch_seen` bumps only KNOWN peers, adopts non-empty names, and never
    /// creates an entry — receiving files must not manufacture trust.
    #[test]
    fn touch_seen_updates_known_ignores_unknown() {
        let path = tmp_file("touch");
        cleanup(&path);
        let mut store = TrustStore::load(path.clone());
        store.set("known".into(), "Old Name".into(), true, None, None);

        assert!(store.touch_seen("known", Some("New Name")));
        assert_eq!(store.get("known").unwrap().name, "New Name");
        assert!(store.get("known").unwrap().last_seen > 0);

        // Whitespace-only DeviceInfo name must not blank the stored one.
        assert!(store.touch_seen("known", Some("   ")));
        assert_eq!(store.get("known").unwrap().name, "New Name");

        assert!(!store.touch_seen("stranger", Some("Intruder")));
        assert!(
            store.get("stranger").is_none(),
            "touch must never create trust"
        );
        cleanup(&path);
    }

    /// Names are clamped like `set_device_name` (63 chars) on both write paths,
    /// so a hostile peer's DeviceInfo cannot bloat the persisted file.
    #[test]
    fn names_are_clamped_on_set_and_touch() {
        let path = tmp_file("clamp");
        cleanup(&path);
        let long = "x".repeat(200);
        let mut store = TrustStore::load(path.clone());
        store.set("id".into(), long.clone(), true, None, None);
        assert_eq!(store.get("id").unwrap().name.chars().count(), 63);
        store.touch_seen("id", Some(&long));
        assert_eq!(store.get("id").unwrap().name.chars().count(), 63);
        cleanup(&path);
    }

    /// M5.7 reset: `clear` empties the store AND the file, and its seq bump
    /// means a pre-clear snapshot persisted late (a racing writer) can never
    /// resurrect the wiped entries on disk.
    #[test]
    fn clear_empties_store_and_outruns_stale_snapshots() {
        let path = tmp_file("clear");
        cleanup(&path);
        let mut store = TrustStore::load(path.clone());
        assert!(!store.clear(), "clearing an empty store is a no-op");

        store.set("id-a".into(), "A".into(), true, None, None);
        store.set("id-b".into(), "B".into(), false, None, None);
        let stale = store.snapshot().expect("serializable");
        assert!(store.clear());
        store.save();
        stale.persist(); // the pre-clear image must be skipped, not written

        let back = TrustStore::load(path.clone());
        assert!(
            back.list().is_empty(),
            "reset must leave an empty trust store on disk"
        );
        cleanup(&path);
    }

    #[test]
    fn remove_reports_whether_it_deleted() {
        let path = tmp_file("remove");
        cleanup(&path);
        let mut store = TrustStore::load(path.clone());
        store.set("id".into(), "Gone Soon".into(), false, None, None);
        assert!(store.remove("id"));
        assert!(store.get("id").is_none());
        assert!(!store.remove("id"), "second remove has nothing to delete");
        cleanup(&path);
    }

    /// Atomic save: the bytes go through a sibling temp file + rename, so the
    /// temp never survives a successful save and the target is always one
    /// complete JSON image — even when saving over a torn wreck with a stale
    /// temp file left beside it.
    #[test]
    fn save_replaces_atomically_and_cleans_up_temp() {
        let path = tmp_file("atomic");
        cleanup(&path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Simulate the aftermath of an interrupted earlier save.
        std::fs::write(&path, "torn {{{").unwrap();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, "stale temp").unwrap();

        let mut store = TrustStore::load(path.clone());
        store.set("id".into(), "Atomic".into(), true, None, None);
        store.save();

        assert!(
            !tmp.exists(),
            "temp must be renamed over the target, not left behind"
        );
        let back = TrustStore::load(path.clone());
        assert_eq!(back.get("id").unwrap().name, "Atomic");
        cleanup(&path);
    }

    /// The snapshot gate: an OLDER image persisted after a NEWER one (writers
    /// race outside the store lock) must be skipped, never roll the file back.
    #[test]
    fn stale_snapshot_cannot_overwrite_newer_one() {
        let path = tmp_file("stale-seq");
        cleanup(&path);
        let mut store = TrustStore::load(path.clone());
        store.set("first".into(), "First".into(), false, None, None);
        let old_snap = store.snapshot().expect("serializable");
        store.set("second".into(), "Second".into(), false, None, None);
        let new_snap = store.snapshot().expect("serializable");

        new_snap.persist(); // the newer image lands first…
        old_snap.persist(); // …so the stale one must be a no-op

        let back = TrustStore::load(path.clone());
        assert_eq!(back.list().len(), 2, "stale image must not shrink the file");
        assert!(back.get("second").is_some(), "newest state must survive");
        cleanup(&path);
    }

    /// Caller-supplied timestamps (the localStorage migration) are honored on
    /// INSERT only, clamped to now — an update never moves either stamp, and
    /// a future-dated stamp cannot land on disk.
    #[test]
    fn set_timestamps_insert_only_and_clamped() {
        let path = tmp_file("stamps");
        cleanup(&path);
        let mut store = TrustStore::load(path.clone());

        store.set(
            "migrated".into(),
            "Old Friend".into(),
            true,
            Some(1_000),
            Some(2_000),
        );
        let p = store.get("migrated").unwrap();
        assert_eq!(
            p.paired_at, 1_000,
            "migration keeps the original pairing date"
        );
        assert_eq!(p.last_seen, 2_000);

        // A liar's future timestamp is clamped to now.
        store.set(
            "liar".into(),
            "Time Traveler".into(),
            false,
            Some(u64::MAX),
            Some(u64::MAX),
        );
        let now = now_unix();
        let p = store.get("liar").unwrap();
        assert!(
            p.paired_at <= now && p.last_seen <= now,
            "stamps must never be in the future"
        );

        // Updates ignore the parameters entirely.
        store.set("migrated".into(), "Renamed".into(), false, Some(7), Some(7));
        let p = store.get("migrated").unwrap();
        assert_eq!(p.paired_at, 1_000, "update must not move paired_at");
        assert_eq!(p.last_seen, 2_000, "update must not move last_seen");
        cleanup(&path);
    }
}
