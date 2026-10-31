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
    tokio::spawn(watch_for_hidden_game(hub.clone()));
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

/// The game running for a minute and a half with none of its traffic in view: past the start screen, something
/// carries it where the packet monitor does not look, most often a VPN or ping booster. Said instead of waiting on.
#[cfg(windows)]
async fn watch_for_hidden_game(hub: Hub) {
    const GRACE: Duration = Duration::from_secs(90);
    let mut running_since: Option<std::time::Instant> = None;
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let running = tokio::task::spawn_blocking(crate::win::game_running).await.unwrap_or(false);
        running_since = running.then(|| running_since.unwrap_or_else(std::time::Instant::now));
        let hidden = running_since.is_some_and(|since| since.elapsed() >= GRACE) && hub.game_quiet_for(GRACE);
        let status = hub.status().borrow().clone();
        match status {
            Status::WaitingForGame if hidden => {
                warn!("the game is running, but none of its traffic is in view");
                hub.set_status(Status::GameUnseen);
            }
            Status::GameUnseen if !hidden => hub.set_status(Status::WaitingForGame),
            _ => {}
        }
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
