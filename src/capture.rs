//! Reads the game's traffic as the platform captures it, Windows' built-in packet monitor (pktmon, so there is
//! no driver to install) or a Linux packet socket, into the decoder the Android app shares.

#[cfg(windows)]
use std::time::Duration;

#[cfg(target_os = "linux")]
use futures::Stream;
use sync_live::decoder::Decoder;
#[cfg(windows)]
use tracing::info;
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
    let restart = std::sync::Arc::new(tokio::sync::Notify::new());
    tokio::spawn(watch_processes(hub.clone(), restart.clone()));
    let mut decoder = Decoder::default();
    loop {
        match pktmon::start() {
            Ok(frames) => tokio::select! {
                () = decoder.read(&hub, frames) => warn!("capture ended; restarting"),
                () = restart.notified() => {
                    info!("Reliquary Archiver started or stopped; starting the capture over");
                    continue;
                }
            },
            Err(e) => {
                warn!(%e, "packet capture did not start");
                hub.set_status(Status::Failed(e));
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Reliquary Archiver reads packets through the same monitor. On Windows 10 its driver is one for every app: the
/// Archiver's start resets it and its exit stops it, so this capture starts over whenever one comes or goes.
#[cfg(any(windows, test))]
fn archivers(processes: &[String]) -> usize {
    processes.iter().filter(|name| name.starts_with("reliquary-archiver")).count()
}

/// Watches what runs beside the app: Reliquary Archiver coming or going, and the game running for a minute and a
/// half with none of its traffic in view. Past the start screen, that means something carries it where the packet
/// monitor does not look, most often a VPN or ping booster: said instead of waiting on.
#[cfg(windows)]
async fn watch_processes(hub: Hub, restart: std::sync::Arc<tokio::sync::Notify>) {
    const GRACE: Duration = Duration::from_secs(90);
    let mut running_since: Option<std::time::Instant> = None;
    let mut archivers_before = None;
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let processes = tokio::task::spawn_blocking(crate::win::process_names).await.unwrap_or_default();
        let now = archivers(&processes);
        if archivers_before.is_some_and(|before| before != now) {
            restart.notify_waiters();
        }
        archivers_before = Some(now);
        let running = processes.iter().any(|name| name == "starrail.exe");
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

#[cfg(test)]
mod tests {
    #[test]
    fn either_archiver_build_counts_and_nothing_else_does() {
        let running = ["explorer.exe", "reliquary-archiver-pktmon-x64.exe", "reliquary-archiver-pcap-x64.exe", "starrail.exe"];
        assert_eq!(super::archivers(&running.map(String::from)), 2);
        assert_eq!(super::archivers(&["nous-relic-importer.exe".to_owned()]), 0);
    }
}
