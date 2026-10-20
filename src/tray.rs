//! The notification-area icon: its colour and tooltip say what the app is doing, a left click opens the site,
//! and the menu holds the few settings there are. Runs the Win32 message loop on the main thread.

use std::time::Duration;

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, TranslateMessage, WM_QUIT,
};

use crate::hub::{Hub, Status};
use crate::{APP_NAME, install};

const GREY: [u8; 3] = [0x8a, 0x8f, 0x98];
const TEAL: [u8; 3] = [0x19, 0x9e, 0x70];
const AMBER: [u8; 3] = [0xd9, 0xa2, 0x1b];

fn thousands(n: usize) -> String {
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

fn describe(status: &Status) -> (String, [u8; 3]) {
    match status {
        Status::Starting => ("Starting…".into(), GREY),
        Status::Failed(why) => (format!("Not working: {why}"), AMBER),
        Status::WaitingForGame => ("Waiting for the game: start it and click to start".into(), GREY),
        Status::LoggingIn => ("Game found, loading your account…".into(), GREY),
        Status::NeedsRelogin => ("Log out of the game and back in to sync (if the game just updated, this app may need an update too)".into(), AMBER),
        Status::Synced { relics, .. } => (format!("Synced {} relics", thousands(*relics)), TEAL),
    }
}

/// A filled disc with a white centre, drawn at 32px with soft edges.
fn icon(colour: [u8; 3]) -> Icon {
    const SIZE: u32 = 32;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = ((x as f32 + 0.5 - 16.0).powi(2) + (y as f32 + 0.5 - 16.0).powi(2)).sqrt();
            let outer = (14.5 - d).clamp(0.0, 1.0);
            let inner = (5.5 - d).clamp(0.0, 1.0);
            let mix = |c: u8| (c as f32 * (1.0 - inner) + 255.0 * inner) as u8;
            rgba.extend([mix(colour[0]), mix(colour[1]), mix(colour[2]), (outer * 255.0) as u8]);
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).expect("a 32x32 RGBA buffer is a valid icon")
}

pub fn run(hub: Hub, site: &str, task_args: &str) {
    let status_item = MenuItem::new("Starting…", false, None);
    let open_item = MenuItem::new("Open StarRailSim", true, None);
    let startup_item = CheckMenuItem::new("Start with Windows", install::running_installed(), install::startup_registered(), None);
    let uninstall_item = MenuItem::new("Uninstall", install::running_installed(), None);
    let quit_item = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &status_item,
        &PredefinedMenuItem::separator(),
        &open_item,
        &startup_item,
        &PredefinedMenuItem::separator(),
        &uninstall_item,
        &quit_item,
    ])
    .expect("menu items append");

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip(APP_NAME)
        .with_icon(icon(GREY))
        .build()
        .expect("the notification area accepts the icon");

    let mut status = hub.status();
    let mut clients = hub.clients();
    let mut shown: Option<(Status, usize)> = None;
    let open_site = || {
        open::that_detached(site).ok();
    };

    loop {
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    return;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == *open_item.id() {
                open_site();
            } else if event.id == *startup_item.id() {
                let wanted = startup_item.is_checked();
                let result = if wanted { install::register_startup(task_args) } else { install::unregister_startup() };
                if let Err(e) = result {
                    tracing::warn!(%e, wanted, "could not change start with Windows");
                }
                startup_item.set_checked(install::startup_registered());
            } else if event.id == *uninstall_item.id() {
                if let Err(e) = install::uninstall() {
                    tracing::warn!(%e, "uninstall failed");
                }
                return;
            } else if event.id == *quit_item.id() {
                return;
            }
        }

        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                open_site();
            }
        }

        let now = (status.borrow_and_update().clone(), *clients.borrow_and_update());
        if shown.as_ref() != Some(&now) {
            let (text, colour) = describe(&now.0);
            let hint = match (&now.0, now.1) {
                (Status::Synced { .. }, 0) => "\nOpen StarRailSim to load it",
                (Status::Synced { .. }, _) => "\nStarRailSim is up to date",
                _ => "",
            };
            status_item.set_text(&text);
            let tooltip: String = format!("{APP_NAME}\n{text}{hint}").chars().take(127).collect();
            tray.set_tooltip(Some(tooltip)).ok();
            tray.set_icon(Some(icon(colour))).ok();
            shown = Some(now);
        }

        unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, Duration::from_millis(250).as_millis() as u32, QS_ALLINPUT) };
    }
}
