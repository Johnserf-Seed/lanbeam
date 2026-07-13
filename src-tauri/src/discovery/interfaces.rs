//! IPv4 interface enumeration (broadcast read directly from `if-addrs`, no netmask math).

use std::net::Ipv4Addr;

#[derive(Clone, Copy)]
pub struct Iface {
    pub ip: Ipv4Addr,
    pub broadcast: Option<Ipv4Addr>,
}

/// Non-loopback IPv4 interfaces. Multicast loopback covers same-box discovery.
pub fn enumerate() -> Vec<Iface> {
    let mut out = Vec::new();
    if let Ok(list) = if_addrs::get_if_addrs() {
        for iface in list {
            if iface.is_loopback() {
                continue;
            }
            if let if_addrs::IfAddr::V4(v4) = iface.addr {
                out.push(Iface {
                    ip: v4.ip,
                    broadcast: v4.broadcast,
                });
            }
        }
    }
    out
}

/// Narrow an enumeration to the user-selected interface (M5.6). Returns the
/// kept set plus whether the filter fell back. Pure — takes the enumeration
/// as input — so the selection matrix is unit-testable without real NICs.
///
/// WHY the fallback: the filter stores an IP, and IPs are not stable — DHCP
/// renumbers, the NIC gets unplugged, a VPN tears down. A stale filter must
/// degrade discovery to "all interfaces" (the pre-filter behavior), never to
/// none, or the device silently vanishes from the LAN with nothing in the UI
/// to explain why. The caller logs the fallback so the drift is visible.
pub fn select(all: Vec<Iface>, filter: Option<Ipv4Addr>) -> (Vec<Iface>, bool) {
    let Some(ip) = filter else {
        return (all, false);
    };
    let matched: Vec<Iface> = all.iter().filter(|i| i.ip == ip).copied().collect();
    if matched.is_empty() {
        (all, true) // nothing matches — fall back to every interface
    } else {
        (matched, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(a: u8, b: u8, c: u8, d: u8) -> Iface {
        Iface {
            ip: Ipv4Addr::new(a, b, c, d),
            broadcast: Some(Ipv4Addr::new(a, b, c, 255)),
        }
    }

    /// No filter = the untouched enumeration (the default for every machine).
    #[test]
    fn select_without_filter_keeps_everything() {
        let all = vec![iface(192, 168, 1, 5), iface(10, 0, 0, 7)];
        let (kept, fell_back) = select(all, None);
        assert_eq!(kept.len(), 2);
        assert!(!fell_back);
    }

    /// A matching filter keeps ONLY the selected interface.
    #[test]
    fn select_with_matching_filter_keeps_only_that_interface() {
        let all = vec![iface(192, 168, 1, 5), iface(10, 0, 0, 7)];
        let (kept, fell_back) = select(all, Some(Ipv4Addr::new(10, 0, 0, 7)));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].ip, Ipv4Addr::new(10, 0, 0, 7));
        assert!(!fell_back);
    }

    /// A stale filter (interface gone) must fall back to ALL interfaces and
    /// report it — never strand discovery on an empty set.
    #[test]
    fn select_with_stale_filter_falls_back_to_all() {
        let all = vec![iface(192, 168, 1, 5), iface(10, 0, 0, 7)];
        let (kept, fell_back) = select(all, Some(Ipv4Addr::new(172, 16, 0, 1)));
        assert_eq!(kept.len(), 2, "fallback must keep the full enumeration");
        assert!(fell_back, "the caller needs to know, to log the drift");
    }

    /// Degenerate case: no interfaces at all (airplane mode). The result is
    /// the empty set either way; the fallback flag still fires because the
    /// filter genuinely matched nothing — the caller's warn-once latch keeps
    /// that from flooding the log while the machine is link-down.
    #[test]
    fn select_on_empty_enumeration_reports_fallback_once_worthy() {
        let (kept, fell_back) = select(Vec::new(), Some(Ipv4Addr::new(10, 0, 0, 7)));
        assert!(kept.is_empty());
        assert!(
            fell_back,
            "an empty enumeration cannot match — fallback is reported"
        );
    }
}
