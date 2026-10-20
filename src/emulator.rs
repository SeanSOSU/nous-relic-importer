//! Test mode for the PearlSR emulator (`--emulator`), so the whole path can run without a live account. The
//! emulator's client talks plain TCP to 127.0.0.1:23301 with no encryption, and packet capture cannot see
//! loopback traffic. So this mode stands in the connection: Windows hands a connection for 127.0.0.1:23301 to a
//! listener bound to exactly that address ahead of PearlSR's 0.0.0.0 one, and the relay passes it on to PearlSR
//! through 127.0.0.2, reading the server's game frames on the way into the same exporter the live capture feeds.
//! It is in the path only while the app runs. The frames and protobufs are the live game's.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};

use reliquary::network::command::GameCommand;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::accounts::Accounts;
use crate::hub::{Hub, Status};

const GAME_PORT: u16 = 23301;
const HEAD_MAGIC: [u8; 4] = 0x9D74_C714u32.to_be_bytes();
const TAIL_MAGIC: [u8; 4] = 0xD7A1_52C8u32.to_be_bytes();

/// Removes every complete game frame from the front of `buffer` (magic, command id, header and body lengths,
/// header, body, tail magic), skipping bytes that do not start one.
fn frames(buffer: &mut Vec<u8>) -> Vec<Vec<u8>> {
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

/// A chunk the server sent on one relayed connection; `None` once that connection closes.
type Tap = mpsc::UnboundedSender<(u32, Option<Vec<u8>>)>;

async fn relay(client: TcpStream, server: TcpStream, id: u32, tap: Tap) {
    client.set_nodelay(true).ok();
    server.set_nodelay(true).ok();
    let (mut from_client, mut to_client) = client.into_split();
    let (mut from_server, mut to_server) = server.into_split();
    let upstream = tokio::io::copy(&mut from_client, &mut to_server);
    let downstream = async {
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let n = from_server.read(&mut chunk).await?;
            if n == 0 {
                return Ok::<_, std::io::Error>(());
            }
            to_client.write_all(&chunk[..n]).await?;
            tap.send((id, Some(chunk[..n].to_vec()))).ok();
        }
    };
    // Either side closing ends the connection, as it would without the relay.
    tokio::select! {
        _ = upstream => {}
        _ = downstream => {}
    }
    tap.send((id, None)).ok();
}

pub async fn run(hub: Hub) {
    let listener = TcpSocket::new_v4()
        .and_then(|socket| {
            socket.set_reuseaddr(true)?;
            socket.bind(SocketAddr::from((Ipv4Addr::LOCALHOST, GAME_PORT)))?;
            socket.listen(64)
        });
    let listener = match listener {
        Ok(listener) => listener,
        Err(e) => {
            warn!(%e, "emulator relay did not start");
            return hub.set_status(Status::Failed(format!("could not stand in front of the emulator ({e})")));
        }
    };
    info!("relaying the emulator's game connection");
    if !hub.is_synced() {
        hub.set_status(Status::WaitingForGame);
    }

    let (tap, mut taps) = mpsc::unbounded_channel::<(u32, Option<Vec<u8>>)>();
    let reader = hub.clone();
    tokio::spawn(async move {
        let mut accounts = Accounts::default();
        let mut buffers: HashMap<u32, Vec<u8>> = HashMap::new();
        while let Some((id, chunk)) = taps.recv().await {
            let Some(chunk) = chunk else {
                buffers.remove(&id);
                info!(id, "game disconnected");
                if !reader.is_synced() {
                    reader.set_status(Status::WaitingForGame);
                }
                continue;
            };
            let buffer = buffers.entry(id).or_default();
            buffer.extend_from_slice(&chunk);
            for frame in frames(buffer) {
                match GameCommand::try_new(frame) {
                    Ok(command) => accounts.command(&reader, id, command),
                    Err(e) => debug!(%e, "unreadable emulator frame"),
                }
            }
        }
    });

    let mut next_id = 0u32;
    loop {
        let (client, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                warn!(%e, "emulator relay stopped accepting");
                continue;
            }
        };
        let server = match TcpStream::connect(SocketAddr::from(([127, 0, 0, 2], GAME_PORT))).await {
            Ok(server) => server,
            Err(e) => {
                warn!(%e, "the emulator's gameserver is not answering on 127.0.0.2");
                continue;
            }
        };
        next_id += 1;
        info!(id = next_id, port = peer.port(), "game connected");
        if !hub.is_synced() {
            hub.set_status(Status::GameFound);
        }
        tokio::spawn(relay(client, server, next_id, tap.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::{HEAD_MAGIC, TAIL_MAGIC, frames};

    fn frame(command: u16, body: &[u8]) -> Vec<u8> {
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
