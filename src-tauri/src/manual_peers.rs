//! Hand-added peers (M7.2) that outlive a restart: Device ID → [`ManualPeer`],
//! backing `manual_peers.json`.
//!
//! WHY persisted: these are exactly the devices discovery cannot see — another
//! subnet, or discovery off on that box — reached once by address
//! (`connect_by_addr`) or by pairing code (`join_by_code`, and the host's side
//! of it). Kept only in memory, the table emptied on every restart, so a device
//! paired across subnets came back trusted but unreachable: no address to dial
//! until someone typed it in again.
//!
//! Same file contract as `trust.rs`: a missing file is a first run, an
//! unreadable entry is skipped with a warning while the rest survives, a wholly
//! corrupted file falls back to an empty table — logged, never silent — and
//! writes are atomic (temp file + fsync + rename) behind a sequence gate. The
//! file is a JSON array, oldest first: the eviction order has to survive a
//! restart, and an [`Instant`] cannot be written down.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::consts::DEFAULT_TCP_PORT;

/// A peer added by hand through `connect_by_addr` (M7.2) or a pairing code
/// (M7.1) rather than seen via discovery. Kept in its own table — never expired
/// by the discovery loop — so an IP-dialed device stays on the devices page;
/// merged BEHIND live discovery entries in `list_discovered_devices`, and
/// consulted as a fallback dial target when a send/connect can't find the id
/// in the discovery table.
#[derive(Clone)]
pub struct ManualPeer {
    pub name: String,
    pub addr: Ipv4Addr,
    pub port: u16,
    /// The name it was added by (`nas.local`), when it was added by name (F6).
    /// A send resolves the name again, so the device stays reachable after its
    /// address changes; `addr` is what it resolved to when added, and the
    /// fallback when the name doesn't resolve.
    pub host: Option<String>,
    /// When this entry was last inserted or refreshed (a dial / re-pair).
    /// Monotonic [`Instant`], never displayed — the eviction key
    /// [`ManualPeers::insert`] uses to keep the table bounded. Manual peers never
    /// expire (the discovery expiry loop deliberately skips them), so without an
    /// ordering carrier there would be no "oldest" to evict. The file keeps only
    /// the ORDER, and [`ManualPeers::load`] rebuilds the stamps from it.
    pub last_used: Instant,
}

/// Cap on how many hand-added peers the table retains. Manual peers never
/// expire, so this bound is the only thing stopping years of IP dials / re-pairs
/// — e.g. a counterpart that keeps resetting its identity, minting a fresh
/// `device_id` each time — from growing the table, and now the file, without
/// limit.
pub const MAX_MANUAL_PEERS: usize = 256;

/// One entry of `manual_peers.json`. The id and the address are required — an
/// entry with nothing to dial is no entry — and the rest default, so a later
/// build can add fields without this one dropping the file.
#[derive(Serialize, Deserialize)]
struct Record {
    device_id: String,
    #[serde(default)]
    name: String,
    addr: Ipv4Addr,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host: Option<String>,
}

fn default_port() -> u16 {
    DEFAULT_TCP_PORT
}

/// The manual peer table. Lives behind `Arc<Mutex<…>>` on `AppState` (shared
/// with the listener's `TransportCtx` and the discovery loop); every mutation is
/// followed by a persist at the call site — [`ManualPeers::snapshot`] under the
/// guard, then [`persist_async`] after it drops — so the file never lags the
/// table and the lock never waits on a disk. `Default` is an in-memory table
/// that never touches the disk, for tests.
#[derive(Default)]
pub struct ManualPeers {
    /// Where the table persists — captured at load so save sites don't need the
    /// app handle (the host side of a pairing only has a `TransportCtx`).
    path: Option<PathBuf>,
    peers: HashMap<String, ManualPeer>,
    /// Bumped on every mutation and stamped into snapshots, so an image
    /// persisted out of order can never roll the file back.
    seq: u64,
    /// Write-out gate shared by this table's snapshots: serializes the file I/O
    /// and remembers the newest sequence already on disk.
    gate: Arc<Mutex<u64>>,
}

/// A point-in-time serialized image of [`ManualPeers`], taken under the table's
/// guard (cheap, memory-only) and persisted OUTSIDE it — the trust store's
/// [`crate::trust::TrustSnapshot`], for the same reasons.
pub struct ManualPeersSnapshot {
    seq: u64,
    json: String,
    path: PathBuf,
    gate: Arc<Mutex<u64>>,
}

impl ManualPeersSnapshot {
    /// Atomically replace the file with this image (sibling temp file, fsync,
    /// rename), skipping stale images via the sequence gate. Best-effort: losing
    /// an address is an inconvenience, failing the pairing over it would be
    /// worse — so every failure is logged and a leftover temp file cleaned up.
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
                log::warn!("manual peers save failed ({}): {e}", self.path.display());
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
}

/// Persist a snapshot on the blocking pool — every save site runs on an async
/// worker or the main thread, where even a post-guard `sync_all` on a slow disk
/// would stall the caller. Ordering across tasks is the gate's job.
pub fn persist_async(snapshot: Option<ManualPeersSnapshot>) {
    if let Some(snap) = snapshot {
        tauri::async_runtime::spawn_blocking(move || snap.persist());
    }
}

impl ManualPeers {
    /// Load the table from `path`. Missing file = first run = empty table. A
    /// broken entry is skipped (the rest survive); a broken FILE yields an empty
    /// table — both logged, so the caller must run after logger init.
    pub fn load(path: PathBuf) -> ManualPeers {
        ManualPeers {
            peers: read_file(&path),
            path: Some(path),
            seq: 0,
            gate: Arc::new(Mutex::new(0)),
        }
    }

    pub fn get(&self, device_id: &str) -> Option<&ManualPeer> {
        self.peers.get(device_id)
    }

    /// Every entry, for the merged device list.
    pub fn peers(&self) -> &HashMap<String, ManualPeer> {
        &self.peers
    }

    /// Insert (or refresh) a peer, then evict the least-recently-used entries
    /// while the table exceeds [`MAX_MANUAL_PEERS`]. Re-dialing an existing id
    /// refreshes its stamp so it is not the next victim.
    pub fn insert(&mut self, device_id: String, peer: ManualPeer) {
        insert_bounded(&mut self.peers, device_id, peer);
        self.seq += 1;
    }

    /// Remove one peer. `true` when there was one — the only case worth a save.
    pub fn remove(&mut self, device_id: &str) -> bool {
        let removed = self.peers.remove(device_id).is_some();
        if removed {
            self.seq += 1;
        }
        removed
    }

    /// Empty the table (identity reset). `true` when it held anything.
    pub fn clear(&mut self) -> bool {
        if self.peers.is_empty() {
            return false;
        }
        self.peers.clear();
        self.seq += 1;
        true
    }

    /// Serialize the table into a persistable image. Called UNDER the caller's
    /// guard; the returned snapshot's [`ManualPeersSnapshot::persist`] does the
    /// file I/O and must run AFTER the guard is dropped. `None` for an in-memory
    /// table, or when serialization failed (logged).
    pub fn snapshot(&self) -> Option<ManualPeersSnapshot> {
        let path = self.path.clone()?;
        let mut entries: Vec<(&String, &ManualPeer)> = self.peers.iter().collect();
        // Oldest first: the order is the part of `last_used` the file keeps.
        entries.sort_by_key(|(_, p)| p.last_used);
        let records: Vec<Record> = entries
            .into_iter()
            .map(|(id, p)| Record {
                device_id: id.clone(),
                name: p.name.clone(),
                addr: p.addr,
                port: p.port,
                host: p.host.clone(),
            })
            .collect();
        match serde_json::to_string_pretty(&records) {
            Ok(json) => Some(ManualPeersSnapshot {
                seq: self.seq,
                json,
                path,
                gate: self.gate.clone(),
            }),
            Err(e) => {
                log::warn!("manual peers serialize failed: {e}");
                None
            }
        }
    }
}

/// Insert, then evict by the `last_used` stamp (oldest first) down to the cap.
fn insert_bounded(map: &mut HashMap<String, ManualPeer>, device_id: String, peer: ManualPeer) {
    map.insert(device_id, peer);
    while map.len() > MAX_MANUAL_PEERS {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, p)| p.last_used)
            .map(|(id, _)| id.clone())
        else {
            break;
        };
        map.remove(&oldest);
    }
}

/// The file's entries, rebuilt into a bounded table.
fn read_file(path: &Path) -> HashMap<String, ManualPeer> {
    let mut peers = HashMap::new();
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return peers,
        Err(e) => {
            log::warn!(
                "manual peers unreadable, starting empty ({}): {e}",
                path.display()
            );
            return peers;
        }
    };
    let entries = match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(serde_json::Value::Array(entries)) => entries,
        Ok(_) => {
            log::warn!(
                "manual peers file is not a JSON array, starting empty ({})",
                path.display()
            );
            return peers;
        }
        Err(e) => {
            log::warn!(
                "manual peers file corrupted, starting empty ({}): {e}",
                path.display()
            );
            return peers;
        }
    };
    // Stamps one nanosecond apart in file order: all that eviction needs is
    // which entry is older, and the file is written oldest first.
    let base = Instant::now();
    for (i, entry) in entries.into_iter().enumerate() {
        // Per-entry tolerance: one bad record must not cost every other address.
        match serde_json::from_value::<Record>(entry) {
            Ok(rec) => insert_bounded(
                &mut peers,
                rec.device_id,
                ManualPeer {
                    name: rec.name,
                    addr: rec.addr,
                    port: rec.port,
                    host: rec.host,
                    last_used: base + Duration::from_nanos(i as u64),
                },
            ),
            Err(e) => log::warn!("manual peer entry {i} unreadable, skipped: {e}"),
        }
    }
    peers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(tag: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("lanbeam-manual-{tag}-{}", std::process::id()))
            .join("manual_peers.json")
    }

    fn cleanup(path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    fn peer(name: &str, port: u16, last_used: Instant) -> ManualPeer {
        ManualPeer {
            name: name.into(),
            addr: Ipv4Addr::new(10, 0, 0, 7),
            port,
            host: None,
            last_used,
        }
    }

    fn save(table: &ManualPeers) {
        table.snapshot().expect("a loaded table persists").persist();
    }

    /// Filling past the cap evicts the least-recently-used entry (smallest
    /// `last_used`), keeping the table bounded to [`MAX_MANUAL_PEERS`].
    #[test]
    fn insert_evicts_oldest_beyond_cap() {
        let mut table = ManualPeers::default();
        let t0 = Instant::now();
        for i in 0..(MAX_MANUAL_PEERS as u64) {
            table.insert(
                format!("id-{i}"),
                peer(&format!("dev-{i}"), 5000, t0 + Duration::from_millis(i)),
            );
        }
        assert_eq!(table.peers().len(), MAX_MANUAL_PEERS);

        // One more with the newest stamp pushes over the cap: the oldest (id-0) goes.
        let newest = t0 + Duration::from_millis(MAX_MANUAL_PEERS as u64 + 100);
        table.insert("id-new".into(), peer("new", 5000, newest));
        assert_eq!(table.peers().len(), MAX_MANUAL_PEERS, "still bounded");
        assert!(table.get("id-0").is_none(), "least-recently-used evicted");
        assert!(table.get("id-1").is_some(), "next-oldest survives");
        assert!(table.get("id-new").is_some(), "the new peer is retained");
    }

    /// Re-inserting an existing id refreshes it in place (new fields, same slot)
    /// without growing the table.
    #[test]
    fn insert_refresh_updates_without_growing() {
        let mut table = ManualPeers::default();
        let t0 = Instant::now();
        table.insert("a".into(), peer("a", 1, t0));
        table.insert("a".into(), peer("a2", 2, t0 + Duration::from_secs(1)));
        assert_eq!(table.peers().len(), 1, "refresh replaces, does not add");
        assert_eq!(table.get("a").unwrap().name, "a2");
        assert_eq!(table.get("a").unwrap().port, 2);
    }

    /// Re-dialing the oldest entry refreshes its `last_used` stamp, so the NEXT
    /// eviction takes a different (now-oldest) victim rather than the refreshed one.
    #[test]
    fn insert_refresh_spares_from_eviction() {
        let mut table = ManualPeers::default();
        let t0 = Instant::now();
        for i in 0..(MAX_MANUAL_PEERS as u64) {
            table.insert(
                format!("id-{i}"),
                peer("d", 1, t0 + Duration::from_millis(i)),
            );
        }
        // Refresh the oldest (id-0) with a fresh stamp so it is no longer the victim.
        table.insert("id-0".into(), peer("d", 1, t0 + Duration::from_secs(3600)));
        // A new peer pushes over the cap: now id-1 (next-oldest) is evicted, not id-0.
        table.insert(
            "id-new".into(),
            peer("d", 1, t0 + Duration::from_secs(7200)),
        );
        assert_eq!(table.peers().len(), MAX_MANUAL_PEERS);
        assert!(table.get("id-0").is_some(), "refreshed peer spared");
        assert!(table.get("id-1").is_none(), "next-oldest evicted instead");
    }

    /// The point of the file: what one run recorded, the next run can dial.
    #[test]
    fn roundtrip_preserves_entries() {
        let path = tmp_file("roundtrip");
        cleanup(&path);
        let mut table = ManualPeers::load(path.clone());
        assert!(table.peers().is_empty(), "no file yet = empty table");
        table.insert(
            "joiner".into(),
            ManualPeer {
                name: "书房 · MacBook Pro".into(),
                addr: Ipv4Addr::new(192, 168, 7, 20),
                port: 51999,
                host: Some("study-mbp.local".into()),
                last_used: Instant::now(),
            },
        );
        save(&table);

        let back = ManualPeers::load(path.clone());
        let rec = back
            .get("joiner")
            .expect("the address survived the restart");
        assert_eq!(rec.name, "书房 · MacBook Pro");
        assert_eq!(rec.addr, Ipv4Addr::new(192, 168, 7, 20));
        assert_eq!(rec.port, 51999);
        assert_eq!(rec.host.as_deref(), Some("study-mbp.local"));
        cleanup(&path);
    }

    /// Recency survives a restart: after a reload, the entry used longest ago is
    /// still the first one a full table evicts — not whichever the hash map
    /// happens to yield first.
    #[test]
    fn eviction_order_survives_a_reload() {
        let path = tmp_file("order");
        cleanup(&path);
        let mut table = ManualPeers::load(path.clone());
        let t0 = Instant::now();
        // Inserted in REVERSE id order, so the oldest is "id-255", not "id-0".
        for i in 0..(MAX_MANUAL_PEERS as u64) {
            let id = MAX_MANUAL_PEERS as u64 - 1 - i;
            table.insert(
                format!("id-{id}"),
                peer("p", 5000, t0 + Duration::from_millis(i)),
            );
        }
        save(&table);

        let mut back = ManualPeers::load(path.clone());
        assert_eq!(back.peers().len(), MAX_MANUAL_PEERS);
        back.insert("id-new".into(), peer("new", 5000, Instant::now()));
        let oldest = format!("id-{}", MAX_MANUAL_PEERS - 1);
        assert!(back.get(&oldest).is_none(), "the oldest entry is evicted");
        assert!(back.get("id-0").is_some(), "the newest old entry survives");
        cleanup(&path);
    }

    /// A removal reaches the disk — a forgotten device stays forgotten.
    #[test]
    fn remove_and_clear_persist() {
        let path = tmp_file("remove");
        cleanup(&path);
        let mut table = ManualPeers::load(path.clone());
        let now = Instant::now();
        table.insert("a".into(), peer("a", 1, now));
        table.insert("b".into(), peer("b", 2, now));
        assert!(table.remove("a"));
        assert!(!table.remove("a"), "nothing left to remove");
        save(&table);
        let back = ManualPeers::load(path.clone());
        assert!(back.get("a").is_none(), "the removed address stays gone");
        assert!(back.get("b").is_some());

        assert!(table.clear());
        assert!(!table.clear(), "already empty");
        save(&table);
        assert!(ManualPeers::load(path.clone()).peers().is_empty());
        cleanup(&path);
    }

    /// One bad entry costs only itself; a file that is not an array at all costs
    /// the table, but never the startup.
    #[test]
    fn tolerant_load() {
        let path = tmp_file("tolerant");
        cleanup(&path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"[
                {"device_id": "no-addr", "name": "Broken"},
                {"device_id": "ok", "addr": "10.1.2.3", "future_field": true},
                {"device_id": "bad-ip", "addr": "not an ip"}
            ]"#,
        )
        .unwrap();
        let table = ManualPeers::load(path.clone());
        assert_eq!(table.peers().len(), 1, "only the readable entry survives");
        let ok = table.get("ok").unwrap();
        assert_eq!(ok.port, DEFAULT_TCP_PORT, "a missing port defaults");
        assert_eq!(ok.name, "", "a missing name defaults");
        assert_eq!(ok.host, None, "an entry from before names has none");

        std::fs::write(&path, "{not json").unwrap();
        assert!(ManualPeers::load(path.clone()).peers().is_empty());
        std::fs::write(&path, r#"{"ok": {"addr": "10.1.2.3"}}"#).unwrap();
        assert!(ManualPeers::load(path.clone()).peers().is_empty());
        cleanup(&path);
    }

    /// Two snapshots persisted out of order: the stale one must be a no-op.
    #[test]
    fn stale_snapshot_cannot_overwrite_newer_one() {
        let path = tmp_file("stale-seq");
        cleanup(&path);
        let mut table = ManualPeers::load(path.clone());
        table.insert("first".into(), peer("First", 1, Instant::now()));
        let old_snap = table.snapshot().unwrap();
        table.insert("second".into(), peer("Second", 2, Instant::now()));
        let new_snap = table.snapshot().unwrap();

        new_snap.persist(); // the newer image lands first…
        old_snap.persist(); // …so the stale one must be a no-op

        let back = ManualPeers::load(path.clone());
        assert_eq!(
            back.peers().len(),
            2,
            "stale image must not shrink the file"
        );

        // Same for a removal: an image from before it must not bring the
        // forgotten device back.
        let before = table.snapshot().unwrap();
        assert!(table.remove("first"));
        table.snapshot().unwrap().persist();
        before.persist();
        let back = ManualPeers::load(path.clone());
        assert!(back.get("first").is_none(), "a removal stays removed");
        cleanup(&path);
    }

    /// The in-memory table tests build never writes anywhere.
    #[test]
    fn default_table_has_no_file() {
        let mut table = ManualPeers::default();
        table.insert("a".into(), peer("a", 1, Instant::now()));
        assert!(table.snapshot().is_none());
    }
}
