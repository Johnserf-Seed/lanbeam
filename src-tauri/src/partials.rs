//! Persisted resume state for interrupted receives (M6.4): device_id → the
//! files a still-incomplete transfer left half-written on disk, so a later
//! transfer of the same file can continue from `bytes_written` instead of
//! starting over.
//!
//! WHY its own JSON file (`partials.json`) instead of a key inside settings or
//! trust: partials churn on a completely different cadence (every interrupted
//! or resumed receive) and are pure disposable bookkeeping — losing the file
//! costs at most a re-download, so a corrupted `partials.json` must never take
//! settings or pairing decisions down with it, and vice versa.
//!
//! A file's IDENTITY is its sanitized manifest-relative path + declared size +
//! optional content hash ([`FileIdentity`]) — the same tuple a NEW manifest
//! produces, which is how the receive path matches an incoming file to a stored
//! partial. The record additionally remembers `disk_rel` (where the bytes
//! actually live, after any auto-organize prefix / de-dupe suffix applied at
//! first write) so a resume reopens the SAME file rather than recomputing
//! today's organized path.
//!
//! Robustness + persistence mirror `trust.rs`: unknown JSON fields are ignored,
//! missing fields default per-field, a wholly corrupted file falls back to an
//! empty store (logged, never silent), and writes are atomic (temp + fsync +
//! rename) with a monotonic sequence gate so an image persisted out of order
//! can never roll the file back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// The stable identity of a file across transfers: the sanitized manifest
/// relative path (forward-slash), the declared byte size, and the optional
/// SHA-256. Resume only ever matches a file whose hash is present (see the
/// receive path), so in practice `sha256` is `Some` for every stored record —
/// the field is compared so a changed file (new hash) never resumes onto stale
/// bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    pub rel: String,
    pub size: u64,
    pub sha256: Option<String>,
}

impl FileIdentity {
    fn matches(&self, r: &PartialRecord) -> bool {
        r.rel == self.rel && r.size == self.size && r.sha256 == self.sha256
    }
}

/// One half-written file recorded for resume. Every field carries
/// `#[serde(default)]` so a record written by an older build still parses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartialRecord {
    /// Sanitized manifest relative path (forward-slash) — the identity key
    /// together with `size` + `sha256`.
    #[serde(default)]
    pub rel: String,
    /// Declared total size from the manifest.
    #[serde(default)]
    pub size: u64,
    /// Whole-file SHA-256 the manifest carried; resume needs it to verify the
    /// reassembled `old-bytes + tail`.
    #[serde(default)]
    pub sha256: Option<String>,
    /// On-disk path RELATIVE to the download root where the partial bytes live
    /// — the organized/de-duped path chosen at first write. Resume reopens
    /// exactly this so a date-organized transfer continues in its original day
    /// folder, not today's.
    #[serde(default)]
    pub disk_rel: String,
    /// Bytes already on disk — the resume offset.
    #[serde(default)]
    pub bytes_written: u64,
}

impl PartialRecord {
    /// This record's identity tuple (for removal / matching).
    pub fn identity(&self) -> FileIdentity {
        FileIdentity {
            rel: self.rel.clone(),
            size: self.size,
            sha256: self.sha256.clone(),
        }
    }
}

/// The persistent partials table. Lives behind `Arc<RwLock<…>>` on `AppState`
/// (and `TransportCtx`); every mutation is followed by a persist at the call
/// site — [`PartialsStore::snapshot`] under the write guard, then
/// [`PartialsSnapshot::persist`] after it drops — mirroring `TrustStore`.
pub struct PartialsStore {
    path: PathBuf,
    map: HashMap<String, Vec<PartialRecord>>,
    seq: u64,
    gate: Arc<Mutex<u64>>,
}

/// A point-in-time serialized image, persisted OUTSIDE the store lock.
pub struct PartialsSnapshot {
    seq: u64,
    json: String,
    path: PathBuf,
    gate: Arc<Mutex<u64>>,
}

impl PartialsSnapshot {
    /// Atomically replace the store file with this image (temp + fsync +
    /// rename), skipping stale images via the sequence gate — the exact
    /// contract as `TrustSnapshot::persist`. Best-effort: every failure is
    /// logged and a leftover temp is cleaned up.
    pub fn persist(self) {
        let Ok(mut last_written) = self.gate.lock() else {
            return;
        };
        if self.seq < *last_written {
            return;
        }
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = self.path.with_extension("json.tmp");
        let written = (|| -> std::io::Result<()> {
            use std::io::Write as _;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(self.json.as_bytes())?;
            f.sync_all()
        })();
        match written.and_then(|()| std::fs::rename(&tmp, &self.path)) {
            Ok(()) => *last_written = self.seq,
            Err(e) => {
                log::warn!("partials store save failed ({}): {e}", self.path.display());
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
}

/// Persist a snapshot on the blocking pool — for call sites on an async worker
/// (the receive path), where even a post-guard `sync_all` on a slow disk would
/// stall the caller. Ordering is the gate's job, not the pool's.
pub fn persist_async(snapshot: Option<PartialsSnapshot>) {
    if let Some(snap) = snapshot {
        tauri::async_runtime::spawn_blocking(move || snap.persist());
    }
}

impl PartialsStore {
    /// Load the store from `path`. Missing file = first run = empty store; a
    /// broken file = empty store (logged). Runs after logger init.
    pub fn load(path: PathBuf) -> PartialsStore {
        let mut store = PartialsStore {
            path,
            map: HashMap::new(),
            seq: 0,
            gate: Arc::new(Mutex::new(0)),
        };
        let raw = match std::fs::read_to_string(&store.path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return store,
            Err(e) => {
                log::warn!(
                    "partials store unreadable, starting empty ({}): {e}",
                    store.path.display()
                );
                return store;
            }
        };
        match serde_json::from_str::<HashMap<String, Vec<PartialRecord>>>(&raw) {
            Ok(map) => store.map = map,
            Err(e) => {
                log::warn!(
                    "partials store corrupted, starting empty ({}): {e}",
                    store.path.display()
                );
            }
        }
        store
    }

    /// Serialize the current table into a persistable image (called UNDER the
    /// write guard; persist AFTER it drops). `None` = serialization failed.
    pub fn snapshot(&self) -> Option<PartialsSnapshot> {
        match serde_json::to_string_pretty(&self.map) {
            Ok(json) => Some(PartialsSnapshot {
                seq: self.seq,
                json,
                path: self.path.clone(),
                gate: self.gate.clone(),
            }),
            Err(e) => {
                log::warn!("partials store serialize failed: {e}");
                None
            }
        }
    }

    /// Synchronous snapshot + write — for tests and simple single-threaded
    /// callers; the concurrent receive path snapshots under the guard and
    /// persists outside it via [`persist_async`] instead.
    pub fn save(&self) {
        if let Some(snap) = self.snapshot() {
            snap.persist();
        }
    }

    /// The stored partial matching `id`, if any (cloned so the caller need not
    /// hold the read guard while it touches the filesystem).
    pub fn lookup(&self, device_id: &str, id: &FileIdentity) -> Option<PartialRecord> {
        self.map
            .get(device_id)?
            .iter()
            .find(|r| id.matches(r))
            .cloned()
    }

    /// Replace this manifest's records for `device_id`: drop every entry whose
    /// identity is in `remove`, then append `keep`. One call serves both
    /// receive outcomes — success passes `keep = []` (the files are delivered,
    /// so their partials are cleared), an interruption passes the still-on-disk
    /// records (persisted for the next resume). The device key is dropped when
    /// its list empties, so a completed transfer leaves no residue.
    ///
    /// Returns whether anything actually changed, so the caller can skip a
    /// needless persist on the overwhelmingly common "no partials involved" case.
    pub fn set_files(
        &mut self,
        device_id: &str,
        remove: &[FileIdentity],
        keep: Vec<PartialRecord>,
    ) -> bool {
        // Nothing to remember AND nothing recorded for this device: no-op.
        if keep.is_empty() && !self.map.contains_key(device_id) {
            return false;
        }
        let before: Option<Vec<PartialRecord>> = self.map.get(device_id).cloned();
        let list = self.map.entry(device_id.to_string()).or_default();
        list.retain(|r| !remove.iter().any(|id| id.matches(r)));
        list.extend(keep);
        let after = if list.is_empty() {
            self.map.remove(device_id);
            None
        } else {
            self.map.get(device_id).cloned()
        };
        if before == after {
            return false; // idempotent call — don't bump the sequence or persist
        }
        self.seq += 1;
        true
    }

    /// Forget every partial for `device_id` (the user-initiated "discard"),
    /// returning the removed records so the caller can delete the on-disk
    /// files. Empty when the device had none.
    pub fn clear_device(&mut self, device_id: &str) -> Vec<PartialRecord> {
        match self.map.remove(device_id) {
            Some(list) => {
                self.seq += 1;
                list
            }
            None => Vec::new(),
        }
    }

    /// Total recorded files across all devices — observability for tests.
    #[doc(hidden)]
    pub fn len(&self) -> usize {
        self.map.values().map(Vec::len).sum()
    }

    /// Whether any partial is recorded — observability for tests.
    #[doc(hidden)]
    pub fn is_empty(&self) -> bool {
        self.map.values().all(Vec::is_empty)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn tmp_file(tag: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("lanbeam-partials-{tag}-{}", std::process::id()))
            .join("partials.json")
    }

    fn cleanup(path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    fn rec(rel: &str, bytes: u64) -> PartialRecord {
        PartialRecord {
            rel: rel.into(),
            size: 1000,
            sha256: Some("deadbeef".into()),
            disk_rel: rel.into(),
            bytes_written: bytes,
        }
    }

    /// What save() writes, load() reads back unchanged — including bytes_written.
    #[test]
    fn roundtrip_preserves_records() {
        let path = tmp_file("roundtrip");
        cleanup(&path);
        let mut store = PartialsStore::load(path.clone());
        assert_eq!(store.len(), 0, "missing file must mean empty store");
        store.set_files("dev-a", &[], vec![rec("a.bin", 400), rec("b.bin", 500)]);
        store.save();

        let back = PartialsStore::load(path.clone());
        assert_eq!(back.len(), 2);
        let id = FileIdentity {
            rel: "a.bin".into(),
            size: 1000,
            sha256: Some("deadbeef".into()),
        };
        let got = back
            .lookup("dev-a", &id)
            .expect("a.bin survives the round-trip");
        assert_eq!(got.bytes_written, 400);
        cleanup(&path);
    }

    /// Lookup matches on the WHOLE identity — a differing size or hash (a
    /// changed file) must not resume onto stale bytes.
    #[test]
    fn lookup_requires_full_identity_match() {
        let path = tmp_file("lookup");
        cleanup(&path);
        let mut store = PartialsStore::load(path.clone());
        store.set_files("dev", &[], vec![rec("f.bin", 300)]);

        let same = FileIdentity {
            rel: "f.bin".into(),
            size: 1000,
            sha256: Some("deadbeef".into()),
        };
        assert!(store.lookup("dev", &same).is_some());

        let changed_hash = FileIdentity {
            rel: "f.bin".into(),
            size: 1000,
            sha256: Some("00000000".into()),
        };
        assert!(
            store.lookup("dev", &changed_hash).is_none(),
            "a new hash must not match"
        );

        let changed_size = FileIdentity {
            rel: "f.bin".into(),
            size: 2000,
            sha256: Some("deadbeef".into()),
        };
        assert!(
            store.lookup("dev", &changed_size).is_none(),
            "a new size must not match"
        );

        assert!(
            store.lookup("other-dev", &same).is_none(),
            "partials are per device"
        );
        cleanup(&path);
    }

    /// `set_files` replaces this manifest's identities: a completed transfer
    /// (`keep = []`) clears them and drops the empty device; an interruption
    /// keeps the still-on-disk records.
    #[test]
    fn set_files_removes_then_appends_and_prunes_empty_device() {
        let path = tmp_file("setfiles");
        cleanup(&path);
        let mut store = PartialsStore::load(path.clone());
        let a = rec("a.bin", 400).identity();
        let b = rec("b.bin", 500).identity();
        store.set_files("dev", &[], vec![rec("a.bin", 400), rec("b.bin", 500)]);
        assert_eq!(store.len(), 2);

        // Interruption: a advanced to 800, b is done (kept at full size here for
        // the test); both remembered, replacing the old entries.
        store.set_files("dev", &[a.clone(), b.clone()], vec![rec("a.bin", 800)]);
        assert_eq!(store.len(), 1, "only the still-partial file remains");
        let id_a = FileIdentity {
            rel: "a.bin".into(),
            size: 1000,
            sha256: Some("deadbeef".into()),
        };
        assert_eq!(store.lookup("dev", &id_a).unwrap().bytes_written, 800);

        // Completion: keep nothing, remove all → the device key is dropped.
        store.set_files("dev", &[a, b], vec![]);
        assert_eq!(store.len(), 0);
        assert!(store.lookup("dev", &id_a).is_none());
        cleanup(&path);
    }

    /// `clear_device` (the discard path) returns the removed records and empties
    /// the device.
    #[test]
    fn clear_device_returns_and_empties() {
        let path = tmp_file("clear");
        cleanup(&path);
        let mut store = PartialsStore::load(path.clone());
        store.set_files("dev", &[], vec![rec("a.bin", 400), rec("b.bin", 500)]);
        let removed = store.clear_device("dev");
        assert_eq!(removed.len(), 2);
        assert_eq!(store.len(), 0);
        assert!(
            store.clear_device("dev").is_empty(),
            "second discard has nothing"
        );
        cleanup(&path);
    }

    /// The sequence gate: an OLDER image persisted after a NEWER one (writers
    /// race outside the store lock) must be skipped, never roll the file back.
    #[test]
    fn stale_snapshot_cannot_overwrite_newer_one() {
        let path = tmp_file("stale");
        cleanup(&path);
        let mut store = PartialsStore::load(path.clone());
        store.set_files("dev", &[], vec![rec("a.bin", 400)]);
        let old_snap = store.snapshot().expect("serializable");
        store.set_files("dev", &[], vec![rec("b.bin", 500)]);
        let new_snap = store.snapshot().expect("serializable");

        new_snap.persist();
        old_snap.persist(); // stale → no-op

        let back = PartialsStore::load(path.clone());
        assert_eq!(back.len(), 2, "stale image must not shrink the file");
        cleanup(&path);
    }

    /// A wholly corrupted `partials.json` (unparseable JSON) falls back to an
    /// empty store rather than propagating the error.
    #[test]
    fn corrupted_file_falls_back_to_empty() {
        let path = tmp_file("corrupt");
        cleanup(&path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ this is not valid json ]").unwrap();

        let store = PartialsStore::load(path.clone());
        assert!(store.is_empty(), "corrupt file must yield an empty store");
        assert_eq!(store.len(), 0);
        cleanup(&path);
    }

    /// A path that exists but cannot be read as a file (it is a directory) is a
    /// non-NotFound read error — the store still starts empty, logged.
    #[test]
    fn unreadable_path_falls_back_to_empty() {
        let path = tmp_file("unreadable");
        cleanup(&path);
        // Make the store path itself a directory so read_to_string fails with a
        // non-NotFound error.
        std::fs::create_dir_all(&path).unwrap();

        let store = PartialsStore::load(path.clone());
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        cleanup(&path);
    }

    /// An idempotent `set_files` — removing and re-adding the identical record —
    /// leaves the table unchanged and reports no change (no sequence bump).
    #[test]
    fn set_files_idempotent_reports_no_change() {
        let path = tmp_file("idempotent");
        cleanup(&path);
        let mut store = PartialsStore::load(path.clone());
        let a = rec("a.bin", 400).identity();
        assert!(store.set_files("dev", &[], vec![rec("a.bin", 400)]));

        // Remove a and append an identical a: net table is the same.
        let changed = store.set_files("dev", &[a], vec![rec("a.bin", 400)]);
        assert!(!changed, "identical result must report no change");
        assert_eq!(store.len(), 1);

        // A pure no-op on an unknown device also reports no change.
        assert!(!store.set_files("nobody", &[], vec![]));
        cleanup(&path);
    }

    /// `is_empty` reflects presence of any record and returns to true once
    /// cleared.
    #[test]
    fn is_empty_tracks_records() {
        let path = tmp_file("isempty");
        cleanup(&path);
        let mut store = PartialsStore::load(path.clone());
        assert!(store.is_empty());
        store.set_files("dev", &[], vec![rec("a.bin", 400)]);
        assert!(!store.is_empty());
        store.clear_device("dev");
        assert!(store.is_empty());
        cleanup(&path);
    }
}
