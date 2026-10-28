//! The Android app's engine, behind the calls its Java side makes (`Engine` in the app). It runs the importer the
//! desktop app runs, with the app's own VPN for the game as the packet source (`relay`): the game's traffic
//! passes through and its login is read on the way, and StarRailSim pages open in the phone's browser connect to
//! the same local socket the desktop app serves.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use futures::stream;
use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jint, jstring};
use serde_json::json;
use sync_live::decoder::Decoder;
use sync_live::hub::{Hub, Status};
use sync_live::server::serve;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::warn;

mod relay;
use relay::{Tapped, relay};

/// The desktop app's port: the site looks for an importer here.
const PAGE_PORT: u16 = 23314;

struct Engine {
    hub: Hub,
    /// To the decoder, which lives as long as the engine: a capture restarted without a new login keeps its key.
    frames: mpsc::UnboundedSender<Vec<u8>>,
    capturing: watch::Sender<bool>,
    capture: Mutex<Option<JoinHandle<()>>>,
    runtime: Runtime,
}

static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();

/// Clears `capturing` when a capture ends, however it ends.
struct Capturing(watch::Sender<bool>);

impl Drop for Capturing {
    fn drop(&mut self) {
        self.0.send_replace(false);
    }
}

#[cfg(target_os = "android")]
mod logcat {
    use std::ffi::CString;
    use std::io::{self, Write};
    use std::os::raw::{c_char, c_int};

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    /// One log line, written to logcat when tracing drops it.
    pub struct Line(Vec<u8>);

    impl Write for Line {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for Line {
        fn drop(&mut self) {
            self.0.retain(|&b| b != 0);
            if let Ok(text) = CString::new(std::mem::take(&mut self.0)) {
                unsafe { __android_log_write(4, c"NousRelicImporter".as_ptr(), text.as_ptr()) };
            }
        }
    }

    pub fn line() -> Line {
        Line(Vec::new())
    }
}

impl Engine {
    fn start(data_dir: PathBuf, page_port: u16) -> Result<Engine, String> {
        let logs = tracing_subscriber::fmt().with_ansi(false).without_time();
        #[cfg(target_os = "android")]
        let logs = logs.with_writer(logcat::line);
        logs.try_init().ok();

        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().map_err(|e| e.to_string())?;
        let hub = Hub::new(Some(data_dir.join("last-import.json")));
        hub.restore();
        let (frames, mut incoming) = mpsc::unbounded_channel();
        let decoding = hub.clone();
        runtime.spawn(async move {
            Decoder::default().read(&decoding, stream::poll_fn(move |cx| incoming.poll_recv(cx))).await;
        });
        let pages = hub.clone();
        runtime.spawn(async move {
            if let Err(e) = serve(pages.clone(), page_port, Vec::new(), None).await {
                warn!(%e, "the page socket did not open");
                pages.set_status(Status::Failed(format!("port {page_port} is in use")));
            }
        });
        Ok(Engine { hub, frames, capturing: watch::channel(false).0, capture: Mutex::new(None), runtime })
    }

    /// Relays the game's traffic through `device` (the VPN interface) and reads it, until `stop` or the interface
    /// closes. Ends the capture before it, if one is running.
    fn capture<D>(&self, device: impl FnOnce() -> std::io::Result<D> + Send + 'static)
    where
        D: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        self.stop();
        let (frames, capturing) = (self.frames.clone(), self.capturing.clone());
        *self.capture.lock().unwrap() = Some(self.runtime.spawn(async move {
            capturing.send_replace(true);
            let _on = Capturing(capturing);
            match device() {
                Ok(device) => relay(Tapped::new(device, frames)).await,
                Err(e) => warn!(%e, "the VPN interface did not open"),
            }
        }));
    }

    fn stop(&self) {
        if let Some(running) = self.capture.lock().unwrap().take() {
            running.abort();
            self.runtime.block_on(async { running.await.ok() });
        }
    }

    /// What the app shows, as JSON: the importer's state, whether a capture is running, and the account.
    fn status(&self) -> serde_json::Value {
        let current = self.hub.status().borrow().clone();
        let (state, detail) = match &current {
            Status::Starting => ("starting", None),
            Status::Failed(reason) => ("failed", Some(reason.clone())),
            Status::WaitingForGame => ("waiting", None),
            Status::GameFound => ("gameFound", None),
            Status::LoggingIn => ("loggingIn", None),
            Status::NeedsRelogin => ("needsRelogin", None),
            Status::Synced { .. } => ("synced", None),
        };
        let at = match current {
            Status::Synced { at, fresh, .. } => Some((at, fresh)),
            _ => None,
        };
        let summary = self.hub.summary();
        json!({
            "state": state,
            "detail": detail,
            "capturing": *self.capturing.borrow(),
            "pages": *self.hub.clients().borrow(),
            "relics": summary.as_ref().map(|s| s.relics),
            "lightCones": summary.as_ref().map(|s| s.light_cones),
            "characters": summary.as_ref().map(|s| s.characters),
            "trailblazer": summary.as_ref().and_then(|s| s.trailblazer.clone()),
            "at": at.map(|(at, _)| at),
            "fresh": at.is_some_and(|(_, fresh)| fresh),
            "accountFile": self.hub.cache_file().map(|path| path.display().to_string()),
        })
    }
}

fn engine() -> Option<&'static Engine> {
    ENGINE.get().and_then(|e| e.as_ref().ok())
}

/// Starts the engine once per process, keeping its account in `dataDir`: 0, or -1 if it could not (`status`
/// says why).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_starrailsim_importer_Engine_start<'local>(mut env: JNIEnv<'local>, _class: JClass<'local>, data_dir: JString<'local>) -> jint {
    let Ok(dir) = env.get_string(&data_dir).map(String::from) else { return -1 };
    match ENGINE.get_or_init(|| Engine::start(PathBuf::from(dir), PAGE_PORT)) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

/// Takes the VPN interface `fd` (detached from its ParcelFileDescriptor: the engine closes it) and starts relaying.
#[cfg(unix)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_starrailsim_importer_Engine_capture<'local>(_env: JNIEnv<'local>, _class: JClass<'local>, fd: jint) {
    use std::os::fd::{FromRawFd, OwnedFd};
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    match engine() {
        Some(engine) => engine.capture(move || relay::interface::Interface::new(fd)),
        None => drop(fd),
    }
}

/// Ends the capture; the VPN interface closes with it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_starrailsim_importer_Engine_stop<'local>(_env: JNIEnv<'local>, _class: JClass<'local>) {
    if let Some(engine) = engine() {
        engine.stop();
    }
}

/// The engine's status as JSON; before `start`, `{"state":"stopped"}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_starrailsim_importer_Engine_status<'local>(env: JNIEnv<'local>, _class: JClass<'local>) -> jstring {
    let text = match ENGINE.get() {
        Some(Ok(engine)) => engine.status().to_string(),
        Some(Err(reason)) => json!({ "state": "failed", "detail": reason }).to_string(),
        None => json!({ "state": "stopped" }).to_string(),
    };
    env.new_string(text).map_or(std::ptr::null_mut(), |s| s.into_raw())
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::net::Ipv4Addr;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use sync_core::packets::{Transport, transport};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::net::UdpSocket;
    use tokio::sync::mpsc;

    use super::Engine;

    /// A stand-in VPN interface: packets the test sends come out of it, packets written to it go back to the test.
    struct Packets {
        from_game: mpsc::UnboundedReceiver<Vec<u8>>,
        to_game: mpsc::UnboundedSender<Vec<u8>>,
    }

    impl AsyncRead for Packets {
        fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            match self.from_game.poll_recv(cx) {
                Poll::Ready(Some(packet)) => {
                    buf.put_slice(&packet);
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(None) => Poll::Ready(Ok(())),
                Poll::Pending => Poll::Pending,
            }
        }
    }

    impl AsyncWrite for Packets {
        fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, packet: &[u8]) -> Poll<io::Result<usize>> {
            self.to_game.send(packet.to_vec()).ok();
            Poll::Ready(Ok(packet.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    /// An IPv4 UDP packet with a correct header checksum and no UDP checksum.
    fn udp_packet(src: (Ipv4Addr, u16), dst: (Ipv4Addr, u16), payload: &[u8]) -> Vec<u8> {
        let total = (28 + payload.len()) as u16;
        let mut ip = vec![0x45, 0, 0, 0, 0, 0, 0x40, 0, 64, 17, 0, 0];
        ip[2..4].copy_from_slice(&total.to_be_bytes());
        ip.extend(src.0.octets());
        ip.extend(dst.0.octets());
        let sum = ip.chunks(2).map(|w| u32::from(u16::from_be_bytes([w[0], w[1]]))).sum::<u32>();
        let folded = !(((sum & 0xffff) + (sum >> 16)) as u16);
        ip[10..12].copy_from_slice(&folded.to_be_bytes());
        ip.extend(src.1.to_be_bytes());
        ip.extend(dst.1.to_be_bytes());
        ip.extend((8 + payload.len() as u16).to_be_bytes());
        ip.extend([0, 0]);
        ip.extend(payload);
        ip
    }

    #[test]
    fn the_games_datagrams_reach_their_server_and_its_answers_come_back() {
        let dir = std::env::temp_dir().join(format!("nri-android-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let engine = Engine::start(dir.clone(), 0).unwrap();
        assert_eq!(engine.status()["capturing"], false);

        // A server that answers each datagram with it reversed.
        let server = engine.runtime.block_on(UdpSocket::bind("127.0.0.1:0")).unwrap();
        let server_port = server.local_addr().unwrap().port();
        engine.runtime.spawn(async move {
            let mut buf = [0; 1500];
            while let Ok((n, from)) = server.recv_from(&mut buf).await {
                buf[..n].reverse();
                server.send_to(&buf[..n], from).await.ok();
            }
        });

        let (from_game_tx, from_game) = mpsc::unbounded_channel();
        let (to_game, mut to_game_rx) = mpsc::unbounded_channel();
        engine.capture(move || Ok(Packets { from_game, to_game }));
        let game = (Ipv4Addr::new(10, 111, 0, 2), 40000);
        from_game_tx.send(udp_packet(game, (Ipv4Addr::LOCALHOST, server_port), b"ping")).unwrap();

        let answer = engine.runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), to_game_rx.recv()).await }).unwrap().unwrap();
        assert!(matches!(transport(&answer), Transport::Udp { src, dst: 40000 } if src == server_port));
        assert!(answer.ends_with(b"gnip"), "the server's answer, back to the game unchanged");
        assert_eq!(engine.status()["capturing"], true);

        engine.stop();
        assert_eq!(engine.status()["capturing"], false);
        std::fs::remove_dir_all(&dir).ok();
    }
}
