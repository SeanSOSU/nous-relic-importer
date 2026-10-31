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

fn by_version(ip: &[u8]) -> Option<(u16, &[u8])> {
    match ip.first()? >> 4 {
        4 => Some((0x0800, ip)),
        6 => Some((0x86dd, ip)),
        _ => None,
    }
}

/// The IP packet inside an unencrypted 802.11 data frame: after its MAC header and LLC/SNAP header.
fn ip_in_wifi(frame: &[u8]) -> Option<(u16, &[u8])> {
    let (control, flags) = (*frame.first()?, *frame.get(1)?);
    if (control >> 2) & 3 != 2 || flags & 0x40 != 0 {
        return None;
    }
    let qos = control & 0x80 != 0;
    let header = 24 + if flags & 3 == 3 { 6 } else { 0 } + if qos { 2 } else { 0 } + if qos && flags & 0x80 != 0 { 4 } else { 0 };
    let snap = frame.get(header..header + 8)?;
    if snap[..6] != [0xaa, 0xaa, 0x03, 0, 0, 0] {
        return None;
    }
    match u16::from_be_bytes([snap[6], snap[7]]) {
        ethertype @ (0x0800 | 0x86dd) => Some((ethertype, &frame[header + 8..])),
        _ => None,
    }
}

/// The IP packet inside a captured frame, with its EtherType.
pub fn ip_packet(link: u32, data: &[u8]) -> Option<(u16, &[u8])> {
    match link {
        // Ethernet, possibly VLAN-tagged.
        1 => ip_in_ethernet(data),
        105 => ip_in_wifi(data),
        // Raw IP: PCAPdroid on Android, macOS rvictl and many tcpdump captures.
        12 | 14 | 101 => by_version(data),
        228 => Some((0x0800, data)),
        229 => Some((0x86dd, data)),
        // BSD loopback: a four-byte address family first.
        0 | 108 => by_version(data.get(4..)?),
        // Linux "any" device, cooked headers v1 and v2.
        113 => by_version(data.get(16..)?),
        276 => by_version(data.get(20..)?),
        // macOS PKTAP: its own header, then a frame of the link type it names.
        258 | 149 => {
            let length = u32::from_le_bytes(data.get(0..4)?.try_into().ok()?) as usize;
            let inner = u32::from_le_bytes(data.get(8..12)?.try_into().ok()?);
            ip_packet(inner, data.get(length..)?)
        }
        _ => None,
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
    use super::{Transport, ethernet_frame, ip_in_ethernet, ip_packet, transport};

    const IPV4: [u8; 20] = [0x45, 0, 0, 20, 0, 0, 0, 0, 64, 17, 0, 0, 127, 0, 0, 1, 127, 0, 0, 1];

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

    #[test]
    fn each_link_type_unwraps_to_the_same_ip_packet() {
        let with = |prefix: &[u8]| [prefix, &IPV4[..]].concat();
        for (link, frame) in [
            (101, IPV4.to_vec()),
            (0, with(&[2, 0, 0, 0])),
            (113, with(&[0; 16])),
            (276, with(&[0; 20])),
        ] {
            assert_eq!(ip_packet(link, &frame), Some((0x0800, &IPV4[..])), "link type {link}");
        }
        assert_eq!(ip_packet(147, &IPV4), None, "an unknown link type is skipped, not guessed at");
    }

    #[test]
    fn a_wifi_data_frame_unwraps_after_its_llc_header() {
        const SNAP: [u8; 8] = [0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00];
        let frame = |control: u8, flags: u8, extra: usize| [&[control, flags][..], &[0; 22], &vec![0; extra], &SNAP, &IPV4].concat();
        assert_eq!(ip_packet(105, &frame(0x08, 0x01, 0)), Some((0x0800, &IPV4[..])), "plain data, to the access point");
        assert_eq!(ip_packet(105, &frame(0x88, 0x02, 2)), Some((0x0800, &IPV4[..])), "QoS data, from it");
        assert_eq!(ip_packet(105, &frame(0x88, 0x83, 12)), Some((0x0800, &IPV4[..])), "QoS, four addresses, HT control");
        assert_eq!(ip_packet(105, &frame(0x08, 0x41, 0)), None, "an encrypted body is not read");
        assert_eq!(ip_packet(105, &frame(0x80, 0x00, 0)), None, "a beacon carries no packet");
    }
}
