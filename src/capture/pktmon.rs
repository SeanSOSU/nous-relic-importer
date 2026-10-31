//! Windows' built-in packet monitor, filtered to the game's UDP ports. Needs administrator rights.

use futures::{Stream, StreamExt, future};
use pktmon::filter::{PktMonFilter, TransportProtocol};
use pktmon::{Capture, PacketPayload};
use sync_core::packets::{GAME_PORTS, ethernet_frame, ip_packet, transport};

pub fn start() -> Result<impl Stream<Item = Vec<u8>> + Unpin, String> {
    let mut capture = Capture::new().map_err(|e| e.to_string())?;
    for port in GAME_PORTS {
        let filter = PktMonFilter {
            name: format!("Nous Relic Importer {port}"),
            transport_protocol: Some(TransportProtocol::UDP),
            port: port.into(),
            ..PktMonFilter::default()
        };
        capture.add_filter(filter).map_err(|e| e.to_string())?;
    }
    capture.start().map_err(|e| e.to_string())?;
    let stream = capture.stream().map_err(|e| e.to_string())?;
    Ok(stream.filter_map(|packet| future::ready(as_ethernet(packet.payload))))
}

const LINK_RAW_IP: u32 = 101;
const LINK_WIFI: u32 = 105;

/// The decoder reads Ethernet frames, but pktmon hands over each packet as the component it passed saw it: a
/// VPN or ping booster's adapter carries bare IP, a Wi-Fi card's own layer 802.11 frames. Dropping those left
/// the game unseen behind a VPN.
fn as_ethernet(payload: PacketPayload) -> Option<Vec<u8>> {
    match payload {
        PacketPayload::Ethernet(frame) => Some(frame),
        PacketPayload::IP(data) => rewrap(LINK_RAW_IP, &data),
        PacketPayload::WiFi(data) => rewrap(LINK_WIFI, &data),
        // Windows 10's backend names only Ethernet and Wi-Fi: anything else may be bare IP.
        PacketPayload::Unknown(data) => rewrap(LINK_RAW_IP, &data).or(Some(data)),
        _ => None,
    }
}

fn rewrap(link: u32, data: &[u8]) -> Option<Vec<u8>> {
    let (ethertype, ip) = ip_packet(link, data)?;
    transport(ip).is_game().then(|| ethernet_frame(ethertype, ip))
}

#[cfg(test)]
mod tests {
    use pktmon::PacketPayload;
    use sync_core::packets::{ethernet_frame, ip_in_ethernet};

    use super::as_ethernet;

    fn game_ip() -> Vec<u8> {
        let mut ip = vec![0x45, 0, 0, 32, 0, 0, 0, 0, 64, 17, 0, 0, 10, 0, 0, 1, 10, 0, 0, 2];
        ip.extend([0xc3, 0x50, 0x5b, 0x05, 0, 12, 0, 0, 1, 2, 3, 4]); // 50000 -> 23301
        ip
    }

    #[test]
    fn every_payload_the_game_can_arrive_in_becomes_one_ethernet_frame() {
        let ip = game_ip();
        let ethernet = ethernet_frame(0x0800, &ip);
        let wifi = [&[0x88, 0x01][..], &[0; 24], &[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00], &ip].concat();
        for (name, payload) in [
            ("Ethernet", PacketPayload::Ethernet(ethernet.clone())),
            ("IP", PacketPayload::IP(ip.clone())),
            ("Wi-Fi", PacketPayload::WiFi(wifi)),
            ("Windows 10's bare IP", PacketPayload::Unknown(ip.clone())),
            ("Windows 10's Ethernet", PacketPayload::Unknown(ethernet.clone())),
        ] {
            let frame = as_ethernet(payload).unwrap_or_else(|| panic!("{name} was dropped"));
            assert_eq!(ip_in_ethernet(&frame).map(|(_, ip)| ip.to_vec()), Some(ip.clone()), "{name}");
        }
        assert_eq!(as_ethernet(PacketPayload::UDP(ip)), None);
    }
}
