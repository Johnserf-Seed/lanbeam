//! Discovery UDP socket construction via `socket2`, with the ANY-error bind
//! fallback that fixes LanDrop's `EACCES` hang (a Windows peer holding the port
//! with `SO_EXCLUSIVEADDRUSE` yields `PermissionDenied`, not `AddrInUse`).

use std::io;
use std::net::{Ipv4Addr, SocketAddr};

use socket2::{Domain, Protocol, Socket, Type};

use crate::consts::{DISCOVERY_PORT, MULTICAST_TTL};

fn make_socket() -> io::Result<Socket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    // MUST be set before bind so a lingering socket / second instance can't lock the port.
    s.set_reuse_address(true)?;
    s.set_broadcast(true)?;
    Ok(s)
}

/// Returns `(socket, receiving)`. `receiving == false` means the well-known port was
/// unavailable and we fell back to an ephemeral send-only socket (announces still egress).
pub fn build_discovery_socket() -> io::Result<(tokio::net::UdpSocket, bool)> {
    let sock = make_socket()?;
    let well_known: SocketAddr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT));

    let (sock, receiving) = match sock.bind(&well_known.into()) {
        Ok(()) => (sock, true),
        Err(_any) => {
            // ANY error → fresh ephemeral socket (no deadlock).
            let eph = make_socket()?;
            let any: SocketAddr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0));
            eph.bind(&any.into())?;
            (eph, false)
        }
    };

    sock.set_multicast_ttl_v4(MULTICAST_TTL)?;
    sock.set_multicast_loop_v4(true)?;
    sock.set_nonblocking(true)?;

    let std_sock: std::net::UdpSocket = sock.into();
    let tok = tokio::net::UdpSocket::from_std(std_sock)?;
    Ok((tok, receiving))
}

/// A send-only ephemeral socket with the same egress options as the discovery
/// socket (broadcast, multicast TTL + loopback) — for one-shot sends OUTSIDE
/// the announce loop (the reset tombstone, M5.7), where binding the
/// well-known port would needlessly race the running listener. Multicast
/// loopback stays on so a same-machine test instance sees the datagram too.
pub fn build_send_only_socket() -> io::Result<tokio::net::UdpSocket> {
    let sock = make_socket()?;
    let any: SocketAddr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0));
    sock.bind(&any.into())?;
    sock.set_multicast_ttl_v4(MULTICAST_TTL)?;
    sock.set_multicast_loop_v4(true)?;
    sock.set_nonblocking(true)?;
    let std_sock: std::net::UdpSocket = sock.into();
    tokio::net::UdpSocket::from_std(std_sock)
}
