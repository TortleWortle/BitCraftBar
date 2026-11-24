//! BitCraft Tracker: tray app showing the status of a BitCraft character.
//! Rust/GPUI port of the macOS app in `../BitCraftTrackerBar` (see
//! `../AGENTS.md` for behavior and data sources, `README.md` for this port).

mod core;
mod model;
mod net;
mod notifier;
mod platform;
mod settings;
mod tray;
mod ui;

use std::time::Duration;

use gpui::{
    App, Application, AsyncApp, Entity, Pixels, Point, Size, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, point, prelude::*, px, size,
};

use crate::model::Model;
use crate::settings::Settings;
use crate::ui::{AppState, Edge, PANEL_WIDTH, Panel, Placement};

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
        #[cfg(target_os = "macos")]
        platform::hide_dock_icon();

        // Linux and Windows quit when the last window closes; the popup
        // comes and goes, so keep an invisible window around.
        #[cfg(not(target_os = "macos"))]
        open_keepalive_window(cx);

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
        let state = cx.new(|_| AppState::new(Model::new(Settings::load()), net, tray));

        // Tray clicks → toggle the popup.
        let (click_tx, click_rx) = async_channel::unbounded::<Anchor>();
        tray_icon::TrayIconEvent::set_event_handler(Some(move |event: tray_icon::TrayIconEvent| {
            if let tray_icon::TrayIconEvent::Click {
                rect,
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } = event
            {
                let scale = platform::scale_factor();
                let _ = click_tx.try_send(Anchor {
                    origin: point(px((rect.position.x / scale) as f32), px((rect.position.y / scale) as f32)),
                    size: size(px((rect.size.width as f64 / scale) as f32), px((rect.size.height as f64 / scale) as f32)),
                });
            }
        }));

        spawn_loops(state.clone(), out_rx, click_rx, cx);

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
        state.update(cx, |s, _| s.popup = None);
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
        window_bounds: Some(WindowBounds::Windowed(frame)),
        titlebar: None,
        focus: true,
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
    match cx.open_window(options, move |window, cx| cx.new(|cx| Panel::new(st, window, cx))) {
        Ok(handle) => {
            state.update(cx, |s, _| s.popup = Some(handle));
            cx.activate(true);
        }
        Err(e) => eprintln!("failed to open panel: {e}"),
    }
}

const INITIAL_HEIGHT: f32 = 420.0;

/// Below the icon when the tray is at the top of the screen (macOS, most
/// Linux panels): the top edge is fixed and the panel grows down. Above it
/// when at the bottom (Windows taskbar): the bottom edge is fixed. Clamped
/// to the display. Without an anchor: top-right corner.
fn placement(anchor: Option<Anchor>, cx: &App) -> Placement {
    let displays = cx.displays();
    let display = anchor
        .and_then(|a| displays.iter().find(|d| d.bounds().contains(&a.origin)).cloned())
        .or_else(|| cx.primary_display());
    let width = px(PANEL_WIDTH);
    let margin = px(6.0);
    let Some(screen) = display.map(|d| d.bounds()) else {
        return Placement { x: px(100.0), edge: Edge::Top(px(100.0)), max_height: px(600.0) };
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
        Placement { x, edge: Edge::Top(top), max_height: screen.bottom() - top - margin }
    } else {
        let bottom = a.origin.y - margin;
        Placement { x, edge: Edge::Bottom(bottom), max_height: bottom - screen.top() - margin }
    }
}

#[cfg(not(target_os = "macos"))]
fn open_keepalive_window(cx: &mut App) {
    struct Empty;
    impl gpui::Render for Empty {
        fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            gpui::div()
        }
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(gpui::Bounds { origin: point(px(-10000.0), px(-10000.0)), size: size(px(1.0), px(1.0)) })),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("bitcraft-tracker".into()),
        ..Default::default()
    };
    if let Err(e) = cx.open_window(options, |_, cx| cx.new(|_| Empty)) {
        eprintln!("keepalive window failed: {e}");
    }
}

