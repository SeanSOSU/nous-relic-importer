//! The game's packets as any capture sees them: which IP packets are the game's, and the emulator's TCP stream
//! put back in order. Shared by the app's live capture and the capture-file decoder.

use crate::frames::frames;

/// The live game's KCP-over-UDP ports.
pub const GAME_PORTS: [u16; 2] = [23301, 23302];
/// The PearlSR emulator serves the same protocol over plain TCP on this port.
pub const EMULATOR_PORT: u16 = 23301;

pub enum Transport<'a> {
    Udp { src: u16, dst: u16 },
    Tcp { src: u16, dst: u16, seq: u32, syn: bool, payload: &'a [u8] },
    Other,
}

impl Transport<'_> {
    /// One of the live game's packets.
    pub fn is_game(&self) -> bool {
        matches!(self, Transport::Udp { src, dst } if GAME_PORTS.contains(src) || GAME_PORTS.contains(dst))
    }
}

/// The transport inside an IPv4 or IPv6 packet.
pub fn transport(ip: &[u8]) -> Transport<'_> {
    let (protocol, l4) = match ip.first().map(|b| b >> 4) {
        Some(4) => {
            let header = usize::from(ip[0] & 0x0f) * 4;
            let total = ip.get(2..4).map_or(ip.len(), |b| usize::from(u16::from_be_bytes([b[0], b[1]])));
            (ip.get(9).copied(), ip.get(header..total.min(ip.len())))
        }
        Some(6) => {
            let length = ip.get(4..6).map_or(0, |b| usize::from(u16::from_be_bytes([b[0], b[1]])));
            (ip.get(6).copied(), ip.get(40..(40 + length).min(ip.len())))
        }
        _ => return Transport::Other,
    };
    let Some(l4) = l4.filter(|l| l.len() >= 8) else { return Transport::Other };
    let (src, dst) = (u16::from_be_bytes([l4[0], l4[1]]), u16::from_be_bytes([l4[2], l4[3]]));
    match protocol {
        Some(17) => Transport::Udp { src, dst },
        Some(6) if l4.len() >= 20 => {
            let offset = usize::from(l4[12] >> 4) * 4;
            Transport::Tcp {
                src,
                dst,
                seq: u32::from_be_bytes([l4[4], l4[5], l4[6], l4[7]]),
                syn: l4[13] & 0x02 != 0,
                payload: l4.get(offset..).unwrap_or_default(),
            }
        }
        _ => Transport::Other,
    }
}

/// reliquary reads Ethernet frames; a capture of one app's traffic is often bare IP.
pub fn ethernet_frame(ethertype: u16, ip: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(14 + ip.len());
    frame.extend_from_slice(&[0; 12]);
    frame.extend_from_slice(&ethertype.to_be_bytes());
    frame.extend_from_slice(ip);
    frame
}

/// The IP packet inside an Ethernet frame, and its ethertype. Skips VLAN tags.
pub fn ip_in_ethernet(frame: &[u8]) -> Option<(u16, &[u8])> {
    let mut at = 12;
    loop {
        let ethertype = u16::from_be_bytes(frame.get(at..at + 2)?.try_into().ok()?);
        match ethertype {
            0x8100 | 0x88a8 => at += 4,
            0x0800 | 0x86dd => return Some((ethertype, frame.get(at + 2..)?)),
            _ => return None,
        }
    }
}

/// One direction of one TCP connection, put back in order.
#[derive(Default)]
pub struct TcpStream {
    next_seq: Option<u32>,
    buffer: Vec<u8>,
}

impl TcpStream {
    /// Takes one segment and returns the whole game frames it completes.
    pub fn push(&mut self, seq: u32, syn: bool, payload: &[u8]) -> Vec<Vec<u8>> {
        let seq = if syn {
            self.buffer.clear();
            self.next_seq = Some(seq.wrapping_add(1));
            seq.wrapping_add(1)
        } else {
            seq
        };
        if payload.is_empty() {
            return Vec::new();
        }
        match self.next_seq {
            Some(next) if seq == next => self.buffer.extend_from_slice(payload),
            // A resend overlapping bytes already taken: keep only what is new.
            Some(next) if (next.wrapping_sub(seq) as i32) > 0 => {
                let taken = next.wrapping_sub(seq) as usize;
                if taken >= payload.len() {
                    return Vec::new();
                }
                self.buffer.extend_from_slice(&payload[taken..]);
            }
            // First sight of the stream, or a gap: start over and resynchronise on the next frame.
            _ => {
                self.buffer.clear();
                self.buffer.extend_from_slice(payload);
            }
        }
        self.next_seq = Some(seq.wrapping_add(payload.len() as u32));
        frames(&mut self.buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::{Transport, ethernet_frame, ip_in_ethernet, transport};

    fn ipv4_udp(src: u16, dst: u16) -> Vec<u8> {
        let mut ip = vec![0x45, 0, 0, 28, 0, 0, 0, 0, 64, 17, 0, 0, 10, 0, 0, 1, 10, 0, 0, 2];
        ip.extend(src.to_be_bytes());
        ip.extend(dst.to_be_bytes());
        ip.extend([0, 8, 0, 0]);
        ip
    }

    #[test]
    fn game_packets_are_udp_on_either_game_port() {
        assert!(transport(&ipv4_udp(51000, 23301)).is_game());
        assert!(transport(&ipv4_udp(23302, 51000)).is_game());
        assert!(!transport(&ipv4_udp(51000, 443)).is_game());
        let frame = ethernet_frame(0x0800, &ipv4_udp(23301, 51000));
        let (ethertype, ip) = ip_in_ethernet(&frame).unwrap();
        assert_eq!(ethertype, 0x0800);
        assert!(matches!(transport(ip), Transport::Udp { src: 23301, dst: 51000 }));
    }
}
