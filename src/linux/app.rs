//! Runs the app on Linux once it is set up: the socket server with the window's page, the capture (or a replay,
//! or the emulator relay), the tray icon, and what the tray and the window ask for, until Quit.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use ksni::TrayMethods;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

use super::tray::SyncTray;
use super::{Action, Signal, install, web};
use crate::hub::{Hub, Status};
use crate::record::Recorder;
use crate::update::Updater;
use crate::view::{self, Setup};
use crate::{PORT, capture, emulator, server};

pub enum Work {
    Capture(Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>),
    Replay(PathBuf),
    Emulator(Option<Arc<Recorder>>),
    /// The capture could not start; the status already says why.
    Nothing,
}

pub struct Launch {
    pub site: String,
    pub origins: Vec<String>,
    /// What the login autostart starts the installed copy with.
    pub startup_args: Vec<String>,
    pub show_window: bool,
    /// False for `--headless`.
    pub tray: bool,
}

fn open(target: &str) {
    if let Err(e) = open::that_detached(target) {
        warn!(%e, target, "could not open");
    }
}

/// The tray icon, kept current with the account, the startup setting and updates. None where the desktop has
/// no tray.
async fn tray(
    hub: &Hub,
    updater: &Updater,
    setup: &watch::Receiver<Setup>,
    actions: mpsc::UnboundedSender<Action>,
) -> Option<ksni::Handle<SyncTray>> {
    let current = *setup.borrow();
    let update = updater.state();
    let (headline, tone, _) = view::state(hub, &current, &update);
    let sending = *hub.sending().borrow();
    let tray = SyncTray { headline, tone, setup: current, sending, update: view::update_item(&update), actions };
    // Started at login, the app can be up before the panel is: the icon then appears when the panel does.
    let handle = match tray.assume_sni_available(true).spawn().await {
        Ok(handle) => handle,
        Err(e) => {
            warn!(%e, "no tray icon");
            return None;
        }
    };
    let (icon, hub) = (handle.clone(), hub.clone());
    let (mut status, mut clients, mut setup, mut updates) = (hub.status(), hub.clients(), setup.clone(), updater.subscribe());
    let mut sending = hub.sending();
    tokio::spawn(async move {
        loop {
            let alive = tokio::select! {
                r = status.changed() => r.is_ok(),
                r = clients.changed() => r.is_ok(),
                r = setup.changed() => r.is_ok(),
                r = updates.changed() => r.is_ok(),
                r = sending.changed() => r.is_ok(),
            };
            if !alive {
                break;
            }
            let current = *setup.borrow_and_update();
            let update = updates.borrow_and_update().clone();
            let on = *sending.borrow_and_update();
            let (headline, tone, _) = view::state(&hub, &current, &update);
            let item = view::update_item(&update);
            let changed = icon.update(move |t: &mut SyncTray| {
                (t.headline, t.tone, t.setup, t.sending, t.update) = (headline, tone, current, on, item);
            });
            if changed.await.is_none() {
                break;
            }
        }
    });
    Some(handle)
}

pub fn run(hub: Hub, updater: Updater, launch: Launch, work: Work, quit: Signal, show: Signal) {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime");
    runtime.block_on(async move {
        let (actions_tx, mut actions) = mpsc::unbounded_channel();
        // Another copy's signals arrive as actions too.
        let forward = actions_tx.clone();
        std::thread::spawn(move || {
            loop {
                show.wait();
                if forward.send(Action::Show).is_err() {
                    break;
                }
            }
        });
        let forward = actions_tx.clone();
        std::thread::spawn(move || {
            quit.wait();
            forward.send(Action::Quit).ok();
        });

        let (setup_tx, setup) =
            watch::channel(Setup { installed: install::running_installed(), startup: install::startup_registered() });
        let window = web::Window {
            token: super::token().into(),
            hub: hub.clone(),
            setup: setup.clone(),
            updater: updater.clone(),
            actions: actions_tx.clone(),
        };

        let server = tokio::spawn(server::serve(hub.clone(), PORT, launch.origins, Some(web::routes(window.clone()))));
        let failed = hub.clone();
        tokio::spawn(async move {
            if let Ok(Err(e)) = server.await {
                tracing::error!(%e, "socket server stopped");
                failed.set_status(Status::Failed(format!("port {PORT} is in use")));
            }
        });
        match work {
            Work::Capture(frames) => {
                tokio::spawn(capture::run(hub.clone(), frames));
            }
            Work::Replay(path) => match std::fs::read_to_string(&path).map(|s| serde_json::from_str(&s)) {
                Ok(Ok(export)) => hub.publish_saved(export),
                _ => hub.set_status(Status::Failed(format!("could not read {}", path.display()))),
            },
            Work::Emulator(recorder) => {
                tokio::spawn(emulator::run(hub.clone(), recorder));
            }
            Work::Nothing => {}
        }

        let tray = if launch.tray { tray(&hub, &updater, &setup, actions_tx.clone()).await } else { None };
        if launch.show_window {
            actions_tx.send(Action::Show).ok();
        }
        while let Some(action) = actions.recv().await {
            match action {
                Action::Show => open(&window.address()),
                Action::Settings => open(&format!("{}#settings", window.address())),
                Action::OpenSite => open(&launch.site),
                Action::ShowFile => {
                    if let Some(folder) = hub.cache_file().and_then(|f| f.parent()) {
                        open(&folder.to_string_lossy());
                    }
                }
                Action::Startup(on) => {
                    let changed = if on { install::register_startup(&launch.startup_args) } else { install::unregister_startup() };
                    if let Err(e) = changed {
                        warn!(%e, on, "could not change starting at login");
                    }
                    setup_tx.send_modify(|s| s.startup = install::startup_registered());
                }
                Action::Sending(on) => crate::settings::set_sending(&hub, on),
                Action::Update(request) => updater.ask(request),
                Action::Quit => break,
                Action::Uninstall => {
                    if let Err(e) = install::uninstall() {
                        warn!(%e, "uninstall did not finish");
                    }
                    break;
                }
            }
        }
        info!("quitting");
        if let Some(tray) = tray {
            tray.shutdown().await;
        }
    });
}
