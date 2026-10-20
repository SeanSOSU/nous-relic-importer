//! Reads the game's traffic with Windows' built-in packet monitor (pktmon, so there is no driver to install)
//! and feeds it through Reliquary's decoder into the archiver's exporter. Adapted from Reliquary Archiver's
//! capture and worker modules (MIT, https://github.com/IceDynamix/reliquary-archiver).

use std::collections::HashMap;
use std::time::Duration;

use futures::{Stream, StreamExt, future};
use pktmon::filter::{PktMonFilter, TransportProtocol};
use pktmon::{Capture, PacketPayload};
use reliquary::network::command::GameCommandError;
use reliquary::network::command::command_id::PlayerGetTokenScRsp;
use reliquary::network::{GamePacket, GameSniffer, NetworkError};
use crate::export::Exporter;
use crate::export::fribbels::{OptimizerEvent, OptimizerExporter};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::hub::{Hub, Status};

const GAME_PORTS: [u16; 2] = [23301, 23302];

/// One logged-in session. The game opens a new conversation per login, so this is keyed by conversation id.
struct Session {
    exporter: OptimizerExporter,
    events: broadcast::Receiver<OptimizerEvent>,
}

impl Session {
    fn new() -> Self {
        let exporter = OptimizerExporter::new();
        let (_, events) = exporter.subscribe();
        Self { exporter, events }
    }
}

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
    let mut sessions: HashMap<u32, Session> = HashMap::new();
    // The session whose account the page sees: the most recent login.
    let mut current: Option<u32> = None;

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
                    if current.is_none() {
                        hub.set_status(Status::NeedsRelogin);
                    }
                    continue;
                }
                Err(_) => continue,
            };
            for packet in packets {
                let GamePacket::Commands { conv_id, result } = packet else { continue };
                let command = match result {
                    Ok(command) => command,
                    Err(GameCommandError::DecryptionKeyMissing) => {
                        if current.is_none() {
                            hub.set_status(Status::NeedsRelogin);
                        }
                        continue;
                    }
                    Err(e) => {
                        debug!(conv_id, %e, "undecodable command");
                        continue;
                    }
                };
                if command.command_id == PlayerGetTokenScRsp {
                    info!(conv_id, "login");
                    // A new login replaces every earlier session; their data is stale from here on.
                    sessions.retain(|id, _| *id == conv_id);
                    current = Some(conv_id);
                    hub.set_status(Status::LoggingIn);
                }
                let session = sessions.entry(conv_id).or_insert_with(Session::new);
                session.exporter.read_command(command);
                loop {
                    match session.events.try_recv() {
                        Ok(event) if current == Some(conv_id) => hub.publish(&event, &session.exporter),
                        Ok(_) => {}
                        // Missed deltas: a fresh snapshot covers them.
                        Err(broadcast::error::TryRecvError::Lagged(_)) if current == Some(conv_id) => {
                            if let Some(export) = session.exporter.export() {
                                hub.publish(&OptimizerEvent::InitialScan(export), &session.exporter);
                            }
                        }
                        Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                        Err(_) => break,
                    }
                }
            }
        }

        warn!("capture ended; restarting");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
