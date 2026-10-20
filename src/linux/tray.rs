//! The notification-area icon on Linux, through the desktop's StatusNotifierItem host: KDE (the Steam Deck's
//! desktop mode too) natively, GNOME with the AppIndicator extension Ubuntu ships. Where there is no host there
//! is no icon, and starting the app again from the app menu opens its window instead.

use ksni::menu::{CheckmarkItem, StandardItem};
use ksni::{Icon, MenuItem, ToolTip};
use tokio::sync::mpsc::UnboundedSender;

use super::Action;
use crate::APP_NAME;
use crate::view::{Setup, Tone, disc};

pub struct SyncTray {
    pub headline: String,
    pub tone: Tone,
    pub setup: Setup,
    pub actions: UnboundedSender<Action>,
}

/// ksni takes ARGB in network byte order; `disc` draws RGBA.
fn icon(tone: Tone, size: u32) -> Icon {
    let data = disc(tone.colour(), size).chunks_exact(4).flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
    Icon { width: size as i32, height: size as i32, data }
}

impl ksni::Tray for SyncTray {
    fn id(&self) -> String {
        "starrailsim-sync".into()
    }

    fn title(&self) -> String {
        APP_NAME.into()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.actions.send(Action::Show).ok();
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![icon(self.tone, 22), icon(self.tone, 32), icon(self.tone, 64)]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip { title: APP_NAME.into(), description: self.headline.clone(), ..Default::default() }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let item = |label: &str, action: fn() -> Action| -> MenuItem<Self> {
            StandardItem {
                label: label.into(),
                activate: Box::new(move |tray: &mut Self| {
                    tray.actions.send(action()).ok();
                }),
                ..Default::default()
            }
            .into()
        };
        vec![
            StandardItem { label: self.headline.clone(), enabled: false, ..Default::default() }.into(),
            MenuItem::Separator,
            item("Show StarRailSim Sync", || Action::Show),
            item("Open StarRailSim", || Action::OpenSite),
            MenuItem::Separator,
            CheckmarkItem {
                label: "Start when you log in".into(),
                enabled: self.setup.installed,
                checked: self.setup.startup,
                activate: Box::new(|tray: &mut Self| {
                    tray.actions.send(Action::Startup(!tray.setup.startup)).ok();
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            item("Quit", || Action::Quit),
        ]
    }
}
