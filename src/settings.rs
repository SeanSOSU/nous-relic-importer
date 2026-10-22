//! The reader's switches that live in the app rather than in the system: `settings.json` beside the logs.

use std::path::PathBuf;

fn file() -> PathBuf {
    crate::data_dir().join("settings.json")
}

fn read() -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn get(key: &str, default: bool) -> bool {
    read().get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

pub fn set(key: &str, on: bool) {
    let mut settings = read();
    settings.insert(key.into(), on.into());
    if let Err(e) = std::fs::write(file(), serde_json::Value::Object(settings).to_string()) {
        tracing::warn!(%e, key, "could not save a setting");
    }
}

/// Install updates without waiting for Install (Windows).
pub const INSTALL_UPDATES: &str = "installUpdates";
/// Send the account to StarRailSim pages; off pauses them where they are.
const SEND_TO_SITE: &str = "sendToSite";

/// The Auto import switch as the last run left it.
pub fn sending() -> bool {
    get(SEND_TO_SITE, true)
}

/// Flips Auto import: open pages hear at once, and the next start keeps it.
pub fn set_sending(hub: &crate::hub::Hub, on: bool) {
    hub.set_sending(on);
    set(SEND_TO_SITE, on);
}
