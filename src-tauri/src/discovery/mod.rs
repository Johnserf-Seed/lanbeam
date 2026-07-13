//! LAN peer discovery: announce our presence and maintain a live peer table from
//! incoming announces. UDP multicast + broadcast on `DISCOVERY_PORT`. DESIGN §1.1, §3.5.

// pub (M5.1): `get_network_info` surfaces the same enumeration discovery
// announces on — one source of truth for "which addresses is this box on".
pub mod interfaces;
mod packet;
pub mod socket;

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::consts::{
    ANNOUNCE_INTERVAL, DISCOVERY_GROUP, DISCOVERY_PORT, PEER_EXPIRY, PROTO_VERSION,
};
use crate::settings::Settings;
use crate::state::{ManualPeer, NetDegraded};
use packet::DiscoveryPacket;

/// A peer as tracked locally. `addr` comes from the UDP source address, never from the packet.
pub struct Peer {
    pub id: String,
    pub name: String,
    pub addr: Ipv4Addr,
    pub port: u16,
    pub expires: Instant,
}

pub type PeerTable = HashMap<String, Peer>;

/// What the UI sees for each discovered device (camelCase over the bridge).
#[derive(Serialize, Clone)]
pub struct DiscoveredDevice {
    #[serde(rename = "deviceId")]
    pub device_id: String,
    pub name: String,
    pub address: String,
    pub port: u16,
}

/// Snapshot the peer table into a sorted device list for the UI.
pub fn snapshot(peers: &Arc<Mutex<PeerTable>>) -> Vec<DiscoveredDevice> {
    // Poisoned lock degrades this snapshot to an empty list rather than
    // panicking every caller (`list_discovered_devices` and every
    // `devices_updated` emit): a poisoned lock degrades discovery, never
    // tears it down.
    let Ok(table) = peers.lock() else {
        return Vec::new();
    };
    let mut list: Vec<DiscoveredDevice> = table
        .values()
        .map(|p| DiscoveredDevice {
            device_id: p.id.clone(),
            name: p.name.clone(),
            address: p.addr.to_string(),
            port: p.port,
        })
        .collect();
    list.sort_by_key(|d| d.name.to_lowercase());
    list
}

/// Apply one received packet to the peer table. Returns whether the visible set changed.
/// Pure-ish (uses `Instant::now` for expiry); the `changed` result is deterministic and tested.
pub fn apply_packet(
    peers: &mut PeerTable,
    my_id: &str,
    src_ip: Ipv4Addr,
    pkt: &DiscoveryPacket,
) -> bool {
    if pkt.v != PROTO_VERSION || pkt.id == my_id {
        return false;
    }
    if !pkt.disc {
        return peers.remove(&pkt.id).is_some();
    }
    let expires = Instant::now() + PEER_EXPIRY;
    // Clamp the peer-supplied name at this trust boundary, exactly as the
    // DeviceInfo path does — the announce's `name` is an honest-sender convention
    // only (the receiver reads up to the whole 2048-byte datagram), so bound it
    // once here, before the stored value can flow into the device list,
    // notification titles, event payloads or logs unbounded.
    let name = crate::trust::clamp_name(&pkt.name);
    match peers.get_mut(&pkt.id) {
        Some(p) => {
            let changed = p.name != name || p.addr != src_ip || p.port != pkt.port;
            p.name = name;
            p.addr = src_ip;
            p.port = pkt.port;
            p.expires = expires;
            changed
        }
        None => {
            peers.insert(
                pkt.id.clone(),
                Peer {
                    id: pkt.id.clone(),
                    name,
                    addr: src_ip,
                    port: pkt.port,
                    expires,
                },
            );
            true
        }
    }
}

#[derive(Clone)]
pub struct DiscoveryCtx {
    pub app: AppHandle,
    pub my_id: String,
    pub settings: Arc<RwLock<Settings>>,
    pub peers: Arc<Mutex<PeerTable>>,
    pub tcp_port: Arc<AtomicU16>,
    /// Manually-added peers (M7.2), shared with `AppState`. Threaded here so the
    /// `devices_updated` emit merges them BEHIND the live discovery snapshot —
    /// otherwise a discovery change would push a discovery-only payload and the
    /// frontend, which replaces its whole list, would drop every IP-dialed /
    /// code-paired peer. See [`emit_devices`].
    pub manual_peers: Arc<Mutex<HashMap<String, ManualPeer>>>,
    /// Live browser shares (M8.3), shared with `AppState`. The announcer reads it
    /// each tick to decide whether to advertise the HTTP share port — see
    /// [`share_http_advert`].
    pub shares: crate::share::ShareRegistry,
    /// The browser-share server's bound port (M8.3); `0` until it binds. Carried
    /// in a discoverable announce's `http` field only while a share is live.
    pub share_port: Arc<AtomicU16>,
}

/// Spawn the discovery service (announce + listen + expiry tasks) on the Tauri runtime.
/// A receive fallback is pushed into `degraded` BEFORE the emit — the emit fires
/// during `setup()`, before the webview exists, so the recorded entry (served by
/// `get_net_status`) is what actually reaches the user (M4.6).
pub fn spawn(ctx: DiscoveryCtx, degraded: Arc<Mutex<Vec<NetDegraded>>>) {
    tauri::async_runtime::spawn(async move {
        let (sock, receiving) = match socket::build_discovery_socket() {
            Ok(s) => s,
            Err(e) => {
                // No socket at all: this device neither announces nor sees peers.
                log::error!("discovery socket build failed (discovery disabled): {e}");
                return;
            }
        };
        let sock = Arc::new(sock);
        if receiving {
            log::info!("discovery announcing + listening on UDP {DISCOVERY_PORT}");
        } else {
            // Degraded, not broken: our announces still egress from the ephemeral
            // socket, so OTHERS see us — but we never see them. That asymmetry is
            // baffling in the field, so surface it to the UI as well (M4.6).
            let detail = format!(
                "UDP port {DISCOVERY_PORT} is held by another process — announcing only; \
                 other devices will not appear here until the port is free again"
            );
            log::warn!("{detail}");
            let event = NetDegraded {
                kind: "udp_recv_fallback".into(),
                detail,
            };
            if let Ok(mut list) = degraded.lock() {
                list.push(event.clone());
            }
            let _ = ctx.app.emit("net_degraded", &event);
        }
        join_all(&sock, &active_interfaces(&ctx.settings).0);

        {
            let (s, c) = (sock.clone(), ctx.clone());
            tokio::spawn(async move { announce_loop(s, c).await });
        }
        if receiving {
            let (s, c) = (sock.clone(), ctx.clone());
            tokio::spawn(async move { listen_loop(s, c).await });
        }
        tokio::spawn(async move { expiry_loop(ctx).await });
    });
}

fn join_all(sock: &tokio::net::UdpSocket, ifaces: &[interfaces::Iface]) {
    for iface in ifaces {
        let _ = sock.join_multicast_v4(DISCOVERY_GROUP, iface.ip);
    }
}

/// Warn-once latch for a stale `iface_filter` (see [`active_interfaces`]).
/// WHY: the fallback re-evaluates on every 2s announce tick, and an unplugged
/// NIC can stay unplugged for days — warning each tick would drown the log.
/// The latch resets when the filter matches again, so each new drift episode
/// gets exactly one warning.
static IFACE_FALLBACK_WARNED: AtomicBool = AtomicBool::new(false);

/// The interfaces discovery actually uses — `enumerate()` narrowed by the
/// live `iface_filter` setting (M5.6) — plus whether that filter is actively
/// CONFINING egress (a filter is set AND it matched a live interface). This is
/// the single choke point for the discovery side (multicast joins, announces,
/// per-iface broadcasts), so a changed filter takes effect within one announce
/// tick, no restart. A filter matching no current interface falls back to ALL
/// interfaces (and reports `confined = false`) — see `interfaces::select`.
/// `get_network_info` deliberately keeps calling `enumerate()` raw: the
/// settings UI must list every address, or the user could never re-pick after
/// a filter goes stale.
fn active_interfaces(settings: &Arc<RwLock<Settings>>) -> (Vec<interfaces::Iface>, bool) {
    let filter = settings.read().ok().and_then(|s| {
        s.iface_filter
            .as_deref()
            .and_then(|v| v.parse::<Ipv4Addr>().ok())
    });
    let (kept, fell_back) = interfaces::select(interfaces::enumerate(), filter);
    if fell_back {
        if !IFACE_FALLBACK_WARNED.swap(true, Ordering::Relaxed) {
            log::warn!(
                "iface filter {filter:?} matches no current interface; \
                 falling back to all interfaces"
            );
        }
    } else {
        IFACE_FALLBACK_WARNED.store(false, Ordering::Relaxed);
    }
    // Confined ONLY when a filter is set and actually matched: a stale filter
    // that fell back to every interface must NOT steer/suppress egress, or the
    // fallback (which exists so discovery never strands) would be defeated.
    let confined = filter.is_some() && !fell_back;
    (kept, confined)
}

/// Emit `devices_updated` with the SAME merged list `list_discovered_devices`
/// returns: the live discovery snapshot with manual peers (M7.2) behind it. WHY
/// merge here and not just `snapshot(peers)`: the frontend replaces its whole
/// device list with this payload, so a discovery-only payload would evict every
/// manually IP-dialed / code-paired peer on the next discovery change.
fn emit_devices(ctx: &DiscoveryCtx) {
    let _ = ctx.app.emit(
        "devices_updated",
        devices_payload(&ctx.peers, &ctx.manual_peers),
    );
}

/// The HTTP share port to advertise in a discoverable announce (M8.3): the bound
/// share port when the server is up AND at least one share is serveable, else
/// `None` (the packet then omits `http`). Recomputed each announce tick so the
/// advertisement appears and disappears with the share's own lifetime — a
/// stopped/expired/exhausted share stops advertising within a tick.
fn share_http_advert(ctx: &DiscoveryCtx) -> Option<u16> {
    let port = ctx.share_port.load(Ordering::Relaxed);
    if port != 0 && crate::share::has_active_share(&ctx.shares, Instant::now()) {
        Some(port)
    } else {
        None
    }
}

/// Build the merged device list (discovery snapshot + manual peers) for
/// `devices_updated`. Split out so it is unit-testable without a Tauri app and so
/// it reuses [`crate::commands::merge_devices`] — the one merge both this emit
/// and `list_discovered_devices` go through. `snapshot` locks (and releases)
/// `peers`, then `manual` is locked separately, so no two locks are ever held at
/// once (and none across an await — this is sync).
fn devices_payload(
    peers: &Arc<Mutex<PeerTable>>,
    manual: &Arc<Mutex<HashMap<String, ManualPeer>>>,
) -> Vec<DiscoveredDevice> {
    let discovered = snapshot(peers);
    match manual.lock() {
        Ok(m) => crate::commands::merge_devices(discovered, &m),
        // Poisoned lock: emit discovery alone rather than skip the update.
        Err(_) => discovered,
    }
}

async fn send_all(
    sock: &tokio::net::UdpSocket,
    ifaces: &[interfaces::Iface],
    pkt: &DiscoveryPacket,
    confined: bool,
) {
    let buf = match serde_json::to_vec(pkt) {
        Ok(b) => b,
        Err(_) => return,
    };
    if confined {
        // A live iface filter (M5.6) is confining discovery to `ifaces`. The
        // default multicast egress interface is chosen by the OS routing table
        // (typically the default-route NIC — often the very one the user
        // excluded), so steer IP_MULTICAST_IF to each kept interface and send
        // the group datagram once per interface. tokio's UdpSocket does not
        // expose set_multicast_if_v4, so reach the underlying socket through
        // socket2::SockRef (sends here are sequential on one task, so there is
        // no concurrent-setsockopt race). The limited 255.255.255.255 broadcast
        // is dropped entirely — IP_MULTICAST_IF cannot steer it and it would
        // leak onto the excluded interface; the per-iface directed broadcasts
        // below already cover the selected subnets.
        for iface in ifaces {
            if socket2::SockRef::from(sock)
                .set_multicast_if_v4(&iface.ip)
                .is_ok()
            {
                let _ = sock
                    .send_to(&buf, SocketAddr::from((DISCOVERY_GROUP, DISCOVERY_PORT)))
                    .await;
            }
            if let Some(b) = iface.broadcast {
                let _ = sock
                    .send_to(&buf, SocketAddr::from((b, DISCOVERY_PORT)))
                    .await;
            }
        }
        return;
    }
    // Unfiltered (all interfaces): the widest reach — multicast on the OS
    // default interface + directed broadcast per iface + limited broadcast.
    // Reset IP_MULTICAST_IF to the default route first, in case a prior
    // confined tick pinned it to a since-cleared filter's interface.
    let _ = socket2::SockRef::from(sock).set_multicast_if_v4(&Ipv4Addr::UNSPECIFIED);
    let _ = sock
        .send_to(&buf, SocketAddr::from((DISCOVERY_GROUP, DISCOVERY_PORT)))
        .await;
    for iface in ifaces {
        if let Some(b) = iface.broadcast {
            let _ = sock
                .send_to(&buf, SocketAddr::from((b, DISCOVERY_PORT)))
                .await;
        }
    }
    let _ = sock
        .send_to(
            &buf,
            SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT)),
        )
        .await;
}

/// Send ONE `disc:false` tombstone for `my_id` from a throwaway socket —
/// best-effort, fire and forget (same egress fan-out as the announce loop).
/// WHY not the loop's own socket: this is called by `reset_identity` (M5.7)
/// moments before a restart, so the loop's next tick may never come — and
/// without a tombstone, peers would show the OLD Device ID as a ghost until
/// `PEER_EXPIRY` (or forever, if the fresh identity reuses the same name and
/// they never correlate the two).
pub async fn send_tombstone(my_id: String, name: String, port: u16) {
    let Ok(sock) = socket::build_send_only_socket() else {
        return; // best-effort: peers still expire the entry on their own
    };
    let pkt = DiscoveryPacket {
        v: PROTO_VERSION,
        id: my_id,
        port,
        name,
        disc: false,
        req: false,
        // A tombstone means "I'm leaving" — never advertise a share on it.
        http: None,
    };
    // Deliberately UNFILTERED (M5.6): the tombstone must reach every interface
    // we may EVER have announced on — the iface filter can have changed during
    // this run — and one extra broadcast moments before a restart costs nothing.
    // `confined = false` keeps the widest fan-out (multicast + directed +
    // limited broadcast) regardless of the current filter.
    send_all(&sock, &interfaces::enumerate(), &pkt, false).await;
}

async fn announce_loop(sock: Arc<tokio::net::UdpSocket>, ctx: DiscoveryCtx) {
    let mut ticker = tokio::time::interval(ANNOUNCE_INTERVAL);
    let mut was_visible = false;
    let mut first = true;
    loop {
        ticker.tick().await;
        // Filtered (M5.6): a changed/cleared filter lands here within one tick.
        // `confined` says whether the filter is actively narrowing egress, so
        // `send_all` can steer multicast to the kept interfaces and drop the
        // limited broadcast instead of leaking onto the excluded NIC.
        let (ifaces, confined) = active_interfaces(&ctx.settings);
        for iface in &ifaces {
            let _ = sock.join_multicast_v4(DISCOVERY_GROUP, iface.ip); // pick up new NICs
        }
        let (name, disc) = {
            // Poisoned settings lock: skip this announce tick rather than panic
            // the loop task — a poisoned lock degrades discovery for one tick,
            // it never terminates the announcer for the process lifetime. The
            // guard is read into locals inside this block so it is never held
            // across the `send_all` await below.
            let Ok(s) = ctx.settings.read() else {
                continue;
            };
            (s.device_name.clone(), s.discoverable)
        };
        let port = ctx.tcp_port.load(Ordering::Relaxed);
        if disc {
            let pkt = DiscoveryPacket {
                v: PROTO_VERSION,
                id: ctx.my_id.clone(),
                port,
                name,
                disc: true,
                req: first, // solicit the peer set once at startup
                // Advertise the browser-share port only while a share is live
                // (M8.3); omitted otherwise, keeping the packet pre-M8-identical.
                http: share_http_advert(&ctx),
            };
            send_all(&sock, &ifaces, &pkt, confined).await;
            was_visible = true;
            first = false;
        } else if was_visible {
            // one tombstone, then go quiet.
            let pkt = DiscoveryPacket {
                v: PROTO_VERSION,
                id: ctx.my_id.clone(),
                port,
                name,
                disc: false,
                req: false,
                http: None, // leaving — no share advertisement
            };
            send_all(&sock, &ifaces, &pkt, confined).await;
            was_visible = false;
        }
    }
}

async fn listen_loop(sock: Arc<tokio::net::UdpSocket>, ctx: DiscoveryCtx) {
    let mut buf = [0u8; 2048];
    loop {
        let (n, src) = match sock.recv_from(&mut buf).await {
            Ok(x) => x,
            Err(_) => continue,
        };
        let src_ip = match src {
            SocketAddr::V4(v4) => *v4.ip(),
            _ => continue,
        };
        let pkt: DiscoveryPacket = match serde_json::from_slice(&buf[..n]) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let solicited = pkt.req && pkt.v == PROTO_VERSION && pkt.id != ctx.my_id;
        let changed = match ctx.peers.lock() {
            Ok(mut peers) => apply_packet(&mut peers, &ctx.my_id, src_ip, &pkt),
            // Poisoned peers lock: skip applying this packet rather than kill the
            // listen loop — a poisoned lock degrades this tick, not the subsystem.
            Err(_) => false,
        };
        if solicited {
            let (name, disc) = match ctx.settings.read() {
                Ok(s) => (s.device_name.clone(), s.discoverable),
                // Poisoned settings lock: treat as not discoverable for this
                // reply (send none) instead of panicking the loop. The guard
                // drops with the match arm, never spanning the await below.
                Err(_) => (String::new(), false),
            };
            if disc {
                let reply = DiscoveryPacket {
                    v: PROTO_VERSION,
                    id: ctx.my_id.clone(),
                    port: ctx.tcp_port.load(Ordering::Relaxed),
                    name,
                    disc: true,
                    req: false,
                    // A solicited reply is a full announce — carry the share port
                    // if one is live, so the solicitor learns it at once (M8.3).
                    http: share_http_advert(&ctx),
                };
                if let Ok(b) = serde_json::to_vec(&reply) {
                    let _ = sock.send_to(&b, src).await; // unicast reply to the solicitor
                }
            }
        }
        if changed {
            emit_devices(&ctx);
        }
    }
}

async fn expiry_loop(ctx: DiscoveryCtx) {
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    loop {
        ticker.tick().await;
        let changed = match ctx.peers.lock() {
            Ok(mut peers) => {
                let now = Instant::now();
                let before = peers.len();
                peers.retain(|_, p| p.expires > now);
                peers.len() != before
            }
            // Poisoned peers lock: skip this expiry tick rather than kill the
            // loop — a poisoned lock degrades discovery, it never tears it down.
            Err(_) => false,
        };
        if changed {
            emit_devices(&ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(id: &str, name: &str, disc: bool) -> DiscoveryPacket {
        DiscoveryPacket {
            v: PROTO_VERSION,
            id: id.into(),
            port: 51704,
            name: name.into(),
            disc,
            req: false,
            http: None,
        }
    }

    #[test]
    fn packet_json_roundtrip() {
        let p = pkt("aBc-123", "Johns-PC", true);
        let bytes = serde_json::to_vec(&p).unwrap();
        let back: DiscoveryPacket = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.id, "aBc-123");
        assert_eq!(back.port, 51704);
        assert!(back.disc);
    }

    /// The M8.3 `http` field is ADDITIVE and OPTIONAL, both directions:
    /// - a LEGACY packet (no `http` key) deserializes with `http == None`, so an
    ///   old peer's datagram still round-trips;
    /// - a `None` packet omits the key entirely, so it is byte-identical to a
    ///   pre-M8 packet and a legacy peer never sees an unknown field;
    /// - a `Some(port)` packet round-trips the port, and every form stays far
    ///   under the 2048-byte discovery recv buffer.
    #[test]
    fn http_field_is_additive_and_optional() {
        // A legacy blob without the key → None (serde default), never an error.
        let legacy =
            br#"{"v":1,"id":"aBc-123","port":51704,"name":"Old PC","disc":true,"req":false}"#;
        let back: DiscoveryPacket = serde_json::from_slice(legacy).unwrap();
        assert_eq!(back.http, None, "a packet without http defaults to None");
        assert_eq!(back.id, "aBc-123");

        // A None packet omits the key (skip_serializing_if) — byte-compatible
        // with a pre-M8 announce.
        let none_json = serde_json::to_string(&pkt("id", "N", true)).unwrap();
        assert!(
            !none_json.contains("http"),
            "http is omitted when None: {none_json}"
        );

        // A Some(port) packet round-trips the advertised port.
        let mut adv = pkt("id2", "Sharer", true);
        adv.http = Some(52200);
        let bytes = serde_json::to_vec(&adv).unwrap();
        assert!(
            bytes.len() < 2048,
            "the packet stays under the recv buffer: {}",
            bytes.len()
        );
        let back: DiscoveryPacket = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            back.http,
            Some(52200),
            "the advertised share port round-trips"
        );
    }

    /// The `devices_updated` payload is the MERGED list, not discovery alone: a
    /// manual peer (M7.2 `connect_by_addr` / `join_by_code`) that discovery has
    /// never seen still appears, so a discovery change never evicts it from the
    /// UI. Tests the payload builder directly (no Tauri app needed).
    #[test]
    fn devices_payload_includes_manual_peer() {
        use crate::state::ManualPeer;
        let peers: Arc<Mutex<PeerTable>> = Arc::new(Mutex::new(PeerTable::new()));
        peers.lock().unwrap().insert(
            "disc1".into(),
            Peer {
                id: "disc1".into(),
                name: "Discovered".into(),
                addr: Ipv4Addr::new(192, 168, 1, 2),
                port: 51704,
                expires: Instant::now() + Duration::from_secs(60),
            },
        );
        let manual: Arc<Mutex<HashMap<String, ManualPeer>>> = Arc::new(Mutex::new(HashMap::new()));
        manual.lock().unwrap().insert(
            "man1".into(),
            ManualPeer {
                name: "Manual".into(),
                addr: Ipv4Addr::new(10, 0, 0, 9),
                port: 51704,
                last_used: Instant::now(),
            },
        );

        let payload = devices_payload(&peers, &manual);
        assert!(
            payload.iter().any(|d| d.device_id == "disc1"),
            "the discovered peer is present"
        );
        let man = payload
            .iter()
            .find(|d| d.device_id == "man1")
            .expect("the manual peer is merged into the devices_updated payload");
        assert_eq!(
            man.address, "10.0.0.9",
            "the manual peer keeps its dialed address"
        );
    }

    #[test]
    fn apply_packet_upsert_dedup_tombstone_self() {
        let mut peers = PeerTable::new();
        let ip = Ipv4Addr::new(192, 168, 1, 5);
        // new peer → changed
        assert!(apply_packet(&mut peers, "me", ip, &pkt("peerA", "A", true)));
        assert_eq!(peers.len(), 1);
        // identical → no change
        assert!(!apply_packet(
            &mut peers,
            "me",
            ip,
            &pkt("peerA", "A", true)
        ));
        // renamed → changed
        assert!(apply_packet(
            &mut peers,
            "me",
            ip,
            &pkt("peerA", "A2", true)
        ));
        // our own announce → ignored
        assert!(!apply_packet(
            &mut peers,
            "peerA",
            ip,
            &pkt("peerA", "A2", true)
        ));
        // tombstone → removed + changed
        assert!(apply_packet(
            &mut peers,
            "me",
            ip,
            &pkt("peerA", "A2", false)
        ));
        assert!(peers.is_empty());
    }

    // Real loopback discovery over multicast. Ignored by default (env-dependent);
    // run explicitly: `cargo test -- --ignored two_sockets`.
    #[tokio::test]
    #[ignore]
    async fn two_sockets_discover_over_multicast() {
        let (a, a_recv) = socket::build_discovery_socket().expect("socket a");
        let (b, b_recv) = socket::build_discovery_socket().expect("socket b");
        assert!(
            a_recv && b_recv,
            "both bind the discovery port via SO_REUSEADDR"
        );
        for iface in interfaces::enumerate() {
            let _ = a.join_multicast_v4(DISCOVERY_GROUP, iface.ip);
            let _ = b.join_multicast_v4(DISCOVERY_GROUP, iface.ip);
        }
        let announce = serde_json::to_vec(&pkt("peerA", "A", true)).unwrap();
        let recv = tokio::spawn(async move {
            let mut rb = [0u8; 2048];
            let r = tokio::time::timeout(Duration::from_secs(3), b.recv_from(&mut rb)).await;
            r.map(|res| res.map(|(n, _)| rb[..n].to_vec()))
        });
        for _ in 0..6 {
            let _ = a
                .send_to(
                    &announce,
                    SocketAddr::from((DISCOVERY_GROUP, DISCOVERY_PORT)),
                )
                .await;
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
        let got = recv
            .await
            .expect("join")
            .expect("timed out: no multicast received")
            .expect("recv error");
        let decoded: DiscoveryPacket = serde_json::from_slice(&got).unwrap();
        assert_eq!(decoded.id, "peerA");
    }

    /// A packet from a different protocol version is dropped whole — never
    /// inserted, never reported as a change (forward/backward compat guard).
    #[test]
    fn apply_packet_ignores_wrong_proto_version() {
        let mut peers = PeerTable::new();
        let ip = Ipv4Addr::new(192, 168, 1, 5);
        let mut stranger = pkt("peerA", "A", true);
        stranger.v = PROTO_VERSION + 1;
        assert!(!apply_packet(&mut peers, "me", ip, &stranger));
        assert!(peers.is_empty(), "an alien-version packet is never stored");
    }

    /// A tombstone for a peer we never tracked is a no-op — nothing to remove,
    /// so the visible set is unchanged.
    #[test]
    fn apply_packet_tombstone_for_unknown_peer_is_no_change() {
        let mut peers = PeerTable::new();
        let ip = Ipv4Addr::new(192, 168, 1, 5);
        assert!(!apply_packet(
            &mut peers,
            "me",
            ip,
            &pkt("ghost", "Ghost", false)
        ));
        assert!(peers.is_empty());
    }

    /// A re-announce that keeps id/name/addr but moves the TCP port is a visible
    /// change (the port flows into the dial address the UI shows).
    #[test]
    fn apply_packet_port_change_is_change() {
        let mut peers = PeerTable::new();
        let ip = Ipv4Addr::new(192, 168, 1, 5);
        assert!(apply_packet(&mut peers, "me", ip, &pkt("peerA", "A", true)));
        let mut moved = pkt("peerA", "A", true);
        moved.port = 51705;
        assert!(apply_packet(&mut peers, "me", ip, &moved));
        assert_eq!(peers.get("peerA").unwrap().port, 51705);
    }

    /// The same peer seen from a new source IP (DHCP renumber, NIC switch) is a
    /// visible change, and the stored addr is the UDP source — never the packet.
    #[test]
    fn apply_packet_addr_change_is_change() {
        let mut peers = PeerTable::new();
        let ip1 = Ipv4Addr::new(192, 168, 1, 5);
        let ip2 = Ipv4Addr::new(192, 168, 1, 6);
        assert!(apply_packet(
            &mut peers,
            "me",
            ip1,
            &pkt("peerA", "A", true)
        ));
        assert!(apply_packet(
            &mut peers,
            "me",
            ip2,
            &pkt("peerA", "A", true)
        ));
        assert_eq!(peers.get("peerA").unwrap().addr, ip2);
    }

    /// The peer-supplied name is trimmed and clamped at the trust boundary
    /// before it is stored — an over-long announce name never lands unbounded
    /// in the device list.
    #[test]
    fn apply_packet_trims_and_clamps_stored_name() {
        let mut peers = PeerTable::new();
        let ip = Ipv4Addr::new(192, 168, 1, 5);
        let long: String = "x".repeat(200);
        let padded = format!("  {long}  ");
        apply_packet(&mut peers, "me", ip, &pkt("peerA", &padded, true));
        let stored = &peers.get("peerA").unwrap().name;
        assert!(
            !stored.starts_with(' ') && !stored.ends_with(' '),
            "trimmed"
        );
        assert!(
            stored.chars().count() <= 63,
            "clamped to the name cap, was {}",
            stored.chars().count()
        );
    }

    /// Default settings carry no `iface_filter`, so discovery is NOT confined —
    /// egress fans out over every interface (the pre-filter default).
    #[test]
    fn active_interfaces_default_is_not_confined() {
        let settings = Arc::new(RwLock::new(Settings::default()));
        let (_kept, confined) = active_interfaces(&settings);
        assert!(!confined, "no filter set means egress is never confined");
    }

    /// A filter that is not a valid IPv4 literal parses to `None`, so it cannot
    /// confine — discovery stays on all interfaces.
    #[test]
    fn active_interfaces_unparseable_filter_is_not_confined() {
        let s = Settings {
            iface_filter: Some("not-an-ip".into()),
            ..Default::default()
        };
        let settings = Arc::new(RwLock::new(s));
        let (_kept, confined) = active_interfaces(&settings);
        assert!(
            !confined,
            "an unparseable filter is ignored, never confining"
        );
    }

    /// A syntactically valid but stale filter (an IP no live NIC has — here a
    /// TEST-NET-3 address) falls back to every interface and reports NOT
    /// confined, so a stale filter can never strand or steer egress.
    #[test]
    fn active_interfaces_stale_filter_falls_back_not_confined() {
        let s = Settings {
            iface_filter: Some("203.0.113.99".into()),
            ..Default::default()
        };
        let settings = Arc::new(RwLock::new(s));
        let (kept, confined) = active_interfaces(&settings);
        assert!(!confined, "a stale filter must not confine egress");
        assert_eq!(
            kept.len(),
            interfaces::enumerate().len(),
            "the fallback keeps the full enumeration"
        );
    }

    /// A filter matching a live interface confines egress to exactly that one.
    /// Environment-guarded: if the box has no non-loopback IPv4 NIC, there is
    /// nothing to match, so the confining branch is simply not exercised here.
    #[test]
    fn active_interfaces_matching_filter_confines() {
        let all = interfaces::enumerate();
        let Some(first) = all.first().copied() else {
            return; // no NIC to match against on this host
        };
        let s = Settings {
            iface_filter: Some(first.ip.to_string()),
            ..Default::default()
        };
        let settings = Arc::new(RwLock::new(s));
        let (kept, confined) = active_interfaces(&settings);
        assert!(confined, "a live-matching filter confines egress");
        assert!(
            kept.iter().all(|i| i.ip == first.ip),
            "only the selected interface is kept"
        );
    }
}
