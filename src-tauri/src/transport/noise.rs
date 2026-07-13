//! `NoiseSession`: a Noise_XX_25519_ChaChaPoly_BLAKE2s channel over a `TcpStream`.
//! Mutual static-key authentication + forward secrecy. The 6-digit SAS is derived
//! from the handshake hash (channel binding) for out-of-band MITM verification.

use snow::{Builder, HandshakeState, TransportState};
use tokio::net::TcpStream;

use super::frame::{read_frame, write_frame};
use crate::consts::{
    HANDSHAKE_FRAME_TIMEOUT, MAX_PLAINTEXT, NOISE_PARAMS, NOISE_PROLOGUE, NOISE_TAG,
};
use crate::error::{LanBeamError, Result};

const HS_BUF: usize = 65535;

pub struct NoiseSession {
    stream: TcpStream,
    transport: TransportState,
    sas: String,
    remote_static: [u8; 32],
    // Reused ciphertext scratch for `write_msg`, sized to the largest possible
    // transport frame (MAX_PLAINTEXT + tag). Reusing it avoids a fresh
    // allocation + zero-fill on every 64 KiB chunk on the send hot path.
    scratch: Vec<u8>,
}

impl NoiseSession {
    /// Outbound (dialer). XX flow: `-> e` / `<- e,ee,s,es` / `-> s,se`.
    /// When `expected_remote` is `Some`, the peer's static key is pinned before msg 3.
    pub async fn handshake_initiator(
        mut stream: TcpStream,
        local_private: &[u8; 32],
        expected_remote: Option<[u8; 32]>,
    ) -> Result<NoiseSession> {
        let mut hs = build(local_private, true)?;
        send_hs(&mut stream, &mut hs).await?; // -> e
        recv_hs(&mut stream, &mut hs).await?; // <- e,ee,s,es
        let remote = remote_static(&hs)?;
        if let Some(exp) = expected_remote {
            if remote != exp {
                return Err(LanBeamError::IdentityMismatch);
            }
        }
        send_hs(&mut stream, &mut hs).await?; // -> s,se
        finish(stream, hs, remote)
    }

    /// Inbound (listener). Learns and returns the remote static key.
    pub async fn handshake_responder(
        mut stream: TcpStream,
        local_private: &[u8; 32],
    ) -> Result<NoiseSession> {
        let mut hs = build(local_private, false)?;
        recv_hs(&mut stream, &mut hs).await?; // <- e
        send_hs(&mut stream, &mut hs).await?; // -> e,ee,s,es
        recv_hs(&mut stream, &mut hs).await?; // <- s,se
        let remote = remote_static(&hs)?;
        finish(stream, hs, remote)
    }

    pub async fn write_msg(&mut self, plaintext: &[u8]) -> Result<()> {
        if plaintext.len() > MAX_PLAINTEXT {
            return Err(LanBeamError::Protocol(
                "plaintext exceeds 65519 bytes".into(),
            ));
        }
        // `scratch` is pre-sized to the max frame, so it holds any plaintext up
        // to MAX_PLAINTEXT plus the tag; write into it and frame the first `n`.
        let n = self.transport.write_message(plaintext, &mut self.scratch)?;
        write_frame(&mut self.stream, &self.scratch[..n]).await
    }

    #[allow(dead_code)] // used by tests + the M3 receive loop
    pub async fn read_msg(&mut self) -> Result<Option<Vec<u8>>> {
        let frame = match read_frame(&mut self.stream).await? {
            Some(f) => f,
            None => return Ok(None),
        };
        let mut buf = vec![0u8; frame.len()];
        let n = self.transport.read_message(&frame, &mut buf)?;
        buf.truncate(n);
        Ok(Some(buf))
    }

    pub fn sas(&self) -> &str {
        &self.sas
    }

    #[allow(dead_code)] // used by the transfer trust check in M3/M4
    pub fn remote_static(&self) -> &[u8; 32] {
        &self.remote_static
    }

    /// The remote socket address of the underlying TCP stream — the source IP
    /// the pairing throttle keys on (M7.1). `None` when the peer address is
    /// unavailable (the connection was already torn down).
    pub fn peer_addr(&self) -> Option<std::net::SocketAddr> {
        self.stream.peer_addr().ok()
    }
}

fn build(local_private: &[u8; 32], initiator: bool) -> Result<HandshakeState> {
    let params = NOISE_PARAMS
        .parse()
        .map_err(|_| LanBeamError::Crypto("invalid noise params".into()))?;
    let builder = Builder::new(params)
        .prologue(NOISE_PROLOGUE)
        .local_private_key(local_private);
    let hs = if initiator {
        builder.build_initiator()?
    } else {
        builder.build_responder()?
    };
    Ok(hs)
}

// WHY the per-frame deadlines below: the listener runs a responder handshake
// for ANY TCP client, and the initiator trusts an unauthenticated discovery
// hint — either side stalling (or a stranger connecting and going silent)
// must release the task within HANDSHAKE_FRAME_TIMEOUT, not never (M4.5).

async fn send_hs(stream: &mut TcpStream, hs: &mut HandshakeState) -> Result<()> {
    let mut buf = vec![0u8; HS_BUF];
    let n = hs.write_message(&[], &mut buf)?;
    match tokio::time::timeout(HANDSHAKE_FRAME_TIMEOUT, write_frame(stream, &buf[..n])).await {
        Ok(res) => res,
        Err(_) => Err(LanBeamError::Timeout),
    }
}

async fn recv_hs(stream: &mut TcpStream, hs: &mut HandshakeState) -> Result<()> {
    let frame = match tokio::time::timeout(HANDSHAKE_FRAME_TIMEOUT, read_frame(stream)).await {
        Ok(res) => res?,
        Err(_) => return Err(LanBeamError::Timeout),
    }
    .ok_or_else(|| LanBeamError::Handshake("connection closed during handshake".into()))?;
    let mut buf = vec![0u8; HS_BUF];
    hs.read_message(&frame, &mut buf)?;
    Ok(())
}

fn remote_static(hs: &HandshakeState) -> Result<[u8; 32]> {
    hs.get_remote_static()
        .ok_or_else(|| LanBeamError::Handshake("no remote static key".into()))?
        .try_into()
        .map_err(|_| LanBeamError::Handshake("unexpected remote static length".into()))
}

/// Capture the channel-binding hash + remote key, then transition to transport mode.
fn finish(stream: TcpStream, hs: HandshakeState, remote: [u8; 32]) -> Result<NoiseSession> {
    let sas = sas_code(hs.get_handshake_hash());
    let transport = hs.into_transport_mode()?;
    Ok(NoiseSession {
        stream,
        transport,
        sas,
        remote_static: remote,
        scratch: vec![0u8; MAX_PLAINTEXT + NOISE_TAG],
    })
}

/// 6-digit SAS from the first 8 bytes of the handshake hash (big-endian). DESIGN §0.5.
pub fn sas_code(h: &[u8]) -> String {
    let mut b = [0u8; 8];
    b.copy_from_slice(&h[0..8]);
    format!("{:06}", u64::from_be_bytes(b) % 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn keypair() -> ([u8; 32], [u8; 32]) {
        let kp = Builder::new(NOISE_PARAMS.parse().unwrap())
            .generate_keypair()
            .unwrap();
        (
            kp.public.as_slice().try_into().unwrap(),
            kp.private.as_slice().try_into().unwrap(),
        )
    }

    #[test]
    fn sas_code_is_zero_padded_to_six_digits() {
        // Big-endian value 42 -> "000042".
        let h = [0u8, 0, 0, 0, 0, 0, 0, 42];
        assert_eq!(sas_code(&h), "000042");
        assert_eq!(sas_code(&h).len(), 6);
    }

    #[test]
    fn sas_code_all_zero_hash_is_all_zero_code() {
        let h = [0u8; 32];
        assert_eq!(sas_code(&h), "000000");
    }

    #[test]
    fn sas_code_takes_modulo_one_million() {
        // u64::MAX = 18446744073709551615 -> last six decimal digits are 551615.
        let h = [0xffu8; 8];
        assert_eq!(sas_code(&h), "551615");
    }

    #[test]
    fn sas_code_only_reads_first_eight_bytes() {
        // Trailing bytes beyond the first 8 must not affect the derived code.
        let mut a = [0u8; 32];
        a[7] = 7;
        let mut b = [0xffu8; 32];
        b[0..8].copy_from_slice(&[0, 0, 0, 0, 0, 0, 0, 7]);
        assert_eq!(sas_code(&a), sas_code(&b));
        assert_eq!(sas_code(&a), "000007");
    }

    #[tokio::test]
    async fn responder_handshake_learns_remote_and_matches_sas() {
        let (a_pub, a_priv) = keypair();
        let (b_pub, b_priv) = keypair();

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let sess = NoiseSession::handshake_responder(stream, &b_priv)
                .await
                .unwrap();
            (sess.sas().to_string(), *sess.remote_static())
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        let client = NoiseSession::handshake_initiator(stream, &a_priv, Some(b_pub))
            .await
            .unwrap();

        let (server_sas, server_saw) = server.await.unwrap();
        assert_eq!(client.sas(), server_sas);
        assert_eq!(client.sas().len(), 6);
        assert_eq!(*client.remote_static(), b_pub);
        assert_eq!(server_saw, a_pub);
        // The learned peer address is reachable through the accessor.
        assert!(client.peer_addr().is_some());
    }
}
