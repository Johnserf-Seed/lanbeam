//! Tauri commands (UI → Rust). M1: identity + settings.
//! `camelCase` on the JS side is auto-converted to `snake_case` params here.

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::consts::DEFAULT_TCP_PORT;
use crate::discovery::{self, DiscoveredDevice};
use crate::error::LanBeamError;
use crate::settings;
use crate::state::{
    AppState, ConflictAction, ManualPeer, NetDegraded, PairingSession, ReplyDecision,
};
use crate::trust::{self, TrustedPeerDto};
use crate::{identity, share, transfer, transport};

#[tauri::command]
pub fn list_discovered_devices(state: State<'_, AppState>) -> Vec<DiscoveredDevice> {
    list_devices_snapshot(&state)
}

/// The devices-page list: the live discovery snapshot with manually-added peers
/// (M7.2) merged in behind it. One source of truth for both the command and the
/// `devices_updated` emit `connect_by_addr` fires.
fn list_devices_snapshot(state: &AppState) -> Vec<DiscoveredDevice> {
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
fn resolve_peer(
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

// ── network info (M5.1) ────────────────────────────────────────────────

/// One local IPv4 endpoint as the sidebar/settings page shows it.
#[derive(Serialize)]
pub struct NetworkInfo {
    pub ip: String,
    /// `null` for interfaces without a broadcast address (e.g. P2P links).
    pub broadcast: Option<String>,
}

/// The machine's non-loopback IPv4 addresses — the same enumeration
/// discovery announces on. Sorted numerically because OS enumeration order
/// can reshuffle between calls, and the UI list must not jump around.
#[tauri::command]
pub fn get_network_info() -> Vec<NetworkInfo> {
    let mut ifaces = discovery::interfaces::enumerate();
    ifaces.sort_by_key(|i| i.ip);
    ifaces
        .into_iter()
        .map(|i| NetworkInfo {
            ip: i.ip.to_string(),
            broadcast: i.broadcast.map(|b| b.to_string()),
        })
        .collect()
}

/// The TCP port the transfer listener is bound to right now — the port peers
/// dial and firewall rules should target. It can differ from the `port`
/// setting, which only takes effect on the next launch.
#[tauri::command]
pub fn get_listen_port(state: State<'_, AppState>) -> u16 {
    state.tcp_port.load(Ordering::Relaxed)
}

/// Open an authenticated Noise channel to a discovered peer, pinning its Device ID.
/// Returns the 6-digit SAS; emits `sas_code`.
#[tauri::command]
pub async fn connect_device(
    app: AppHandle,
    state: State<'_, AppState>,
    device_id: String,
) -> Result<String, LanBeamError> {
    let (addr, port, expected) = resolve_peer(&state, &device_id)?;
    let local_private = *state.identity.private_bytes();
    let mut sess = transport::connect(addr, port, &local_private, expected).await?;
    let sas = sess.sas().to_string();
    // Real opening dialogue instead of M2's plaintext probe (which the peer's
    // decoder rejected as an unknown frame kind): introduce ourselves, read
    // the peer's introduction, then bow out with a deliberate `Bye` that the
    // responder swallows silently (M4.3). The SAS is only returned once that
    // round-trip proves the channel speaks the real protocol.
    let peer =
        transfer::initiator_hello(&mut sess, transfer::local_device_name(&state.settings)).await?;
    transfer::send_bye(&mut sess).await?;
    log::info!(
        "connect_device: {device_id} verified (proto v{}, name {:?})",
        peer.version,
        peer.name
    );
    let _ = app.emit(
        "sas_code",
        serde_json::json!({ "deviceId": device_id, "sas": sas }),
    );
    Ok(sas)
}

// ── pairing + IP direct connect (M7.1/7.2) ─────────────────────────────

/// A fresh pairing invitation: the 6-digit code plus a QR/deep-link payload
/// another device can scan to join.
#[derive(Serialize)]
pub struct PairingInvite {
    pub code: String,
    pub qr: String,
}

/// The peer a pairing or IP-direct handshake reached: its Device ID, friendly
/// name, and the 6-digit SAS to compare out of band.
#[derive(Serialize)]
pub struct ConnectResult {
    #[serde(rename = "deviceId")]
    pub device_id: String,
    pub name: String,
    pub sas: String,
}

/// Start pairing: show a fresh 6-digit code (valid for 10 minutes) plus a QR
/// payload another device can scan. Only one code is active at a time — calling
/// this again replaces the previous one. Pair by entering the code (or scanning
/// the QR) on the other device; cancel it with `cancel_pairing`.
#[tauri::command]
pub fn start_pairing(state: State<'_, AppState>) -> PairingInvite {
    let code = fresh_pairing_code();
    let expires = Instant::now() + crate::consts::PAIRING_CODE_TTL;
    // Single-active: replace whatever was minted before.
    if let Ok(mut slot) = state.pairing.session.lock() {
        *slot = Some(PairingSession {
            code: code.clone(),
            expires,
        });
    }
    let device_id = state.identity.device_id();
    let (name, filter) = state
        .settings
        .read()
        .map(|s| {
            let filter = s
                .iface_filter
                .as_deref()
                .and_then(|v| v.parse::<Ipv4Addr>().ok());
            (s.device_name.clone(), filter)
        })
        .unwrap_or_default();
    let ip = first_lan_ip(filter);
    let port = state.tcp_port.load(Ordering::Relaxed);
    let qr = build_pair_uri(&device_id, &name, ip, port, &code);
    PairingInvite { code, qr }
}

/// Cancel the active pairing code so it can no longer be redeemed. Safe to call
/// when no pairing is in progress.
#[tauri::command]
pub fn cancel_pairing(state: State<'_, AppState>) {
    if let Ok(mut slot) = state.pairing.session.lock() {
        *slot = None;
    }
}

/// Take the pairing link this app was launched with (a cold-start `lanbeam://`
/// deep link), or `null` if it was launched normally. Returns the link once and
/// clears it, so a later reload starts clean. The webview calls this on mount to
/// open the pairing form for a link that arrived before it could listen; a link
/// that arrives while the app is already running comes through the `deep_link`
/// event instead.
#[tauri::command]
pub fn take_pending_deep_link(state: State<'_, AppState>) -> Option<String> {
    state
        .pending_deep_link
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
}

/// Redeem a device that is showing a pairing code. Pass its address (`ip:port`,
/// or the scanned `lanbeam://pair…` link) and the 6-digit code; returns the
/// peer's Device ID, name, and the SAS. A wrong or expired code errors.
///
/// This GRANTS NO TRUST, by design. The SAS is the only thing standing between a
/// pairing and a machine-in-the-middle, and a code nobody read is not a check —
/// so trust is the caller's to record with `set_trusted`, and only after a human
/// has compared this SAS against the one on the other device's screen. Both ends
/// derive the same SAS from the handshake and both must confirm it independently.
#[tauri::command]
pub async fn join_by_code(
    state: State<'_, AppState>,
    addr: String,
    code: String,
) -> Result<ConnectResult, LanBeamError> {
    let target = parse_dial_target(&addr)
        .ok_or_else(|| LanBeamError::Protocol(format!("not a valid address: {addr}")))?;
    // Prefer an explicitly-typed code; fall back to one carried in a scanned link.
    let code = pick_code(code, target.code);
    if code.is_empty() {
        return Err(LanBeamError::Protocol("no pairing code provided".into()));
    }
    let local_private = *state.identity.private_bytes();
    // TOFU: no pinned key — the identity is learned from the handshake and the
    // returned SAS is the out-of-band MITM check the UI surfaces.
    let mut sess = transport::connect_unpinned(target.ip, target.port, &local_private).await?;
    let sas = sess.sas().to_string();
    let device_id = URL_SAFE_NO_PAD.encode(sess.remote_static());
    // Pairing with yourself is not a pairing. Bail before the manual table (below)
    // files this machine as its own peer — an entry discovery can never expire.
    if device_id == state.identity.device_id() {
        return Err(LanBeamError::SelfPeer);
    }
    let my_name = transfer::local_device_name(&state.settings);
    let peer = transfer::initiator_hello(&mut sess, my_name).await?;
    // Pairing is a v2 capability: a peer that only speaks v1 cannot redeem a code.
    if peer.version < crate::consts::TRANSFER_V2 {
        return Err(LanBeamError::PeerTooOld("peer too old for pairing".into()));
    }
    transfer::send_pair_request(&mut sess, &code).await?;
    let (accept, peer_name, reason) = transfer::recv_pair_confirm(&mut sess).await?;
    if !accept {
        // Surface the responder's reason (wrong/expired code) to the UI.
        return Err(LanBeamError::Protocol(
            reason.unwrap_or_else(|| "pairing rejected".into()),
        ));
    }
    let name = if peer_name.trim().is_empty() {
        device_id.chars().take(8).collect()
    } else {
        peer_name
    };
    // NO trust is granted here — the UI records it once the human has compared
    // the SAS below. See this command's doc comment.
    //
    // Remember the dial target though, exactly as `connect_by_addr` does. Pairing by
    // code/IP is the case where discovery CANNOT see the peer (a different subnet,
    // or discovery off), so without this the just-trusted device would be absent
    // from `list_discovered_devices` and `resolve_peer` — a trusted peer no later
    // `send_text`/`send_files` could reach (PeerNotFound). The manual table keeps
    // it visible and dialable, independent of the discovery table's expiry.
    if let Ok(mut manual) = state.manual_peers.lock() {
        crate::state::insert_manual_peer(
            &mut manual,
            device_id.clone(),
            ManualPeer {
                name: name.clone(),
                addr: target.ip,
                port: target.port,
                last_used: Instant::now(),
            },
        );
    }
    Ok(ConnectResult {
        device_id,
        name,
        sas,
    })
}

/// Connect to a device by address (`ip:port`, or a scanned `lanbeam://pair…`
/// link) so it appears on the devices page even when it is not visible through
/// automatic discovery. Returns the peer's Device ID, name, and the SAS to
/// compare out loud. This sends nothing and grants no trust — use it to add a
/// device you can then send to or pair with. Re-query `list_discovered_devices`
/// after this resolves to pick the added device up.
#[tauri::command]
pub async fn connect_by_addr(
    state: State<'_, AppState>,
    addr: String,
) -> Result<ConnectResult, LanBeamError> {
    let target = parse_dial_target(&addr)
        .ok_or_else(|| LanBeamError::Protocol(format!("not a valid address: {addr}")))?;
    let local_private = *state.identity.private_bytes();
    let mut sess = transport::connect_unpinned(target.ip, target.port, &local_private).await?;
    let sas = sess.sas().to_string();
    let device_id = URL_SAFE_NO_PAD.encode(sess.remote_static());
    // Adding yourself is not adding a device. Bail before the manual table (below)
    // files this machine as its own peer — an entry discovery can never expire.
    if device_id == state.identity.device_id() {
        return Err(LanBeamError::SelfPeer);
    }
    let my_name = transfer::local_device_name(&state.settings);
    let peer = transfer::initiator_hello(&mut sess, my_name).await?;
    // Bow out cleanly (like connect_device): the responder swallows the Bye.
    transfer::send_bye(&mut sess).await?;
    let name = peer
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(String::from)
        .unwrap_or_else(|| device_id.chars().take(8).collect());
    // Remember it so `list_discovered_devices` shows it and a later send can
    // reach it — discovery never announced it (a different subnet, or discovery
    // off on that box), so it lives in the manual table rather than the
    // discovery table (which the expiry loop would evict).
    if let Ok(mut manual) = state.manual_peers.lock() {
        crate::state::insert_manual_peer(
            &mut manual,
            device_id.clone(),
            ManualPeer {
                name: name.clone(),
                addr: target.ip,
                port: target.port,
                last_used: Instant::now(),
            },
        );
    }
    Ok(ConnectResult {
        device_id,
        name,
        sas,
    })
}

/// A parsed dial target from `join_by_code` / `connect_by_addr`: the IPv4
/// endpoint plus any pairing code embedded in a scanned link.
struct DialTarget {
    ip: Ipv4Addr,
    port: u16,
    /// The `c=` code from a `lanbeam://pair` link, if present.
    code: Option<String>,
}

/// Parse a dial target from either a plain `ip[:port]` or a `lanbeam://pair?…`
/// deep link. `None` for anything that does not yield a valid IPv4 address.
/// `port` defaults to LanBeam's default TCP port when absent.
fn parse_dial_target(input: &str) -> Option<DialTarget> {
    let s = input.trim();
    if let Some(rest) = s.strip_prefix("lanbeam://") {
        return parse_pair_uri(rest);
    }
    // Plain `ip` or `ip:port`. IPv4 has no internal colons, so the last colon
    // (if any) separates the port.
    let (host, port) = match s.rsplit_once(':') {
        Some((h, p)) => (h.trim(), p.trim().parse::<u16>().ok()?),
        None => (s, DEFAULT_TCP_PORT),
    };
    let ip: Ipv4Addr = host.parse().ok()?;
    Some(DialTarget {
        ip,
        port,
        code: None,
    })
}

/// Parse the `pair?…` tail of a `lanbeam://pair` link into a dial target.
/// Reads `a` (IPv4), `p` (port), `c` (code); other keys (`d`, `n`) are ignored
/// here — the joiner learns the Device ID from the handshake, not the link.
fn parse_pair_uri(rest: &str) -> Option<DialTarget> {
    let query = rest.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut ip: Option<Ipv4Addr> = None;
    let mut port = DEFAULT_TCP_PORT;
    let mut code: Option<String> = None;
    for pair in query.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        match k {
            "a" => ip = percent_decode(v).parse().ok(),
            "p" => {
                if let Ok(p) = v.parse() {
                    port = p;
                }
            }
            "c" => {
                let c = percent_decode(v);
                if !c.is_empty() {
                    code = Some(c);
                }
            }
            _ => {}
        }
    }
    ip.map(|ip| DialTarget { ip, port, code })
}

/// Build the QR/deep-link payload `start_pairing` returns:
/// `lanbeam://pair?d=<deviceId>&n=<urlenc name>&a=<ip>&p=<port>&c=<code>`.
/// `a=` is omitted when the machine has no non-loopback address.
fn build_pair_uri(
    device_id: &str,
    name: &str,
    ip: Option<Ipv4Addr>,
    port: u16,
    code: &str,
) -> String {
    let mut s = format!("lanbeam://pair?d={}", percent_encode(device_id));
    s.push_str(&format!("&n={}", percent_encode(name)));
    if let Some(ip) = ip {
        s.push_str(&format!("&a={ip}"));
    }
    s.push_str(&format!("&p={port}"));
    s.push_str(&format!("&c={code}"));
    s
}

/// Percent-encode a string for a URL query value, keeping the RFC 3986
/// unreserved set (`A–Z a–z 0–9 - _ . ~`) verbatim.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Reverse [`percent_encode`]; also decodes `+` to space (form encoding).
/// Lossy on malformed input — a stray `%` that is not followed by two hex
/// digits keeps its literal byte.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(hi), Some(lo)) => {
                        out.push((hi * 16 + lo) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A fresh random 6-digit pairing code. Sourced from `uuid` v4's OS CSPRNG (no
/// new RNG dependency); the modulo bias over 128 random bits is negligible for
/// a short-lived, rate-limited code.
fn fresh_pairing_code() -> String {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let n = u64::from_be_bytes(bytes[..8].try_into().unwrap()) % 1_000_000;
    format!("{n:06}")
}

/// Best non-loopback IPv4 to advertise as the pairing link's `a=` hint, or
/// `None` when the machine is link-down. Honors the user's interface filter
/// (M5.6) first; otherwise ranks interfaces so a real LAN address beats a
/// virtual adapter (see [`lan_ip_rank`]). Ties break on the numeric IP so the
/// choice is stable across calls.
fn first_lan_ip(filter: Option<Ipv4Addr>) -> Option<Ipv4Addr> {
    let all = discovery::interfaces::enumerate();
    let (ifaces, _fell_back) = discovery::interfaces::select(all, filter);
    best_lan_ip(ifaces)
}

/// Pick the best-ranked IP from an already-selected interface set. Split from
/// [`first_lan_ip`] so the ranking is unit-testable without real NICs.
fn best_lan_ip(mut ifaces: Vec<discovery::interfaces::Iface>) -> Option<Ipv4Addr> {
    ifaces.sort_by_key(|i| (lan_ip_rank(i.ip), i.ip));
    ifaces.first().map(|i| i.ip)
}

/// Rank an IPv4 for "most likely reachable from a LAN peer" (lower = better).
/// The pairing link advertises exactly ONE address, and a box running WSL,
/// Hyper-V or Docker carries a virtual adapter — those default to 172.16/12 —
/// next to the real NIC. Demoting 172.16/12 and APIPA (169.254/16) below the
/// ordinary 192.168/16 and 10/8 ranges makes the reachable address win when
/// both exist. When a virtual/APIPA range is all the machine has it is still
/// returned (the only option), and a genuine 172.x LAN with no 192.168/10 also
/// still wins.
fn lan_ip_rank(ip: Ipv4Addr) -> u8 {
    let [a, b, ..] = ip.octets();
    match (a, b) {
        (169, 254) => 3,           // APIPA link-local: never routable
        (172, 16..=31) => 2,       // private, but the Docker / WSL / Hyper-V default
        (192, 168) | (10, _) => 0, // the common home / corporate LAN ranges
        _ => 1,                    // public or unusual — above virtual, below real-private
    }
}

/// Choose the pairing code: the explicitly-entered one wins; a code carried in
/// a scanned link is the fallback. Trimmed; an empty result means "none given".
fn pick_code(entered: String, from_link: Option<String>) -> String {
    let entered = entered.trim().to_string();
    if !entered.is_empty() {
        entered
    } else {
        from_link.unwrap_or_default()
    }
}

// ── quick text (M7.3) ──────────────────────────────────────────────────

/// Send a short text or link to a device. Connects to the peer and delivers the
/// text, resolving only once the peer confirms it received it. `alsoClipboard`
/// asks the receiver to also place the text on their clipboard — whether it
/// actually lands there is the receiver's choice (their clipboard-sharing
/// setting must allow it). Errors if the text is empty or too long, the device
/// is unknown, or the peer is too old to receive text.
#[tauri::command]
pub async fn send_text(
    state: State<'_, AppState>,
    device_id: String,
    text: String,
    also_clipboard: bool,
) -> Result<(), LanBeamError> {
    // Reject empty / oversized input BEFORE opening a socket — the size cap
    // mirrors the receiver's clamp so a text that would be truncated on arrival
    // is refused here instead (M7.3).
    if text.trim().is_empty() {
        return Err(LanBeamError::Protocol("no text to send".into()));
    }
    if text.len() > crate::consts::MAX_TEXT_BYTES {
        return Err(LanBeamError::Protocol("text is too long".into()));
    }
    let (addr, port, expected) = resolve_peer(&state, &device_id)?;
    let local_private = *state.identity.private_bytes();
    let mut session = transport::connect(addr, port, &local_private, expected).await?;
    let my_name = transfer::local_device_name(&state.settings);
    let peer = transfer::initiator_hello(&mut session, my_name).await?;
    // Quick text is a v2 capability (M7.3): a peer that only speaks v1 (M4–M6)
    // must never be handed the variant, so refuse cleanly before sending.
    if peer.version < crate::consts::TRANSFER_V2 {
        return Err(LanBeamError::PeerTooOld(
            "peer too old for quick text".into(),
        ));
    }
    // The receiver acks after emitting the text, so this resolves on delivery.
    transfer::send_text(&mut session, text, Some(also_clipboard)).await
}

/// Send files/folders to a device. Returns the transfer id; progress and
/// completion are reported via `transfer_*` events. `stripExif` removes photo
/// metadata (location, camera, time) from JPEG/PNG/WebP images before they leave
/// this device — the per-send override the confirm dialog offers.
#[tauri::command]
pub async fn send_files(
    app: AppHandle,
    state: State<'_, AppState>,
    device_id: String,
    paths: Vec<String>,
    strip_exif: bool,
) -> Result<String, LanBeamError> {
    let (addr, port, expected) = resolve_peer(&state, &device_id)?;
    // One id up front: it names the cancel/pause controls, the events, AND the
    // per-transfer scratch dir the metadata strip writes temp copies into.
    let transfer_id = uuid::Uuid::new_v4().to_string();
    // Snapshot the integrity setting NOW (M6.3): if on, the build hashes every
    // file, so the whole build — the directory walk, the hashing, and the M9.1
    // metadata strip — runs on the blocking pool to keep large reads/writes off
    // the async runtime.
    let verify_hash = state.settings.read().map(|s| s.verify_hash).unwrap_or(true);
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    // Per-transfer scratch dir for stripped temp copies (M9.1): app-private
    // cache, NOT the download dir, and its own subfolder so concurrent sends
    // never collide. The returned guard deletes it (and every temp) on drop.
    let scratch_base = app
        .path()
        .app_cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("lanbeam")
        .join("send-scratch")
        .join(&transfer_id);
    let (items, _scratch) = tauri::async_runtime::spawn_blocking(move || {
        transfer::build_send_list_scoped(&paths, verify_hash, strip_exif, scratch_base)
    })
    .await
    .map_err(|e| LanBeamError::Io(format!("send list build task failed: {e}")))??;
    // `_scratch` lives to the end of this command (all exit paths below), so the
    // stripped temp copies are cleaned up whether the send succeeds, errors, or
    // is cancelled — a temp copy of the user's photo never lingers on disk.
    if items.is_empty() {
        return Err(LanBeamError::Protocol("no files to send".into()));
    }
    let total: u64 = items.iter().map(|i| i.meta.size).sum();

    // Register live cancel/pause controls (M6.1/6.2) keyed by this session id.
    // The guard removes them on every exit path below, so cancel/pause/resume
    // of a finished id is a graceful no-op. Registered before the dial so a
    // cancel during setup is honored at the first chunk.
    let (_ctl_guard, ctl) =
        crate::state::TransferCtlGuard::register(&state.transfers_ctl, &transfer_id);

    // Concurrency gate (M6.7): cap simultaneously-streaming transfers across both
    // directions (the receive side draws from the same gate). Acquired BEFORE the
    // dial, so a queued send opens no socket until a slot frees; a lowered cap
    // applies to sends not yet past this gate. The rate cap is read here too,
    // from one settings read, and rides into run_send below.
    let (limit, rate_limit) = state
        .settings
        .read()
        .map(|s| {
            (
                settings::clamp_max_concurrent(s.max_concurrent),
                settings::rate_limit_bytes_per_sec(&s.rate_limit),
            )
        })
        .unwrap_or((settings::clamp_max_concurrent(0), None));
    let _slot = match state.concurrency.try_acquire(limit) {
        Some(guard) => guard,
        None => {
            let _ = app.emit(
                "transfer_queued",
                serde_json::json!({ "sessionId": transfer_id, "direction": "send" }),
            );
            // A cancel issued while QUEUED must win over the wait: without this
            // biased select the acquire ignores the already-registered cancel
            // token, so a cancelled send stays parked and only aborts once a slot
            // frees — after dialing/prompting the peer. Emit the error (mirroring
            // the res-error path below) so the UI resolves, then bail before any
            // socket is opened.
            tokio::select! {
                biased;
                _ = ctl.cancel.cancelled() => {
                    let _ = app.emit(
                        "transfer_error",
                        serde_json::json!({ "sessionId": transfer_id, "error": LanBeamError::Cancelled.to_string(), "code": LanBeamError::Cancelled.ui_code() }),
                    );
                    return Err(LanBeamError::Cancelled);
                }
                g = state.concurrency.acquire(limit) => g,
            }
        }
    };

    let local_private = *state.identity.private_bytes();
    // A dial/handshake failure here must emit a terminal `transfer_error` for
    // this session id: when the concurrency gate was full we already announced
    // `transfer_queued {sessionId}`, so the UI holds a queued row keyed by it —
    // without this it would dangle forever (the peer went offline while queued,
    // the handshake timed out). Mirrors the cancel-while-queued branch above and
    // the post-`transfer_started` error path below. In the non-queued path the
    // frontend never learned this id (it only learns it on Ok), so the extra
    // event has no row to resolve and is harmless.
    let mut session = match transport::connect(addr, port, &local_private, expected).await {
        Ok(s) => s,
        Err(e) => {
            let _ = app.emit(
                "transfer_error",
                serde_json::json!({ "sessionId": transfer_id, "error": e.to_string(), "code": e.ui_code() }),
            );
            return Err(e);
        }
    };

    // [FIX-2] Show the SAS on the sender so the user can compare it against the
    // code the receiver's accept prompt shows — matching codes ⇒ no man-in-the-middle.
    let _ = app.emit(
        "sas_code",
        serde_json::json!({ "sessionId": transfer_id, "sas": session.sas(), "deviceId": device_id }),
    );
    let _ = app.emit(
        "transfer_started",
        serde_json::json!({ "sessionId": transfer_id, "direction": "send", "totalSize": total, "fileCount": items.len() }),
    );

    let app2 = app.clone();
    let sid = transfer_id.clone();
    let mut last_pct = -1i64;
    let my_name = transfer::local_device_name(&state.settings);
    // Per-file progress/completion events (M6.8), shared impl with the receive side.
    let mut on_file = transfer::file_event_sink(app.clone(), transfer_id.clone());
    let res = async {
        // Introduce ourselves and learn the peer's version BEFORE the manifest
        // (M4.2) — the receiver shows our friendly name on its accept prompt,
        // and future capabilities gate on the negotiated version. A peer that
        // drops the exchange surfaces as `peer_too_old` below.
        let peer = transfer::initiator_hello(&mut session, my_name).await?;
        log::debug!(
            "peer {device_id} answered hello (proto v{}, name {:?})",
            peer.version,
            peer.name
        );
        transfer::run_send(
            &mut session,
            &transfer_id,
            &items,
            &ctl,
            rate_limit,
            &mut on_file,
            move |sent, total| {
                transfer::emit_progress(&app2, &sid, "send", sent, total, &mut last_pct);
            },
        )
        .await
    }
    .await;

    match res {
        Ok(()) => {
            let _ = app.emit(
                "transfer_done",
                serde_json::json!({ "sessionId": transfer_id, "direction": "send" }),
            );
            Ok(transfer_id)
        }
        Err(e) => {
            // `code` distinguishes the peer politely declining ("declined") from
            // genuine failures ("timeout"/"io"/"protocol") — the `error` string
            // is unchanged and stays non-contractual (M4.5).
            let _ = app.emit(
                "transfer_error",
                serde_json::json!({ "sessionId": transfer_id, "error": e.to_string(), "code": e.ui_code() }),
            );
            Err(e)
        }
    }
}

/// Answer an incoming file request (fires the receive task's awaited oneshot).
/// Removal is deliberately unconditional — whatever prompt is parked under
/// this id right now is the one the user just answered; the parking session's
/// own cleanup is generation-checked instead (see `transfer::park_pending`).
///
/// `conflict` (M6.5) is the ConflictModal's choice — `"rename"` or `"overwrite"`
/// — folded into the SAME reply so there is one ordered answer per prompt. It is
/// consulted only when the receive-side `conflict` policy is `"ask"` and the
/// transfer actually collides; anything else (including a bare accept from a
/// frontend that predates the modal) leaves it `None`, and the receiver falls
/// back to the safe rename — it never overwrites without this explicit choice.
#[tauri::command]
pub fn reply_file_request(
    state: State<'_, AppState>,
    session_id: String,
    accept: bool,
    conflict: Option<String>,
) {
    let conflict = conflict.as_deref().and_then(ConflictAction::parse);
    // Take the sender out and DROP the pending guard BEFORE sending: the oneshot
    // send never runs under the lock (a latent footgun if that path ever grows),
    // and a poisoned lock is a graceful no-op — same discipline as `park_pending`
    // / `remove_pending_if_owner`, not a panic on the accept/decline command.
    let taken = {
        let Ok(mut pending) = state.pending.lock() else {
            return;
        };
        pending.remove(&session_id)
    };
    if let Some((_, tx)) = taken {
        let _ = tx.send(ReplyDecision { accept, conflict });
    }
}

/// One interrupted file still on disk, waiting to be resumed or discarded.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialDto {
    pub device_id: String,
    /// The file's name as it appears in the download folder.
    pub name: String,
    /// Bytes already on disk.
    pub written: u64,
    /// What the whole file will be.
    pub size: u64,
}

/// Every interrupted receive still holding bytes on disk.
///
/// This exists because a half-written file is INVISIBLE. It is saved under its
/// FINAL name — `holiday.mp4`, 1.2 GB of a 4 GB file — so the download folder
/// shows something that looks perfectly normal and plays for thirty seconds. The
/// backend has known about these all along (it is what makes resume work) and
/// `discard_partials` could already clean them up; nothing ever asked it what it
/// was holding, so the UI could neither show them nor offer to.
#[tauri::command]
pub fn list_partials(state: State<'_, AppState>) -> Vec<PartialDto> {
    let Ok(store) = state.partials.read() else {
        return Vec::new();
    };
    store
        .entries()
        .into_iter()
        .map(|(device_id, rec)| PartialDto {
            device_id,
            // `disk_rel` is where the bytes actually are (organize-by-date/device
            // folds a prefix in); its leaf is what the user sees in the folder.
            name: rec
                .disk_rel
                .rsplit('/')
                .next()
                .unwrap_or(&rec.rel)
                .to_string(),
            written: rec.bytes_written,
            size: rec.size,
        })
        .collect()
}

/// Discard the persisted resume state for a peer (M6.4): forget every partial
/// LanBeam kept for `device_id` and delete the half-written files from disk.
/// The counterpart to letting an interrupted transfer resume — the user chooses
/// to start clean instead. A no-op for a peer with no partials.
#[tauri::command]
pub fn discard_partials(state: State<'_, AppState>, device_id: String) {
    // Take the records out under the write guard, snapshot, then do disk I/O and
    // persist AFTER the guard drops — the trust-store pattern (no I/O under lock).
    let (removed, snapshot) = {
        let Ok(mut store) = state.partials.write() else {
            return;
        };
        let removed = store.clear_device(&device_id);
        if removed.is_empty() {
            return; // nothing to discard — no save
        }
        (removed, store.snapshot())
    };
    // Snapshot the download root to resolve each partial's on-disk path; never
    // hold the settings/dir lock across the deletes.
    let root = state.download_dir.read().ok().map(|d| d.clone());
    if let Some(root) = root {
        for rec in &removed {
            let _ = std::fs::remove_file(root.join(&rec.disk_rel));
        }
    }
    crate::partials::persist_async(snapshot);
}

// ── transfer control (M6.1/6.2) ────────────────────────────────────────

/// Cancel an in-flight transfer by its session id. Works in either direction
/// and ends the transfer promptly on both peers. Always safe to call: a no-op
/// if the id is unknown (the transfer already finished or never started).
#[tauri::command]
pub fn cancel_transfer(state: State<'_, AppState>, session_id: String) {
    // Trip the session's cancel token; the chunk loop returns early and the
    // dropped connection makes the peer fail through its normal error path.
    crate::state::cancel_transfer_ctl(&state.transfers_ctl, &session_id);
}

/// Pause an in-flight transfer by its session id. The transfer holds where it
/// is until `resume_transfer` is called for the same id. A no-op if the id is
/// unknown.
#[tauri::command]
pub fn pause_transfer(state: State<'_, AppState>, session_id: String) {
    // Session-local: the loop stops moving bytes → TCP backpressure stalls the
    // peer, with no protocol message sent.
    crate::state::set_transfer_paused_ctl(&state.transfers_ctl, &session_id, true);
}

/// Resume a transfer previously paused with `pause_transfer`. A no-op if the id
/// is unknown or the transfer was not paused.
#[tauri::command]
pub fn resume_transfer(state: State<'_, AppState>, session_id: String) {
    crate::state::set_transfer_paused_ctl(&state.transfers_ctl, &session_id, false);
}

#[tauri::command]
pub fn get_download_dir(state: State<'_, AppState>) -> String {
    // Poisoned lock: an empty string beats panicking the invoke — same
    // contract as `list_trusted`.
    state
        .download_dir
        .read()
        .map(|d| d.display().to_string())
        .unwrap_or_default()
}

/// Open a path (a received file, or the download folder) with the OS default
/// handler.
///
/// WHY A COMMAND, not the opener plugin's JS `openPath`: that command is
/// SCOPE-GATED, and its scope is a static allow-list. Our download folder is
/// user-configurable to anywhere on disk, so the only static scope that would
/// actually work is `"**"` — i.e. handing the webview a blanket "open any file
/// on this machine" capability. The plugin's RUST api takes the same action
/// without granting that authority to the frontend, so the capability stays
/// narrow (reveal-only) and this stays the one audited door.
///
/// Existence is checked FIRST so a vanished file reports `NotFound` — the UI
/// then says "that file is gone" instead of blaming a stale record for what
/// might really be an open failure.
#[tauri::command]
pub fn open_local_path(app: AppHandle, path: String) -> Result<(), LanBeamError> {
    use tauri_plugin_opener::OpenerExt;
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(LanBeamError::NotFound(path));
    }
    app.opener()
        .open_path(&path, None::<&str>)
        .map_err(|e| LanBeamError::Io(format!("open {path}: {e}")))
}

/// Absolute paths saved by a completed inbound transfer (for reveal/open in the UI).
#[tauri::command]
pub fn reveal_received(state: State<'_, AppState>, session_id: String) -> Vec<String> {
    // Poisoned lock: an empty list beats panicking the invoke — same contract as
    // `list_trusted` / `get_download_dir`.
    let Ok(log) = state.completed.lock() else {
        return Vec::new();
    };
    log.get(&session_id)
        .map(|v| v.iter().map(|p| p.display().to_string()).collect())
        .unwrap_or_default()
}

// ── browser share (M8.1b) ──────────────────────────────────────────────

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

// ── trust store (M4.4) ─────────────────────────────────────────────────

/// The trusted-peer list, sorted for display.
#[tauri::command]
pub fn list_trusted(state: State<'_, AppState>) -> Vec<TrustedPeerDto> {
    // Poisoned lock: an empty list beats panicking the whole invoke — the UI
    // refreshes on the next `trust_updated` anyway.
    state.trusted.read().map(|t| t.list()).unwrap_or_default()
}

/// Add or update a trusted peer. Persists, then emits `trust_updated` with
/// the full list. Malformed device ids (not a base64url 32-byte key) are
/// ignored — same setters-ignore-invalid contract as `set_log_level`.
/// `paired_at`/`last_seen` (unix seconds, optional — additive for the
/// localStorage migration) seed a NEW entry's timestamps so migrated records
/// keep their original dates; updates ignore them (see `TrustStore::set`).
#[tauri::command]
pub fn set_trusted(
    app: AppHandle,
    state: State<'_, AppState>,
    device_id: String,
    name: String,
    auto_accept: bool,
    paired_at: Option<u64>,
    last_seen: Option<u64>,
) {
    if transport::decode_device_id(&device_id).is_none() {
        return;
    }
    // Never trust yourself. Not a philosophical position — a self-record shows up
    // in your own trust circle as a peer you trust and can never reach (discovery
    // drops its own announces, so it is permanently "offline"), and it grants
    // nothing: you do not need permission to receive from yourself. Startup evicts
    // any that an older build let in.
    if device_id == state.identity.device_id() {
        log::debug!("trust: refusing a self-record");
        return;
    }
    // Mutate + snapshot under the write guard; the file write and the emit
    // happen AFTER it drops, so a slow disk (AV scan, sleeping HDD) never
    // stalls readers — or this main-thread command — on the trust lock (M4.6).
    let (snapshot, list) = {
        let Ok(mut store) = state.trusted.write() else {
            return;
        };
        store.set(device_id, name, auto_accept, paired_at, last_seen);
        (store.snapshot(), store.list())
    };
    trust::persist_async(snapshot);
    trust::emit_updated(&app, list);
}

/// Delete a device: drop BOTH its trust record and the manually-added address
/// that keeps it in the device list. Emits `trust_updated` and `devices_updated`
/// for whatever actually changed.
///
/// A device that is still announcing itself on the LAN will come back — as a
/// stranger. This erases LanBeam's memory of the device, not the machine.
///
/// Untrusting (`remove_trusted`) is deliberately NOT this: a device you stop
/// trusting is still one you want to be able to reach. Deleting is the stronger
/// act, and until now it did not exist. `remove_trusted` was all the UI had, so
/// "delete" cleared the trust row while the peer sat untouched in the manual
/// address table — reappearing on the very next list, with nothing anywhere able
/// to take it out. (Dial your own address once and it was permanent: your own id
/// can't be discovered away, because discovery drops its own announces.)
#[tauri::command]
pub fn forget_device(app: AppHandle, state: State<'_, AppState>, device_id: String) {
    // Trust row: same shape as `remove_trusted` — snapshot under the guard, then
    // I/O + emit after it drops.
    let trust_change = match state.trusted.write() {
        // A pattern guard can't mutate, so the remove has to happen in the body.
        Ok(mut store) => store
            .remove(&device_id)
            .then(|| (store.snapshot(), store.list())),
        Err(_) => None,
    };
    if let Some((snapshot, list)) = trust_change {
        trust::persist_async(snapshot);
        trust::emit_updated(&app, list);
    }
    // Manual address. This is the half that was missing.
    let dropped = state
        .manual_peers
        .lock()
        .map(|mut m| m.remove(&device_id).is_some())
        .unwrap_or(false);
    if dropped {
        let _ = app.emit("devices_updated", list_devices_snapshot(&state));
    }
}

/// Remove a peer from the trust store. Persists + emits `trust_updated` only
/// when something was actually deleted.
#[tauri::command]
pub fn remove_trusted(app: AppHandle, state: State<'_, AppState>, device_id: String) {
    // Same shape as `set_trusted`: snapshot under the guard, I/O + emit after.
    let (snapshot, list) = {
        let Ok(mut store) = state.trusted.write() else {
            return;
        };
        if !store.remove(&device_id) {
            return; // nothing deleted — no save, no event
        }
        (store.snapshot(), store.list())
    };
    trust::persist_async(snapshot);
    trust::emit_updated(&app, list);
}

// ── network status (M4.6) ──────────────────────────────────────────────

/// Degradations recorded at bind time (UDP receive fallback, TCP ephemeral
/// port). The UI queries this once at startup: the matching `net_degraded`
/// events fire during `setup()`, before the webview has any listener, and
/// Tauri events have no replay — without this pull path the toasts for the
/// most likely trigger (a port already held at launch) can never appear.
#[tauri::command]
pub fn get_net_status(state: State<'_, AppState>) -> Vec<NetDegraded> {
    state.degraded.lock().map(|v| v.clone()).unwrap_or_default()
}

// ── diagnostics (M4.6) ─────────────────────────────────────────────────

/// How much of the current log file the diagnostics bundle includes (tail).
const DIAG_LOG_TAIL_MAX: u64 = 500 * 1024;

/// The app's log directory, created if missing.
#[tauri::command]
pub fn get_log_dir(app: AppHandle) -> Result<String, LanBeamError> {
    let dir = crate::paths::log_dir(&app);
    std::fs::create_dir_all(&dir)?;
    Ok(dir.display().to_string())
}

/// Write a plain-text diagnostics bundle into the log dir and return its path.
///
/// Contents: app version, OS, a settings snapshot, network interfaces, and the
/// tail of the current log file. Deliberately NO key material — the identity's
/// private key lives only in the OS keychain and is never read here; the
/// settings snapshot must likewise stay free of secrets if any are ever added.
#[tauri::command]
pub fn export_diagnostics(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, LanBeamError> {
    use std::fmt::Write as _;

    let dir = crate::paths::log_dir(&app);
    std::fs::create_dir_all(&dir)?;

    let unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stamp = compact_utc_stamp(unix_secs);

    // Writing into a String cannot fail, hence the `let _ =` on every line.
    let mut out = String::new();
    let pkg = app.package_info();
    let _ = writeln!(out, "LanBeam diagnostics — {stamp} UTC");
    let _ = writeln!(out, "app version: {}", pkg.version);
    let _ = writeln!(
        out,
        "os: {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(out, "tcp port: {}", state.tcp_port.load(Ordering::Relaxed));
    match state.download_dir.read() {
        Ok(d) => {
            let _ = writeln!(out, "download dir: {}", d.display());
        }
        Err(_) => {
            let _ = writeln!(out, "download dir: <unavailable: lock poisoned>");
        }
    }

    let _ = writeln!(out, "\n[settings]");
    match state.settings.read() {
        Ok(s) => {
            let _ = writeln!(out, "device_name: {}", s.device_name);
            let _ = writeln!(out, "discoverable: {}", s.discoverable);
            let _ = writeln!(out, "auto_open_folder: {}", s.auto_open_folder);
            let _ = writeln!(out, "log_level: {}", s.log_level);
            let _ = writeln!(out, "recv_policy: {}", s.recv_policy);
            let _ = writeln!(out, "download_dir_override: {:?}", s.download_dir_override);
            let _ = writeln!(out, "port: {}", s.port);
            let _ = writeln!(out, "tray_close: {}", s.tray_close);
            let _ = writeln!(out, "notif_system: {}", s.notif_system);
            let _ = writeln!(out, "autostart: {}", s.autostart);
            let _ = writeln!(out, "iface_filter: {:?}", s.iface_filter);
            let _ = writeln!(out, "hotkey_enabled: {}", s.hotkey_enabled);
            let _ = writeln!(out, "hotkey: {}", s.hotkey);
            let _ = writeln!(out, "verify_hash: {}", s.verify_hash);
            let _ = writeln!(out, "conflict: {}", s.conflict);
            let _ = writeln!(out, "organize: {}", s.organize);
            let _ = writeln!(out, "max_concurrent: {}", s.max_concurrent);
            let _ = writeln!(out, "rate_limit: {}", s.rate_limit);
            let _ = writeln!(out, "clip_share: {}", s.clip_share);
            let _ = writeln!(out, "strip_exif: {}", s.strip_exif);
        }
        // A poisoned lock means another task panicked; still produce a bundle —
        // a diagnostics command failing during a failure is the worst outcome.
        Err(_) => {
            let _ = writeln!(out, "<unavailable: settings lock poisoned>");
        }
    }

    let _ = writeln!(out, "\n[network interfaces]");
    match if_addrs::get_if_addrs() {
        Ok(ifaces) => {
            for i in &ifaces {
                let _ = writeln!(
                    out,
                    "{}: {}{}",
                    i.name,
                    i.addr.ip(),
                    if i.is_loopback() { " (loopback)" } else { "" }
                );
            }
            if ifaces.is_empty() {
                let _ = writeln!(out, "<none>");
            }
        }
        Err(e) => {
            let _ = writeln!(out, "<enumeration failed: {e}>");
        }
    }

    let _ = writeln!(out, "\n[log tail (up to {DIAG_LOG_TAIL_MAX} bytes)]");
    match newest_log_file(&dir) {
        Some(log_path) => {
            let _ = writeln!(out, "file: {}", log_path.display());
            match read_tail(&log_path, DIAG_LOG_TAIL_MAX) {
                Ok(tail) => out.push_str(&tail),
                Err(e) => {
                    let _ = writeln!(out, "<read failed: {e}>");
                }
            }
        }
        None => {
            let _ = writeln!(out, "<no log file found>");
        }
    }

    let path = dir.join(format!("lanbeam-diag-{stamp}.txt"));
    std::fs::write(&path, out)?;
    log::info!("diagnostics bundle written to {}", path.display());
    Ok(path.display().to_string())
}

/// Most recently modified `*.log` in `dir` — found by mtime rather than by a
/// hard-coded name so it keeps working if the log plugin's naming changes.
fn newest_log_file(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, p)| p)
}

/// Read at most the last `max` bytes of a file (lossy UTF-8: a tail can start
/// mid-multibyte-sequence and log content is diagnostic, not canonical).
fn read_tail(path: &Path, max: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    if len > max {
        f.seek(SeekFrom::Start(len - max))?;
    }
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Format unix seconds as a compact UTC stamp `YYYYMMDD-HHMMSS` (diagnostics
/// file names). Hand-rolled via [`civil_from_days`] to avoid pulling in a date
/// crate for a single call site.
fn compact_utc_stamp(unix_secs: u64) -> String {
    let (y, m, d) = civil_from_days((unix_secs / 86_400) as i64);
    let secs = unix_secs % 86_400;
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Days since 1970-01-01 → (year, month, day) in the proleptic Gregorian
/// calendar — Howard Hinnant's `civil_from_days` algorithm, exact for the
/// whole i64 day range we can ever see.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Public identity shown to the user. Never includes the private key.
#[derive(Serialize)]
pub struct MyIdentity {
    #[serde(rename = "deviceId")]
    pub device_id: String,
    #[serde(rename = "shortId")]
    pub short_id: String,
    pub name: String,
}

#[tauri::command]
pub fn get_my_identity(state: State<'_, AppState>) -> MyIdentity {
    let device_id = state.identity.device_id();
    let short_id = device_id.chars().take(8).collect();
    // device_id/short_id come from the immutable identity Arc; only the name
    // reads the shared settings lock. A poisoned lock falls back to the default
    // device name rather than panicking the invoke.
    let name = state
        .settings
        .read()
        .map(|s| s.device_name.clone())
        .unwrap_or_else(|_| settings::default_device_name());
    MyIdentity {
        device_id,
        short_id,
        name,
    }
}

#[derive(Serialize)]
pub struct SettingsDto {
    #[serde(rename = "deviceName")]
    pub device_name: String,
    pub discoverable: bool,
    #[serde(rename = "autoOpenFolder")]
    pub auto_open_folder: bool,
    #[serde(rename = "logLevel")]
    pub log_level: String,
    #[serde(rename = "recvPolicy")]
    pub recv_policy: String,
    /// Omitted (not `null`) when unset — `downloadDirOverride?` on the TS
    /// side, additive for frontends that predate it (M5.2).
    #[serde(
        rename = "downloadDirOverride",
        skip_serializing_if = "Option::is_none"
    )]
    pub download_dir_override: Option<String>,
    /// `0` = LanBeam's default port.
    pub port: u16,
    /// Whether closing the window hides to the tray instead of quitting (M5.3).
    #[serde(rename = "trayClose")]
    pub tray_close: bool,
    /// OS notifications for incoming prompts / finished receives (M5.4).
    #[serde(rename = "notifSystem")]
    pub notif_system: bool,
    /// Launch at login (M5.5); the setter mirrors it into the OS entry.
    pub autostart: bool,
    /// Discovery interface filter (M5.6). Omitted when unset — same additive
    /// contract as `downloadDirOverride`.
    #[serde(rename = "ifaceFilter", skip_serializing_if = "Option::is_none")]
    pub iface_filter: Option<String>,
    /// Whether the global quick-summon hotkey is registered (M5.5). Additive
    /// camelCase; defaults false so the app never claims Alt+Space unasked.
    #[serde(rename = "hotkeyEnabled")]
    pub hotkey_enabled: bool,
    /// The accelerator the quick-summon hotkey binds (M5.5 rebind), in the
    /// `MOD+KEY` form (e.g. `Alt+Space`). Additive camelCase; the UI formats it
    /// for display but stores this canonical string.
    pub hotkey: String,
    /// Per-file SHA-256 integrity verification (M6.3). Additive camelCase;
    /// defaults true so a fresh install verifies out of the box.
    #[serde(rename = "verifyHash")]
    pub verify_hash: bool,
    /// Name-collision policy (M6.5): `"rename" | "overwrite" | "ask"`. Additive
    /// camelCase; the `"ask"` default drives the ConflictModal.
    pub conflict: String,
    /// Auto-organize mode (M6.6): `"none" | "device" | "date"`. Additive
    /// camelCase; defaults `"none"` (write straight under the download root).
    pub organize: String,
    /// Concurrency cap (M6.7): how many transfers stream at once. Additive
    /// camelCase; defaults 3, range 1..=8.
    #[serde(rename = "maxConcurrent")]
    pub max_concurrent: u32,
    pub ui_zoom: f64,
    /// Per-transfer throughput cap (M6.7): `"unlimited"` or MB/s. Additive
    /// camelCase; defaults `"unlimited"`.
    #[serde(rename = "rateLimit")]
    pub rate_limit: String,
    /// Whether an incoming quick text is also written to this machine's
    /// clipboard (M7.3). Additive camelCase; defaults false (opt-in consent).
    #[serde(rename = "clipShare")]
    pub clip_share: bool,
    /// Strip photo metadata (EXIF/ICC/XMP) from images before sending (M9.1).
    /// Additive camelCase; defaults true so location/camera data does not leak.
    #[serde(rename = "stripExif")]
    pub strip_exif: bool,
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> SettingsDto {
    // Poisoned lock: return a snapshot built from `Settings::default()` rather
    // than panic the invoke — an honest default shape the UI can render, same
    // graceful contract as the other readers. The next explicit read refreshes.
    let s = state.settings.read().map(|g| g.clone()).unwrap_or_default();
    SettingsDto {
        device_name: s.device_name.clone(),
        discoverable: s.discoverable,
        auto_open_folder: s.auto_open_folder,
        log_level: s.log_level.clone(),
        recv_policy: s.recv_policy.clone(),
        download_dir_override: s.download_dir_override.clone(),
        port: s.port,
        tray_close: s.tray_close,
        notif_system: s.notif_system,
        autostart: s.autostart,
        iface_filter: s.iface_filter.clone(),
        hotkey_enabled: s.hotkey_enabled,
        hotkey: s.hotkey.clone(),
        verify_hash: s.verify_hash,
        conflict: s.conflict.clone(),
        organize: s.organize.clone(),
        max_concurrent: s.max_concurrent,
        ui_zoom: s.ui_zoom,
        rate_limit: s.rate_limit.clone(),
        clip_share: s.clip_share,
        strip_exif: s.strip_exif,
    }
}

/// Snapshot-under-guard settings write shared by the simple setters (M5–M9):
/// take the write guard, apply `f`, clone the new state, then persist AFTER the
/// guard drops — never any disk I/O under the settings lock. A poisoned lock is
/// a graceful no-op, the same contract every setter upheld inline before. The
/// bespoke setters (live re-registration, OS reconcile, `Result`-returning)
/// keep their own bodies; this is only for the pure snapshot+mutate+save ones.
fn update_settings(app: &AppHandle, state: &AppState, f: impl FnOnce(&mut settings::Settings)) {
    let snapshot = {
        let Ok(mut s) = state.settings.write() else {
            return;
        };
        f(&mut s);
        s.clone()
    };
    settings::save(app, &snapshot);
}

#[tauri::command]
pub fn set_device_name(app: AppHandle, state: State<'_, AppState>, name: String) {
    let clamped: String = name.trim().chars().take(63).collect();
    update_settings(&app, &state, |s| {
        s.device_name = if clamped.is_empty() {
            settings::default_device_name()
        } else {
            clamped
        };
    });
}

#[tauri::command]
pub fn set_discoverable(app: AppHandle, state: State<'_, AppState>, discoverable: bool) {
    update_settings(&app, &state, |s| s.discoverable = discoverable);
}

#[tauri::command]
pub fn set_auto_open(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    update_settings(&app, &state, |s| s.auto_open_folder = enabled);
}

/// Set the diagnostics log level (`"errors" | "normal" | "verbose"`).
/// Unknown values are ignored so the stored blob only ever holds a level
/// the logging layer can map.
#[tauri::command]
pub fn set_log_level(app: AppHandle, state: State<'_, AppState>, level: String) {
    if !settings::LOG_LEVELS.contains(&level.as_str()) {
        return;
    }
    update_settings(&app, &state, |s| s.log_level = level);
}

/// Set the inbound transfer policy (`"ask" | "trusted" | "all"`).
/// Unknown values are ignored — same contract as `set_log_level`.
#[tauri::command]
pub fn set_recv_policy(app: AppHandle, state: State<'_, AppState>, policy: String) {
    if !settings::RECV_POLICIES.contains(&policy.as_str()) {
        return;
    }
    update_settings(&app, &state, |s| s.recv_policy = policy);
}

/// Set the folder incoming files are saved to. The path must be an existing
/// directory; it is canonicalized before use. Returns the canonical path as
/// stored and shown by `get_download_dir`.
#[tauri::command]
pub fn set_download_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<String, LanBeamError> {
    // Reject bad input up front (nonexistent path, plain file) — receiving
    // must never be pointed at something writes can't land in (M5.2).
    let canon = settings::canonical_dir(&path)
        .ok_or_else(|| LanBeamError::Io(format!("not an existing directory: {path}")))?;
    // Retarget the LIVE root first, so the very next inbound session lands in
    // the new folder without a restart; sessions already streaming keep the
    // root they snapshotted at start (see `transfer::handle_incoming`).
    match state.download_dir.write() {
        Ok(mut d) => *d = canon.clone(),
        // Poisoned: keep serving the old root rather than panic the command;
        // the persisted override below still applies on the next launch.
        Err(_) => log::warn!("download_dir lock poisoned; live root unchanged"),
    }
    let stored = canon.display().to_string();
    let snapshot = {
        let Ok(mut s) = state.settings.write() else {
            // The live root DID change — report success so the UI reflects it.
            return Ok(stored);
        };
        s.download_dir_override = Some(stored.clone());
        s.clone()
    };
    settings::save(&app, &snapshot);
    Ok(stored)
}

/// Set the TCP listen port (`0` = LanBeam's default). Persists only — it
/// applies on the next launch, while discovery keeps advertising the port
/// actually bound now, so peers stay correct throughout. Privileged values
/// (1..=1023) are ignored — same contract as `set_log_level`.
#[tauri::command]
pub fn set_listen_port(app: AppHandle, state: State<'_, AppState>, port: u16) {
    if !settings::valid_listen_port(port) {
        return;
    }
    let snapshot = {
        let Ok(mut s) = state.settings.write() else {
            return;
        };
        s.port = port;
        s.clone()
    };
    settings::save(&app, &snapshot);
}

/// Toggle "closing the window hides to the tray" (M5.3). Effective on the
/// very next close — the window-event handler reads the live setting each
/// time, so no restart is involved.
#[tauri::command]
pub fn set_tray_close(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    update_settings(&app, &state, |s| s.tray_close = enabled);
}

/// Toggle OS notifications (M5.4). Effective on the very next prompt or
/// completion — the notify path reads the live setting each time.
#[tauri::command]
pub fn set_notif_system(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    update_settings(&app, &state, |s| s.notif_system = enabled);
}

/// Toggle launch-at-login (M5.5). The OS entry changes FIRST and the setting
/// persists only after that succeeds — persisting a toggle the OS refused
/// would show the user a lie that startup "reconciles" back into the failure
/// on every launch.
#[tauri::command]
pub fn set_autostart(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), LanBeamError> {
    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        let autolaunch = app.autolaunch();
        // Skip the OS call when it already agrees: `disable` on some platforms
        // errors when no entry exists, and that must not fail a no-op toggle.
        let already = autolaunch.is_enabled().unwrap_or(!enabled);
        if already != enabled {
            let res = if enabled {
                autolaunch.enable()
            } else {
                autolaunch.disable()
            };
            res.map_err(|e| LanBeamError::Io(e.to_string()))?;
        }
    }
    let snapshot = {
        let Ok(mut s) = state.settings.write() else {
            return Ok(());
        };
        s.autostart = enabled;
        s.clone()
    };
    settings::save(&app, &snapshot);
    Ok(())
}

/// Set the discovery interface filter (M5.6): an interface's IPv4 address as
/// shown by `get_network_info`, empty to clear (= all interfaces). Applied by
/// the announce loop within one 2s tick — no restart. Unparseable input is
/// ignored — same contract as `set_log_level`.
#[tauri::command]
pub fn set_iface_filter(app: AppHandle, state: State<'_, AppState>, ip: String) {
    let Some(value) = settings::parse_iface_filter(&ip) else {
        return;
    };
    update_settings(&app, &state, |s| s.iface_filter = value);
}

/// Toggle the global quick-summon hotkey (M5.5). Registers or unregisters the
/// currently-configured chord immediately, so the change takes effect without a
/// restart. A registration failure (another app already owns the chord) is
/// reported to the log and the preference is still saved — the same
/// warn-and-continue behavior as startup, so the toggle never fails the command.
#[tauri::command]
pub fn set_hotkey_enabled(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    #[cfg(desktop)]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        // Bind/unbind the user's CONFIGURED combo, not the hardcoded default —
        // a prior `set_hotkey` may have rebound it. The const is only the
        // fall-back when the lock is unreadable.
        let combo = state
            .settings
            .read()
            .map(|s| s.hotkey.clone())
            .unwrap_or_else(|_| crate::consts::DEFAULT_HOTKEY.to_string());
        if enabled {
            if let Err(e) = crate::register_summon_hotkey(&app, &combo) {
                log::warn!(
                    "global shortcut {combo} could not be registered (held by another app?): {e}"
                );
            }
        } else if let Err(e) = app.global_shortcut().unregister(combo.as_str()) {
            log::warn!("global shortcut {combo} unregister failed: {e}");
        }
    }
    let snapshot = {
        let Ok(mut s) = state.settings.write() else {
            return;
        };
        s.hotkey_enabled = enabled;
        s.clone()
    };
    settings::save(&app, &snapshot);
}

/// Rebind the global quick-summon accelerator (M5.5). Pass a shortcut in the
/// accelerator form `MOD(+MOD)*+KEY` — one or more of `Ctrl` / `Alt` / `Shift` /
/// `Super` plus one key, e.g. `Alt+Space` or `Ctrl+Shift+K`. A malformed or
/// modifier-less accelerator is refused. While the hotkey is enabled the new
/// chord is bound live and the old one released; if another app already holds it
/// the previous binding is kept and this returns an error to surface. Effective
/// immediately — no restart.
#[tauri::command]
pub fn set_hotkey(
    app: AppHandle,
    state: State<'_, AppState>,
    combo: String,
) -> Result<(), LanBeamError> {
    let combo = combo.trim().to_string();
    if !settings::valid_hotkey(&combo) {
        return Err(LanBeamError::Protocol(format!(
            "not a valid shortcut: {combo}"
        )));
    }
    // Snapshot the previous combo + whether it is live, so a rejected rebind can
    // leave the old binding untouched.
    let (old, enabled) = {
        let Ok(s) = state.settings.read() else {
            return Err(LanBeamError::Io("settings unavailable".into()));
        };
        (s.hotkey.clone(), s.hotkey_enabled)
    };
    if combo == old {
        return Ok(()); // no change — nothing to rebind or persist
    }
    #[cfg(desktop)]
    if enabled {
        // Bind the NEW chord FIRST: if the plugin refuses it (already held by the
        // OS/another app) the OLD chord was never unregistered, so the existing
        // binding stays live and we surface the conflict without persisting.
        crate::register_summon_hotkey(&app, &combo).map_err(|e| {
            LanBeamError::Io(format!("hotkey {combo} could not be registered: {e}"))
        })?;
        // The new chord is live — release the old one. A failure here only leaks
        // one stale registration; log it rather than fail the (successful) rebind.
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Err(e) = app.global_shortcut().unregister(old.as_str()) {
            log::warn!("global shortcut {old} unregister after rebind failed: {e}");
        }
    }
    #[cfg(not(desktop))]
    let _ = enabled; // no global-shortcut plugin off desktop; persist only
    let snapshot = {
        let Ok(mut s) = state.settings.write() else {
            // The live rebind (if any) already applied; report success so the UI
            // adopts the new combo — the next successful setter re-persists.
            return Ok(());
        };
        s.hotkey = combo;
        s.clone()
    };
    settings::save(&app, &snapshot);
    Ok(())
}

/// Toggle per-file SHA-256 integrity verification (M6.3). Read at send time,
/// so it takes effect on the very next transfer without a restart. A plain
/// boolean with no invalid states, so — unlike the string setters — there is
/// nothing to validate.
#[tauri::command]
pub fn set_verify_hash(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    update_settings(&app, &state, |s| s.verify_hash = enabled);
}

/// Set the name-collision policy (`"rename" | "overwrite" | "ask"`, M6.5). Read
/// at receive time, so it takes effect on the very next transfer. Unknown values
/// are ignored — same contract as `set_log_level`.
#[tauri::command]
pub fn set_conflict_policy(app: AppHandle, state: State<'_, AppState>, policy: String) {
    if !settings::CONFLICT_POLICIES.contains(&policy.as_str()) {
        return;
    }
    update_settings(&app, &state, |s| s.conflict = policy);
}

/// Set the auto-organize mode (`"none" | "device" | "date"`, M6.6). Read at
/// receive time; the subfolder is computed when a transfer starts. Unknown
/// values are ignored — same contract as `set_log_level`.
#[tauri::command]
pub fn set_organize(app: AppHandle, state: State<'_, AppState>, mode: String) {
    if !settings::ORGANIZE_MODES.contains(&mode.as_str()) {
        return;
    }
    update_settings(&app, &state, |s| s.organize = mode);
}

/// Set the concurrency cap (M6.7): how many transfers stream at once, clamped to
/// the allowed range. Read live at each transfer's gate, so a lower value throttles
/// only transfers not yet started; those in flight keep going. Applies to the next
/// launch's default plus every subsequent transfer — no restart.
/// Set the interface scale (1.0 = the design size). Applies immediately and
/// persists. Out-of-range values are clamped rather than refused.
#[tauri::command]
pub fn set_ui_zoom(app: AppHandle, state: State<'_, AppState>, zoom: f64) {
    let z = settings::clamp_ui_zoom(zoom);
    update_settings(&app, &state, |s| s.ui_zoom = z);
    #[cfg(desktop)]
    if let Some(win) = app.get_webview_window("main") {
        crate::apply_ui_zoom(&win, z);
    }
}

#[tauri::command]
pub fn set_max_concurrent(app: AppHandle, state: State<'_, AppState>, max: u32) {
    let clamped = settings::clamp_max_concurrent(max);
    update_settings(&app, &state, |s| s.max_concurrent = clamped);
}

/// Set the per-transfer throughput cap (M6.7): `"unlimited"` or a positive
/// integer count of MB/s. Read when a transfer starts streaming, so it takes
/// effect on the very next transfer. Invalid values (`"0"`, junk) are ignored —
/// same contract as `set_log_level`.
#[tauri::command]
pub fn set_rate_limit(app: AppHandle, state: State<'_, AppState>, limit: String) {
    if !settings::valid_rate_limit(&limit) {
        return;
    }
    let normalized = limit.trim().to_string();
    update_settings(&app, &state, |s| s.rate_limit = normalized);
}

/// Toggle whether an incoming quick text is also written to this machine's
/// clipboard (M7.3). Off by default — turning it on lets a sender's text land on
/// your clipboard when they ask for it. Read at receive time, so it takes effect
/// on the very next text without a restart. A plain boolean with no invalid
/// states, so — unlike the string setters — there is nothing to validate.
#[tauri::command]
pub fn set_clip_share(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    update_settings(&app, &state, |s| s.clip_share = enabled);
}

/// Toggle stripping photo metadata (EXIF/ICC/XMP) from images before sending
/// (M9.1). This is the persisted default; the send confirm dialog can still
/// override it per transfer. Read at send time, so it takes effect on the very
/// next transfer without a restart. A plain boolean with no invalid states, so
/// — unlike the string setters — there is nothing to validate.
#[tauri::command]
pub fn set_strip_exif(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    update_settings(&app, &state, |s| s.strip_exif = enabled);
}

// ── identity reset (M5.7) ──────────────────────────────────────────────

/// Factory-reset this device's identity. After it, other devices see us as a
/// brand-new device and MUST re-verify (their pinned Device ID no longer
/// matches), and our own trust list starts empty.
///
/// Order matters: (1) go invisible NOW — discoverable off in-memory plus one
/// best-effort tombstone broadcast — so peers drop the old ID immediately
/// instead of showing a ghost until expiry; (2) delete the keychain identity,
/// the only step that can fail, placed before anything irreversible happens;
/// (3) clear the trust store, persisted before the restart; (4) restart —
/// the identity is snapshotted at startup and threaded through the live
/// listener/announcer, so hot-swapping the keypair is not realistic, and a
/// fresh one is generated on the way back up.
#[tauri::command]
pub async fn reset_identity(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), LanBeamError> {
    // (1) In-memory only, deliberately NOT persisted: the next launch (fresh
    // identity) comes up with whatever discoverability the user configured.
    // Capture the PRIOR discoverable so step (2) can roll it back if the
    // keychain delete fails — otherwise the box goes silently invisible
    // (tombstone already out, announces stopped) while the UI still shows
    // 可被发现. A poisoned lock degrades to an empty tombstone name — removal is
    // keyed on the id, and a recovery action must not be blocked by a lock.
    let (prev_disc, name) = state
        .settings
        .write()
        .map(|mut s| {
            let prev = s.discoverable;
            s.discoverable = false;
            (prev, s.device_name.clone())
        })
        .unwrap_or((false, String::new()));
    discovery::send_tombstone(
        state.identity.device_id(),
        name,
        state.tcp_port.load(Ordering::Relaxed),
    )
    .await;

    // (2) The point of commitment — but failing here must not strand the box
    // invisible: restore the discoverable flag flipped in (1) before returning,
    // so the announce loop re-announces within one 2s tick and peers that
    // consumed the best-effort tombstone re-add the device. The identity itself
    // is intact, so the UI can simply show the error and stop.
    if let Err(e) = identity::delete_persisted(crate::instance_id().as_deref()) {
        if let Ok(mut s) = state.settings.write() {
            s.discoverable = prev_disc;
        }
        return Err(e);
    }

    // (3) Forget every pairing decision, and WAIT for the file write — the
    // restart below must not race the persist. A poisoned trust lock is only
    // logged: the identity is already gone, so aborting now would strand the
    // user half-reset with no way forward.
    let snapshot = match state.trusted.write() {
        Ok(mut t) => {
            if t.clear() {
                t.snapshot()
            } else {
                None // already empty — nothing to persist
            }
        }
        Err(_) => {
            log::warn!("trust lock poisoned; trust file left as-is during reset");
            None
        }
    };
    if let Some(snap) = snapshot {
        let _ = tauri::async_runtime::spawn_blocking(move || snap.persist()).await;
    }

    log::info!("identity reset: keychain entry deleted, trust cleared; restarting");
    // (4) Mark quit intent so close-to-tray can never intercept teardown.
    state.quitting.store(true, Ordering::Relaxed);
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known calendar anchors: the epoch itself, a famous round number
    /// (1e9 = 2001-09-09 01:46:40 UTC), and a leap day.
    #[test]
    fn civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_723 + 31 + 28), (2024, 2, 29)); // leap day
        assert_eq!(civil_from_days(-1), (1969, 12, 31)); // pre-epoch stays exact
    }

    #[test]
    fn compact_utc_stamp_formats_and_pads() {
        assert_eq!(compact_utc_stamp(0), "19700101-000000");
        assert_eq!(compact_utc_stamp(1_000_000_000), "20010909-014640");
    }

    /// The tail must honor the byte cap and keep the END of the file.
    #[test]
    fn read_tail_caps_and_keeps_the_end() {
        let dir = std::env::temp_dir().join(format!("lanbeam-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.log");
        std::fs::write(&p, "AAAAAAAAAAtail-marker").unwrap();
        let tail = read_tail(&p, 11).unwrap();
        assert_eq!(tail, "tail-marker");
        // a cap larger than the file returns everything
        let all = read_tail(&p, 10_000).unwrap();
        assert_eq!(all, "AAAAAAAAAAtail-marker");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only `*.log` files count, and the most recently modified one wins.
    #[test]
    fn newest_log_file_picks_latest_log() {
        let dir = std::env::temp_dir().join(format!("lanbeam-logs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.log"), "old").unwrap();
        std::fs::write(dir.join("ignore.txt"), "not a log").unwrap();
        // ensure a strictly later mtime on filesystems with coarse timestamps
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(dir.join("new.log"), "new").unwrap();
        let got = newest_log_file(&dir).expect("a log file exists");
        assert_eq!(got.file_name().unwrap(), "new.log");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── pairing / direct-connect helpers (M7.1/7.2) ───────────────────────

    /// A plain `ip:port`, a bare `ip` (defaulting the port), and a full
    /// `lanbeam://pair` link all parse; junk and out-of-range addresses do not.
    #[test]
    fn parse_dial_target_handles_addr_and_link() {
        let t = parse_dial_target("192.168.1.5:52637").expect("ip:port parses");
        assert_eq!(t.ip, Ipv4Addr::new(192, 168, 1, 5));
        assert_eq!(t.port, 52637);
        assert!(t.code.is_none());

        let t = parse_dial_target("192.168.1.5").expect("bare ip parses");
        assert_eq!(
            t.port, DEFAULT_TCP_PORT,
            "a missing port falls back to the default"
        );

        let t = parse_dial_target("  10.0.0.7:51704  ").expect("surrounding space is trimmed");
        assert_eq!(t.ip, Ipv4Addr::new(10, 0, 0, 7));

        let link = "lanbeam://pair?d=abc&n=My%20PC&a=192.168.1.9&p=51999&c=482913";
        let t = parse_dial_target(link).expect("link parses");
        assert_eq!(t.ip, Ipv4Addr::new(192, 168, 1, 9));
        assert_eq!(t.port, 51999);
        assert_eq!(t.code.as_deref(), Some("482913"));

        assert!(parse_dial_target("not-an-ip").is_none());
        assert!(
            parse_dial_target("999.1.1.1:5").is_none(),
            "an out-of-range octet is rejected"
        );
        assert!(
            parse_dial_target("192.168.1.5:70000").is_none(),
            "an out-of-range port is rejected"
        );
    }

    /// `build_pair_uri` produces a link the parser reads back to the same
    /// endpoint + code, and the name is percent-encoded (space → %20).
    #[test]
    fn pair_uri_round_trips_through_the_parser() {
        let ip = Ipv4Addr::new(192, 168, 1, 20);
        let uri = build_pair_uri("dev-Id_1", "My PC", Some(ip), 51704, "482913");
        assert!(uri.starts_with("lanbeam://pair?"), "got {uri}");
        assert!(
            uri.contains("n=My%20PC"),
            "the name must be percent-encoded: {uri}"
        );
        assert!(uri.contains("a=192.168.1.20"));

        let t = parse_dial_target(&uri).expect("the built link must parse");
        assert_eq!(t.ip, ip);
        assert_eq!(t.port, 51704);
        assert_eq!(t.code.as_deref(), Some("482913"));

        // With no LAN address the `a=` hint is omitted, so the parser cannot
        // recover an endpoint from the link alone.
        let no_ip = build_pair_uri("dev", "n", None, 51704, "111111");
        assert!(!no_ip.contains("&a="), "no address → no a= param: {no_ip}");
        assert!(
            parse_dial_target(&no_ip).is_none(),
            "a link without a= yields no target"
        );
    }

    /// The pairing link must advertise an address a peer can actually reach. A
    /// Windows box with WSL/Hyper-V/Docker carries a virtual adapter (172.16/12)
    /// whose IP sorts BELOW the real NIC — the old lowest-IP pick handed peers
    /// the unreachable virtual address. Real LAN ranges must win.
    #[test]
    fn pairing_ip_prefers_real_lan_over_virtual_adapter() {
        use crate::discovery::interfaces::Iface;
        let mk = |a, b, c, d| Iface {
            ip: Ipv4Addr::new(a, b, c, d),
            broadcast: None,
        };

        // The exact failure the user hit: 172.24.x (Hyper-V/WSL) alongside a
        // real 192.168.x NIC must resolve to the 192.168 address.
        assert_eq!(
            best_lan_ip(vec![mk(172, 24, 32, 1), mk(192, 168, 1, 20)]),
            Some(Ipv4Addr::new(192, 168, 1, 20)),
        );
        // 10/8 is equally real and also beats the virtual 172.
        assert_eq!(
            best_lan_ip(vec![mk(172, 24, 32, 1), mk(10, 0, 0, 5)]),
            Some(Ipv4Addr::new(10, 0, 0, 5)),
        );
        // A genuine 172.x LAN with no better range is still chosen (only option).
        assert_eq!(
            best_lan_ip(vec![mk(172, 20, 5, 5)]),
            Some(Ipv4Addr::new(172, 20, 5, 5)),
        );
        // APIPA is the last resort but still returned when it is all there is.
        assert_eq!(
            best_lan_ip(vec![mk(169, 254, 3, 3)]),
            Some(Ipv4Addr::new(169, 254, 3, 3)),
        );
        // Link-down: nothing to advertise.
        assert_eq!(best_lan_ip(vec![]), None);
    }

    /// Percent-encoding round-trips arbitrary UTF-8 (a Chinese name with a space
    /// and a middle dot), and `+` decodes to a space (form encoding).
    #[test]
    fn percent_codec_round_trips_utf8() {
        for original in ["书房 · MacBook Pro", "plain", "a/b?c=d&e", "空格 spaces"] {
            let enc = percent_encode(original);
            assert_eq!(
                percent_decode(&enc),
                original,
                "round-trip must be lossless"
            );
        }
        assert_eq!(percent_decode("a+b"), "a b", "+ decodes to space");
        assert_eq!(
            percent_decode("bad%zz"),
            "bad%zz",
            "a malformed escape keeps its bytes"
        );
    }

    /// A minted pairing code is always exactly six decimal digits.
    #[test]
    fn fresh_pairing_code_is_six_digits() {
        for _ in 0..64 {
            let code = fresh_pairing_code();
            assert_eq!(code.len(), 6, "code must be 6 chars: {code}");
            assert!(
                code.bytes().all(|b| b.is_ascii_digit()),
                "code must be all digits: {code}"
            );
        }
    }

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

    /// A minimal `AppState` with every table empty — the base both the manual-peer
    /// tests and the browser-share tests build on, so the big literal lives once.
    fn test_app_state() -> AppState {
        use std::sync::atomic::{AtomicBool, AtomicU16};
        use std::sync::{Arc, Mutex, RwLock};

        let tmp = std::env::temp_dir().join(format!("lanbeam-cmd-{}", std::process::id()));
        AppState {
            identity: Arc::new(identity::generate_identity().unwrap()),
            settings: Arc::new(RwLock::new(settings::Settings::default())),
            peers: Arc::new(Mutex::new(discovery::PeerTable::new())),
            tcp_port: Arc::new(AtomicU16::new(0)),
            download_dir: Arc::new(RwLock::new(PathBuf::from("."))),
            pending: Arc::new(Mutex::new(HashMap::new())),
            trusted: Arc::new(RwLock::new(crate::trust::TrustStore::load(
                tmp.join("trusted.json"),
            ))),
            completed: Arc::new(Mutex::new(crate::state::CompletedLog::new())),
            degraded: Arc::new(Mutex::new(Vec::new())),
            quitting: Arc::new(AtomicBool::new(false)),
            transfers_ctl: Arc::new(Mutex::new(HashMap::new())),
            partials: Arc::new(RwLock::new(crate::partials::PartialsStore::load(
                tmp.join("partials.json"),
            ))),
            concurrency: Arc::new(crate::state::ConcurrencyGate::new()),
            pairing: Arc::new(crate::state::PairingState::default()),
            manual_peers: Arc::new(Mutex::new(HashMap::new())),
            shares: crate::share::new_registry(),
            share_port: Arc::new(AtomicU16::new(0)),
            pending_deep_link: Arc::new(Mutex::new(None)),
        }
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

    // ── browser share commands (M8.1b) ────────────────────────────────────

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

    // ── resolve / dial-target / code helpers ──────────────────────────────

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

    /// `pick_code` prefers an explicitly-entered code (trimmed), falls back to a
    /// scanned link's code, and yields empty when neither is present.
    #[test]
    fn pick_code_prefers_entered_then_link() {
        assert_eq!(
            pick_code(" 482913 ".into(), Some("999999".into())),
            "482913",
            "an explicitly-entered code wins and is trimmed"
        );
        assert_eq!(
            pick_code("   ".into(), Some("123456".into())),
            "123456",
            "a blank entry falls back to the scanned link's code"
        );
        assert_eq!(
            pick_code(String::new(), None),
            "",
            "nothing entered and no link yields an empty code"
        );
    }

    /// A `lanbeam://pair` link with malformed query parts still parses off `a=`:
    /// a bare key (no `=`) is skipped, an empty `c=` carries no code, and an
    /// unparseable `p=` keeps the default port. A link with no `a=` yields nothing.
    #[test]
    fn parse_pair_uri_tolerates_malformed_query_params() {
        let link = "lanbeam://pair?flag&a=192.168.1.9&c=&p=notaport";
        let t = parse_dial_target(link).expect("a= still yields a target");
        assert_eq!(t.ip, Ipv4Addr::new(192, 168, 1, 9));
        assert_eq!(
            t.port, DEFAULT_TCP_PORT,
            "an unparseable port keeps the default"
        );
        assert!(t.code.is_none(), "an empty c= carries no code");

        assert!(
            parse_dial_target("lanbeam://pair?p=51999&c=123456").is_none(),
            "a link without a= yields no dial target"
        );
    }
}
