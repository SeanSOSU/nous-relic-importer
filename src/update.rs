//! Keeps the installed app current. A few seconds after it starts (a minute and a half when it starts at sign-in),
//! then every six hours, it reads the release feed. A newer build is downloaded, checked against the release key
//! built into this one, and kept beside the installed copy. It then takes over the way a downloaded update does:
//! it asks this copy to quit and copies itself into place. On Windows that happens by itself once the game has
//! been quiet for five minutes, so it never cuts into a game session. On Linux it waits for Install, as the new
//! file needs the reader's password for the capture permission again.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use minisign_verify::{PublicKey, Signature};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::hub::Hub;
use crate::settings;
#[cfg(windows)]
use crate::install;
#[cfg(target_os = "linux")]
use crate::linux::install;

/// The release key's public half. A test build can trust another key: its base64 line in NRI_UPDATE_KEY at
/// build time.
const KEY: &str = match option_env!("NRI_UPDATE_KEY") {
    Some(key) => key,
    None => include_str!("../update-key.pub"),
};

/// What `nri-release sign` looks for, to be sure a build is the version it is signing.
const MARK: &str = concat!("nous-relic-importer-version:", env!("CARGO_PKG_VERSION"), "\0");

const FIRST_CHECK: Duration = Duration::from_secs(5);
const FIRST_CHECK_AT_SIGN_IN: Duration = Duration::from_secs(90);
const EVERY: Duration = Duration::from_secs(6 * 3600);
const RETRY: Duration = Duration::from_secs(30 * 60);
/// How long the game must have been quiet before an update installs by itself.
const QUIET: Duration = Duration::from_secs(5 * 60);
/// A version that would not install is not downloaded again for this long.
const FAILED_PAUSE: Duration = Duration::from_secs(24 * 3600);
const FEED_LIMIT: u64 = 256 * 1024;
const BUILD_LIMIT: u64 = 256 * 1024 * 1024;

const EXE: &str = if cfg!(windows) { "nous-relic-importer.exe" } else { "nous-relic-importer" };
const PARTIAL: &str = if cfg!(windows) { "nous-relic-importer.exe.part" } else { "nous-relic-importer.part" };
/// Beside a staged build that could not replace the installed copy: the version that failed.
const FAILED: &str = "failed";

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// This build's version, read out of the mark so the mark is always in the file.
pub fn version() -> &'static str {
    let mark = std::hint::black_box(MARK);
    &mark["nous-relic-importer-version:".len()..mark.len() - 1]
}

/// Which build of a release this copy takes, as the feed names them: windows-x86_64, linux-x86_64.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// This build carries no usable release key.
    Off,
    /// Before the first check.
    Waiting,
    Checking,
    /// `at`: when, in milliseconds since the Unix epoch.
    Current { at: u64 },
    Downloading { version: String },
    /// Downloaded, verified and waiting beside the installed copy.
    Ready { version: String },
    /// For a copy that is not installed, so is never replaced: it points at the download instead.
    Available { version: String },
    Installing { version: String },
    Failed { why: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub step: Step,
    /// Installs by itself rather than waiting for Install.
    pub auto: bool,
    /// Whether this copy can install by itself at all: the installed copy, on Windows.
    pub can_auto: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Request {
    Check,
    Install,
    /// Open the new version's download, for a copy that is not installed.
    Download,
    Auto(bool),
}

impl Request {
    /// What the window's update controls send.
    pub fn from_message(message: &str) -> Option<Self> {
        Some(match message {
            "update-check" => Request::Check,
            "update-install" => Request::Install,
            "update-download" => Request::Download,
            "update-auto-on" => Request::Auto(true),
            "update-auto-off" => Request::Auto(false),
            _ => return None,
        })
    }
}

/// What the tray, the window and another thread use to see and steer the updater.
#[derive(Clone)]
pub struct Updater {
    state: Arc<watch::Sender<State>>,
    requests: mpsc::Sender<Request>,
}

impl Updater {
    pub fn subscribe(&self) -> watch::Receiver<State> {
        self.state.subscribe()
    }

    pub fn state(&self) -> State {
        self.state.borrow().clone()
    }

    pub fn ask(&self, request: Request) {
        self.requests.send(request).ok();
    }
}

/// What this run of the app is, as far as updating goes.
pub struct Plan {
    pub feed: String,
    /// Only the installed copy replaces itself.
    pub installed: bool,
    /// What the new copy is started with: this copy's own arguments, less the one-time ones.
    pub relaunch: Vec<String>,
    /// Started at sign-in, when the network may not be up yet.
    pub background: bool,
    /// Where a downloaded build waits, beside the installed copy.
    pub staging: PathBuf,
    pub current: String,
    pub platform: String,
}

impl Plan {
    pub fn new(feed: String, relaunch: Vec<String>, background: bool) -> Self {
        Plan {
            feed,
            installed: install::running_installed(),
            relaunch,
            background,
            staging: install::install_dir().join("update"),
            current: version().to_owned(),
            platform: platform(),
        }
    }
}

fn release_key(text: &str) -> Option<PublicKey> {
    let line = text.lines().map(str::trim).rfind(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))?;
    PublicKey::from_base64(line).ok()
}

/// Starts the updater's thread. With no usable release key it never checks, and says so.
pub fn start(plan: Plan, hub: Hub) -> Updater {
    let can_auto = cfg!(windows) && plan.installed;
    let auto = can_auto && settings::get(settings::INSTALL_UPDATES, true);
    spawn(plan, hub, release_key(KEY), State { step: Step::Waiting, auto, can_auto })
}

fn spawn(plan: Plan, hub: Hub, key: Option<PublicKey>, state: State) -> Updater {
    let (requests, receiver) = mpsc::channel();
    let Some(key) = key else {
        warn!("no release key in this build; updates are off");
        let state = Arc::new(watch::channel(State { step: Step::Off, ..state }).0);
        return Updater { state, requests };
    };
    let state = Arc::new(watch::channel(state).0);
    let agent = ureq::AgentBuilder::new()
        .https_only(plan.feed.starts_with("https://"))
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(60))
        .user_agent(&format!("NousRelicImporter/{}", plan.current))
        .try_proxy_from_env(true)
        .build();
    let worker = Worker { plan, key, hub, state: state.clone(), agent, staged: None, download: None };
    std::thread::Builder::new().name("updates".into()).spawn(move || worker.run(receiver)).ok();
    Updater { state, requests }
}

/// The feed: the newest version, and where each platform's build of it is.
#[derive(serde::Deserialize)]
struct Feed {
    version: String,
    platforms: HashMap<String, Build>,
}

#[derive(serde::Deserialize)]
struct Build {
    url: String,
    size: u64,
    /// The build's minisign signature, whose trusted comment names the program, version and platform.
    signature: String,
}

/// A verified build waiting to run, with its signature to check it again first.
struct Staged {
    version: String,
    path: PathBuf,
    signature: String,
}

struct Worker {
    plan: Plan,
    key: PublicKey,
    hub: Hub,
    state: Arc<watch::Sender<State>>,
    agent: ureq::Agent,
    staged: Option<Staged>,
    /// For a copy that is not installed: the new build's address.
    download: Option<String>,
}

impl Worker {
    fn run(mut self, requests: mpsc::Receiver<Request>) {
        let mut next_check = Instant::now() + if self.plan.background { FIRST_CHECK_AT_SIGN_IN } else { FIRST_CHECK };
        loop {
            let mut wait = next_check.saturating_duration_since(Instant::now());
            if self.waiting_to_install() {
                wait = wait.min(Duration::from_secs(30));
            }
            match requests.recv_timeout(wait) {
                Ok(Request::Check) => {
                    std::fs::remove_file(self.plan.staging.join(FAILED)).ok();
                    next_check = Instant::now();
                }
                Ok(Request::Install) => self.install(false),
                Ok(Request::Download) => {
                    if let Some(url) = &self.download {
                        open::that_detached(url).ok();
                    }
                }
                Ok(Request::Auto(on)) => {
                    settings::set(settings::INSTALL_UPDATES, on);
                    self.state.send_modify(|s| s.auto = on && s.can_auto);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            if Instant::now() >= next_check {
                next_check = Instant::now() + if self.check() { EVERY } else { RETRY };
            }
            if self.waiting_to_install() && self.hub.game_quiet_for(QUIET) {
                self.install(true);
            }
        }
    }

    fn waiting_to_install(&self) -> bool {
        self.staged.is_some() && self.state.borrow().auto
    }

    fn set(&self, step: Step) {
        self.state.send_if_modified(|s| {
            let changed = s.step != step;
            s.step = step;
            changed
        });
    }

    /// Reads the feed and fetches a newer build. False when the feed or the build could not be had, to try
    /// again sooner than the usual six hours.
    fn check(&mut self) -> bool {
        if self.staged.is_none() {
            for leftover in [EXE, PARTIAL] {
                std::fs::remove_file(self.plan.staging.join(leftover)).ok();
            }
            self.set(Step::Checking);
        }
        let feed = match self.feed() {
            Ok(feed) => feed,
            Err(why) => {
                warn!(why, "update check failed");
                match &self.staged {
                    Some(staged) => self.set(Step::Ready { version: staged.version.clone() }),
                    None => self.set(Step::Failed { why }),
                }
                return false;
            }
        };
        let build = match feed.platforms.get(&self.plan.platform) {
            Some(build) if newer(&feed.version, &self.plan.current) => build,
            _ => {
                info!(latest = feed.version, "up to date");
                self.staged = None;
                self.set(Step::Current { at: now_ms() });
                return true;
            }
        };
        let version = feed.version.clone();
        if let Some(staged) = self.staged.as_ref().filter(|s| s.version == version) {
            self.set(Step::Ready { version: staged.version.clone() });
            return true;
        }
        if !self.plan.installed {
            self.download = Some(build.url.clone());
            self.set(Step::Available { version });
            return true;
        }
        if self.failed_recently(&version) {
            self.set(Step::Failed { why: format!("version {version} did not install, so it tries again tomorrow") });
            return true;
        }
        info!(version, "downloading an update");
        self.set(Step::Downloading { version: version.clone() });
        match self.fetch(&version, build) {
            Ok(staged) => {
                info!(version, "update downloaded and verified");
                self.staged = Some(staged);
                self.set(Step::Ready { version });
                true
            }
            Err(why) => {
                warn!(version, why, "update refused");
                self.staged = None;
                self.set(Step::Failed { why });
                false
            }
        }
    }

    fn feed(&self) -> Result<Feed, String> {
        let body = get(&self.agent, &self.plan.feed, FEED_LIMIT)?;
        serde_json::from_slice(&body).map_err(|_| "the update feed is unreadable".to_owned())
    }

    fn fetch(&self, version: &str, build: &Build) -> Result<Staged, String> {
        if build.size > BUILD_LIMIT {
            return Err("the new version is too large".to_owned());
        }
        let bytes = get(&self.agent, &build.url, build.size)?;
        if bytes.len() as u64 != build.size {
            return Err("the download was cut short".to_owned());
        }
        verify(&self.key, &bytes, &build.signature, version, &self.plan.platform)?;
        let path = stage(&self.plan.staging, &bytes).map_err(|e| format!("could not save the download ({e})"))?;
        Ok(Staged { version: version.to_owned(), path, signature: build.signature.clone() })
    }

    fn failed_recently(&self, version: &str) -> bool {
        let marker = self.plan.staging.join(FAILED);
        let recent = std::fs::metadata(&marker)
            .and_then(|m| m.modified())
            .is_ok_and(|at| at.elapsed().is_ok_and(|age| age < FAILED_PAUSE));
        recent && std::fs::read_to_string(&marker).is_ok_and(|failed| failed.trim() == version)
    }

    /// Starts the staged build to take over. `background`: the new copy starts without its window.
    fn install(&mut self, background: bool) {
        let Some(staged) = self.staged.take() else { return };
        // The file waited on disk: it is checked again before it runs.
        let checked = std::fs::read(&staged.path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| verify(&self.key, &bytes, &staged.signature, &staged.version, &self.plan.platform));
        if let Err(why) = checked {
            warn!(why, "the staged update changed");
            return self.set(Step::Failed { why: format!("the downloaded update changed on disk: {why}") });
        }
        info!(version = staged.version, background, "handing over to the update");
        self.set(Step::Installing { version: staged.version.clone() });
        let mut command = std::process::Command::new(&staged.path);
        command.arg("--update").args(&self.plan.relaunch);
        if background {
            command.arg("--background");
        }
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
        let why = match command.spawn() {
            // It asks this copy to quit within seconds; ending first, it could not take over.
            Ok(mut child) => match wait(&mut child, Duration::from_secs(60)) {
                Some(status) if !status.success() => format!("the new version could not take over ({status})"),
                _ => return,
            },
            Err(e) => format!("could not start the new version ({e})"),
        };
        warn!(why, "update did not install");
        self.set(Step::Failed { why });
    }
}

fn wait(child: &mut std::process::Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// Whether `candidate` is a later major.minor.patch than `current`.
pub fn newer(candidate: &str, current: &str) -> bool {
    fn number(v: &str) -> Option<[u64; 3]> {
        let mut parts = v.trim().split('.');
        let mut number = [0; 3];
        for n in &mut number {
            *n = parts.next()?.parse().ok()?;
        }
        parts.next().is_none().then_some(number)
    }
    matches!((number(candidate), number(current)), (Some(c), Some(v)) if c > v)
}

/// A build is good when the release key signed it and the signature names this program, `version` and
/// `platform`, so a genuine older build or another platform's cannot pass for this one.
pub fn verify(key: &PublicKey, bytes: &[u8], signature: &str, version: &str, platform: &str) -> Result<(), String> {
    let signature = Signature::decode(signature).map_err(|_| "the download's signature is unreadable".to_owned())?;
    key.verify(bytes, &signature, false).map_err(|_| "the download is not signed with the release key".to_owned())?;
    let expected = format!("nous-relic-importer {version} {platform}");
    if signature.trusted_comment() != expected {
        warn!(signed = signature.trusted_comment(), expected, "an update signed for something else");
        return Err("the download is signed for another version or platform".to_owned());
    }
    Ok(())
}

/// Plain HTTP only to this machine, for tests; the agent itself refuses it once the feed is HTTPS.
fn allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else { return url.starts_with("https://") };
    let host = rest.split('/').next().unwrap_or_default();
    let host = host.rsplit_once(':').map_or(host, |(name, _)| name);
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

fn get(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    if !allowed(url) {
        return Err("the update address is not HTTPS".to_owned());
    }
    let response = agent.get(url).call().map_err(|e| match e {
        ureq::Error::Status(code, _) => format!("the update server answered {code}"),
        ureq::Error::Transport(t) => {
            warn!(%t, url, "no connection for the update");
            "could not reach the update server".to_owned()
        }
    })?;
    let mut body = Vec::new();
    response.into_reader().take(limit + 1).read_to_end(&mut body).map_err(|_| "the download was cut short".to_owned())?;
    if body.len() as u64 > limit {
        return Err("the download is larger than the feed says".to_owned());
    }
    Ok(body)
}

fn stage(dir: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let (path, partial) = (dir.join(EXE), dir.join(PARTIAL));
    std::fs::write(&partial, bytes)?;
    #[cfg(unix)]
    std::fs::set_permissions(&partial, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    std::fs::rename(&partial, &path)?;
    Ok(path)
}

/// The new copy's half of an update, run as `--update` from where the running copy downloaded it: the running
/// copy steps aside, this file replaces the installed one, and the installed one starts with `args`. False if
/// the running copy would not quit or nothing could be started.
pub fn take_over(args: &[String]) -> bool {
    #[cfg(target_os = "linux")]
    use crate::linux::{Signal, SingleInstance};
    #[cfg(windows)]
    use crate::win::{Signal, SingleInstance};

    if SingleInstance::other_running() && !Signal::quit_and_wait(Duration::from_secs(20)) {
        warn!("the running copy did not quit; the update waits for the next try");
        return false;
    }
    let target = install::installed_exe();
    match replace(&target) {
        Ok(()) => {
            info!(version = version(), "update installed");
            #[cfg(windows)]
            if crate::win::has_uninstall_entry() {
                crate::win::add_uninstall_entry(&target, &install::install_dir());
            }
        }
        // The copy that was there starts again, so sync carries on; this version waits a day before it retries.
        Err(e) => {
            warn!(%e, "could not replace the installed copy");
            if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
                std::fs::write(dir.join(FAILED), version()).ok();
            }
        }
    }
    let mut installed = std::process::Command::new(&target);
    installed.args(args);
    // Inherited from the copy that was installed: the new file has not been granted anything yet.
    #[cfg(target_os = "linux")]
    installed.env_remove(crate::GRANTED);
    match installed.spawn() {
        Ok(_) => true,
        Err(e) => {
            warn!(%e, "could not start the installed copy");
            false
        }
    }
}

/// Copies this executable over `target`: beside it first, then renamed into place. The copy that just quit can
/// hold its file a moment longer, so this tries for a few seconds.
fn replace(target: &Path) -> std::io::Result<()> {
    let this = std::env::current_exe()?;
    let temporary = target.with_extension("new");
    let attempt = || -> std::io::Result<()> {
        std::fs::copy(&this, &temporary)?;
        #[cfg(unix)]
        std::fs::set_permissions(&temporary, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
        std::fs::rename(&temporary, target)
    };
    let mut tries = 0;
    loop {
        match attempt() {
            Ok(()) => return Ok(()),
            Err(e) if tries == 50 => {
                std::fs::remove_file(&temporary).ok();
                return Err(e);
            }
            Err(_) => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    use super::*;

    /// A release key made for the test, and a build signed with it as `trusted`.
    struct Release {
        key: PublicKey,
        secret: minisign::SecretKey,
        public: minisign::PublicKey,
    }

    impl Release {
        fn new() -> Self {
            let minisign::KeyPair { pk, sk } = minisign::KeyPair::generate_encrypted_keypair(Some(String::new())).unwrap();
            Release { key: PublicKey::from_base64(&pk.to_base64()).unwrap(), secret: sk, public: pk }
        }

        fn sign(&self, bytes: &[u8], trusted: &str) -> String {
            minisign::sign(Some(&self.public), &self.secret, bytes, Some(trusted), None).unwrap().to_string()
        }
    }

    /// Serves fixed bodies by path on 127.0.0.1, one connection at a time. `routes` is given the address.
    fn serve(routes: impl FnOnce(&str) -> Vec<(String, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let routes = routes(&address);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split(' ').nth(1).unwrap_or_default().to_owned();
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                let mut stream = stream;
                match routes.iter().find(|(p, _)| *p == path) {
                    Some((_, body)) => {
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                        stream.write_all(body).unwrap();
                    }
                    None => write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(),
                }
            }
        });
        address
    }

    fn worker(release: &Release, feed: String, installed: bool, staging: &Path) -> Worker {
        let plan = Plan {
            feed,
            installed,
            relaunch: Vec::new(),
            background: false,
            staging: staging.to_path_buf(),
            current: "0.1.0".into(),
            platform: "windows-x86_64".into(),
        };
        let state = Arc::new(watch::channel(State { step: Step::Waiting, auto: true, can_auto: true }).0);
        let agent = ureq::AgentBuilder::new().timeout_read(Duration::from_secs(5)).build();
        Worker { plan, key: release.key.clone(), hub: Hub::new(None), state, agent, staged: None, download: None }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nri-update-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    /// A feed for `version` whose Windows build is `served`, signed over `signed` as `trusted`.
    fn release_feed(release: &Release, version: &str, served: &[u8], signed: &[u8], trusted: &str) -> String {
        let address = serve(|address| {
            let build = serde_json::json!({ "url": format!("{address}/build"), "size": served.len(), "signature": release.sign(signed, trusted) });
            let feed = serde_json::json!({ "version": version, "platforms": { "windows-x86_64": build } });
            vec![("/latest.json".into(), feed.to_string().into_bytes()), ("/build".into(), served.to_vec())]
        });
        format!("{address}/latest.json")
    }

    #[test]
    fn a_newer_signed_build_is_staged_and_anything_else_is_refused() {
        let release = Release::new();
        let build = b"the new build".to_vec();
        let trusted = "nous-relic-importer 0.2.0 windows-x86_64";

        let dir = scratch("good");
        let mut good = worker(&release, release_feed(&release, "0.2.0", &build, &build, trusted), true, &dir);
        assert!(good.check());
        assert_eq!(good.state.borrow().step, Step::Ready { version: "0.2.0".into() });
        assert_eq!(std::fs::read(dir.join(EXE)).unwrap(), build, "the staged file is the build itself");

        let tampered = b"the new build, changed".to_vec();
        let dir = scratch("tampered");
        let mut bad = worker(&release, release_feed(&release, "0.2.0", &tampered, &build, trusted), true, &dir);
        assert!(!bad.check());
        assert!(matches!(&bad.state.borrow().step, Step::Failed { why } if why.contains("not signed")));
        assert!(!dir.join(EXE).exists(), "a refused build is never kept");

        // A genuine 0.1.5 build offered as 0.2.0 names its real version in the signature.
        let dir = scratch("replayed");
        let older = "nous-relic-importer 0.1.5 windows-x86_64";
        let mut replayed = worker(&release, release_feed(&release, "0.2.0", &build, &build, older), true, &dir);
        assert!(!replayed.check());
        assert!(matches!(&replayed.state.borrow().step, Step::Failed { why } if why.contains("another version")));

        let other_key = Release::new();
        let dir = scratch("other-key");
        let mut foreign = worker(&release, release_feed(&other_key, "0.2.0", &build, &build, trusted), true, &dir);
        assert!(!foreign.check());
        assert!(!dir.join(EXE).exists());
    }

    #[test]
    fn the_same_or_an_older_version_is_current_and_an_uninstalled_copy_only_points_at_the_download() {
        let release = Release::new();
        let build = b"build".to_vec();
        let dir = scratch("current");
        let same = release_feed(&release, "0.1.0", &build, &build, "nous-relic-importer 0.1.0 windows-x86_64");
        let mut current = worker(&release, same, true, &dir);
        assert!(current.check());
        assert!(matches!(current.state.borrow().step, Step::Current { .. }));

        let newer_feed = release_feed(&release, "0.2.0", &build, &build, "nous-relic-importer 0.2.0 windows-x86_64");
        let mut portable = worker(&release, newer_feed, false, &dir);
        assert!(portable.check());
        assert_eq!(portable.state.borrow().step, Step::Available { version: "0.2.0".into() });
        assert!(portable.download.as_deref().is_some_and(|url| url.ends_with("/build")));
        assert!(!dir.join(EXE).exists(), "a copy that is not installed downloads nothing itself");

        let mut unreachable = worker(&release, "http://127.0.0.1:9/latest.json".into(), true, &dir);
        assert!(!unreachable.check());
        assert!(matches!(unreachable.state.borrow().step, Step::Failed { .. }));
    }

    #[test]
    fn versions_compare_by_number_and_feeds_must_be_https_off_this_machine() {
        assert!(newer("0.2.0", "0.1.9") && newer("0.10.0", "0.9.0") && newer("1.0.0", "0.99.99"));
        assert!(!newer("0.1.0", "0.1.0") && !newer("0.1.0", "0.2.0") && !newer("0.2", "0.1.0") && !newer("0.2.0-beta", "0.1.0"));
        assert!(allowed(crate::UPDATE_FEED) && allowed("http://127.0.0.1:8765/latest.json") && allowed("http://localhost/x"));
        assert!(!allowed("http://example.com/latest.json") && !allowed("http://127.0.0.1.example.com/x") && !allowed("ftp://x"));
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        assert!(release_key(include_str!("../update-key.pub")).is_some(), "the committed release key reads");
    }

    #[test]
    fn a_version_that_would_not_install_waits_a_day() {
        let release = Release::new();
        let build = b"build".to_vec();
        let dir = scratch("failed");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FAILED), "0.2.0").unwrap();
        let feed = release_feed(&release, "0.2.0", &build, &build, "nous-relic-importer 0.2.0 windows-x86_64");
        let mut worker = worker(&release, feed, true, &dir);
        assert!(worker.check());
        assert!(matches!(&worker.state.borrow().step, Step::Failed { why } if why.contains("tomorrow")));
        assert!(!dir.join(EXE).exists());
    }
}
