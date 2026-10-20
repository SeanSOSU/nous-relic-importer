//! The state the capture worker, the socket server and the tray share: what the app is doing, the account
//! snapshot a newly connected page starts from, and the live frames every connected page receives.

use std::sync::{Arc, Mutex};

use crate::export::Exporter;
use crate::export::fribbels::{OptimizerEvent, OptimizerExporter};
use tokio::sync::{broadcast, watch};
use tracing::warn;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Starting,
    Failed(String),
    WaitingForGame,
    LoggingIn,
    /// Packets arrived that need a session key this app never saw: the game was already logged in.
    NeedsRelogin,
    Synced { relics: usize, uid: Option<u32> },
}

/// One message as a page receives it: `{"event": ..., "data": ...}`, serialized once for every client.
pub type Frame = Arc<str>;

#[derive(Clone)]
pub struct Hub {
    inner: Arc<Inner>,
}

struct Inner {
    status: watch::Sender<Status>,
    clients: watch::Sender<usize>,
    frames: broadcast::Sender<Frame>,
    // The current account as one InitialScan frame. Held under the same lock a subscriber takes, so a page
    // never misses a delta that lands between its snapshot and its subscription.
    snapshot: Mutex<Option<Frame>>,
}

impl Hub {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                status: watch::channel(Status::Starting).0,
                clients: watch::channel(0).0,
                frames: broadcast::channel(64).0,
                snapshot: Mutex::new(None),
            }),
        }
    }

    pub fn status(&self) -> watch::Receiver<Status> {
        self.inner.status.subscribe()
    }

    pub fn clients(&self) -> watch::Receiver<usize> {
        self.inner.clients.subscribe()
    }

    pub fn set_status(&self, status: Status) {
        self.inner.status.send_if_modified(|current| {
            let changed = *current != status;
            *current = status;
            changed
        });
    }

    pub fn is_synced(&self) -> bool {
        matches!(*self.inner.status.borrow(), Status::Synced { .. })
    }

    /// Forwards one exporter event to every page and refreshes the snapshot from the exporter's state.
    pub fn publish(&self, event: &OptimizerEvent, exporter: &OptimizerExporter) {
        let Some(export) = exporter.export() else { return };
        let relics = export.relics.len();
        let uid = export.metadata.uid;
        let snapshot = match serde_json::to_string(&OptimizerEvent::InitialScan(export)) {
            Ok(json) => Frame::from(json),
            Err(e) => return warn!(%e, "could not serialize the account"),
        };
        let frame = match event {
            OptimizerEvent::InitialScan(_) => snapshot.clone(),
            _ => match serde_json::to_string(event) {
                Ok(json) => Frame::from(json),
                Err(e) => return warn!(%e, "could not serialize an update"),
            },
        };
        let mut held = self.inner.snapshot.lock().unwrap();
        *held = Some(snapshot);
        self.inner.frames.send(frame).ok();
        drop(held);
        self.set_status(Status::Synced { relics, uid });
    }

    /// Serves a saved export as if it had just been captured (`--replay`).
    pub fn publish_saved(&self, export: serde_json::Value) {
        let relics = export.get("relics").and_then(|r| r.as_array()).map_or(0, Vec::len);
        let uid = export.pointer("/metadata/uid").and_then(|u| u.as_u64()).map(|u| u as u32);
        let frame = Frame::from(serde_json::json!({ "event": "InitialScan", "data": export }).to_string());
        *self.inner.snapshot.lock().unwrap() = Some(frame.clone());
        self.inner.frames.send(frame).ok();
        self.set_status(Status::Synced { relics, uid });
    }

    pub fn subscribe(&self) -> (Option<Frame>, broadcast::Receiver<Frame>) {
        let held = self.inner.snapshot.lock().unwrap();
        (held.clone(), self.inner.frames.subscribe())
    }

    pub fn snapshot(&self) -> Option<Frame> {
        self.inner.snapshot.lock().unwrap().clone()
    }

    pub fn client_joined(&self) {
        self.inner.clients.send_modify(|n| *n += 1);
    }

    pub fn client_left(&self) {
        self.inner.clients.send_modify(|n| *n = n.saturating_sub(1));
    }
}
