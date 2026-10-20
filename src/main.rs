//! StarRailSim Sync: reads your Honkai: Star Rail account from the game's own network traffic and serves it
//! to StarRailSim over a local socket, so the site's relic inventory follows the game without exports.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accounts;
#[cfg(windows)]
mod app;
mod capture;
mod emulator;
mod hub;
#[cfg(windows)]
mod install;
#[cfg(target_os = "linux")]
mod linux;
mod record;
mod server;
mod view;
#[cfg(windows)]
mod win;

use std::path::PathBuf;

pub const APP_NAME: &str = "StarRailSim Sync";
// `?sync=1` turns sync on as the page opens, so arriving from the app needs no click there.
const DEFAULT_SITE: &str = "https://starrailsim.com/import?sync=1";
pub const PORT: u16 = 23314;

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
    /// With --emulator: also write the relayed session to this pcap file, for testing the capture-file decoder.
    record: Option<PathBuf>,
    /// Linux, run as root by pkexec at install: give the installed copy the capability to capture, then exit.
    grant_capture: bool,
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
            record: None,
            grant_capture: false,
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
                "--grant-capture" => options.grant_capture = true,
                "--record" => options.record = args.next().map(PathBuf::from),
                "--site" => options.site = args.next().unwrap_or(options.site),
                "--allow-origin" => options.allow_origins.extend(args.next()),
                "--replay" => options.replay = args.next().map(PathBuf::from),
                other => tracing::warn!(other, "unknown argument"),
            }
        }
        options
    }

    /// What the installed copy is started with, so a custom site and origins survive the install.
    fn carried(&self) -> Vec<String> {
        let mut args = Vec::new();
        if self.site != DEFAULT_SITE {
            args.extend(["--site".to_owned(), self.site.clone()]);
        }
        for origin in &self.allow_origins {
            args.extend(["--allow-origin".to_owned(), origin.clone()]);
        }
        args
    }

    /// `carried`, as one Windows command line.
    #[cfg(windows)]
    fn carried_args(&self) -> String {
        let quoted: Vec<String> = self.carried().into_iter().map(|a| if a.starts_with("--") { a } else { format!("\"{a}\"") }).collect();
        quoted.join(" ")
    }
}

/// Logs and the saved account live here, apart from the program folder.
#[cfg(windows)]
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join(APP_NAME)
}

#[cfg(target_os = "linux")]
pub fn data_dir() -> PathBuf {
    linux::install::data_dir()
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn data_dir() -> PathBuf {
    std::env::temp_dir().join(APP_NAME)
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

    // Below normal, so the game never waits on it.
    win::lower_priority();

    let hub = hub::Hub::new(cache_file(&options));
    hub.restore();
    {
        let hub = hub.clone();
        let origins = options.allow_origins.clone();
        let replay = options.replay.clone();
        let emulator = options.emulator;
        let recorder = recorder(&options);
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(async move {
                let server = tokio::spawn(server::serve(hub.clone(), PORT, origins, None));
                match replay {
                    Some(path) => match std::fs::read_to_string(&path).map(|s| serde_json::from_str(&s)) {
                        Ok(Ok(export)) => hub.publish_saved(export),
                        _ => hub.set_status(hub::Status::Failed(format!("could not read {}", path.display()))),
                    },
                    None if emulator => {
                        tokio::spawn(emulator::run(hub.clone(), recorder));
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

/// The account is kept on disk between runs; a test against the emulator keeps its own file, and a replay none.
fn cache_file(options: &Options) -> Option<PathBuf> {
    match (&options.replay, options.emulator) {
        (Some(_), _) => None,
        (None, true) => Some(data_dir().join("last-import-emulator.json")),
        (None, false) => Some(data_dir().join("last-import.json")),
    }
}

fn recorder(options: &Options) -> Option<std::sync::Arc<record::Recorder>> {
    options.record.as_deref().and_then(|path| match record::Recorder::create(path) {
        Ok(r) => Some(std::sync::Arc::new(r)),
        Err(e) => {
            tracing::warn!(%e, path = %path.display(), "cannot record");
            None
        }
    })
}

/// Set on a copy started right after the password prompt granted the capability, so a system that does not
/// honour it (a home folder mounted nosuid) explains itself instead of asking again.
#[cfg(target_os = "linux")]
const GRANTED: &str = "STARRAILSIM_SYNC_GRANTED";

/// Asks for the capability for `exe`; false, having said why, when it was not given.
#[cfg(target_os = "linux")]
fn may_capture(exe: &std::path::Path) -> bool {
    use linux::install::{self, Grant};
    match install::grant_capture(exe) {
        Grant::Granted => true,
        Grant::Refused => {
            linux::message(
                "StarRailSim Sync needs your password once, to let it read the game's network traffic. Start it again and enter your password when asked.",
            );
            false
        }
        Grant::Unavailable => {
            linux::message(&format!(
                "This system has no password prompt for apps (pkexec), so StarRailSim Sync cannot ask for the permission it needs. Run this once in a terminal, then start it again:\n\n{}",
                install::manual_grant(exe)
            ));
            false
        }
    }
}

#[cfg(target_os = "linux")]
fn main() {
    use std::time::Duration;

    use linux::app::{Launch, Work};
    use linux::{Signal, SingleInstance, install, message};

    let options = Options::parse();
    if options.grant_capture {
        std::process::exit(install::set_own_capability());
    }
    if unsafe { libc::geteuid() } == 0 {
        // Root has no desktop session to show a tray icon in, and would keep its files in root's home.
        eprintln!("Run {APP_NAME} as yourself, not as root: it asks for the password itself when it needs it.");
        std::process::exit(2);
    }
    init_logging();
    let open_site = || {
        open::that_detached(&options.site).ok();
    };

    // The packet socket comes first, while this process has the capability the installed file gives it; the
    // capability goes before any other thread exists.
    let capturing = options.replay.is_none() && !options.emulator;
    let socket = (capturing && !options.uninstall).then(capture::packet_socket::PacketSocket::open);
    linux::drop_capabilities();

    if options.uninstall {
        if SingleInstance::other_running() && !Signal::quit_and_wait(Duration::from_secs(10)) {
            message("Quit StarRailSim Sync from its tray icon, then uninstall it again.");
            return;
        }
        match install::uninstall() {
            Ok(()) => message("StarRailSim Sync is uninstalled."),
            Err(e) => message(&format!("Uninstalling did not finish: {e}")),
        }
        return;
    }

    let startup_args: Vec<String> = std::iter::once("--background".to_owned()).chain(options.carried()).collect();
    if capturing {
        // This same build is already running: running it again brings its window up.
        if SingleInstance::other_running() && (install::running_installed() || install::installed_is_current()) {
            if !Signal::send(Signal::SHOW) {
                open_site();
            }
            return;
        }
        if !options.portable && !install::running_installed() {
            // A different build: an update. The running copy steps aside so this one can replace it.
            if SingleInstance::other_running() && !Signal::quit_and_wait(Duration::from_secs(10)) {
                message("Quit StarRailSim Sync from its tray icon, then run this again to update it.");
                return;
            }
            match install::install(&startup_args) {
                Ok(exe) => {
                    if !may_capture(&exe) {
                        return;
                    }
                    let started = std::process::Command::new(&exe).arg("--first-run").args(options.carried()).env(GRANTED, "1").spawn();
                    if started.is_ok() {
                        return;
                    }
                    tracing::warn!("could not start the installed copy; running from here");
                }
                Err(e) => tracing::warn!(%e, "install failed; running from here"),
            }
        }
        // This file has no capability yet (replaced by an update, or run with --portable): ask, then start
        // over as the same process, which can open the socket now.
        if matches!(&socket, Some(Err(e)) if e.kind() == std::io::ErrorKind::PermissionDenied) {
            let exe = std::env::current_exe().unwrap_or_default();
            if std::env::var_os(GRANTED).is_some() {
                message(&format!(
                    "This system did not apply the permission to read network traffic, usually because the home folder does not allow it. Run this once in a terminal, then start StarRailSim Sync again:\n\n{}",
                    install::manual_grant(&exe)
                ));
            } else if may_capture(&exe) {
                use std::os::unix::process::CommandExt;
                let e = std::process::Command::new(&exe).args(std::env::args_os().skip(1)).env(GRANTED, "1").exec();
                tracing::warn!(%e, "could not restart with the capability");
            }
            return;
        }
    }

    let Some(_instance) = SingleInstance::acquire() else {
        if !Signal::send(Signal::SHOW) {
            open_site();
        }
        return;
    };
    let quit = Signal::create(Signal::QUIT).expect("creating the quit signal");
    let show = Signal::create(Signal::SHOW).expect("creating the show signal");
    tracing::info!(version = env!("CARGO_PKG_VERSION"), background = options.background, "started");

    let hub = hub::Hub::new(cache_file(&options));
    hub.restore();
    // The capture thread starts before the priority drops: the rest runs below normal, but it must keep up.
    let work = match socket {
        Some(Ok(socket)) => Work::Capture(Box::pin(socket.frames())),
        Some(Err(e)) => {
            tracing::warn!(%e, "packet capture did not start");
            hub.set_status(hub::Status::Failed(format!("Packet capture did not start: {e}")));
            Work::Nothing
        }
        None => match options.replay.clone() {
            Some(path) => Work::Replay(path),
            None => Work::Emulator(recorder(&options)),
        },
    };
    linux::lower_priority();

    if options.first_run {
        open_site();
    }
    let launch = Launch {
        site: options.site.clone(),
        origins: options.allow_origins.clone(),
        startup_args,
        // The site opens on the first run instead: two new tabs at once is one too many.
        show_window: !options.background && !options.first_run && !options.headless,
        tray: !options.headless,
    };
    linux::app::run(hub, launch, work, quit, show);
}

#[cfg(not(any(windows, target_os = "linux")))]
fn main() {
    let _ = (init_logging, Options::parse, cache_file, recorder);
    eprintln!("{APP_NAME} runs on Windows and Linux.");
}
