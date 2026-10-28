//! The Android app's engine, behind the two calls its Java side makes (`Engine.start`, `Engine.status`). It runs
//! the importer the desktop app runs, with PCAPdroid as the packet source: PCAPdroid sends its capture of the
//! game to the collector port, and StarRailSim pages open in the phone's browser connect to the same local
//! socket the desktop app serves.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::OnceLock;

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jint, jstring};
use serde_json::json;
use sync_live::collector::collect;
use sync_live::hub::{Hub, Status};
use sync_live::server::serve;
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::sync::watch;
use tracing::{info, warn};

/// The desktop app's port: the site looks for an importer here.
const PAGE_PORT: u16 = 23314;

struct Engine {
    hub: Hub,
    /// Where PCAPdroid sends the capture.
    collector_port: u16,
    capturing: watch::Receiver<bool>,
    _runtime: Runtime,
}

static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();

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

fn start(data_dir: PathBuf, page_port: u16) -> Result<Engine, String> {
    let logs = tracing_subscriber::fmt().with_ansi(false).without_time();
    #[cfg(target_os = "android")]
    let logs = logs.with_writer(logcat::line);
    logs.try_init().ok();

    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().map_err(|e| e.to_string())?;
    let hub = Hub::new(Some(data_dir.join("last-import.json")));
    hub.restore();
    let (capturing_tx, capturing) = watch::channel(false);
    let collector = runtime.block_on(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))).map_err(|e| e.to_string())?;
    let collector_port = collector.local_addr().map_err(|e| e.to_string())?.port();
    info!(collector_port, "waiting for PCAPdroid");
    runtime.spawn(collect(hub.clone(), collector, capturing_tx));
    let pages = hub.clone();
    runtime.spawn(async move {
        if let Err(e) = serve(pages.clone(), page_port, Vec::new(), None).await {
            warn!(%e, "the page socket did not open");
            pages.set_status(Status::Failed(format!("port {page_port} is in use")));
        }
    });
    Ok(Engine { hub, collector_port, capturing, _runtime: runtime })
}

/// What the app shows, as JSON: the importer's state, whether a capture is coming in, and the account.
fn status(engine: &Engine) -> serde_json::Value {
    let current = engine.hub.status().borrow().clone();
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
    let summary = engine.hub.summary();
    json!({
        "state": state,
        "detail": detail,
        "capturing": *engine.capturing.borrow(),
        "pages": *engine.hub.clients().borrow(),
        "relics": summary.as_ref().map(|s| s.relics),
        "lightCones": summary.as_ref().map(|s| s.light_cones),
        "characters": summary.as_ref().map(|s| s.characters),
        "trailblazer": summary.as_ref().and_then(|s| s.trailblazer.clone()),
        "at": at.map(|(at, _)| at),
        "fresh": at.is_some_and(|(_, fresh)| fresh),
        "accountFile": engine.hub.cache_file().map(|path| path.display().to_string()),
    })
}

/// Starts the engine once per process, keeping its account in `dataDir`. Returns the port PCAPdroid sends the
/// capture to, or -1 if it could not start (`status` says why).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_starrailsim_importer_Engine_start<'local>(mut env: JNIEnv<'local>, _class: JClass<'local>, data_dir: JString<'local>) -> jint {
    let Ok(dir) = env.get_string(&data_dir).map(String::from) else { return -1 };
    match ENGINE.get_or_init(|| start(PathBuf::from(dir), PAGE_PORT)) {
        Ok(engine) => jint::from(engine.collector_port),
        Err(_) => -1,
    }
}

/// The engine's status as JSON; before `start`, `{"state":"stopped"}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_starrailsim_importer_Engine_status<'local>(env: JNIEnv<'local>, _class: JClass<'local>) -> jstring {
    let text = match ENGINE.get() {
        Some(Ok(engine)) => status(engine).to_string(),
        Some(Err(reason)) => json!({ "state": "failed", "detail": reason }).to_string(),
        None => json!({ "state": "stopped" }).to_string(),
    };
    env.new_string(text).map_or(std::ptr::null_mut(), |s| s.into_raw())
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::{Duration, Instant};

    use super::{start, status};

    #[test]
    fn the_app_sees_a_capture_arrive_and_the_importer_wait_for_the_game() {
        let dir = std::env::temp_dir().join(format!("nri-android-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let engine = start(dir.clone(), 0).unwrap();
        let before = status(&engine);
        assert_eq!((before["state"].as_str(), before["capturing"].as_bool()), (Some("starting"), Some(false)));

        // PCAPdroid connects and sends the file header of a bare-IP capture; the game has not started yet.
        let mut pcapdroid = std::net::TcpStream::connect(("127.0.0.1", engine.collector_port)).unwrap();
        let mut header = 0xa1b2_c3d4u32.to_le_bytes().to_vec();
        header.extend([2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 101, 0, 0, 0]);
        pcapdroid.write_all(&header).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while status(&engine)["state"] != "waiting" && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let during = status(&engine);
        assert_eq!((during["state"].as_str(), during["capturing"].as_bool()), (Some("waiting"), Some(true)));
        assert!(during["relics"].is_null(), "no account before a login");
        std::fs::remove_dir_all(&dir).ok();
    }
}
