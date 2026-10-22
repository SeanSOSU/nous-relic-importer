//! How the app words and draws its state, whatever shows it: the Windows window and tray, the Linux tray, and
//! the window page in a browser.

use serde_json::{Value, json};

use crate::hub::{Hub, Status};
use crate::i18n::{self, t, tf};
use crate::update::{self, Request, Step};

#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    Idle,
    Ok,
    Warn,
}

impl Tone {
    pub fn colour(self) -> [u8; 3] {
        match self {
            Tone::Idle => [0x8a, 0x8f, 0x98],
            Tone::Ok => [0x19, 0x9e, 0x70],
            Tone::Warn => [0xd9, 0xa2, 0x1b],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tone::Idle => "idle",
            Tone::Ok => "ok",
            Tone::Warn => "warn",
        }
    }
}

pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The one place the app words its state: the window's headline and next step, the tray's tooltip and colour.
/// `sending`: the Auto import switch.
pub fn describe(status: &Status, clients: usize, sending: bool) -> (String, String, Tone) {
    let synced = |relics: usize| tf("Synced {n} relics", &[("n", &thousands(relics))]);
    let (headline, detail, tone): (String, &'static str, Tone) = match status {
        Status::Starting => (t("Starting…").into(), "", Tone::Idle),
        Status::Failed(why) => {
            let logs = crate::data_dir().display().to_string();
            let detail = tf("{why}. Quit and start it again; if it keeps happening, the log is in {logs}.", &[("why", why), ("logs", &logs)]);
            return (t("Not working").into(), detail, Tone::Warn);
        }
        Status::WaitingForGame => (
            t("Waiting for the game").into(),
            "Start Honkai: Star Rail and click to start. If you are already in game, log out and back in.",
            Tone::Idle,
        ),
        Status::GameFound => (t("Game found").into(), "Log in and your account loads here.", Tone::Idle),
        Status::LoggingIn => (t("Loading your account…").into(), "", Tone::Idle),
        Status::NeedsRelogin => (
            t("Log out and back in").into(),
            "The game was already running when sync started, so it missed the login. Log out to the start screen and back in. If the game just updated, this app may need an update too.",
            Tone::Warn,
        ),
        Status::Synced { relics, fresh: false, .. } => (
            synced(*relics),
            "Saved from your last session. Start the game and log in to bring it up to date.",
            Tone::Ok,
        ),
        Status::Synced { relics, fresh: true, .. } => (
            synced(*relics),
            if !sending {
                "Auto import is off, so the site keeps what it last got. Changes in game are still saved here."
            } else if clients == 0 {
                "Open StarRailSim to load them."
            } else {
                "StarRailSim is up to date. Changes you make in game show up there as they happen."
            },
            Tone::Ok,
        ),
    };
    (headline, t(detail).to_owned(), tone)
}

/// A filled disc with a white centre and soft edges, as RGBA.
pub fn disc(colour: [u8; 3], size: u32) -> Vec<u8> {
    let (c, outer, inner) = (size as f32 / 2.0, size as f32 * 14.5 / 32.0, size as f32 * 5.5 / 32.0);
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            let a = (outer - d).clamp(0.0, 1.0);
            let w = (inner - d).clamp(0.0, 1.0);
            let mix = |ch: u8| (ch as f32 * (1.0 - w) + 255.0 * w) as u8;
            rgba.extend([mix(colour[0]), mix(colour[1]), mix(colour[2]), (a * 255.0) as u8]);
        }
    }
    rgba
}

/// Where the app is installed and whether it starts at sign-in, as the window shows it.
#[derive(Clone, Copy)]
pub struct Setup {
    pub installed: bool,
    pub startup: bool,
}

pub const STARTUP: (&str, &str) = ("Launch on startup", "Automatically runs on computer restart.");

pub const SENDING: (&str, &str) = ("Auto import", "Automatically imports to StarRailSim and keeps your equipment in sync.");

/// The window's update line: what is happening, and the one thing it offers to do.
pub fn describe_update(state: &update::State) -> Value {
    let v = |text: &'static str, version: &str| tf(text, &[("version", version)]);
    let (text, action) = match &state.step {
        Step::Off => return Value::Null,
        Step::Waiting => (t("Checks for a new version shortly.").to_owned(), Some(("check", "Check now"))),
        Step::Checking => (t("Checking for a new version…").to_owned(), None),
        Step::Current { .. } => (t("Up to date.").to_owned(), Some(("check", "Check now"))),
        Step::Downloading { version } => (v("Downloading version {version}…", version), None),
        Step::Ready { version } if state.auto => (
            v("Version {version} is ready. It installs by itself once the game has been closed for a few minutes.", version),
            Some(("install", "Install now")),
        ),
        Step::Ready { version } if cfg!(windows) => (v("Version {version} is ready to install.", version), Some(("install", "Install"))),
        Step::Ready { version } => (
            v("Version {version} is ready. Installing it asks for your password, as the first install did.", version),
            Some(("install", "Install")),
        ),
        Step::Available { version } => (v("Version {version} is out.", version), Some(("download", "Download"))),
        Step::Installing { version } => (v("Installing version {version}…", version), None),
        Step::Failed { why } => (tf("Could not update: {why}.", &[("why", why)]), Some(("check", "Try again"))),
    };
    json!({
        "text": text,
        "checkedAt": match state.step { Step::Current { at } => Some(at), _ => None },
        "action": action.map(|a| a.0),
        "actionLabel": action.map(|a| t(a.1)),
        "auto": state.auto,
        "canAuto": state.can_auto,
        "installing": matches!(state.step, Step::Installing { .. }),
    })
}

/// The tray menu's update item: its label, and what a click asks for (None greys it out).
pub fn update_item(state: &update::State) -> (String, Option<Request>) {
    let v = |text: &'static str, version: &str| tf(text, &[("version", version)]);
    match &state.step {
        Step::Off => (t("Updates are off").into(), None),
        Step::Checking => (t("Checking for updates…").into(), None),
        Step::Downloading { version } => (v("Downloading version {version}…", version), None),
        Step::Installing { version } => (v("Installing version {version}…", version), None),
        Step::Ready { version } => (v("Install version {version}", version), Some(Request::Install)),
        Step::Available { version } => (v("Download version {version}", version), Some(Request::Download)),
        Step::Waiting | Step::Current { .. } | Step::Failed { .. } => (t("Check for updates").into(), Some(Request::Check)),
    }
}

/// Everything the window page renders, and the headline and tone the tray shows.
pub fn state(hub: &Hub, setup: &Setup, update: &update::State) -> (String, Tone, Value) {
    let status = hub.status().borrow().clone();
    let clients = *hub.clients().borrow();
    let sending = *hub.sending().borrow();
    let (headline, detail, tone) = describe(&status, clients, sending);
    let (uid, updated_at, fresh) = match status {
        Status::Synced { uid, at, fresh, .. } => (uid, Some(at), fresh),
        _ => (None, None, false),
    };
    let json = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "headline": headline,
        "detail": detail,
        "tone": tone.name(),
        // Pulsing: an account from this session, feeding a page that is open right now.
        "live": fresh && clients > 0 && sending,
        "fresh": fresh,
        "uid": uid,
        "updatedAt": updated_at,
        "summary": hub.summary(),
        "savedFile": hub.cache_file().map(|p| p.display().to_string()),
        "sending": sending,
        "sendingLabel": t(SENDING.0),
        "sendingHint": t(SENDING.1),
        "startup": setup.startup,
        "startupLabel": t(STARTUP.0),
        "startupHint": t(STARTUP.1),
        "installed": setup.installed,
        "update": describe_update(update),
        // The page's own words and the language picker: what is shown, what was picked, what Automatic means here.
        "lang": i18n::current().code(),
        "language": i18n::choice(),
        "systemLanguage": i18n::system().name(),
        "strings": i18n::page_strings(),
    });
    (headline, tone, json)
}

#[cfg(test)]
mod tests {
    use super::{describe, describe_update, thousands, update_item};
    use crate::hub::Status;
    use crate::update::{Request, State, Step};

    #[test]
    fn a_ready_update_says_when_it_installs_and_offers_to_install_now() {
        let ready = |auto| State { step: Step::Ready { version: "0.2.0".into() }, auto, can_auto: auto };
        let auto = describe_update(&ready(true));
        assert!(auto["text"].as_str().unwrap().contains("closed for a few minutes"));
        assert_eq!((auto["action"].as_str(), auto["actionLabel"].as_str()), (Some("install"), Some("Install now")));
        assert_eq!(describe_update(&ready(false))["action"], "install");
        assert_eq!(update_item(&ready(false)), ("Install version 0.2.0".to_owned(), Some(Request::Install)));
        let off = State { step: Step::Off, auto: false, can_auto: false };
        assert!(describe_update(&off).is_null(), "a build with no release key shows no update line");
        let checking = State { step: Step::Checking, ..off };
        assert_eq!(update_item(&checking).1, None, "nothing to click while it checks");
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(["0", "999", "1,000", "1,234,567"], [0, 999, 1000, 1234567].map(thousands));
    }

    #[test]
    fn a_synced_account_points_at_the_site_until_a_page_is_open() {
        let synced = Status::Synced { relics: 1412, uid: Some(600000001), at: 0, fresh: true };
        assert_eq!(describe(&synced, 0, true).0, "Synced 1,412 relics");
        assert!(describe(&synced, 0, true).1.contains("Open StarRailSim"));
        assert!(describe(&synced, 2, true).1.contains("up to date"));
        assert!(describe(&synced, 2, false).1.contains("is off"), "a paused page is not told it is up to date");
        let saved = Status::Synced { relics: 1412, uid: None, at: 0, fresh: false };
        assert!(describe(&saved, 2, true).1.contains("last session"), "a saved copy never claims to be current");
    }
}
