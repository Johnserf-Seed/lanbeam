//! Length-prefix framing over an async stream: 2-byte big-endian length + payload.
//! Every Noise message (handshake or transport) travels as exactly one frame.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{LanBeamError, Result};

/// Write one frame: 2-byte BE length prefix, then `payload` (must be <= 65535).
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> Result<()> {
    if payload.len() > u16::MAX as usize {
        return Err(LanBeamError::Protocol("frame exceeds 65535 bytes".into()));
    }
    let len = (payload.len() as u16).to_be_bytes();
    w.write_all(&len).await?;
    w.write_all(payload).await?;
    w.flush().await?;
    Ok(())
}

/// Read one frame. Returns `Ok(None)` on a clean EOF at a frame boundary.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 2];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u16::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
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
}
