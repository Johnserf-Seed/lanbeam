//! Length-prefix framing over an async stream: 2-byte big-endian length + payload.
//! Every Noise message (handshake or transport) travels as exactly one frame.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{LanBeamError, Result};

/// A failed read or write on the PEER'S socket — a reset, a broken pipe, the
/// network going away — is the connection being lost. It used to fall through
/// `From<io::Error>` into `Io`, which the UI words as "a file or disk error":
/// a peer cancelling mid-transfer told the other side its disk had failed.
fn lost(e: std::io::Error) -> LanBeamError {
    LanBeamError::ConnectionLost(e.to_string())
}

/// Write one frame: 2-byte BE length prefix, then `payload` (must be <= 65535).
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> Result<()> {
    if payload.len() > u16::MAX as usize {
        return Err(LanBeamError::Protocol("frame exceeds 65535 bytes".into()));
    }
    let len = (payload.len() as u16).to_be_bytes();
    w.write_all(&len).await.map_err(lost)?;
    w.write_all(payload).await.map_err(lost)?;
    w.flush().await.map_err(lost)?;
    Ok(())
}

/// Read one frame. Returns `Ok(None)` on a clean EOF at a frame boundary.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 2];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(lost(e)),
    }
    let len = u16::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    // A stream that ends INSIDE a frame is a connection cut mid-message.
    r.read_exact(&mut buf).await.map_err(lost)?;
    Ok(Some(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frame_roundtrip_boundaries() {
        for len in [0usize, 1, 16, 1000, 65535] {
            let payload = vec![0xABu8; len];
            let mut buf = Vec::new();
            write_frame(&mut buf, &payload).await.unwrap();
            assert_eq!(buf.len(), len + 2);
            let mut cursor = std::io::Cursor::new(buf);
            let got = read_frame(&mut cursor).await.unwrap().unwrap();
            assert_eq!(got, payload);
        }
    }

    #[tokio::test]
    async fn read_frame_clean_eof() {
        let mut empty = std::io::Cursor::new(Vec::<u8>::new());
        assert!(read_frame(&mut empty).await.unwrap().is_none());
    }

    /// A stream cut in the middle of a frame is a lost connection, not a disk
    /// error (the UI used to say "a file or disk error occurred").
    #[tokio::test]
    async fn a_frame_cut_short_is_a_lost_connection() {
        let mut cut = std::io::Cursor::new(vec![0x00, 0x10, 0xAB, 0xAB]); // says 16, has 2
        let err = read_frame(&mut cut).await.unwrap_err();
        assert!(
            matches!(err, LanBeamError::ConnectionLost(_)),
            "got {err:?}"
        );
        assert_eq!(err.ui_code(), "connection_lost");
    }

    /// Writing to a peer that has gone away is a lost connection too.
    #[tokio::test]
    async fn writing_to_a_closed_peer_is_a_lost_connection() {
        let (mut ours, theirs) = tokio::io::duplex(64);
        drop(theirs);
        let err = write_frame(&mut ours, b"hello").await.unwrap_err();
        assert!(
            matches!(err, LanBeamError::ConnectionLost(_)),
            "got {err:?}"
        );
    }
}
