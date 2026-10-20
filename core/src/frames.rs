//! The game's message frames, as they appear in a plain byte stream (the PearlSR emulator's TCP connection;
//! the live game's arrive inside KCP and are reassembled by reliquary instead).

pub const HEAD_MAGIC: [u8; 4] = 0x9D74_C714u32.to_be_bytes();
pub const TAIL_MAGIC: [u8; 4] = 0xD7A1_52C8u32.to_be_bytes();

/// Removes every complete game frame from the front of `buffer` (magic, command id, header and body lengths,
/// header, body, tail magic), skipping bytes that do not start one.
pub fn frames(buffer: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        match buffer.windows(4).position(|w| w == HEAD_MAGIC) {
            Some(0) => {}
            Some(start) => {
                buffer.drain(..start);
            }
            None => {
                let keep = buffer.len().min(3);
                buffer.drain(..buffer.len() - keep);
                return out;
            }
        }
        if buffer.len() < 12 {
            return out;
        }
        let header = usize::from(u16::from_be_bytes([buffer[6], buffer[7]]));
        let body = u32::from_be_bytes([buffer[8], buffer[9], buffer[10], buffer[11]]) as usize;
        if body > 0xFF_FFFF {
            buffer.drain(..4);
            continue;
        }
        let total = 12 + header + body + 4;
        if buffer.len() < total {
            return out;
        }
        if buffer[total - 4..total] != TAIL_MAGIC {
            buffer.drain(..4);
            continue;
        }
        out.push(buffer.drain(..total).collect());
    }
}

#[cfg(test)]
mod tests {
    use super::{HEAD_MAGIC, TAIL_MAGIC, frames};

    pub fn frame(command: u16, body: &[u8]) -> Vec<u8> {
        let mut f = HEAD_MAGIC.to_vec();
        f.extend(command.to_be_bytes());
        f.extend(2u16.to_be_bytes());
        f.extend((body.len() as u32).to_be_bytes());
        f.extend([0xAA, 0xBB]);
        f.extend(body);
        f.extend(TAIL_MAGIC);
        f
    }

    #[test]
    fn frames_arriving_in_pieces_and_after_noise_come_out_whole() {
        let (a, b) = (frame(16, b"token"), frame(525, &[7; 300]));
        let mut wire = b"noise".to_vec();
        wire.extend(&a);
        wire.extend(&b);

        let mut buffer = Vec::new();
        let mut out = Vec::new();
        for piece in wire.chunks(37) {
            buffer.extend_from_slice(piece);
            out.extend(frames(&mut buffer));
        }
        assert_eq!(out, vec![a, b]);
        assert!(buffer.is_empty());
    }
}
