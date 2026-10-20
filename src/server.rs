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

use crate::hub::{Frame, Hub};

/// What a page gets while the reader has Send to StarRailSim off: it keeps what it has, and says so.
const PAUSED: &str = r#"{"event":"Paused","data":null}"#;

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

/// `pages`: more routes on the same port (the window page, where the window opens in a browser).
pub async fn serve(hub: Hub, port: u16, extra_origins: Vec<String>, pages: Option<Router>) -> std::io::Result<()> {
    let mut app = Router::new()
        .route("/ws", get(upgrade))
        .with_state(ServerState { hub, extra_origins });
    if let Some(pages) = pages {
        app = app.merge(pages);
    }
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
    let mut sending = hub.sending();
    hub.client_joined();
    let first = if *sending.borrow_and_update() { snapshot } else { Some(Frame::from(PAUSED)) };
    let mut open = match first {
        Some(frame) => socket.send(Message::Text(frame.as_ref().into())).await.is_ok(),
        None => true,
    };
    while open {
        let frame = tokio::select! {
            frame = frames.recv() => match frame {
                Ok(_) if !*sending.borrow() => continue,
                Ok(frame) => Some(frame),
                // Too slow to keep up: start it over from the whole account.
                Err(broadcast::error::RecvError::Lagged(_)) if *sending.borrow() => hub.snapshot(),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            changed = sending.changed() => match changed {
                // Back on: the account as it is now covers whatever changed while it was off.
                Ok(()) if *sending.borrow_and_update() => hub.snapshot(),
                Ok(()) => Some(Frame::from(PAUSED)),
                Err(_) => break,
            },
            message = socket.recv() => match message {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => continue,
            },
        };
        if let Some(frame) = frame {
            open = socket.send(Message::Text(frame.as_ref().into())).await.is_ok();
        }
    }
    hub.client_left();
}
