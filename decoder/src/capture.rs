//! Capture files, read just far enough to get each packet and its link type: classic pcap (either byte order,
//! micro- or nanosecond) and pcapng (sections, interfaces, enhanced, simple and obsolete packet blocks), and the
//! link types phones and desktops write, down to the IP packet inside.

pub struct Packet<'a> {
    pub link: u32,
    pub data: &'a [u8],
}

fn u16_at(bytes: &[u8], at: usize, big: bool) -> Option<u16> {
    let b: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
    Some(if big { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) })
}

fn u32_at(bytes: &[u8], at: usize, big: bool) -> Option<u32> {
    let b: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
}

pub fn read_capture(file: &[u8]) -> Result<Vec<Packet<'_>>, String> {
    match file.get(..4) {
        Some([0xd4, 0xc3, 0xb2, 0xa1] | [0x4d, 0x3c, 0xb2, 0xa1]) => Ok(read_pcap(file, false)),
        Some([0xa1, 0xb2, 0xc3, 0xd4] | [0xa1, 0xb2, 0x3c, 0x4d]) => Ok(read_pcap(file, true)),
        Some([0x0a, 0x0d, 0x0d, 0x0a]) => Ok(read_pcapng(file)),
        _ => Err("That is not a capture file: choose the .pcap or .pcapng your capture app saved.".into()),
    }
}

fn read_pcap(file: &[u8], big: bool) -> Vec<Packet<'_>> {
    let link = u32_at(file, 20, big).unwrap_or(0) & 0x0fff_ffff;
    let mut packets = Vec::new();
    let mut at = 24;
    while let Some(captured) = u32_at(file, at + 8, big) {
        let start = at + 16;
        let Some(data) = file.get(start..start + captured as usize) else { break };
        packets.push(Packet { link, data });
        at = start + captured as usize;
    }
    packets
}

fn read_pcapng(file: &[u8]) -> Vec<Packet<'_>> {
    let mut packets = Vec::new();
    let mut interfaces: Vec<u32> = Vec::new();
    let mut big = false;
    let mut at = 0;
    while at + 12 <= file.len() {
        let kind = u32_at(file, at, big).unwrap_or(0);
        if kind == 0x0a0d_0d0a {
            // A section header sets the byte order of everything after it, and starts a new set of interfaces.
            big = file.get(at + 8..at + 12) == Some(&[0x1a, 0x2b, 0x3c, 0x4d][..]);
            interfaces.clear();
        }
        let Some(length) = u32_at(file, at + 4, big).map(|l| l as usize) else { break };
        if length < 12 || at + length > file.len() {
            break;
        }
        let body = &file[at + 8..at + length - 4];
        match kind {
            1 => interfaces.push(u32::from(u16_at(body, 0, big).unwrap_or(0))),
            6 => {
                let interface = u32_at(body, 0, big).unwrap_or(0) as usize;
                let captured = u32_at(body, 12, big).unwrap_or(0) as usize;
                if let (Some(&link), Some(data)) = (interfaces.get(interface), body.get(20..20 + captured)) {
                    packets.push(Packet { link, data });
                }
            }
            3 => {
                let original = u32_at(body, 0, big).unwrap_or(0) as usize;
                if let (Some(&link), Some(data)) = (interfaces.first(), body.get(4..(4 + original).min(body.len()))) {
                    packets.push(Packet { link, data });
                }
            }
            2 => {
                let interface = usize::from(u16_at(body, 0, big).unwrap_or(0));
                let captured = u32_at(body, 12, big).unwrap_or(0) as usize;
                if let (Some(&link), Some(data)) = (interfaces.get(interface), body.get(20..20 + captured)) {
                    packets.push(Packet { link, data });
                }
            }
            _ => {}
        }
        at += length;
    }
    packets
}

#[cfg(test)]
mod tests {
    use super::read_capture;
    use sync_core::packets::ip_packet;

    const IPV4: [u8; 20] = [0x45, 0, 0, 20, 0, 0, 0, 0, 64, 17, 0, 0, 127, 0, 0, 1, 127, 0, 0, 1];

    #[test]
    fn pcap_and_pcapng_both_give_their_packets_and_link_types() {
        let mut pcap = 0xa1b2_c3d4u32.to_le_bytes().to_vec();
        pcap.extend([2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 101, 0, 0, 0]);
        pcap.extend([0; 8]);
        pcap.extend(20u32.to_le_bytes());
        pcap.extend(20u32.to_le_bytes());
        pcap.extend(IPV4);
        let packets = read_capture(&pcap).unwrap();
        assert_eq!((packets.len(), packets[0].link, packets[0].data), (1, 101, &IPV4[..]));

        // Section header, one Ethernet interface, one enhanced packet block.
        let block = |kind: u32, body: Vec<u8>| {
            let length = (12 + body.len()) as u32;
            [kind.to_le_bytes().to_vec(), length.to_le_bytes().to_vec(), body, length.to_le_bytes().to_vec()].concat()
        };
        let mut ethernet = vec![0u8; 12];
        ethernet.extend([0x08, 0x00]);
        ethernet.extend(IPV4);
        let mut epb = vec![0u8; 12];
        epb.extend((ethernet.len() as u32).to_le_bytes());
        epb.extend((ethernet.len() as u32).to_le_bytes());
        epb.extend(&ethernet);
        epb.extend([0u8; 2]);
        let ng = [
            block(0x0a0d_0d0a, [0x4d, 0x3c, 0x2b, 0x1a, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff].to_vec()),
            block(1, [1, 0, 0, 0, 0, 0, 4, 0].to_vec()),
            block(6, epb),
        ]
        .concat();
        let packets = read_capture(&ng).unwrap();
        assert_eq!((packets.len(), packets[0].link), (1, 1));
        assert_eq!(ip_packet(packets[0].link, packets[0].data), Some((0x0800, &IPV4[..])));
    }
}
