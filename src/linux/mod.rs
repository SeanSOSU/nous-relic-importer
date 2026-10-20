//! What Linux needs that Windows does through Win32: one running copy, the signals another copy sends it, the
//! capability the capture needs, priority, and a message when there is no window yet.

pub mod app;
pub mod install;
mod tray;
mod web;

/// What the tray menu, the window page and another copy's signals ask the running app to do.
pub enum Action {
    Show,
    OpenSite,
    ShowFile,
    Startup(bool),
    Quit,
    Uninstall,
}

use std::io::{Read, Write};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::process::{Command, Stdio};

/// Abstract sockets live in the network namespace, so each name carries the user it belongs to.
fn address(name: &str) -> Option<SocketAddr> {
    let uid = unsafe { libc::getuid() };
    SocketAddr::from_abstract_name(format!("starrailsim-sync-{uid}-{name}")).ok()
}

/// Held for the life of the running copy; the kernel lets it go when the process ends, however it ends.
pub struct SingleInstance(#[allow(dead_code)] UnixListener);

impl SingleInstance {
    /// None when another copy is already running.
    pub fn acquire() -> Option<Self> {
        UnixListener::bind_addr(&address("instance")?).ok().map(Self)
    }

    pub fn other_running() -> bool {
        Self::acquire().is_none()
    }
}

/// How another copy reaches the running one, as on Windows: QUIT lets a newer build step in, SHOW brings the
/// window up when the app is started again.
pub struct Signal(UnixListener);

impl Signal {
    pub const QUIT: &str = "quit";
    pub const SHOW: &str = "show";

    pub fn create(name: &str) -> Option<Self> {
        UnixListener::bind_addr(&address(name)?).ok().map(Self)
    }

    /// Blocks until another copy raises this signal.
    pub fn wait(&self) {
        loop {
            match self.0.accept() {
                Ok((mut stream, _)) => {
                    stream.write_all(b"ok").ok();
                    return;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        }
    }

    /// Raises the running copy's signal. False when nothing is listening.
    pub fn send(name: &str) -> bool {
        let Some(mut stream) = address(name).and_then(|a| UnixStream::connect_addr(&a).ok()) else { return false };
        stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).ok();
        let mut reply = [0u8; 2];
        stream.read_exact(&mut reply).is_ok()
    }

    /// Asks the running copy to quit and waits for it to let go. False if it is still running.
    pub fn quit_and_wait(timeout: std::time::Duration) -> bool {
        if !Self::send(Self::QUIT) {
            return false;
        }
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if !SingleInstance::other_running() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        false
    }
}

/// Gives up every capability this process started with (CAP_NET_RAW, from the installed file), once the
/// packet socket is open. Capabilities belong to threads, so this runs before any other thread exists.
pub fn drop_capabilities() {
    #[repr(C)]
    struct Header {
        version: u32,
        pid: libc::c_int,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Data {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    let header = Header { version: 0x2008_0522, pid: 0 };
    let none = [Data { effective: 0, permitted: 0, inheritable: 0 }; 2];
    if unsafe { libc::syscall(libc::SYS_capset, &header, none.as_ptr()) } != 0 {
        tracing::warn!(error = %std::io::Error::last_os_error(), "could not drop capabilities");
    }
}

/// Runs this process below normal priority. Threads started afterwards inherit it; ones already running
/// (the capture's) keep theirs.
pub fn lower_priority() {
    unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 10) };
}

/// A message box from whichever dialog tool the desktop has, before the app has a window of its own.
pub fn message(text: &str) {
    tracing::info!(text, "message");
    let title = crate::APP_NAME;
    let shown = |program: &str, args: &[&str]| {
        Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success() || s.code() == Some(1))
    };
    if !shown("zenity", &["--info", "--no-wrap", "--title", title, "--text", text])
        && !shown("kdialog", &["--title", title, "--msgbox", text])
        && !shown("notify-send", &[title, text])
    {
        eprintln!("{title}: {text}");
    }
}

/// 128 random bits as hex, for the window page's address.
pub fn token() -> String {
    let mut bytes = [0u8; 16];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).is_err() {
        let fallback = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        bytes = (fallback ^ u128::from(std::process::id()) << 64).to_le_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
