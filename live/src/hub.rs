//! The state the capture worker, the socket server and the tray share: what the app is doing, the account
//! snapshot a newly connected page starts from, the live frames every connected page receives, and the copy
//! of the last account kept on disk so the next start has it before the game is even open.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sync_core::export::Exporter;
use sync_core::export::fribbels::{OptimizerEvent, OptimizerExporter};
use tokio::sync::{broadcast, watch};
use tracing::{info, warn};

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Starting,
    Failed(String),
    WaitingForGame,
    /// The game opened its connection; the login that carries the account has not arrived yet.
    GameFound,
    LoggingIn,
    /// Packets arrived that need a session key this app never saw: the game was already logged in.
    NeedsRelogin,
    /// `at` is when the account last changed, in milliseconds since the Unix epoch. `fresh` is false for the
    /// copy loaded from disk at start, until a login in this session replaces it.
    Synced { relics: usize, uid: Option<u32>, at: u64, fresh: bool },
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// What the window shows of the account: its size, and who and what it is mostly made of.
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub relics: usize,
    pub light_cones: usize,
    pub characters: usize,
    pub trailblazer: Option<String>,
    /// Avatar ids of the characters wearing the most relics, most first.
    pub featured: Vec<String>,
    /// The commonest five-star relic sets as (set id, pieces), most first.
    pub top_sets: Vec<(String, usize)>,
}

fn top<K: Ord + Clone>(counts: HashMap<K, usize>, n: usize) -> Vec<(K, usize)> {
    let mut ranked: Vec<_> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(n);
    ranked
}

pub fn summarize(export: &serde_json::Value) -> Summary {
    let list = |key: &str| export.get(key).and_then(|v| v.as_array()).map(Vec::as_slice).unwrap_or_default();
    let text = |item: &serde_json::Value, key: &str| item.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
    let relics = list("relics");
    let mut wearers = HashMap::new();
    let mut sets = HashMap::new();
    for relic in relics {
        let owner = text(relic, "location");
        if !owner.is_empty() {
            *wearers.entry(owner).or_insert(0) += 1;
        }
        if relic.get("rarity").and_then(|r| r.as_u64()) == Some(5) {
            *sets.entry(text(relic, "set_id")).or_insert(0) += 1;
        }
    }
    Summary {
        relics: relics.len(),
        light_cones: list("light_cones").len(),
        characters: list("characters").len(),
        trailblazer: export.pointer("/metadata/trailblazer").and_then(|t| t.as_str()).map(str::to_owned),
        featured: top(wearers, 8).into_iter().map(|(id, _)| id).collect(),
        top_sets: top(sets, 4),
    }
}

/// One message as a page receives it: `{"event": ..., "data": ...}`, serialized once for every client.
pub type Frame = Arc<str>;

#[derive(Clone)]
pub struct Hub {
    inner: Arc<Inner>,
}

struct Inner {
    status: watch::Sender<Status>,
    clients: watch::Sender<usize>,
    /// Whether pages get the account: the reader's Auto import switch.
    sending: watch::Sender<bool>,
    frames: broadcast::Sender<Frame>,
    // The current account as one InitialScan frame. Held under the same lock a subscriber takes, so a page
    // never misses a delta that lands between its snapshot and its subscription.
    snapshot: Mutex<Option<Frame>>,
    summary: Mutex<Option<Summary>>,
    // The same account as a bare export, for the file; `saves` wakes the thread that writes it.
    export: Arc<Mutex<Option<Arc<str>>>>,
    saves: Option<mpsc::Sender<()>>,
    cache: Option<PathBuf>,
    // When the game last sent anything, in milliseconds after `started`, plus one: 0 is never.
    started: Instant,
    last_game: AtomicU64,
}

fn initial_scan(export_json: &str) -> Frame {
    Frame::from(format!(r#"{{"event":"InitialScan","data":{export_json}}}"#))
}

/// Writes the latest export a moment after it changes, so a burst of in-game edits is one write. Written
/// beside the target and renamed over it, so a crash mid-write never leaves half a file.
fn spawn_saver(path: PathBuf, export: Arc<Mutex<Option<Arc<str>>>>) -> mpsc::Sender<()> {
    let (tx, rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        while rx.recv().is_ok() {
            std::thread::sleep(Duration::from_millis(300));
            while rx.try_recv().is_ok() {}
            let Some(json) = export.lock().unwrap().clone() else { continue };
            let temporary = path.with_extension("json.tmp");
            let written = std::fs::write(&temporary, json.as_bytes()).and_then(|_| std::fs::rename(&temporary, &path));
            if let Err(e) = written {
                warn!(%e, path = %path.display(), "could not save the account");
            }
        }
    });
    tx
}

impl Hub {
    /// `cache`: where the last account is kept between runs; None keeps nothing on disk.
    pub fn new(cache: Option<PathBuf>) -> Self {
        let export = Arc::new(Mutex::new(None));
        let saves = cache.clone().map(|path| spawn_saver(path, export.clone()));
        Self {
            inner: Arc::new(Inner {
                status: watch::channel(Status::Starting).0,
                clients: watch::channel(0).0,
                sending: watch::channel(true).0,
                frames: broadcast::channel(64).0,
                snapshot: Mutex::new(None),
                summary: Mutex::new(None),
                export,
                saves,
                cache,
                started: Instant::now(),
                last_game: AtomicU64::new(0),
            }),
        }
    }

    fn since_start(&self) -> u64 {
        self.inner.started.elapsed().as_millis() as u64 + 1
    }

    /// Notes game traffic, so an update never restarts the app while a session is being read.
    pub fn game_seen(&self) {
        self.inner.last_game.store(self.since_start(), Ordering::Relaxed);
    }

    /// Whether the game has sent nothing for `quiet`, or nothing at all since the app started.
    pub fn game_quiet_for(&self, quiet: Duration) -> bool {
        let last = self.inner.last_game.load(Ordering::Relaxed);
        last == 0 || self.since_start() - last >= quiet.as_millis() as u64
    }

    pub fn status(&self) -> watch::Receiver<Status> {
        self.inner.status.subscribe()
    }

    pub fn clients(&self) -> watch::Receiver<usize> {
        self.inner.clients.subscribe()
    }

    pub fn sending(&self) -> watch::Receiver<bool> {
        self.inner.sending.subscribe()
    }

    /// Off, open pages are told the app is paused and keep what they have; on, they start over from the account.
    pub fn set_sending(&self, on: bool) {
        self.inner.sending.send_if_modified(|current| std::mem::replace(current, on) != on);
    }

    pub fn set_status(&self, status: Status) {
        self.inner.status.send_if_modified(|current| {
            let changed = *current != status;
            *current = status;
            changed
        });
    }

    pub fn is_synced(&self) -> bool {
        matches!(*self.inner.status.borrow(), Status::Synced { .. })
    }

    pub fn summary(&self) -> Option<Summary> {
        self.inner.summary.lock().unwrap().clone()
    }

    /// The saved account file, once one exists.
    pub fn cache_file(&self) -> Option<&Path> {
        self.inner.cache.as_deref().filter(|path| path.exists())
    }

    /// Makes `export` the account every page gets, sends `frame` to the pages already open, and saves it.
    fn set_account(&self, export: &serde_json::Value, export_json: String, frame: Option<Frame>, at: u64, fresh: bool) {
        let summary = summarize(export);
        let uid = export.pointer("/metadata/uid").and_then(|u| u.as_u64()).map(|u| u as u32);
        let status = Status::Synced { relics: summary.relics, uid, at, fresh };
        let snapshot = initial_scan(&export_json);
        let mut held = self.inner.snapshot.lock().unwrap();
        *held = Some(snapshot.clone());
        self.inner.frames.send(frame.unwrap_or(snapshot)).ok();
        drop(held);
        *self.inner.summary.lock().unwrap() = Some(summary);
        *self.inner.export.lock().unwrap() = Some(Arc::from(export_json));
        if let Some(saves) = &self.inner.saves {
            saves.send(()).ok();
        }
        self.set_status(status);
    }

    /// Forwards one exporter event to every page and refreshes the snapshot from the exporter's state.
    pub fn publish(&self, event: &OptimizerEvent, exporter: &OptimizerExporter) {
        let Some(export) = exporter.export() else { return };
        let export = match serde_json::to_value(&export) {
            Ok(value) => value,
            Err(e) => return warn!(%e, "could not serialize the account"),
        };
        let frame = match event {
            OptimizerEvent::InitialScan(_) => None,
            _ => match serde_json::to_string(event) {
                Ok(json) => Some(Frame::from(json)),
                Err(e) => return warn!(%e, "could not serialize an update"),
            },
        };
        self.set_account(&export, export.to_string(), frame, now_ms(), true);
    }

    /// Serves a saved export as if it had just been captured (`--replay`).
    pub fn publish_saved(&self, export: serde_json::Value) {
        self.set_account(&export, export.to_string(), None, now_ms(), true);
    }

    /// Starts from the account the last run saved, if there is one: pages get it before the game is open.
    pub fn restore(&self) {
        let Some(path) = self.inner.cache.clone() else { return };
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let Ok(export) = serde_json::from_str::<serde_json::Value>(&text) else {
            return warn!(path = %path.display(), "the saved account is not readable; ignoring it");
        };
        if export.get("relics").and_then(|r| r.as_array()).is_none() {
            return;
        }
        let saved = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        info!(path = %path.display(), "restored the last saved account");
        // No page is open yet and the file already holds this, so nothing is sent or rewritten.
        let summary = summarize(&export);
        let uid = export.pointer("/metadata/uid").and_then(|u| u.as_u64()).map(|u| u as u32);
        let status = Status::Synced { relics: summary.relics, uid, at: saved, fresh: false };
        *self.inner.snapshot.lock().unwrap() = Some(initial_scan(&text));
        *self.inner.summary.lock().unwrap() = Some(summary);
        *self.inner.export.lock().unwrap() = Some(Arc::from(text));
        self.set_status(status);
    }

    pub fn subscribe(&self) -> (Option<Frame>, broadcast::Receiver<Frame>) {
        let held = self.inner.snapshot.lock().unwrap();
        (held.clone(), self.inner.frames.subscribe())
    }

    pub fn snapshot(&self) -> Option<Frame> {
        self.inner.snapshot.lock().unwrap().clone()
    }

    pub fn client_joined(&self) {
        self.inner.clients.send_modify(|n| *n += 1);
    }

    pub fn client_left(&self) {
        self.inner.clients.send_modify(|n| *n = n.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::{Hub, Status};

    #[test]
    fn a_page_starts_from_the_snapshot_and_then_follows_the_frames() {
        let hub = Hub::new(None);
        let (before, mut early) = hub.subscribe();
        assert!(before.is_none());

        hub.publish_saved(serde_json::json!({ "metadata": { "uid": 600000001 }, "relics": [{ "_uid": "1" }, { "_uid": "2" }] }));
        assert!(matches!(*hub.status().borrow(), Status::Synced { relics: 2, uid: Some(600000001), fresh: true, .. }));
        assert!(early.try_recv().unwrap().contains(r#""event":"InitialScan""#));

        let (snapshot, mut late) = hub.subscribe();
        assert!(snapshot.unwrap().contains(r#""_uid":"2""#));
        assert!(late.try_recv().is_err(), "a late page gets the snapshot, not a replay of old frames");
    }

    #[test]
    fn the_last_account_is_saved_and_the_next_start_begins_from_it() {
        let dir = std::env::temp_dir().join(format!("nri-hub-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("last-import.json");
        std::fs::remove_file(&file).ok();

        let hub = Hub::new(Some(file.clone()));
        hub.publish_saved(serde_json::json!({ "metadata": { "uid": 7 }, "relics": [{ "_uid": "1" }] }));
        for _ in 0..50 {
            if file.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(saved["relics"][0]["_uid"], "1", "the file is the export itself, importable anywhere");

        let next = Hub::new(Some(file.clone()));
        next.restore();
        assert!(matches!(*next.status().borrow(), Status::Synced { relics: 1, uid: Some(7), fresh: false, .. }));
        assert!(next.subscribe().0.unwrap().starts_with(r#"{"event":"InitialScan","data":{"#));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_game_is_quiet_until_it_sends_something_and_for_as_long_as_it_has_not() {
        let hub = Hub::new(None);
        let minutes = std::time::Duration::from_secs(300);
        assert!(hub.game_quiet_for(minutes), "no game since the app started");
        hub.game_seen();
        assert!(!hub.game_quiet_for(minutes));
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(hub.game_quiet_for(std::time::Duration::from_millis(20)));
    }

    #[test]
    fn the_summary_names_the_best_dressed_characters_and_commonest_sets() {
        let relic = |set: &str, rarity: u64, owner: &str| serde_json::json!({ "set_id": set, "rarity": rarity, "location": owner });
        let export = serde_json::json!({
            "metadata": { "trailblazer": "Stelle" },
            "relics": [relic("102", 5, "1102"), relic("102", 5, "1102"), relic("108", 5, "1005"), relic("108", 4, ""), relic("302", 5, "")],
            "light_cones": [{}, {}],
            "characters": [{}, {}, {}],
        });
        let s = super::summarize(&export);
        assert_eq!((s.relics, s.light_cones, s.characters, s.trailblazer.as_deref()), (5, 2, 3, Some("Stelle")));
        assert_eq!(s.featured, ["1102", "1005"]);
        assert_eq!(s.top_sets, [("102".to_owned(), 2), ("108".to_owned(), 1), ("302".to_owned(), 1)]);
    }
}
