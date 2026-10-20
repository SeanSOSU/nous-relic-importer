//! How the app words and draws its state, whatever shows it: the Windows window and tray, the Linux tray, and
//! the window page in a browser.

use serde_json::{Value, json};

use crate::hub::{Hub, Status};

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
pub fn describe(status: &Status, clients: usize) -> (String, String, Tone) {
    let (headline, detail, tone): (String, &str, Tone) = match status {
        Status::Starting => ("Starting…".into(), "", Tone::Idle),
        Status::Failed(why) => {
            let logs = crate::data_dir();
            let detail = format!("{why}. Quit and start it again; if it keeps happening, the log is in {}.", logs.display());
            return ("Not working".into(), detail, Tone::Warn);
        }
        Status::WaitingForGame => (
            "Waiting for the game".into(),
            "Start Honkai: Star Rail and click to start. If you are already in game, log out and back in.",
            Tone::Idle,
        ),
        Status::GameFound => ("Game found".into(), "Log in and your account loads here.", Tone::Idle),
        Status::LoggingIn => ("Loading your account…".into(), "", Tone::Idle),
        Status::NeedsRelogin => (
            "Log out and back in".into(),
            "The game was already running when sync started, so it missed the login. Log out to the start screen and back in. If the game just updated, this app may need an update too.",
            Tone::Warn,
        ),
        Status::Synced { relics, fresh: false, .. } => (
            format!("Synced {} relics", thousands(*relics)),
            "Saved from your last session. Start the game and log in to bring it up to date.",
            Tone::Ok,
        ),
        Status::Synced { relics, fresh: true, .. } => (
            format!("Synced {} relics", thousands(*relics)),
            if clients == 0 {
                "Open StarRailSim to load them."
            } else {
                "StarRailSim is up to date. Changes you make in game show up there as they happen."
            },
            Tone::Ok,
        ),
    };
    (headline, detail.to_owned(), tone)
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

#[cfg(windows)]
const STARTUP: (&str, &str) =
    ("Start with Windows", "Starts quietly when you sign in, so it is already running before you open the game.");
#[cfg(not(windows))]
const STARTUP: (&str, &str) =
    ("Start when you log in", "Starts quietly when you log in, so it is already running before you open the game.");

/// Everything the window page renders, and the headline and tone the tray shows.
pub fn state(hub: &Hub, setup: &Setup) -> (String, Tone, Value) {
    let status = hub.status().borrow().clone();
    let clients = *hub.clients().borrow();
    let (headline, detail, tone) = describe(&status, clients);
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
        "live": fresh && clients > 0,
        "fresh": fresh,
        "uid": uid,
        "updatedAt": updated_at,
        "summary": hub.summary(),
        "savedFile": hub.cache_file().map(|p| p.display().to_string()),
        "startup": setup.startup,
        "startupLabel": STARTUP.0,
        "startupHint": STARTUP.1,
        "installed": setup.installed,
    });
    (headline, tone, json)
}

#[cfg(test)]
mod tests {
    use super::{describe, thousands};
    use crate::hub::Status;

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(["0", "999", "1,000", "1,234,567"], [0, 999, 1000, 1234567].map(thousands));
    }

    #[test]
    fn a_synced_account_points_at_the_site_until_a_page_is_open() {
        let synced = Status::Synced { relics: 1412, uid: Some(600000001), at: 0, fresh: true };
        assert_eq!(describe(&synced, 0).0, "Synced 1,412 relics");
        assert!(describe(&synced, 0).1.contains("Open StarRailSim"));
        assert!(describe(&synced, 2).1.contains("up to date"));
        let saved = Status::Synced { relics: 1412, uid: None, at: 0, fresh: false };
        assert!(describe(&saved, 2).1.contains("last session"), "a saved copy never claims to be current");
    }
}
