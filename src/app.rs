//! What the user sees: a notification-area icon that is always there, and a small window that opens on a
//! manual launch or a click on the icon. The window is built when shown and torn down when closed, so the
//! copy that starts at sign-in costs nothing until someone looks at it.

use tao::dpi::LogicalSize;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Theme, Window, WindowBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use wry::{WebContext, WebView, WebViewBuilder};

use crate::hub::Hub;
use crate::view::{self, Setup, Tone, disc};
use crate::win::Signal;
use crate::{APP_NAME, install};

const UI: &str = include_str!("ui.html");
const PAGE: (u8, u8, u8, u8) = (10, 15, 28, 255);

enum UserEvent {
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    Ui(String),
    Changed,
    Show,
    Quit,
}

pub struct Launch {
    pub site: String,
    /// What the sign-in task starts the installed copy with.
    pub task_args: String,
    pub show_window: bool,
}

struct Tray {
    icon: TrayIcon,
    status: MenuItem,
    show: MenuItem,
    open: MenuItem,
    startup: CheckMenuItem,
    quit: MenuItem,
    tone: Option<Tone>,
}

impl Tray {
    /// None only if Windows refuses the icon's hidden window; a taskbar that is not up yet (an early sign-in)
    /// is fine, as tray-icon adds the icon when it appears.
    fn new(startup: bool, installed: bool) -> Option<Self> {
        let status = MenuItem::new("Starting…", false, None);
        let show = MenuItem::new("Show StarRailSim Sync", true, None);
        let open = MenuItem::new("Open StarRailSim", true, None);
        let startup = CheckMenuItem::new("Start with Windows", installed, startup, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &status,
            &PredefinedMenuItem::separator(),
            &show,
            &open,
            &PredefinedMenuItem::separator(),
            &startup,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .ok()?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip(APP_NAME)
            .with_icon(tray_icon::Icon::from_rgba(disc(Tone::Idle.colour(), 32), 32, 32).ok()?)
            .build()
            .map_err(|e| tracing::warn!(%e, "no tray icon"))
            .ok()?;
        Some(Self { icon, status, show, open, startup, quit, tone: None })
    }

    fn update(&mut self, headline: &str, tone: Tone) {
        self.status.set_text(headline);
        let tooltip: String = format!("{APP_NAME}\n{headline}").chars().take(127).collect();
        self.icon.set_tooltip(Some(tooltip)).ok();
        if self.tone != Some(tone) {
            self.icon.set_icon(tray_icon::Icon::from_rgba(disc(tone.colour(), 32), 32, 32).ok()).ok();
            self.tone = Some(tone);
        }
    }
}

fn open_window(
    target: &EventLoopWindowTarget<UserEvent>,
    context: &mut WebContext,
    proxy: &EventLoopProxy<UserEvent>,
) -> Option<(Window, WebView)> {
    let window = WindowBuilder::new()
        .with_title(APP_NAME)
        .with_inner_size(LogicalSize::new(460.0, 590.0))
        .with_resizable(false)
        .with_maximizable(false)
        .with_theme(Some(Theme::Dark))
        .with_window_icon(tao::window::Icon::from_rgba(disc(Tone::Ok.colour(), 64), 64, 64).ok())
        .build(target)
        .map_err(|e| tracing::warn!(%e, "could not open the window"))
        .ok()?;
    let proxy = proxy.clone();
    let webview = WebViewBuilder::new_with_web_context(context)
        .with_html(UI)
        .with_background_color(PAGE)
        .with_ipc_handler(move |request| {
            proxy.send_event(UserEvent::Ui(request.body().clone())).ok();
        })
        .build(&window)
        .map_err(|e| tracing::warn!(%e, "could not start the window's web view"))
        .ok()?;
    Some((window, webview))
}

/// Runs the tray and the window until Quit. Never returns.
pub fn run(hub: Hub, launch: Launch, quit: Signal, show_signal: Signal) -> ! {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let forward = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        forward.send_event(UserEvent::Tray(event)).ok();
    }));
    let forward = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event| {
        forward.send_event(UserEvent::Menu(event)).ok();
    }));

    // Status changes arrive on tokio watch channels; one small thread turns them into loop events.
    let (forward, mut status, mut clients) = (proxy.clone(), hub.status(), hub.clients());
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().build().expect("watch runtime");
        runtime.block_on(async move {
            loop {
                let changed = tokio::select! {
                    r = status.changed() => r.is_ok(),
                    r = clients.changed() => r.is_ok(),
                };
                if !changed || forward.send_event(UserEvent::Changed).is_err() {
                    break;
                }
            }
        });
    });
    let forward = proxy.clone();
    std::thread::spawn(move || {
        quit.wait();
        forward.send_event(UserEvent::Quit).ok();
    });
    let forward = proxy.clone();
    std::thread::spawn(move || {
        loop {
            show_signal.wait();
            if forward.send_event(UserEvent::Show).is_err() {
                break;
            }
        }
    });

    let installed = install::running_installed();
    let mut startup = install::startup_registered();
    let mut tray: Option<Tray> = None;
    let mut window: Option<(Window, WebView)> = None;
    let mut context = WebContext::new(Some(crate::data_dir().join("webview")));

    let files = hub.clone();
    let state = move |startup: bool| view::state(&hub, &Setup { installed, startup });

    event_loop.run(move |event, target, control_flow| {
        *control_flow = ControlFlow::Wait;
        let mut refresh = false;
        let mut show = false;
        match event {
            Event::NewEvents(StartCause::Init) => {
                tray = Tray::new(startup, installed);
                refresh = true;
                show = launch.show_window;
            }
            Event::UserEvent(UserEvent::Changed) => refresh = true,
            Event::UserEvent(UserEvent::Show) => show = true,
            Event::UserEvent(UserEvent::Tray(TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            })) => show = true,
            Event::UserEvent(UserEvent::Tray(_)) => {}
            Event::UserEvent(UserEvent::Menu(event)) => {
                let Some(t) = &tray else { return };
                if event.id == *t.show.id() {
                    show = true;
                } else if event.id == *t.open.id() {
                    open::that_detached(&launch.site).ok();
                } else if event.id == *t.startup.id() {
                    set_startup(t.startup.is_checked(), &launch.task_args);
                    startup = install::startup_registered();
                    refresh = true;
                } else if event.id == *t.quit.id() {
                    tray = None;
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::UserEvent(UserEvent::Ui(message)) => match message.as_str() {
                "ready" => refresh = true,
                "open-site" => {
                    open::that_detached(&launch.site).ok();
                }
                "show-file" => {
                    if let Some(path) = files.cache_file() {
                        use std::os::windows::process::CommandExt;
                        std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", path.display())).spawn().ok();
                    }
                }
                "startup-on" | "startup-off" => {
                    set_startup(message == "startup-on", &launch.task_args);
                    startup = install::startup_registered();
                    refresh = true;
                }
                "quit" => {
                    tray = None;
                    *control_flow = ControlFlow::Exit;
                }
                "uninstall" => {
                    if let Err(e) = install::uninstall() {
                        tracing::warn!(%e, "uninstall failed");
                    }
                    tray = None;
                    *control_flow = ControlFlow::Exit;
                }
                other => tracing::warn!(other, "unknown message from the window"),
            },
            Event::UserEvent(UserEvent::Quit) => {
                tray = None;
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => window = None,
            _ => {}
        }

        if show {
            match &window {
                Some((w, _)) => {
                    w.set_visible(true);
                    w.set_minimized(false);
                    w.set_focus();
                }
                None => window = open_window(target, &mut context, &proxy),
            }
        }
        if refresh {
            let (headline, tone, json) = state(startup);
            if let Some(t) = &mut tray {
                t.update(&headline, tone);
                t.startup.set_checked(startup);
            }
            if let Some((_, webview)) = &window {
                webview.evaluate_script(&format!("window.render && window.render({json})")).ok();
            }
        }
    })
}

fn set_startup(on: bool, task_args: &str) {
    let result = if on { install::register_startup(task_args) } else { install::unregister_startup() };
    if let Err(e) = result {
        tracing::warn!(%e, on, "could not change start with Windows");
    }
}
