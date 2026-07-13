//! The UDP discovery packet (compact JSON, one datagram). DESIGN §1.1.
//! These are UNAUTHENTICATED HINTS — `id` is proven later at the Noise handshake,
//! never a basis for trust.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DiscoveryPacket {
    /// schema version == PROTO_VERSION
    pub v: u8,
    /// base64url X25519 static public key (Device ID)
    pub id: String,
    /// TCP transfer-listener port on the sender
    pub port: u16,
    /// human device name (sender clamps to <= 63 bytes)
    pub name: String,
    /// discoverable: false => "going invisible" (peers remove me)
    pub disc: bool,
    /// solicitation: true => "reply to me immediately (unicast)"
    pub req: bool,
    /// browser-share HTTP port (M8.3), present ONLY while this device has at
    /// least one live browser share. ADDITIVE + OPTIONAL: `skip_serializing_if`
    /// omits the key entirely when `None`, so a packet with no share is
    /// byte-identical to a pre-M8 one and stays well under the 2048-byte recv
    /// buffer; `#[serde(default)]` makes a packet WITHOUT the key deserialize to
    /// `None`, so a legacy peer's datagram round-trips and a legacy peer simply
    /// ignores ours. Peers MAY surface "browser link available" from this, but
    /// consuming it is frontend-optional — the discovery table does not store it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<u16>,
}
