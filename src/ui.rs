//! GPUI views: `AppState` (owns the model, tray and popup) and the popup
//! `Panel` (port of the Swift `StatusPanelView` + `CharacterPicker`).

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, Context, Entity, FocusHandle, FontWeight,
    Hsla, KeyDownEvent, Pixels, Render, Rgba, SharedString, Subscription, Task, Window,
    WindowAppearance, WindowHandle, div, point, prelude::*, pulsating_between, px, relative, rgb,
    size,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::core::badges::{self, Badge, BadgeState};
use crate::core::skill::Skill;
use crate::core::status::{HealthStatus, food_rgb, stamina_rgb};
use crate::core::trackers::{Craft, TierSummary};
use crate::core::types::{Gather, PlayerSummary};
use crate::model::Model;
use crate::net::{Command, Outgoing, RelayState};
use crate::notifier;
use crate::sound::Sound;
use crate::tray::Tray;

pub const PANEL_WIDTH: f32 = 320.0;

/// Which popup edge stays put when it's resized to fit its content.
#[derive(Clone, Copy, Debug)]
pub enum Edge {
    /// Tray at the top of the screen: grow downwards from here.
    Top(Pixels),
    /// Tray at the bottom (Windows taskbar): grow upwards from here.
    Bottom(Pixels),
}

/// Where the popup goes, in GPUI's logical screen coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub x: Pixels,
    pub edge: Edge,
    /// Room on screen; longer content scrolls.
    pub max_height: Pixels,
}

impl Placement {
    pub fn frame(&self, height: Pixels) -> Bounds<Pixels> {
        let height = height.min(self.max_height);
        let y = match self.edge {
            Edge::Top(top) => top,
            Edge::Bottom(bottom) => bottom - height,
        };
        Bounds {
            origin: point(self.x, y),
            size: size(px(PANEL_WIDTH), height),
        }
    }
}
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
pub(crate) const MAX_RESULTS_SHOWN: usize = 10;

pub struct AppState {
    pub model: Model,
    net: UnboundedSender<Command>,
    tray: Option<Tray>,
    pub popup: Option<WindowHandle<Panel>>,
    pub popup_place: Option<Placement>,
    /// Height the popup window was last sized to (avoids resize loops).
    popup_height: Option<Pixels>,
    /// Clicking the tray while the popup is open first deactivates (closes)
    /// it; don't reopen it from that same click.
    pub popup_closed_at: Option<Instant>,
    /// The main window (`crate::main_window`): always exists once
    /// opened at launch; closing it hides it.
    pub main: Option<WindowHandle<crate::main_window::MainWindow>>,
    /// The floating window (`crate::floating`).
    pub floating: Option<WindowHandle<crate::floating::FloatingWindow>>,
    pub floating_height: Option<Pixels>,
    /// Character search open in the main window.
    pub picking: bool,
    pub query: String,
    pub results: Vec<PlayerSummary>,
    pub searching: bool,
    pub search_error: Option<String>,
    search_task: Option<Task<()>>,
    /// Newer GitHub release, shown in the popup.
    pub update: Option<crate::net::api::Release>,
    last_debug_line: Option<String>,
}

impl AppState {
    pub fn new(model: Model, net: UnboundedSender<Command>, tray: Option<Tray>) -> Self {
        let mut state = Self {
            model,
            net,
            tray,
            popup: None,
            popup_place: None,
            popup_height: None,
            popup_closed_at: None,
            main: None,
            floating: None,
            floating_height: None,
            picking: false,
            query: String::new(),
            results: Vec::new(),
            searching: false,
            search_error: None,
            search_task: None,
            update: None,
            last_debug_line: None,
        };
        state.flush();
        state
    }

    pub fn handle_net(&mut self, message: Outgoing, cx: &mut Context<Self>) {
        match message {
            Outgoing::Event { user, event } => self.model.handle(&user, event),
            Outgoing::SearchResults { query, result } => {
                if query.trim() == self.query.trim() {
                    self.searching = false;
                    match result {
                        Ok(players) => {
                            self.results = players;
                            self.search_error = None;
                        }
                        Err(e) => self.search_error = Some(e),
                    }
                }
            }
            Outgoing::UpdateAvailable(release) => self.update = Some(release),
        }
        self.flush();
        cx.notify();
    }

    pub fn tick(&mut self, cx: &mut Context<Self>) {
        self.model.tick();
        self.flush();
        cx.notify();
    }

    /// Forward model outputs: network commands, notifications, tray.
    fn flush(&mut self) {
        for command in self.model.take_commands() {
            let _ = self.net.send(command);
        }
        for n in self.model.take_notifications() {
            notifier::notify(n.title, n.body);
        }
        let prefs = self.model.settings.sounds.clone();
        for sound in self.model.take_sounds() {
            let enabled = match sound {
                Sound::CraftFinished => prefs.craft_finished,
                Sound::OutOfFood => prefs.out_of_food,
                Sound::IdleFull => prefs.idle_full,
                Sound::ResourceDepleted => prefs.resource_depleted,
                Sound::TradeRequest => prefs.trade_request,
            };
            if enabled {
                crate::sound::play(sound, prefs.volume);
            }
        }
        if let Some(tray) = &mut self.tray {
            let m = &self.model;
            let (rgb, stamina) = if m.is_loading {
                (HealthStatus::Warning.rgb(), Some(0.0))
            } else {
                (m.monitor.overall().rgb(), m.stamina.fraction())
            };
            tray.update(rgb, stamina, &m.label_text());
        }
        // BITCRAFT_DEBUG=1: log label changes (no tray/screenshot needed).
        if std::env::var_os("BITCRAFT_DEBUG").is_some() {
            let m = &self.model;
            let line = format!(
                "[{:?}] {} | offline {} sat_max {} | stamina {:?}/{:?} | relay {:?} | plants {} | passive {} | crafts {} | food {}",
                m.monitor.overall(),
                m.label_text(),
                m.is_offline,
                m.satiation.max,
                m.stamina.stamina,
                m.stamina.max,
                m.relay_state,
                m.plants.tiers.iter().map(|t| t.total).sum::<usize>(),
                m.passive.crafts.len(),
                m.crafts.crafts.len(),
                m.food.as_ref().map_or("?".to_string(), |f| format!(
                    "{} regen_left={:?}",
                    f.summary(m.now),
                    f.regen_ends_at(m.now).map(|e| (e - m.now) as i64)
                )),
            );
            let texts = |list: Vec<Badge>| {
                list.iter()
                    .map(|b| format!("{}:{:?} ({})", b.text(), b.state, b.tooltip(m.now)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let gather = m.active_gather().map_or("-".to_string(), |g| {
                format!(
                    "{} skill {:?} {:?} {:?}/{:?}",
                    g.name,
                    g.skill,
                    g.bar(m.now).map(|b| b.1),
                    g.health,
                    g.max_health
                )
            });
            let line = format!(
                "{line} | gather {gather} | badges plants [{}] passive [{}] crafts [{}]",
                texts(badges::plants(&m.plants.plants)),
                texts(badges::passive(&m.passive.crafts, m.now)),
                texts(badges::crafts(
                    &m.crafts.sorted(),
                    m.now,
                    m.crafts.active_window
                )),
            );
            if self.last_debug_line.as_deref() != Some(line.as_str()) {
                eprintln!("{line}");
                self.last_debug_line = Some(line);
            }
        }
    }

    /// Forget the popup (its window is being removed).
    pub fn popup_closed(&mut self) {
        self.popup = None;
        self.popup_height = None;
        self.popup_closed_at = Some(Instant::now());
    }

    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        self.search_error = None;
        let q = self.query.trim().to_string();
        if q.chars().count() < 2 {
            self.results.clear();
            self.searching = false;
            self.search_task = None;
            cx.notify();
            return;
        }
        self.searching = true;
        let timer = cx.background_executor().timer(SEARCH_DEBOUNCE);
        self.search_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, _| {
                if this.query.trim() == q {
                    let _ = this.net.send(Command::Search(q));
                }
            });
        }));
        cx.notify();
    }

    pub fn select(&mut self, player: PlayerSummary, cx: &mut Context<Self>) {
        self.model.select_player(&player);
        self.picking = false;
        self.query.clear();
        self.results.clear();
        self.flush();
        cx.notify();
    }
}

// MARK: - Panel

pub struct Panel {
    state: Entity<AppState>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Panel {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        let subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            // Close when clicking elsewhere (like a menu bar popover).
            // Linux only: on macOS/Windows the popup doesn't take focus
            // (and AppKit gives a non-activating panel key status briefly
            // on its own), so clicks elsewhere are detected by
            // `main::watch_outside_clicks` instead.
            cx.observe_window_activation(window, |this, window, cx| {
                if !crate::platform::NON_ACTIVATING_POPUP && !window.is_window_active() {
                    this.state.update(cx, |s, _| s.popup_closed());
                    window.remove_window();
                }
            }),
        ];
        Self {
            state,
            focus,
            _subscriptions: subscriptions,
        }
    }

    /// Escape closes the popup (while it has focus: Linux, or after a
    /// click on it).
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            self.state.update(cx, |s, cx| {
                s.popup_closed();
                cx.notify();
            });
            window.remove_window();
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Theme {
    pub bg: Rgba,
    pub text: Rgba,
    pub secondary: Rgba,
    pub border: Rgba,
    pub track: Rgba,
    pub hover: Rgba,
}

impl Theme {
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self {
                bg: rgb(0x242426),
                text: rgb(0xf2f2f2),
                secondary: rgb(0xa8a8ad),
                border: rgb(0x3a3a3c),
                track: rgb(0x3a3a3c),
                hover: rgb(0x343438),
            },
            _ => Self {
                bg: rgb(0xf7f7f7),
                text: rgb(0x1c1c1e),
                secondary: rgb(0x636366),
                border: rgb(0xd8d8dc),
                track: rgb(0xe0e0e4),
                hover: rgb(0xeaeaee),
            },
        }
    }
}

pub(crate) fn status_color(status: HealthStatus) -> Rgba {
    rgb(status.rgb())
}

pub use crate::core::types::duration_string;

pub(crate) fn dot(color: impl Into<Hsla>, size: f32) -> impl IntoElement {
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(color.into())
}

/// Gather bar (resource health / fishing spot time): green, so it reads
/// differently from the blue craft bar.
pub(crate) const GATHER_RGB: u32 = 0x34c759;

pub(crate) fn bar(fraction: f64, color: Rgba, theme: Theme) -> impl IntoElement {
    div()
        .flex_1()
        .min_w(px(40.0))
        .h(px(6.0))
        .rounded_full()
        .bg(theme.track)
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(color)
                .w(relative(fraction.clamp(0.0, 1.0) as f32)),
        )
}

/// Colored "type + tier" pills (see `core::badges`), wrapping.
pub(crate) fn badges(id: &'static str, list: &[Badge], now: f64, theme: Theme) -> impl IntoElement {
    // More than one tier: a row per tier with a "T2" heading, and the
    // badges without the tier.
    let rows = badges::by_tier(list);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .children(
            rows.into_iter()
                .enumerate()
                .map(move |(r, (heading, items))| {
                    let grouped = heading.is_some();
                    div()
                        .flex()
                        .items_start()
                        .gap_1p5()
                        .children(heading.map(|tier| {
                            div()
                                .w(px(22.0))
                                .flex_none()
                                .pt_0p5()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.secondary)
                                .child(tier.map_or("–".to_string(), |t| format!("T{t}")))
                        }))
                        .child(badge_wrap(id, r, &items, grouped, now, theme))
                }),
        )
}

/// One wrapping row of badges. `grouped`: under a tier heading, so the
/// badges leave out the tier. `row` keeps element ids unique.
pub(crate) fn badge_wrap(
    id: &'static str,
    row: usize,
    items: &[Badge],
    grouped: bool,
    now: f64,
    theme: Theme,
) -> impl IntoElement {
    div().flex_1().flex().flex_wrap().gap_1().children(
        items
            .iter()
            .enumerate()
            .map(|(i, b)| pill(b, (id, row * 1000 + i), grouped, now, theme)),
    )
}

/// Badges of things finishing within this many seconds pulse (user request).
const PULSE_WITHIN: f64 = 10.0;

fn pill(b: &Badge, id: (&'static str, usize), grouped: bool, now: f64, theme: Theme) -> AnyElement {
    let pulse = b.ends_soon(now, PULSE_WITHIN);
    let tip: SharedString = b.tooltip(now).into();
    let color = match b.state {
        BadgeState::Action => status_color(HealthStatus::Alert),
        BadgeState::Processing => status_color(HealthStatus::Warning),
        BadgeState::Idle => rgb(0x8e8e93),
        BadgeState::Finished => status_color(HealthStatus::Healthy),
    };
    let mut tint = color;
    tint.a = 0.18;
    let pill = div()
        .flex()
        .items_center()
        .gap_1()
        .px_1p5()
        .rounded_md()
        .bg(tint)
        .border_1()
        .border_color(color)
        .text_xs()
        .text_color(theme.text)
        .child(dot(color, 6.0))
        .child(if grouped { b.short_text() } else { b.text() })
        .id(id.clone())
        .when(!tip.is_empty(), |d| {
            d.tooltip(move |_, cx| {
                cx.new(|_| Tooltip {
                    text: tip.clone(),
                    theme,
                })
                .into()
            })
        });
    if pulse {
        pill.with_animation(
            (SharedString::from(format!("pulse-{}", id.0)), id.1),
            Animation::new(Duration::from_millis(1400))
                .repeat()
                .with_easing(pulsating_between(0.35, 1.0)),
            |el, t| el.opacity(t),
        )
        .into_any_element()
    } else {
        pill.into_any_element()
    }
}

/// Hover text (badges, floating window food row).
pub(crate) struct Tooltip {
    /// One line per `\n`.
    pub text: SharedString,
    pub theme: Theme,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(self.theme.bg)
            .border_1()
            .border_color(self.theme.border)
            .shadow_md()
            .text_xs()
            .text_color(self.theme.text)
            .flex()
            .flex_col()
            .children(
                self.text
                    .split('\n')
                    .map(|line| div().child(line.to_string())),
            )
    }
}

pub(crate) fn section_title(title: &str) -> impl IntoElement {
    div()
        .font_weight(FontWeight::SEMIBOLD)
        .child(title.to_string())
}

pub(crate) fn divider(theme: Theme) -> impl IntoElement {
    div().h(px(1.0)).w_full().bg(theme.border)
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance());
        let state = self.state.read(cx);
        let m = &state.model;

        let header_color = if m.is_loading {
            status_color(HealthStatus::Warning)
        } else {
            status_color(m.monitor.overall())
        };
        let live = match m.relay_state {
            RelayState::Live => "Connected",
            RelayState::Off | RelayState::Connecting => "Connecting…",
            RelayState::Disconnected => "Disconnected",
            RelayState::Unavailable(_) => "Relay unavailable",
        };

        let mut body: Vec<AnyElement> = Vec::new();
        body.push(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(dot(header_color, 12.0))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(14.0))
                        .child("BitCraft Tracker"),
                )
                .child(div().flex_1())
                .child(div().text_xs().text_color(theme.secondary).child(live))
                .into_any_element(),
        );

        if m.is_loading {
            let name = m
                .settings
                .username
                .clone()
                .unwrap_or_else(|| "character".into());
            body.push(
                div()
                    .h(px(60.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.secondary)
                    .child(format!("Loading {name}…"))
                    .into_any_element(),
            );
        } else {
            body.push(self.reports(m, theme).into_any_element());
            body.push(divider(theme).into_any_element());
            body.push(self.stats(m, theme).into_any_element());
            if !m.plants.tiers.is_empty() {
                body.push(divider(theme).into_any_element());
                body.push(self.plants(m, theme).into_any_element());
            }
            if !m.passive.has_loaded {
                body.push(divider(theme).into_any_element());
                body.push(
                    div()
                        .flex()
                        .items_center()
                        .child(section_title("Passive crafts"))
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.secondary)
                                .child("Loading…"),
                        )
                        .into_any_element(),
                );
            } else if !m.passive.crafts.is_empty() {
                body.push(divider(theme).into_any_element());
                body.push(self.passive(m, theme).into_any_element());
            }
            if let Some(gather) = m.active_gather() {
                body.push(divider(theme).into_any_element());
                body.push(self.gathering(gather, m.now, theme).into_any_element());
            }
            let crafts = m.crafts.sorted();
            if !crafts.is_empty() {
                body.push(divider(theme).into_any_element());
                body.push(self.crafts(&crafts, m, theme).into_any_element());
            }
        }

        if let Some(release) = &state.update {
            body.push(divider(theme).into_any_element());
            body.push(update_row(release, theme).into_any_element());
        }
        body.push(divider(theme).into_any_element());
        let name = m.settings.username.clone().unwrap_or_else(|| "Unknown".into());
        body.push(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(theme.secondary)
                        .child(name),
                )
                // Character, floating window and sounds live in the main
                // window; this brings it up.
                .child({
                    let st = self.state.clone();
                    button("settings", "Settings", theme).on_click(move |_, window, cx: &mut App| {
                        crate::main_window::show(&st, cx);
                        st.update(cx, |s, _| s.popup_closed());
                        window.remove_window();
                    })
                })
                .child(button("quit", "Quit", theme).on_click(|_, _, cx: &mut App| cx.quit()))
                .into_any_element(),
        );

        let place = state.popup_place;
        let max_height = place.map_or(px(600.0), |p| p.max_height);
        let fit_state = self.state.clone();
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .flex()
            .flex_col()
            // Anchored at the bottom: keep the panel at the window's bottom.
            .when(
                matches!(place.map(|p| p.edge), Some(Edge::Bottom(_))),
                |d| d.justify_end(),
            )
            // Size the window to the panel once laid out (content changes,
            // e.g. opening the character search, refit it).
            .on_children_prepainted(move |bounds, window, cx| {
                let Some(panel) = bounds.first() else { return };
                fit_window(&fit_state, panel.size.height, window, cx);
            })
            .child(
                div()
                    .id("panel")
                    .w_full()
                    // Natural height (no shrinking to the current window),
                    // so growing content is measured and the window follows.
                    .flex_none()
                    .max_h(max_height)
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .bg(theme.bg)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_lg()
                    .text_size(px(13.0))
                    .text_color(theme.text)
                    .children(body),
            )
    }
}

/// Resize/move the popup window so it's exactly as tall as the panel.
fn fit_window(state: &Entity<AppState>, height: Pixels, window: &mut Window, cx: &mut App) {
    let (place, current) = {
        let s = state.read(cx);
        (
            s.popup_place,
            s.popup_height.unwrap_or(window.bounds().size.height),
        )
    };
    let Some(place) = place else { return };
    let height = height.min(place.max_height).ceil();

    if (height - current).abs() < px(1.0) {
        return;
    }
    state.update(cx, |s, _| s.popup_height = Some(height));
    crate::platform::set_window_frame(window, place.frame(height), cx);
}

pub(crate) fn button(id: &'static str, label: &str, theme: Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2()
        .py_0p5()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .cursor_pointer()
        .hover(move |s| s.bg(theme.hover))
        .child(label.to_string())
}

/// "Update available" row; Download opens the release page.
pub(crate) fn update_row(release: &crate::net::api::Release, theme: Theme) -> impl IntoElement {
        let url = release.html_url.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(dot(status_color(HealthStatus::Healthy), 8.0))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Update available"),
            )
            .child(
                div()
                    .text_color(theme.secondary)
                    .child(release.tag_name.clone()),
            )
            .child(div().flex_1())
            .child(
                button("update", "Download", theme)
                    .on_click(move |_, _, cx: &mut App| cx.open_url(&url)),
            )
}

impl Panel {
    fn reports(&self, m: &Model, theme: Theme) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .children(m.monitor.sorted_reports().into_iter().map(move |r| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(dot(status_color(r.status), 8.0))
                    .child(div().font_weight(FontWeight::MEDIUM).child(r.source))
                    .child(div().text_color(theme.secondary).child(r.message))
            }))
    }

    fn stats(&self, m: &Model, theme: Theme) -> impl IntoElement {
        let stamina_text = match m.stamina.stamina {
            Some(s) => format!(
                "{} / {}",
                s as i64,
                m.stamina.max.map_or("?".into(), |x| (x as i64).to_string())
            ),
            None => "–".into(),
        };
        let satiation_text = match m.satiation.satiation {
            Some(s) => format!("{} / {}", s as i64, m.satiation.max as i64),
            None => "–".into(),
        };
        let low = m.satiation.is_low();
        let satiation_color = status_color(if low {
            HealthStatus::Alert
        } else {
            HealthStatus::Healthy
        });
        let last_gain = match m.experience.last_gain_at {
            Some(t) => {
                let ago = duration_string(m.now - t);
                if m.experience.last_gain_amount > 0 {
                    format!("+{} {ago} ago", m.experience.last_gain_amount)
                } else {
                    format!("{ago} ago")
                }
            }
            None => "–".into(),
        };
        let row = move |label: &str, value: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .w(px(72.0))
                        .flex_none()
                        .text_color(theme.secondary)
                        .child(label.to_string()),
                )
                .child(div().flex_1().flex().items_center().gap_2().child(value))
        };
        let with_bar = |fraction: f64, color: Rgba, text: String, text_color: Rgba| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .gap_2()
                .child(bar(fraction, color, theme))
                .child(div().flex_none().text_color(text_color).child(text))
                .into_any_element()
        };
        // Food: bar of the regen buff's time left; buff summary below it.
        // Only known via the relay ("–" until then); orange under 5 min,
        // full red bar + "None" when out.
        let food_bar = match &m.food {
            None => div()
                .text_color(theme.secondary)
                .child("–")
                .into_any_element(),
            Some(food) => match food.regen_ends_at(m.now) {
                Some(end) => with_bar(
                    food.regen_fraction(m.now).unwrap_or(0.0),
                    rgb(food_rgb(end - m.now)),
                    duration_string(end - m.now),
                    theme.text,
                ),
                None => with_bar(
                    1.0,
                    status_color(HealthStatus::Alert),
                    "None".into(),
                    status_color(HealthStatus::Alert),
                ),
            },
        };
        let food_summary = m
            .food
            .as_ref()
            .map(|f| f.summary(m.now))
            .filter(|s| !s.is_empty());
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(row(
                "Stamina",
                with_bar(
                    m.stamina.fraction().unwrap_or(0.0),
                    rgb(stamina_rgb(m.stamina.fraction().unwrap_or(1.0))),
                    stamina_text,
                    theme.text,
                ),
            ))
            .child(row("Food", food_bar))
            .children(food_summary.map(|summary| {
                row(
                    "",
                    div()
                        .text_xs()
                        .text_color(theme.secondary)
                        .child(summary)
                        .into_any_element(),
                )
            }))
            .child(row(
                "Satiation",
                with_bar(
                    m.satiation.fraction().unwrap_or(0.0),
                    satiation_color,
                    satiation_text,
                    if low {
                        status_color(HealthStatus::Alert)
                    } else {
                        theme.text
                    },
                ),
            ))
            .child(row(
                "Total XP",
                div()
                    .child(m.experience.total.map_or("–".into(), group_thousands))
                    .into_any_element(),
            ))
            .child(row(
                "Last skill",
                div()
                    .child(m.experience.last_skill_name().unwrap_or_else(|| "–".into()))
                    .into_any_element(),
            ))
            .child(row("Last gain", div().child(last_gain).into_any_element()))
    }

    fn plants(&self, m: &Model, theme: Theme) -> impl IntoElement {
        let now = m.now;
        let time_left = move |tier: &TierSummary| match tier.all_grown_at() {
            None => "–".to_string(),
            Some(end) if end <= now => "any moment".into(),
            Some(end) => format!(
                "{}{} left",
                if tier.is_estimate { "~" } else { "" },
                duration_string(end - now)
            ),
        };
        let need_row = |label: &str, color: u32, count: fn(&TierSummary) -> usize| {
            let tiers: Vec<String> = m
                .plants
                .tiers
                .iter()
                .filter(|t| count(t) > 0)
                .map(|t| format!("T{} ×{}", t.tier, count(t)))
                .collect();
            (!tiers.is_empty()).then(|| {
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(dot(rgb(color), 7.0))
                    .child(label.to_string())
                    .child(div().flex_1())
                    .child(div().text_color(theme.secondary).child(tiers.join(", ")))
            })
        };
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(section_title("Plants"))
            .child(badges(
                "plant-badge",
                &badges::plants(&m.plants.plants),
                m.now,
                theme,
            ))
            .children(m.plants.growing_tiers().into_iter().map(move |tier| {
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("T{}", tier.tier)),
                    )
                    .child(
                        div()
                            .text_color(theme.secondary)
                            .child(format!("{} growing", tier.growing)),
                    )
                    .child(div().flex_1())
                    .child(time_left(&tier))
            }))
            .children(need_row("Needs fertilizer", 0xff9500, |t| {
                t.needs_fertilizer
            }))
            .children(need_row("Needs water", 0x0a84ff, |t| t.needs_water))
            .children(need_row("Ready to harvest", 0x34c759, |t| t.grown))
    }

    fn passive(&self, m: &Model, theme: Theme) -> impl IntoElement {
        let now = m.now;
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(section_title("Passive crafts"))
            .child(badges(
                "passive-badge",
                &badges::passive(&m.passive.crafts, now),
                now,
                theme,
            ))
            .children(m.passive.groups(now).into_iter().map(move |g| {
                let status = if g.running == 0 {
                    "Ready".to_string()
                } else {
                    match g.all_done_at.filter(|_| !g.has_unknown_end) {
                        None => "Running".into(),
                        Some(end) if end <= now => "any moment".into(),
                        Some(end) => format!("{} left", duration_string(end - now)),
                    }
                };
                let mut detail = vec![g.building.clone()];
                if g.running > 0 {
                    detail.push(format!("{} running", g.running));
                }
                if g.ready > 0 {
                    detail.push(format!("{} ready to collect", g.ready));
                }
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .child(div().font_weight(FontWeight::MEDIUM).child(g.name.clone()))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_color(if g.running == 0 {
                                        status_color(HealthStatus::Healthy)
                                    } else {
                                        theme.secondary
                                    })
                                    .child(status),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.secondary)
                            .child(detail.join(" · ")),
                    )
            }))
    }

    /// The resource being gathered, like a craft row: name, value (health
    /// %, or a fishing spot's time left), bar, and the details below.
    fn gathering(&self, g: &Gather, now: f64, theme: Theme) -> impl IntoElement {
        let (fraction, text) = g.bar(now).unwrap_or((0.0, String::new()));
        let mut details: Vec<String> = Vec::new();
        if let Some(skill) = g.skill.map(Skill::name_for) {
            details.push(skill);
        }
        if let Some(t) = g.tier {
            details.push(format!("T{t}"));
        }
        if let (Some(h), Some(max)) = (g.health, g.max_health) {
            details.push(format!(
                "{} / {}",
                group_thousands(h as i64),
                group_thousands(max as i64)
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(section_title("Gathering"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(div().font_weight(FontWeight::MEDIUM).child(g.name.clone()))
                            .child(div().flex_1())
                            .child(text),
                    )
                    .child(div().flex().child(bar(fraction, rgb(GATHER_RGB), theme)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.secondary)
                            .child(details.join(" · ")),
                    ),
            )
    }

    fn crafts(&self, crafts: &[Craft], m: &Model, theme: Theme) -> impl IntoElement {
        let now = m.now;
        let window = m.crafts.active_window;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(section_title("Crafts"))
            .child(badges(
                "craft-badge",
                &badges::crafts(crafts, now, window),
                now,
                theme,
            ))
            .children(crafts.iter().cloned().map(move |c| {
                let status = if c.is_complete() {
                    Some("Done")
                } else if c.last_progress_at.is_none_or(|t| now - t > window) {
                    Some("Paused")
                } else {
                    None
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(div().font_weight(FontWeight::MEDIUM).child(c.name.clone()))
                            .child(div().flex_1())
                            .children(status.map(|s| div().text_color(theme.secondary).child(s)))
                            .child(c.percent_string()),
                    )
                    .child(div().flex().child(bar(c.fraction(), rgb(0x0a84ff), theme)))
                    .child(div().text_xs().text_color(theme.secondary).child(format!(
                        "{} · {} / {}",
                        c.building.clone().unwrap_or_default(),
                        group_thousands(c.progress),
                        group_thousands(c.total)
                    )))
            }))
    }
}

pub(crate) fn group_thousands(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coarse_durations() {
        assert_eq!(duration_string(42.0), "42s");
        assert_eq!(duration_string(60.0), "1m");
        assert_eq!(duration_string(252.0), "5m");
        assert_eq!(duration_string(3599.0), "1h 00m");
        assert_eq!(duration_string(3900.0), "1h 05m");
    }
}
