//! The window, on Linux: the page the Windows app shows in its web view, opened in the browser from the app's
//! own local server, with a WebSocket standing in for the web view's bridge. Its address carries a token made
//! at start and is only served to that address on this machine, so no other page can open it or drive the app.

use std::sync::Arc;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use tokio::sync::{mpsc, watch};

use super::Action;
use crate::hub::Hub;
use crate::update::{Request, Updater};
use crate::view::{self, Setup};

const UI: &str = include_str!("../ui.html");

#[derive(Clone)]
pub struct Window {
    pub token: Arc<str>,
    pub hub: Hub,
    pub setup: watch::Receiver<Setup>,
    pub updater: Updater,
    pub actions: mpsc::UnboundedSender<Action>,
}

impl Window {
    pub fn address(&self) -> String {
        format!("http://127.0.0.1:{}/?token={}", crate::PORT, self.token)
    }
}

pub fn routes(window: Window) -> Router {
    Router::new().route("/", get(page)).route("/ui", get(upgrade)).with_state(window)
}

#[derive(serde::Deserialize)]
struct Token {
    token: Option<String>,
}

/// Whether a request names this machine's loopback address as its host (`Host`) or page (`Origin`): a page
/// from elsewhere, or a name rebound to 127.0.0.1, names something else.
fn names_this_app(headers: &HeaderMap, name: header::HeaderName, scheme: &str) -> bool {
    let value = headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default();
    let port = crate::PORT;
    [format!("{scheme}127.0.0.1:{port}"), format!("{scheme}localhost:{port}")].iter().any(|ok| ok == value)
}

async fn page(Query(query): Query<Token>, headers: HeaderMap, State(window): State<Window>) -> Response {
    if !names_this_app(&headers, header::HOST, "") || query.token.as_deref() != Some(&*window.token) {
        return StatusCode::NOT_FOUND.into_response();
    }
    // No referrer: the page loads character art from a CDN, and its address holds the token.
    ([(header::CACHE_CONTROL, "no-store"), (header::REFERRER_POLICY, "no-referrer")], Html(UI)).into_response()
}

async fn upgrade(ws: WebSocketUpgrade, Query(query): Query<Token>, headers: HeaderMap, State(window): State<Window>) -> Response {
    if !names_this_app(&headers, header::ORIGIN, "http://") || query.token.as_deref() != Some(&*window.token) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.on_upgrade(move |socket| serve(socket, window))
}

fn action(message: &str) -> Option<Action> {
    Some(match message {
        "open-site" => Action::OpenSite,
        "show-file" => Action::ShowFile,
        "sending-on" => Action::Sending(true),
        "sending-off" => Action::Sending(false),
        "startup-on" => Action::Startup(true),
        "startup-off" => Action::Startup(false),
        "quit" => Action::Quit,
        "uninstall" => Action::Uninstall,
        other => Action::Update(Request::from_message(other)?),
    })
}

/// Sends the page its state now and after every change, and passes its buttons on as actions.
async fn serve(mut socket: WebSocket, window: Window) {
    let (mut status, mut clients, mut setup) = (window.hub.status(), window.hub.clients(), window.setup.clone());
    let (mut updates, mut sending) = (window.updater.subscribe(), window.hub.sending());
    loop {
        let current = *setup.borrow_and_update();
        let update = updates.borrow_and_update().clone();
        let (_, _, state) = view::state(&window.hub, &current, &update);
        if socket.send(Message::Text(state.to_string().into())).await.is_err() {
            return;
        }
        loop {
            let changed = tokio::select! {
                r = status.changed() => r.is_ok(),
                r = clients.changed() => r.is_ok(),
                r = setup.changed() => r.is_ok(),
                r = updates.changed() => r.is_ok(),
                r = sending.changed() => r.is_ok(),
                message = socket.recv() => match message {
                    Some(Ok(Message::Text(text))) => {
                        match action(text.as_str()) {
                            Some(action) => {
                                window.actions.send(action).ok();
                                continue;
                            }
                            // "ready", or anything else: send the state again.
                            None => true,
                        }
                    }
                    Some(Ok(_)) => continue,
                    _ => return,
                },
            };
            if !changed {
                return;
            }
            break;
        }
    }
}
