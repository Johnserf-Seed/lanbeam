//! Pairing + IP direct connect (M7.1/7.2): minting and redeeming a pairing
//! code, adding a device by its address, and the `lanbeam://pair` link that
//! carries both.

use std::net::Ipv4Addr;
use std::sync::atomic::Ordering;
use std::time::Instant;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Serialize;
use tauri::State;

use crate::consts::DEFAULT_TCP_PORT;
use crate::discovery;
use crate::error::LanBeamError;
use crate::manual_peers::{self, ManualPeer};
use crate::state::{AppState, PairingSession};
use crate::{transfer, transport};

/// Keep a device reached by IP or pairing code dialable: in the manual table,
/// which discovery's expiry never touches — and on disk, which a restart
/// doesn't either.
fn remember_manual_peer(state: &AppState, device_id: &str, name: &str, ip: Ipv4Addr, port: u16) {
    let snapshot = match state.manual_peers.lock() {
        Ok(mut manual) => {
            manual.insert(
                device_id.to_string(),
                ManualPeer {
                    name: name.to_string(),
                    addr: ip,
                    port,
                    last_used: Instant::now(),
                },
            );
            manual.snapshot()
        }
        Err(_) => None,
    };
    // File I/O after the guard drops, and off this async worker.
    manual_peers::persist_async(snapshot);
}

/// A fresh pairing invitation: the 6-digit code, the pair link (`qr` — the
/// same string a QR would carry), the address the other side must dial, and
/// when the code stops working.
#[derive(Serialize)]
pub struct PairingInvite {
    pub code: String,
    pub qr: String,
    /// `ip:port` the joiner types next to the code — the SAME endpoint the link's
    /// `a=`/`p=` carry, so what the host screen shows and what the link says can
    /// never disagree. `None` when this machine has no LAN address at all (then
    /// there is nothing to invite with, and the link carries no `a=` either).
    pub addr: Option<String>,
    /// Unix milliseconds at which the code expires, for the UI's countdown.
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
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

/// Start pairing: mint a fresh 6-digit code (valid for 10 minutes) and return
/// it with this device's dial address and the pair link that packs both. Only
/// one code is active at a time — calling this again replaces the previous one.
/// The other device joins by entering the address + code (or pasting the link);
/// cancel it with `cancel_pairing`.
#[tauri::command]
pub fn start_pairing(state: State<'_, AppState>) -> PairingInvite {
    let code = fresh_pairing_code();
    let expires = Instant::now() + crate::consts::PAIRING_CODE_TTL;
    let expires_at = std::time::SystemTime::now() + crate::consts::PAIRING_CODE_TTL;
    // Single-active: replace whatever was minted before.
    if let Ok(mut slot) = state.pairing.session.lock() {
        *slot = Some(PairingSession {
            code: code.clone(),
            expires,
            expires_at,
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
    let addr = ip.map(|ip| format!("{ip}:{port}"));
    let expires_at = expires_at
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    PairingInvite {
        code,
        qr,
        addr,
        expires_at,
    }
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
/// or the pasted `lanbeam://pair…` link) and the 6-digit code; returns the
/// peer's Device ID, name, and the SAS. A wrong or expired code errors with
/// `PairRejected`; an unparseable address with `InvalidAddress`; a dial that
/// never connects with `Unreachable`.
///
/// `expected_id`, when the UI has it (the `d=` of a pasted link), PINS the
/// handshake: the device that answers must be the one the link named, or the
/// connect fails with `IdentityMismatch` before any code is sent. A link the
/// host generated says who the host is — not using that would hand the SAS
/// compare a job the link already did.
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
    expected_id: Option<String>,
) -> Result<ConnectResult, LanBeamError> {
    let target =
        parse_dial_target(&addr).ok_or_else(|| LanBeamError::InvalidAddress(addr.trim().into()))?;
    // Prefer an explicitly-typed code; fall back to one carried in a pasted link.
    let code = pick_code(code, target.code);
    if code.is_empty() {
        return Err(LanBeamError::PairCodeMissing);
    }
    let local_private = *state.identity.private_bytes();
    // Pin the handshake to the device the link named when we know it (the UI's
    // `expected_id`, or the link's own `d=`); otherwise TOFU — the identity is
    // learned from the handshake and the returned SAS is the out-of-band MITM
    // check the UI surfaces. An `expected_id` that is not a device id at all is
    // ignored rather than fatal: it only ever tightens the check.
    let expected = expected_id
        .as_deref()
        .and_then(transport::decode_device_id)
        .or(target.expected);
    let mut sess = match expected {
        Some(expected) => {
            transport::connect(target.ip, target.port, &local_private, expected).await?
        }
        None => transport::connect_unpinned(target.ip, target.port, &local_private).await?,
    };
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
    // Tell the host our own listen port so it can record us as a dialable peer
    // (pairing is for peers discovery cannot see — in BOTH directions).
    let my_port = state.tcp_port.load(Ordering::Relaxed);
    transfer::send_pair_request(&mut sess, &code, Some(my_port)).await?;
    let (accept, peer_name, reason) = transfer::recv_pair_confirm(&mut sess).await?;
    if !accept {
        // Its own kind, so the UI can say "wrong or expired code" instead of
        // "the connection failed"; the host's reason token rides in the message.
        return Err(LanBeamError::PairRejected(
            reason.unwrap_or_else(|| "pairing rejected".into()),
        ));
    }
    let name = transfer::peer_display_name(Some(&peer_name), &device_id, || None);
    // NO trust is granted here — the UI records it once the human has compared
    // the SAS below. See this command's doc comment.
    //
    // Remember the dial target though, exactly as `connect_by_addr` does. Pairing by
    // code/IP is the case where discovery CANNOT see the peer (a different subnet,
    // or discovery off), so without this the just-trusted device would be absent
    // from `list_discovered_devices` and `resolve_peer` — a trusted peer no later
    // `send_text`/`send_files` could reach (PeerNotFound). The manual table keeps
    // it visible and dialable, independent of the discovery table's expiry.
    remember_manual_peer(&state, &device_id, &name, target.ip, target.port);
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
    let target =
        parse_dial_target(&addr).ok_or_else(|| LanBeamError::InvalidAddress(addr.trim().into()))?;
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
    // Bow out cleanly: the responder swallows the Bye.
    transfer::send_bye(&mut sess).await?;
    let name = transfer::peer_display_name(peer.name.as_deref(), &device_id, || None);
    // Remember it so `list_discovered_devices` shows it and a later send can
    // reach it — discovery never announced it (a different subnet, or discovery
    // off on that box), so it lives in the manual table rather than the
    // discovery table (which the expiry loop would evict).
    remember_manual_peer(&state, &device_id, &name, target.ip, target.port);
    Ok(ConnectResult {
        device_id,
        name,
        sas,
    })
}

/// A parsed dial target from `join_by_code` / `connect_by_addr`: the IPv4
/// endpoint plus whatever a pasted pair link carried along.
struct DialTarget {
    ip: Ipv4Addr,
    port: u16,
    /// The `c=` code from a `lanbeam://pair` link, if present.
    code: Option<String>,
    /// The `d=` Device ID from a `lanbeam://pair` link, decoded, if present and
    /// well-formed. `join_by_code` pins the handshake to it.
    expected: Option<[u8; 32]>,
}

/// Parse a dial target from either a plain `ip[:port]` or a `lanbeam://pair?…`
/// deep link. `None` for anything that does not yield a valid IPv4 address —
/// including any OTHER `lanbeam://` command: only a pair link is an invitation
/// (the frontend's deep-link allowlist makes the same call), so a `connect` or
/// `inbox` link pasted here is a typo, not an address.
/// `port` defaults to LanBeam's default TCP port when absent.
fn parse_dial_target(input: &str) -> Option<DialTarget> {
    let s = input.trim();
    // The scheme is case-insensitive (an OS or chat app may re-case it).
    if s.len() >= "lanbeam://".len() && s[.."lanbeam://".len()].eq_ignore_ascii_case("lanbeam://") {
        let rest = &s["lanbeam://".len()..];
        let (cmd, _) = rest.split_once('?').unwrap_or((rest, ""));
        if !cmd.trim_end_matches('/').eq_ignore_ascii_case("pair") {
            return None;
        }
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
        expected: None,
    })
}

/// Parse the `pair?…` tail of a `lanbeam://pair` link into a dial target.
/// Reads `a` (IPv4), `p` (port), `c` (code) and `d` (the host's Device ID, kept
/// so the joiner can pin the handshake to it); `n` is display-only and ignored.
fn parse_pair_uri(rest: &str) -> Option<DialTarget> {
    let query = rest.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut ip: Option<Ipv4Addr> = None;
    let mut port = DEFAULT_TCP_PORT;
    let mut code: Option<String> = None;
    let mut expected: Option<[u8; 32]> = None;
    for pair in query.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        match k {
            "a" => ip = percent_decode(v).parse().ok(),
            // A malformed `d=` is simply not a pin (the handshake still learns
            // the real identity); it never makes the link unusable.
            "d" => expected = transport::decode_device_id(&percent_decode(v)),
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
    ip.map(|ip| DialTarget {
        ip,
        port,
        code,
        expected,
    })
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
pub(super) fn first_lan_ip(filter: Option<Ipv4Addr>) -> Option<Ipv4Addr> {
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
pub(super) fn lan_ip_rank(ip: Ipv4Addr) -> u8 {
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
    // Normalized the same way the host compares it: digits only, full-width
    // folded — a code typed as it is DISPLAYED ("482 913") must work.
    let entered = transfer::normalize_pair_code(&entered);
    if !entered.is_empty() {
        entered
    } else {
        from_link
            .map(|c| transfer::normalize_pair_code(&c))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(
            t.expected.is_none(),
            "a d= that is not a 32-byte id is no pin, not an error"
        );

        // The scheme is case-insensitive; the command must be `pair`.
        assert!(parse_dial_target("LANBEAM://pair?a=192.168.1.9").is_some());
        assert!(parse_dial_target("lanbeam://PAIR/?a=192.168.1.9").is_some());
        assert!(
            parse_dial_target("lanbeam://connect?a=192.168.1.9:51704").is_none(),
            "only a pair link is an invitation"
        );
        assert!(parse_dial_target("lanbeam://inbox").is_none());

        // A well-formed d= is decoded so the join can pin the handshake to it.
        let id = URL_SAFE_NO_PAD.encode([7u8; 32]);
        let t = parse_dial_target(&format!("lanbeam://pair?d={id}&a=10.0.0.2&c=111111"))
            .expect("link parses");
        assert_eq!(t.expected, Some([7u8; 32]));

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

    /// `pick_code` prefers an explicitly-entered code (normalized), falls back
    /// to a pasted link's code, and yields empty when neither is present.
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
            "a blank entry falls back to the pasted link's code"
        );
        assert_eq!(
            pick_code(String::new(), None),
            "",
            "nothing entered and no link yields an empty code"
        );
        // Typed the way the host screen shows it, or through a CJK IME.
        assert_eq!(
            pick_code("482 913".into(), None),
            "482913",
            "the display grouping space is dropped"
        );
        assert_eq!(
            pick_code("４８２９１３".into(), None),
            "482913",
            "full-width digits fold onto ASCII"
        );
        assert_eq!(
            pick_code("abc".into(), Some("12 34 56".into())),
            "123456",
            "an entry with no digits at all is blank, and the link's code is normalized too"
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

    /// The joiner's half of F1: the address `join_by_code` / `connect_by_addr`
    /// records reaches the disk, so the device is still dialable after a restart.
    #[test]
    fn remembered_address_is_written_to_disk() {
        let dir = std::env::temp_dir().join(format!("lanbeam-remember-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("manual_peers.json");
        let state = crate::commands::test_support::test_app_state();
        *state.manual_peers.lock().unwrap() = manual_peers::ManualPeers::load(file.clone());

        remember_manual_peer(&state, "nas", "NAS", Ipv4Addr::new(10, 2, 0, 9), 52000);

        // Written on the blocking pool — give it a moment.
        let mut reloaded = None;
        for _ in 0..200 {
            if let Some(p) = manual_peers::ManualPeers::load(file.clone()).get("nas") {
                reloaded = Some((p.name.clone(), p.addr, p.port));
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            reloaded,
            Some(("NAS".to_string(), Ipv4Addr::new(10, 2, 0, 9), 52000)),
            "the address survives a restart"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
