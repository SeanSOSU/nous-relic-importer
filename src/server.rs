//! The local socket the StarRailSim page connects to: ws://127.0.0.1:23314/ws, speaking Reliquary Archiver's
//! live-import protocol (an InitialScan on connect, then deltas). Bound to loopback only, and only the site's
//! own origins may open it, so another web page in the same browser cannot read the account.

use std::net::{Ipv4Addr, SocketAddr};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::hub::Hub;

const SITE_ORIGINS: [&str; 4] = [
    "https://starrailsim.com",
    "https://www.starrailsim.com",
    "http://localhost:5173",
    "http://127.0.0.1:5173",
];

#[derive(Clone)]
struct ServerState {
    hub: Hub,
    extra_origins: Vec<String>,
}

pub async fn serve(hub: Hub, port: u16, extra_origins: Vec<String>) -> std::io::Result<()> {
    let app = Router::new()
        .route("/ws", get(upgrade))
        .with_state(ServerState { hub, extra_origins });
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await?;
    info!(port, "listening");
    axum::serve(listener, app).await
}

async fn upgrade(ws: WebSocketUpgrade, headers: HeaderMap, State(state): State<ServerState>) -> Response {
    // A browser always sends Origin; a local tool may not, and could read the packets itself anyway.
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
        let allowed = SITE_ORIGINS.contains(&origin) || state.extra_origins.iter().any(|o| o == origin);
        if !allowed {
            warn!(origin, "refused a page from another site");
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    ws.on_upgrade(move |socket| client(socket, state.hub))
}

async fn client(mut socket: WebSocket, hub: Hub) {
    let (snapshot, mut frames) = hub.subscribe();
    hub.client_joined();
    if let Some(frame) = snapshot {
        if socket.send(Message::Text(frame.as_ref().into())).await.is_err() {
            hub.client_left();
            return;
        }
    }
    loop {
        tokio::select! {
            frame = frames.recv() => {
                let frame = match frame {
                    Ok(frame) => frame,
                    // Too slow to keep up: start it over from the whole account.
                    Err(broadcast::error::RecvError::Lagged(_)) => match hub.snapshot() {
                        Some(frame) => frame,
                        None => continue,
                    },
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                if socket.send(Message::Text(frame.as_ref().into())).await.is_err() {
                    break;
                }
            }
            message = socket.recv() => match message {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
    hub.client_left();
}
