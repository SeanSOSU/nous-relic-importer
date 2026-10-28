//! Turns decoded game commands into the account the page sees, whichever capture produced them: one exporter
//! per logged-in session, and the most recent login is the one published.

use std::collections::HashMap;

use reliquary::network::command::GameCommand;
use reliquary::network::command::command_id::PlayerGetTokenScRsp;
use tokio::sync::broadcast;
use tracing::info;

use sync_core::export::Exporter;
use sync_core::export::fribbels::{OptimizerEvent, OptimizerExporter};
use crate::hub::{Hub, Status};

/// One logged-in session. The game opens a new connection per login, so this is keyed by connection.
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

#[derive(Default)]
pub struct Accounts {
    sessions: HashMap<u32, Session>,
    current: Option<u32>,
}

impl Accounts {
    /// Whether a login has been seen since this app started.
    pub fn has_login(&self) -> bool {
        self.current.is_some()
    }

    pub fn command(&mut self, hub: &Hub, conv_id: u32, command: GameCommand) {
        if command.command_id == PlayerGetTokenScRsp {
            info!(conv_id, "login");
            // A new login replaces every earlier session; their data is stale from here on.
            self.sessions.retain(|id, _| *id == conv_id);
            self.current = Some(conv_id);
            hub.set_status(Status::LoggingIn);
        }
        let session = self.sessions.entry(conv_id).or_insert_with(Session::new);
        session.exporter.read_command(command);
        let published = self.current == Some(conv_id);
        loop {
            match session.events.try_recv() {
                Ok(event) if published => hub.publish(&event, &session.exporter),
                Ok(_) => {}
                // Missed deltas: a fresh snapshot covers them.
                Err(broadcast::error::TryRecvError::Lagged(_)) if published => {
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
