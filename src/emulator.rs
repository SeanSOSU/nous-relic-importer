//! Test mode for the PearlSR emulator (`--emulator`), so the whole path can run without a live account. The
//! emulator's client talks plain TCP to 127.0.0.1:23301 with no encryption, and packet capture cannot see
//! loopback traffic. So this mode stands in the connection: Windows hands a connection for 127.0.0.1:23301 to a
//! listener bound to exactly that address ahead of PearlSR's 0.0.0.0 one, and the relay passes it on to PearlSR
//! through 127.0.0.2, reading the server's game frames on the way into the same exporter the live capture feeds.
//! It is in the path only while the app runs. The frames and protobufs are the live game's.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use reliquary::network::command::GameCommand;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use sync_core::frames::frames;

use crate::accounts::Accounts;
use crate::hub::{Hub, Status};
use crate::record::Recorder;

const GAME_PORT: u16 = 23301;

/// A chunk the server sent on one relayed connection; `None` once that connection closes.
type Tap = mpsc::UnboundedSender<(u32, Option<Vec<u8>>)>;

async fn relay(client: TcpStream, server: TcpStream, id: u32, tap: Tap, recorder: Option<Arc<Recorder>>) {
    client.set_nodelay(true).ok();
    server.set_nodelay(true).ok();
    if let Some(r) = &recorder {
        r.opened(id);
    }
    let (mut from_client, mut to_client) = client.into_split();
    let (mut from_server, mut to_server) = server.into_split();
    let upstream = async {
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let n = from_client.read(&mut chunk).await?;
            if n == 0 {
                return Ok::<_, std::io::Error>(());
            }
            to_server.write_all(&chunk[..n]).await?;
            if let Some(r) = &recorder {
                r.data(id, false, &chunk[..n]);
            }
        }
    };
    let downstream = async {
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let n = from_server.read(&mut chunk).await?;
            if n == 0 {
                return Ok::<_, std::io::Error>(());
            }
            to_client.write_all(&chunk[..n]).await?;
            if let Some(r) = &recorder {
                r.data(id, true, &chunk[..n]);
            }
            tap.send((id, Some(chunk[..n].to_vec()))).ok();
        }
    };
    // Either side closing ends the connection, as it would without the relay.
    tokio::select! {
        _ = upstream => {}
        _ = downstream => {}
    }
    if let Some(r) = &recorder {
        r.closed(id);
    }
    tap.send((id, None)).ok();
}

pub async fn run(hub: Hub, recorder: Option<Arc<Recorder>>) {
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
            reader.game_seen();
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
        tokio::spawn(relay(client, server, next_id, tap.clone(), recorder.clone()));
    }
}
