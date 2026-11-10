//! Framed mode (herdr-core `schema/README.md`): every frame is
//! `[channel u32 LE][kind u8][len u32 LE][payload]`, payload at most 16 MiB.

use tokio::io::{AsyncRead, AsyncReadExt};

pub const HEADER: usize = 9;
pub const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

/// Channel 0: JSON-RPC both ways; others: one message or a stream's bytes.
pub const DATA: u8 = 0;
/// u32 LE: bytes the receiver consumed.
pub const CREDIT: u8 = 1;
/// The channel is over; from the core, a JSON reason.
pub const CLOSE: u8 = 2;
/// u16 LE cols, u16 LE rows: viewer → core, core → observer.
pub const RESIZE: u8 = 3;
/// Viewer → core: 1 gained focus, 0 lost it.
pub const FOCUS: u8 = 4;
/// Viewer → core: JSON display facts.
pub const FACTS: u8 = 5;
/// Viewer → core: terminal-generated replies, not human activity.
pub const AUTOMATIC_INPUT: u8 = 6;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub channel: u32,
    pub kind: u8,
    pub payload: Vec<u8>,
}

pub fn encode(channel: u32, kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(&channel.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// The next frame, or None at a clean end of the stream.
pub async fn read(r: &mut (impl AsyncRead + Unpin)) -> std::io::Result<Option<Frame>> {
    let mut header = [0u8; HEADER];
    match r.read_exact(&mut header).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let channel = u32::from_le_bytes(header[0..4].try_into().unwrap());
    let len = u32::from_le_bytes(header[5..9].try_into().unwrap()) as usize;
    if len > MAX_PAYLOAD {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("frame of {len} bytes")));
    }
    let mut payload = vec![0; len];
    r.read_exact(&mut payload).await?;
    Ok(Some(Frame { channel, kind: header[4], payload }))
}

pub fn size_payload(cols: u16, rows: u16) -> [u8; 4] {
    let [c0, c1] = cols.to_le_bytes();
    let [r0, r1] = rows.to_le_bytes();
    [c0, c1, r0, r1]
}

pub fn parse_size(payload: &[u8]) -> Option<(u16, u16)> {
    let p: [u8; 4] = payload.get(..4)?.try_into().ok()?;
    Some((u16::from_le_bytes([p[0], p[1]]), u16::from_le_bytes([p[2], p[3]])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip_and_end_cleanly() {
        let mut bytes = encode(7, DATA, b"hello");
        bytes.extend(encode(0, CLOSE, b""));
        let mut r = bytes.as_slice();
        assert_eq!(read(&mut r).await.unwrap(), Some(Frame { channel: 7, kind: DATA, payload: b"hello".to_vec() }));
        assert_eq!(read(&mut r).await.unwrap(), Some(Frame { channel: 0, kind: CLOSE, payload: vec![] }));
        assert_eq!(read(&mut r).await.unwrap(), None);
    }

    #[tokio::test]
    async fn oversized_frames_are_refused() {
        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.push(DATA);
        bytes.extend(((MAX_PAYLOAD + 1) as u32).to_le_bytes());
        assert!(read(&mut bytes.as_slice()).await.is_err());
    }

    #[test]
    fn sizes_are_cols_then_rows() {
        assert_eq!(parse_size(&size_payload(120, 40)), Some((120, 40)));
        assert_eq!(parse_size(&[1, 2]), None);
    }
}
