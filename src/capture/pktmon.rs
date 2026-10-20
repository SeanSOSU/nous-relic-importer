//! Windows' built-in packet monitor, filtered to the game's UDP ports. Needs administrator rights.

use futures::{Stream, StreamExt, future};
use pktmon::filter::{PktMonFilter, TransportProtocol};
use pktmon::{Capture, PacketPayload};
use sync_core::packets::GAME_PORTS;

pub fn start() -> Result<impl Stream<Item = Vec<u8>> + Unpin, String> {
    let mut capture = Capture::new().map_err(|e| e.to_string())?;
    for port in GAME_PORTS {
        let filter = PktMonFilter {
            name: format!("StarRailSim Sync {port}"),
            transport_protocol: Some(TransportProtocol::UDP),
            port: port.into(),
            ..PktMonFilter::default()
        };
        capture.add_filter(filter).map_err(|e| e.to_string())?;
    }
    capture.start().map_err(|e| e.to_string())?;
    let stream = capture.stream().map_err(|e| e.to_string())?;
    Ok(stream.filter_map(|packet| {
        future::ready(match packet.payload {
            PacketPayload::Ethernet(data) => Some(data),
            _ => None,
        })
    }))
}
