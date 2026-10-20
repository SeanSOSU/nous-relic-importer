//! Reads the game's traffic with Windows' built-in packet monitor (pktmon, so there is no driver to install)
//! and feeds it through Reliquary's decoder into the archiver's exporter. Adapted from Reliquary Archiver's
//! capture and worker modules (MIT, https://github.com/IceDynamix/reliquary-archiver).

use std::time::Duration;

use futures::{Stream, StreamExt, future};
use pktmon::filter::{PktMonFilter, TransportProtocol};
use pktmon::{Capture, PacketPayload};
use reliquary::network::command::GameCommandError;
use reliquary::network::{ConnectionPacket, GamePacket, GameSniffer, NetworkError};
use tracing::{debug, info, warn};

use crate::accounts::Accounts;
use crate::hub::{Hub, Status};

const GAME_PORTS: [u16; 2] = [23301, 23302];

fn start_capture() -> Result<impl Stream<Item = Vec<u8>> + Unpin, String> {
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

pub async fn run(hub: Hub) {
    let mut sniffer = GameSniffer::new();
    let mut accounts = Accounts::default();

    loop {
        let mut packets = match start_capture() {
            Ok(stream) => stream,
            Err(e) => {
                warn!(%e, "packet capture did not start");
                hub.set_status(Status::Failed(e));
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        if !hub.is_synced() {
            hub.set_status(Status::WaitingForGame);
        }
        info!("capturing");

        while let Some(data) = packets.next().await {
            let packets = match sniffer.receive_packet(data) {
                Ok(packets) => packets,
                Err(NetworkError::GameCommand(GameCommandError::DecryptionKeyMissing)) => {
                    if !accounts.has_login() {
                        hub.set_status(Status::NeedsRelogin);
                    }
                    continue;
                }
                Err(_) => continue,
            };
            for packet in packets {
                let (conv_id, result) = match packet {
                    GamePacket::Connection(ConnectionPacket::HandshakeEstablished { conv_id }) => {
                        info!(conv_id, "game connected");
                        if !hub.is_synced() {
                            hub.set_status(Status::GameFound);
                        }
                        continue;
                    }
                    GamePacket::Connection(ConnectionPacket::Disconnected) => {
                        info!("game disconnected");
                        if !hub.is_synced() {
                            hub.set_status(Status::WaitingForGame);
                        }
                        continue;
                    }
                    GamePacket::Connection(_) => continue,
                    GamePacket::Commands { conv_id, result } => (conv_id, result),
                };
                let command = match result {
                    Ok(command) => command,
                    Err(GameCommandError::DecryptionKeyMissing) => {
                        if !accounts.has_login() {
                            hub.set_status(Status::NeedsRelogin);
                        }
                        continue;
                    }
                    Err(e) => {
                        debug!(conv_id, %e, "undecodable command");
                        continue;
                    }
                };
                accounts.command(&hub, conv_id, command);
            }
        }

        warn!("capture ended; restarting");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
