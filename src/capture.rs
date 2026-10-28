//! Reads the game's traffic as the platform captures it, Windows' built-in packet monitor (pktmon, so there is
//! no driver to install) or a Linux packet socket, into the decoder the Android app shares.

#[cfg(windows)]
use std::time::Duration;

#[cfg(target_os = "linux")]
use futures::Stream;
use sync_live::decoder::Decoder;
use tracing::warn;

use crate::hub::{Hub, Status};

#[cfg(any(target_os = "linux", test))]
mod filter;
#[cfg(target_os = "linux")]
pub mod packet_socket;
#[cfg(windows)]
mod pktmon;

#[cfg(windows)]
pub async fn run(hub: Hub) {
    let mut decoder = Decoder::default();
    loop {
        match pktmon::start() {
            Ok(frames) => decoder.read(&hub, frames).await,
            Err(e) => {
                warn!(%e, "packet capture did not start");
                hub.set_status(Status::Failed(e));
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        }
        warn!("capture ended; restarting");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Reads the packet socket's frames until it fails. It cannot be reopened: the capability that opened it is
/// gone by then.
#[cfg(target_os = "linux")]
pub async fn run(hub: Hub, frames: impl Stream<Item = Vec<u8>> + Unpin) {
    Decoder::default().read(&hub, frames).await;
    warn!("capture ended");
    hub.set_status(Status::Failed("Packet capture stopped".into()));
}
