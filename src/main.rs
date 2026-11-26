//! BitCraft Tracker: tray app showing the status of a BitCraft character.
//! Rust/GPUI port of the macOS app in `../BitCraftTrackerBar` (see
//! `../AGENTS.md` for behavior and data sources, `README.md` for this port).

// Release builds on Windows: no console window next to the tray icon.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod core;
mod floating;
mod main_window;
mod model;
mod net;
mod notifier;
mod platform;
mod settings;
mod sound;
mod tray;
mod ui;

use std::time::Duration;

use gpui::{
    App, Application, AsyncApp, Entity, Pixels, Point, Size, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions, point, prelude::*, px, size,
};

use crate::settings::Settings;
use crate::ui::{AppState, Edge, PANEL_WIDTH, Panel, Placement};

gpui::actions!(bitcraft_tracker, [Quit]);

/// Tray click: icon rect in logical pixels (screen coordinates, top-left origin).
#[derive(Clone, Copy, Debug)]
struct Anchor {
    origin: Point<Pixels>,
    size: Size<Pixels>,
}

fn main() {
    // reqwest and the websockets share rustls; pick its crypto explicitly.
    let _ = rustls::crypto::ring::default_provider().install_default();
    notifier::init();

    Application::new().run(|cx: &mut App| {
        // macOS: a regular Dock app (user request; it used to hide in
        // the Dock as a menu bar-only app). App menu with Quit / ⌘Q.
        #[cfg(target_os = "macos")]
        {
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.bind_keys([gpui::KeyBinding::new("cmd-q", Quit, None)]);
            cx.set_menus(vec![gpui::Menu {
                name: "BitCraft Tracker".into(),
                items: vec![gpui::MenuItem::action("Quit BitCraft Tracker", Quit)],
            }]);
        }

        let (out_tx, out_rx) = async_channel::unbounded();
        let net = net::start(out_tx);

        let tray = match tray::Tray::new() {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("tray icon unavailable ({e}); opening the panel as a window");
                None
            }
        };
        let has_tray = tray.is_some();
        let state = cx.new(|_| AppState::new(Settings::load(), net, tray));

        // Tray clicks → toggle the popup.
        let (click_tx, click_rx) = async_channel::unbounded::<Anchor>();
        tray_icon::TrayIconEvent::set_event_handler(Some(
            move |event: tray_icon::TrayIconEvent| {
                if let tray_icon::TrayIconEvent::Click {
                    rect,
                    button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                } = event
                {
                    let scale = platform::scale_factor();
                    let _ = click_tx.try_send(Anchor {
                        origin: point(
                            px((rect.position.x / scale) as f32),
                            px((rect.position.y / scale) as f32),
                        ),
                        size: size(
                            px((rect.size.width as f64 / scale) as f32),
                            px((rect.size.height as f64 / scale) as f32),
                        ),
                    });
                }
            },
        ));

        spawn_loops(state.clone(), out_rx, click_rx, cx);

        // The main window (character, floating window, sounds). Closing
        // it quits the app, so it also keeps Linux/Windows alive (they
        // quit when the last window closes).
        main_window::open(&state, cx);

        let floating: Vec<String> =
            state.read(cx).settings.characters.iter().filter(|c| c.floating_window).map(|c| c.user_id.clone()).collect();
        for user_id in floating {
            floating::open(&state, &user_id, cx);
        }

        // `--open`: show the panel at launch (also the fallback without a tray).
        if !has_tray || std::env::args().any(|a| a == "--open") {
            toggle_popup(&state, None, cx);
        }
    });
}

fn spawn_loops(
    state: Entity<AppState>,
    out_rx: async_channel::Receiver<net::Outgoing>,
    click_rx: async_channel::Receiver<Anchor>,
    cx: &mut App,
) {
    let s = state.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(message) = out_rx.recv().await {
            if s.update(cx, |s, cx| s.handle_net(message, cx)).is_err() {
                break;
            }
        }
    })
    .detach();

    let s = state.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if s.update(cx, |s, cx| s.tick(cx)).is_err() {
                break;
            }
        }
    })
    .detach();

    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(anchor) = click_rx.recv().await {
            let _ = cx.update(|cx| toggle_popup(&state, Some(anchor), cx));
        }
    })
    .detach();
}

fn toggle_popup(state: &Entity<AppState>, anchor: Option<Anchor>, cx: &mut App) {
    let (open, closed_at) = {
        let s = state.read(cx);
        (s.popup, s.popup_closed_at)
    };
    if let Some(handle) = open {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        state.update(cx, |s, _| s.popup_closed());
        return;
    }
    // This click already closed it (the popup lost focus first).
    if closed_at.is_some_and(|t| t.elapsed() < Duration::from_millis(400)) {
        return;
    }

    let place = placement(anchor, cx);
    // First guess; the panel resizes the window to its content once laid out.
    let frame = place.frame(px(INITIAL_HEIGHT));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(platform::open_bounds(frame))),
        titlebar: None,
        // Don't steal focus from the game (macOS / Windows); it takes focus
        // when clicked, e.g. to type in the character search.
        focus: !platform::NON_ACTIVATING_POPUP,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("bitcraft-tracker".into()),
        ..Default::default()
    };
    state.update(cx, |s, _| s.popup_place = Some(place));
    let st = state.clone();
    match cx.open_window(options, move |window, cx| {
        if platform::NON_ACTIVATING_POPUP {
            platform::set_no_activate(window, true);
        }
        cx.new(|cx| Panel::new(st, window, cx))
    }) {
        Ok(handle) => {
            state.update(cx, |s, _| s.popup = Some(handle));
            if platform::NON_ACTIVATING_POPUP {
                watch_outside_clicks(state.clone(), handle, cx);
            } else {
                cx.activate(true);
            }
        }
        Err(e) => eprintln!("failed to open panel: {e}"),
    }
}

const INITIAL_HEIGHT: f32 = 420.0;

/// A popup that never got focus doesn't hear about clicks elsewhere (they
/// go to an app that's already active), so poll the mouse buttons while
/// it's open and close it on a press outside it. A press on the tray icon
/// closes it here too; the click that follows is then ignored (400ms).
fn watch_outside_clicks(state: Entity<AppState>, handle: gpui::WindowHandle<Panel>, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut was_down = true;
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
            let Ok(Some(bounds)) = cx.update(|cx| {
                (state.read(cx).popup == Some(handle))
                    .then(|| handle.update(cx, |_, w, _| w.bounds()).ok())
                    .flatten()
            }) else {
                break;
            };
            let pressed = platform::pressed_mouse_location();
            let outside = pressed.is_some_and(|p| !bounds.contains(&p));
            if outside && !was_down {
                let _ = cx.update(|cx| {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                    state.update(cx, |s, _| s.popup_closed());
                });
                break;
            }
            was_down = pressed.is_some();
        }
    })
    .detach();
}

/// Below the icon when the tray is at the top of the screen (macOS, most
/// Linux panels): the top edge is fixed and the panel grows down. Above it
/// when at the bottom (Windows taskbar): the bottom edge is fixed. Clamped
/// to the display. Without an anchor: top-right corner.
fn placement(anchor: Option<Anchor>, cx: &App) -> Placement {
    let displays = cx.displays();
    let display = anchor
        .and_then(|a| {
            displays
                .iter()
                .find(|d| d.bounds().contains(&a.origin))
                .cloned()
        })
        .or_else(|| cx.primary_display());
    let width = px(PANEL_WIDTH);
    let margin = px(6.0);
    let Some(screen) = display.map(|d| d.bounds()) else {
        return Placement {
            x: px(100.0),
            edge: Edge::Top(px(100.0)),
            max_height: px(600.0),
        };
    };
    let Some(a) = anchor else {
        let top = screen.top() + px(32.0);
        return Placement {
            x: screen.right() - width - margin * 4.0,
            edge: Edge::Top(top),
            max_height: screen.bottom() - top - margin,
        };
    };
    let center_x = a.origin.x + a.size.width / 2.0;
    let x = (center_x - width / 2.0).clamp(screen.left() + margin, screen.right() - width - margin);
    if a.origin.y < screen.top() + screen.size.height / 2.0 {
        let top = a.origin.y + a.size.height + margin;
        Placement {
            x,
            edge: Edge::Top(top),
            max_height: screen.bottom() - top - margin,
        }
    } else {
        let bottom = a.origin.y - margin;
        Placement {
            x,
            edge: Edge::Bottom(bottom),
            max_height: bottom - screen.top() - margin,
        }
    }
}
