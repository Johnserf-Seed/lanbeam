//! The devices page's list — live discovery with the manually-added peers
//! (M7.2) merged in — and the dial target behind each entry.

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;

use tauri::State;

use crate::discovery::{self, DiscoveredDevice};
use crate::error::LanBeamError;
use crate::state::{AppState, ManualPeer};
use crate::transport;

#[tauri::command]
pub fn list_discovered_devices(state: State<'_, AppState>) -> Vec<DiscoveredDevice> {
    list_devices_snapshot(&state)
}

/// The devices-page list: the live discovery snapshot with manually-added peers
/// (M7.2) merged in behind it. One source of truth for both the command and the
/// `devices_updated` emit `connect_by_addr` fires.
pub(super) fn list_devices_snapshot(state: &AppState) -> Vec<DiscoveredDevice> {
    let discovered = discovery::snapshot(&state.peers);
    let my_id = state.identity.device_id();
    match state.manual_peers.lock() {
        Ok(manual) => merge_devices(discovered, &manual, &my_id),
        // Poisoned lock: show discovery alone rather than panic the command.
        Err(_) => discovered,
    }
}

/// Merge manually-added peers (M7.2) into a discovery snapshot: a live discovery
/// entry always wins (freshest address), and manual peers fill in only the ids
/// discovery has not seen. Re-sorted like [`discovery::snapshot`] so the merged
/// list keeps a stable, case-insensitive-by-name order. `pub(crate)` so the
/// discovery loop's `devices_updated` emit builds the SAME payload this and
/// `list_discovered_devices` return — one source of truth for the device list.
///
/// THIS DEVICE is never in the result. Discovery already drops its own announces
/// (`apply_packet`), but the manual table had no such rule, so dialing your own
/// address filed you as your own peer — and because discovery can't un-announce
/// what it never announced, that entry could not be expired, only deleted, and
/// nothing could delete it. Filtering here makes an already-poisoned table heal
/// itself on the next list.
pub(crate) fn merge_devices(
    mut discovered: Vec<DiscoveredDevice>,
    manual: &HashMap<String, ManualPeer>,
    my_id: &str,
) -> Vec<DiscoveredDevice> {
    discovered.retain(|d| d.device_id != my_id);
    if manual.is_empty() {
        return discovered;
    }
    let seen: HashSet<String> = discovered.iter().map(|d| d.device_id.clone()).collect();
    for (id, mp) in manual {
        if id != my_id && !seen.contains(id) {
            discovered.push(DiscoveredDevice {
                device_id: id.clone(),
                name: mp.name.clone(),
                address: mp.addr.to_string(),
                port: mp.port,
                // Only in the list because someone typed its address — so it is
                // the one kind of peer that CAN be deleted for good.
                manual: true,
            });
        }
    }
    discovered.sort_by_key(|d| d.name.to_lowercase());
    discovered
}

/// Resolve a device id to a dial target: the discovery table first (freshest
/// address), then a manually-added peer (M7.2). The pinned static key is always
/// the id itself — a later send to an IP-added peer therefore pins the identity
/// learned at `connect_by_addr` time (TOFU). `PeerNotFound` when neither table
/// knows the id.
pub(super) fn resolve_peer(
    state: &AppState,
    device_id: &str,
) -> Result<(Ipv4Addr, u16, [u8; 32]), LanBeamError> {
    let expected = transport::decode_device_id(device_id)
        .ok_or_else(|| LanBeamError::Protocol("invalid device id".into()))?;
    if let Some((addr, port)) = state
        .peers
        .lock()
        .ok()
        .and_then(|p| p.get(device_id).map(|d| (d.addr, d.port)))
    {
        return Ok((addr, port, expected));
    }
    if let Some((addr, port)) = state
        .manual_peers
        .lock()
        .ok()
        .and_then(|m| m.get(device_id).map(|d| (d.addr, d.port)))
    {
        return Ok((addr, port, expected));
    }
    Err(LanBeamError::PeerNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::test_app_state;
    use crate::identity;
    use std::time::Instant;

    /// Merging manual peers (M7.2) appends only unseen ids, lets a live discovery
    /// entry win a collision, and keeps the case-insensitive name order.
    #[test]
    fn merge_devices_appends_manual_and_dedupes() {
        let discovered = vec![DiscoveredDevice {
            device_id: "shared".into(),
            name: "Discovered Name".into(),
            address: "192.168.1.5".into(),
            port: 52637,
            manual: false,
        }];
        let mut manual = HashMap::new();
        // Same id as a discovery entry → must NOT duplicate; discovery wins.
        manual.insert(
            "shared".into(),
            ManualPeer {
                name: "Manual Name".into(),
                addr: Ipv4Addr::new(10, 0, 0, 1),
                port: 9,
                last_used: Instant::now(),
            },
        );
        // A new id → appended.
        manual.insert(
            "alpha".into(),
            ManualPeer {
                name: "Alpha".into(),
                addr: Ipv4Addr::new(10, 0, 0, 2),
                port: 51704,
                last_used: Instant::now(),
            },
        );

        let merged = merge_devices(discovered, &manual, "me");
        assert_eq!(merged.len(), 2, "one shared (deduped) + one new");
        let shared = merged.iter().find(|d| d.device_id == "shared").unwrap();
        assert_eq!(
            shared.name, "Discovered Name",
            "the live discovery entry wins the collision"
        );
        assert_eq!(shared.address, "192.168.1.5");
        let alpha = merged.iter().find(|d| d.device_id == "alpha").unwrap();
        assert_eq!(alpha.address, "10.0.0.2");
        // Sorted case-insensitively by name: "Alpha" before "Discovered Name".
        assert_eq!(merged[0].device_id, "alpha");
    }

    /// Build a minimal `AppState` holding a single manual peer under `id`, with
    /// an empty discovery table — the exact shape `join_by_code` /
    /// `connect_by_addr` leave for a device discovery cannot see.
    fn state_with_manual(id: &str, mp: ManualPeer) -> AppState {
        let state = test_app_state();
        state
            .manual_peers
            .lock()
            .unwrap()
            .insert(id.to_string(), mp);
        state
    }

    /// The peer `join_by_code` records in `manual_peers` (mirroring
    /// `connect_by_addr`) must be BOTH reachable by a later send (`resolve_peer`)
    /// and visible on the devices page (`list_devices_snapshot`), even though
    /// discovery never announced it — the cross-subnet / discovery-off case that
    /// IP/QR pairing exists for. A real device id is used so `resolve_peer`'s
    /// decode step succeeds.
    #[test]
    fn manual_recorded_paired_peer_is_reachable_and_listed() {
        let peer_id = identity::generate_identity().unwrap().device_id();
        let addr = Ipv4Addr::new(10, 1, 2, 3);
        let port = 51999;
        let state = state_with_manual(
            &peer_id,
            ManualPeer {
                name: "Paired".into(),
                addr,
                port,
                last_used: Instant::now(),
            },
        );

        // Reachable: resolve the dial target from the manual table.
        let (r_addr, r_port, _key) =
            resolve_peer(&state, &peer_id).expect("the paired peer resolves for a later send");
        assert_eq!(r_addr, addr, "resolve returns the dialed address");
        assert_eq!(r_port, port, "resolve returns the dialed port");

        // Visible: it appears in the devices-page list.
        let list = list_devices_snapshot(&state);
        assert!(
            list.iter()
                .any(|d| d.device_id == peer_id && d.address == "10.1.2.3"),
            "the paired peer appears in the devices snapshot"
        );
    }

    /// `resolve_peer` fails cleanly on a malformed id (before any table lookup)
    /// and on a well-formed id that no table knows.
    #[test]
    fn resolve_peer_rejects_bad_and_unknown_ids() {
        let state = test_app_state();
        // Not base64url-32 → decode fails → Protocol, before any lookup.
        let err = resolve_peer(&state, "!!!").unwrap_err();
        assert!(
            matches!(err, LanBeamError::Protocol(_)),
            "a malformed device id is a protocol error"
        );
        // A well-formed id that neither the discovery nor manual table holds.
        let unknown = identity::generate_identity().unwrap().device_id();
        let err = resolve_peer(&state, &unknown).unwrap_err();
        assert!(
            matches!(err, LanBeamError::PeerNotFound),
            "a valid but unknown id is PeerNotFound"
        );
    }

    /// `resolve_peer` reads the discovery table, and a live discovery entry wins
    /// over a manual one for the same id (discovery is checked first, freshest
    /// address).
    #[test]
    fn resolve_peer_prefers_discovery_over_manual() {
        let state = test_app_state();
        let id = identity::generate_identity().unwrap().device_id();
        let disc_addr = Ipv4Addr::new(192, 168, 1, 50);
        let manual_addr = Ipv4Addr::new(10, 9, 9, 9);
        state.peers.lock().unwrap().insert(
            id.clone(),
            discovery::Peer {
                id: id.clone(),
                name: "Disc".into(),
                addr: disc_addr,
                port: 51000,
                expires: Instant::now() + std::time::Duration::from_secs(60),
            },
        );
        state.manual_peers.lock().unwrap().insert(
            id.clone(),
            ManualPeer {
                name: "Manual".into(),
                addr: manual_addr,
                port: 52000,
                last_used: Instant::now(),
            },
        );
        let (addr, port, _key) = resolve_peer(&state, &id).expect("a discovered peer resolves");
        assert_eq!(addr, disc_addr, "the discovery table wins the address");
        assert_eq!(port, 51000, "the discovery table wins the port");
    }

    /// The devices snapshot merges a live discovery entry with a manual peer and
    /// keeps the case-insensitive-by-name order.
    #[test]
    fn list_devices_snapshot_merges_discovery_and_manual() {
        let state = test_app_state();
        let disc_id = identity::generate_identity().unwrap().device_id();
        state.peers.lock().unwrap().insert(
            disc_id.clone(),
            discovery::Peer {
                id: disc_id.clone(),
                name: "Zeta".into(),
                addr: Ipv4Addr::new(192, 168, 1, 7),
                port: 51000,
                expires: Instant::now() + std::time::Duration::from_secs(60),
            },
        );
        state.manual_peers.lock().unwrap().insert(
            "manual-id".into(),
            ManualPeer {
                name: "Alpha".into(),
                addr: Ipv4Addr::new(10, 0, 0, 4),
                port: 52000,
                last_used: Instant::now(),
            },
        );
        let list = list_devices_snapshot(&state);
        assert_eq!(list.len(), 2, "discovery + manual both appear");
        // Sorted case-insensitively by name: "Alpha" before "Zeta".
        assert_eq!(list[0].device_id, "manual-id");
        assert!(
            list.iter()
                .any(|d| d.device_id == disc_id && d.address == "192.168.1.7"),
            "the discovered peer is present with its source address"
        );
    }

    /// With an empty manual table `merge_devices` returns the discovery snapshot
    /// untouched (the early-return fast path).
    /// THIS DEVICE never appears in its own device list — from either source.
    ///
    /// Dialing your own address used to file you as your own manual peer, and a
    /// self-entry is uniquely un-removable: discovery drops its own announces, so
    /// it can never be expired, and the only thing that could clear it (the trust
    /// row) wasn't where it lived. It sat in the list forever. Deleting it cleared
    /// the trust row and changed nothing; the device came straight back.
    #[test]
    fn merge_devices_never_lists_this_device() {
        let discovered = vec![DiscoveredDevice {
            device_id: "me".into(),
            name: "Me (somehow announced)".into(),
            address: "192.168.1.5".into(),
            port: 52637,
            manual: false,
        }];
        let mut manual = HashMap::new();
        manual.insert(
            "me".into(),
            ManualPeer {
                name: "Me (dialled my own address)".into(),
                addr: Ipv4Addr::new(127, 0, 0, 1),
                port: 51704,
                last_used: Instant::now(),
            },
        );
        manual.insert(
            "someone-else".into(),
            ManualPeer {
                name: "Real Peer".into(),
                addr: Ipv4Addr::new(10, 0, 0, 2),
                port: 51704,
                last_used: Instant::now(),
            },
        );

        let merged = merge_devices(discovered, &manual, "me");

        assert!(
            merged.iter().all(|d| d.device_id != "me"),
            "this device must never be listed as a peer of itself — an entry              nothing can announce away is an entry nothing can remove"
        );
        assert_eq!(merged.len(), 1, "the real peer still comes through");
        assert_eq!(merged[0].device_id, "someone-else");
    }

    /// A manual peer is flagged as such, so the UI can tell 「删除设备」 apart from
    /// 「it will just come back」: nothing announces a manually-typed address, so
    /// deleting one really does end it.
    #[test]
    fn merge_devices_flags_manual_peers() {
        let discovered = vec![DiscoveredDevice {
            device_id: "announced".into(),
            name: "Announced".into(),
            address: "192.168.1.5".into(),
            port: 52637,
            manual: false,
        }];
        let mut manual = HashMap::new();
        manual.insert(
            "typed-in".into(),
            ManualPeer {
                name: "Typed In".into(),
                addr: Ipv4Addr::new(10, 0, 0, 2),
                port: 51704,
                last_used: Instant::now(),
            },
        );

        let merged = merge_devices(discovered, &manual, "me");

        let by = |id: &str| merged.iter().find(|d| d.device_id == id).unwrap().manual;
        assert!(!by("announced"), "it announced itself");
        assert!(by("typed-in"), "someone typed its address");
    }

    #[test]
    fn merge_devices_with_no_manual_returns_discovery_unchanged() {
        let discovered = vec![DiscoveredDevice {
            device_id: "only".into(),
            name: "Only".into(),
            address: "192.168.1.1".into(),
            port: 52637,
            manual: false,
        }];
        let merged = merge_devices(discovered.clone(), &HashMap::new(), "me");
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].device_id, "only");
        assert_eq!(merged[0].address, "192.168.1.1");
    }
}
