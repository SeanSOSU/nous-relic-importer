//! StarRailSim Sync: reads your Honkai: Star Rail account from the game's own network traffic and serves it
//! to StarRailSim over a local socket, so the site's relic inventory follows the game without exports.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod accounts;
#[cfg(windows)]
mod app;
#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod emulator;
// Vendored from Reliquary Archiver as-is (see src/export/VENDORED); not every part of it is used here.
#[allow(dead_code, unused_imports, unused_variables)]
mod export;
mod hub;
#[cfg(windows)]
mod install;
mod server;
#[cfg(windows)]
mod win;

use std::path::PathBuf;

pub const APP_NAME: &str = "StarRailSim Sync";
const DEFAULT_SITE: &str = "https://starrailsim.com/import";
const PORT: u16 = 23314;

struct Options {
    /// Started by the logon task.
    background: bool,
    /// Just installed: open the site once so the reader lands on the step that turns sync on.
    first_run: bool,
    site: String,
    allow_origins: Vec<String>,
    /// Serve a saved archiver export instead of capturing (no admin needed): for trying the site end to end.
    replay: Option<PathBuf>,
    /// Run from where it is, without installing or registering at sign-in.
    portable: bool,
    /// Remove the installed copy, its sign-in task, shortcut and logs (what Settings > Installed apps runs).
    uninstall: bool,
    /// No tray icon and no window: for automated checks.
    headless: bool,
    /// Test against the PearlSR emulator: relay its client's TCP connection instead of capturing the live
    /// game (no admin needed, nothing installed).
    emulator: bool,
}

impl Options {
    fn parse() -> Self {
        let mut options = Options {
            background: false,
            first_run: false,
            site: DEFAULT_SITE.to_owned(),
            allow_origins: Vec::new(),
            replay: None,
            portable: false,
            uninstall: false,
            headless: false,
            emulator: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--background" => options.background = true,
                "--first-run" => options.first_run = true,
                "--portable" => options.portable = true,
                "--uninstall" => options.uninstall = true,
                "--headless" => options.headless = true,
                "--emulator" => options.emulator = true,
                "--site" => options.site = args.next().unwrap_or(options.site),
                "--allow-origin" => options.allow_origins.extend(args.next()),
                "--replay" => options.replay = args.next().map(PathBuf::from),
                other => tracing::warn!(other, "unknown argument"),
            }
        }
        options
    }

    /// What the installed copy is started with, so a custom site and origins survive the install.
    fn carried_args(&self) -> String {
        let mut args = Vec::new();
        if self.site != DEFAULT_SITE {
            args.push(format!("--site \"{}\"", self.site));
        }
        for origin in &self.allow_origins {
            args.push(format!("--allow-origin \"{origin}\""));
        }
        args.join(" ")
    }
}

/// Logs live here, apart from the program folder.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join(APP_NAME)
}

fn init_logging() {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join("sync.log");
    // Appended, never truncated: a second launch that only hands over to the running copy must not wipe its log.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        std::fs::rename(&path, dir.join("sync.old.log")).ok();
    }
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else { return };
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .init();
    std::panic::set_hook(Box::new(|info| tracing::error!(%info, "panic")));
}

#[cfg(windows)]
fn main() {
    init_logging();
    let options = Options::parse();
    let open_site = || {
        open::that_detached(&options.site).ok();
    };

    if options.uninstall {
        if !win::is_elevated() {
            if !win::relaunch_elevated() {
                win::message("Uninstalling StarRailSim Sync needs administrator access, to remove its sign-in task.");
            }
            return;
        }
        if win::SingleInstance::other_running() && !win::Signal::quit_and_wait(std::time::Duration::from_secs(10)) {
            win::message("Quit StarRailSim Sync from its tray icon, then uninstall it again.");
            return;
        }
        if let Err(e) = install::uninstall() {
            tracing::warn!(%e, "uninstall did not finish");
            win::message(&format!("Uninstalling did not finish: {e}"));
        }
        return;
    }

    if options.replay.is_none() && !options.emulator {
        // This same build is already running: running it again brings its window up, without a UAC prompt.
        if win::SingleInstance::other_running() && (install::running_installed() || install::installed_is_current()) {
            if !win::Signal::send(win::Signal::SHOW) {
                open_site();
            }
            return;
        }
        // pktmon only runs elevated.
        if !win::is_elevated() {
            // Installed and registered: start through the sign-in task, which has admin rights, then show the window.
            if install::running_installed() && install::start_through_task() {
                for _ in 0..50 {
                    if win::Signal::send(win::Signal::SHOW) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                return;
            }
            if !win::relaunch_elevated() {
                win::message("StarRailSim Sync needs administrator access to read the game's network traffic. Run it again and choose Yes.");
            }
            return;
        }
        if !options.portable && !install::running_installed() {
            if win::SingleInstance::other_running() {
                // A different build: an update. The running copy steps aside so this one can replace it.
                if !win::Signal::quit_and_wait(std::time::Duration::from_secs(10)) {
                    win::message("Quit StarRailSim Sync from its tray icon, then run this again to update it.");
                    return;
                }
            }
            let carried = options.carried_args();
            match install::install(&format!("--background {carried}")) {
                Ok(exe) => {
                    use std::os::windows::process::CommandExt;
                    let mut installed = std::process::Command::new(exe);
                    installed.arg("--first-run");
                    if !carried.is_empty() {
                        installed.raw_arg(&carried);
                    }
                    if installed.spawn().is_ok() {
                        return;
                    }
                    tracing::warn!("could not start the installed copy; running from here");
                }
                Err(e) => tracing::warn!(%e, "install failed; running from here"),
            }
        }
    }

    let Some(_instance) = win::SingleInstance::acquire() else {
        if !win::Signal::send(win::Signal::SHOW) {
            open_site();
        }
        return;
    };
    let quit = win::Signal::create(win::Signal::QUIT).expect("creating the quit signal");
    let show = win::Signal::create(win::Signal::SHOW).expect("creating the show signal");
    tracing::info!(version = env!("CARGO_PKG_VERSION"), background = options.background, "started");

    let hub = hub::Hub::new();
    {
        let hub = hub.clone();
        let origins = options.allow_origins.clone();
        let replay = options.replay.clone();
        let emulator = options.emulator;
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(async move {
                let server = tokio::spawn(server::serve(hub.clone(), PORT, origins));
                match replay {
                    Some(path) => match std::fs::read_to_string(&path).map(|s| serde_json::from_str(&s)) {
                        Ok(Ok(export)) => hub.publish_saved(export),
                        _ => hub.set_status(hub::Status::Failed(format!("could not read {}", path.display()))),
                    },
                    None if emulator => {
                        tokio::spawn(emulator::run(hub.clone()));
                    }
                    None => {
                        tokio::spawn(capture::run(hub.clone()));
                    }
                }
                if let Ok(Err(e)) = server.await {
                    tracing::error!(%e, "socket server stopped");
                    hub.set_status(hub::Status::Failed(format!("port {PORT} is in use")));
                }
                std::future::pending::<()>().await;
            });
        });
    }

    if options.first_run {
        open_site();
    }
    if options.headless {
        return quit.wait();
    }
    let launch = app::Launch {
        site: options.site.clone(),
        task_args: format!("--background {}", options.carried_args()),
        show_window: !options.background,
    };
    app::run(hub, launch, quit, show);
}

#[cfg(not(windows))]
fn main() {
    let _ = (init_logging, Options::parse, data_dir);
    eprintln!("{APP_NAME} runs on Windows only.");
}
