//! Main window: a grid of every tracked character (user request: the
//! floating window's content plus the full crafts list and total XP, with
//! mute / floating window buttons), "Add character" search and the sound
//! settings. Replaced the invisible keepalive window.
//! Opened at launch; closing it quits the app (user request). The popup's
//! Settings button brings it to the front.

use gpui::{
    App, Bounds, Context, Entity, FocusHandle, FontWeight, KeyDownEvent, Render, SharedString, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowKind, WindowOptions, div, prelude::*, px, rgb, size,
};

use crate::core::status::HealthStatus;
use crate::sound::Sound;
use crate::settings::SoundSettings;
use crate::model::Model;
use crate::ui::{
    AppState, MAX_RESULTS_SHOWN, Theme, button, craft_rows, divider, dot, group_thousands, section_title, status_color,
    update_row,
};

/// Room for two cards side by side.
const WIDTH: f32 = 680.0;
const HEIGHT: f32 = 760.0;
const CARD_WIDTH: f32 = 300.0;

/// Bring the main window to the front (or open it if it failed to open
/// at launch). GPUI's `activate_window` defers the native call (and
/// restores a minimized window on Windows); calling AppKit's
/// `makeKeyAndOrderFront` directly from a click handler re-entered GPUI
/// and crashed.
pub fn show(state: &Entity<AppState>, cx: &mut App) {
    match state.read(cx).main {
        Some(handle) => {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
            cx.activate(true);
        }
        None => open(state, cx),
    }
}

pub fn open(state: &Entity<AppState>, cx: &mut App) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx))),
        titlebar: Some(TitlebarOptions { title: Some("BitCraft Tracker".into()), ..Default::default() }),
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_movable: true,
        is_resizable: true,
        is_minimizable: true,
        window_min_size: Some(size(px(CARD_WIDTH + 40.0), px(300.0))),
        app_id: Some("bitcraft-tracker".into()),
        ..Default::default()
    };
    let st = state.clone();
    match cx.open_window(options, move |window, cx| {
        // Closing the main window quits (user request). `quit` is
        // deferred by GPUI, so it's safe from inside this callback.
        window.on_window_should_close(cx, |_, cx| {
            cx.quit();
            true
        });
        cx.new(|cx| MainWindow::new(st, window, cx))
    }) {
        Ok(handle) => {
            state.update(cx, |s, _| s.main = Some(handle));
            // Bring the app to the front (macOS opens it behind the
            // active app otherwise).
            cx.activate(true);
        }
        Err(e) => eprintln!("failed to open main window: {e}"),
    }
}

pub struct MainWindow {
    state: Entity<AppState>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl MainWindow {
    fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        let subscriptions = vec![cx.observe(&state, |_, _, cx| cx.notify())];
        Self { state, focus, _subscriptions: subscriptions }
    }

    /// Character search: typed via key events (GPUI has no text input).
    /// Escape cancels, Enter picks the first result.
    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let state = self.state.clone();
        if !state.read(cx).picking || keystroke.modifiers.platform || keystroke.modifiers.control {
            return;
        }
        let mut query = state.read(cx).query.clone();
        match keystroke.key.as_str() {
            "escape" => {
                state.update(cx, |s, cx| {
                    s.picking = false;
                    cx.notify();
                });
                return;
            }
            "backspace" => {
                query.pop();
            }
            "enter" => {
                if let Some(first) = state.read(cx).results.first().cloned() {
                    state.update(cx, |s, cx| s.add_character(first, cx));
                }
                return;
            }
            _ => match &keystroke.key_char {
                Some(text) if !text.chars().any(char::is_control) => query.push_str(text),
                _ => return,
            },
        }
        state.update(cx, |s, cx| s.set_query(query, cx));
    }

    /// One character: the floating window's content (status line, bars,
    /// plants / passive / crafts badges) plus total XP and every craft's
    /// progress. Header buttons: mute sounds, floating window, remove.
    fn card(&self, i: usize, m: &Model, muted: bool, floating: bool, theme: Theme) -> impl IntoElement {
        let overall = AppState::display_status(m);
        let (st_mute, st_float, st_remove) = (self.state.clone(), self.state.clone(), self.state.clone());
        let (id_mute, id_float, id_remove) = (m.user_id.clone(), m.user_id.clone(), m.user_id.clone());
        let header = div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(dot(status_color(overall), 9.0))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(m.name()),
            )
            .child(toggle_button(("mute", i), if muted { "Muted" } else { "Mute" }, muted, theme).on_click(
                move |_, _, cx: &mut App| st_mute.update(cx, |s, cx| s.toggle_muted(&id_mute, cx)),
            ))
            .child(toggle_button(("float", i), "Floating", floating, theme).on_click(
                move |_, _, cx: &mut App| crate::floating::toggle(&st_float, &id_float, cx),
            ))
            .child(
                div()
                    .id(("remove", i))
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(theme.secondary)
                    .hover(move |s| s.bg(theme.hover))
                    .child("✕")
                    .tooltip(move |_, cx| {
                        cx.new(|_| crate::ui::Tooltip { text: "Stop tracking this character".into(), theme }).into()
                    })
                    .on_click(move |_, _, cx: &mut App| {
                        crate::floating::close(&st_remove, &id_remove, cx);
                        st_remove.update(cx, |s, cx| s.remove_character(&id_remove, cx));
                    }),
            );

        let mut body: Vec<gpui::AnyElement> = vec![header.into_any_element()];
        if m.is_loading {
            body.push(div().text_color(theme.secondary).child("Loading…").into_any_element());
        } else {
            body.push(crate::floating::status_line(m, theme).into_any_element());
            if !m.is_offline {
                body.push(crate::floating::bars(m, theme).into_any_element());
            }
            body.push(
                div()
                    .flex()
                    .gap_2()
                    .child(div().w(px(50.0)).flex_none().text_color(theme.secondary).child("Total XP"))
                    .child(m.experience.total.map_or("–".into(), group_thousands))
                    .into_any_element(),
            );
            body.push(divider(theme).into_any_element());
            body.push(crate::floating::summaries(m, theme).into_any_element());
            let crafts = m.crafts.sorted();
            if !crafts.is_empty() {
                body.push(divider(theme).into_any_element());
                body.push(craft_rows(&crafts, m.now, m.crafts.active_window, theme).into_any_element());
            }
        }
        // Stateful per card: keeps element ids (badges, tooltips) unique
        // between cards.
        div()
            .id(("card", i))
            .w(px(CARD_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .text_size(px(12.0))
            .children(body)
    }

    /// Sound settings: which events play a sound, volume slider, test.
    fn sounds(&self, prefs: &SoundSettings, theme: Theme) -> impl IntoElement {
        let state = self.state.clone();
        let toggle =
            |id: &'static str, label: &str, on: bool, set: fn(&mut SoundSettings, bool)| {
                let state = state.clone();
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .child(checkbox(on, theme))
                    .child(label.to_string())
                    .on_click(move |_, _, cx: &mut App| {
                        state.update(cx, |s, cx| {
                            set(&mut s.settings.sounds, !on);
                            s.settings.save();
                            cx.notify();
                        })
                    })
            };
        // Volume: 20 segments (5% steps); click or drag across them.
        const STEPS: usize = 20;
        let volume = prefs.volume;
        let filled = (volume * STEPS as f32).round() as usize;
        let slider = div()
            .flex_1()
            .flex()
            .h(px(16.0))
            .items_center()
            .children((1..=STEPS).map(|i| {
                let set_to = i as f32 / STEPS as f32;
                let (down, moved) = (state.clone(), state.clone());
                let set = move |state: &Entity<AppState>, cx: &mut App| {
                    state.update(cx, |s, cx| {
                        if (s.settings.sounds.volume - set_to).abs() > f32::EPSILON {
                            s.settings.sounds.volume = set_to;
                            s.settings.save();
                            cx.notify();
                        }
                    })
                };
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .w_full()
                            .h(px(6.0))
                            .when(i == 1, |d| d.rounded_l_full())
                            .when(i == STEPS, |d| d.rounded_r_full())
                            .bg(if i <= filled {
                                status_color(HealthStatus::Healthy)
                            } else {
                                theme.track
                            }),
                    )
                    .on_mouse_down(gpui::MouseButton::Left, move |_, _, cx| set(&down, cx))
                    .on_mouse_move(move |e: &gpui::MouseMoveEvent, _, cx| {
                        if e.pressed_button == Some(gpui::MouseButton::Left) {
                            set(&moved, cx);
                        }
                    })
            }));
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(section_title("Sounds"))
            .child(toggle(
                "snd-craft",
                "Craft finished",
                prefs.craft_finished,
                |p, v| p.craft_finished = v,
            ))
            .child(toggle(
                "snd-food",
                "Out of food",
                prefs.out_of_food,
                |p, v| p.out_of_food = v,
            ))
            .child(toggle(
                "snd-idle",
                "Idle with stamina above 90%",
                prefs.idle_full,
                |p, v| p.idle_full = v,
            ))
            .child(toggle(
                "snd-depleted",
                "Resource depleted",
                prefs.resource_depleted,
                |p, v| p.resource_depleted = v,
            ))
            .child(toggle(
                "snd-trade",
                "Trade request",
                prefs.trade_request,
                |p, v| p.trade_request = v,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(50.0))
                            .flex_none()
                            .text_color(theme.secondary)
                            .child("Volume"),
                    )
                    .child(slider)
                    .child(
                        div()
                            .w(px(34.0))
                            .flex_none()
                            .flex()
                            .justify_end()
                            .child(format!("{}%", (volume * 100.0).round() as i64)),
                    )
                    .child(
                        button("snd-test", "Test", theme).on_click(move |_, _, _| {
                            crate::sound::play(Sound::CraftFinished, volume)
                        }),
                    ),
            )
    }

    /// "Characters" title with an *Add character* button; while adding,
    /// the username search (typed via key events, see `on_key`).
    fn picker(&self, state: &AppState, theme: Theme, cx: &Context<Self>) -> impl IntoElement {
        if !state.picking {
            return div()
                .flex()
                .items_center()
                .gap_2()
                .child(section_title("Characters"))
                .child(div().flex_1())
                .child(button("add", "Add character", theme).on_click(cx.listener(|this, _, window, cx| {
                    this.focus.focus(window);
                    this.state.update(cx, |s, cx| {
                        s.picking = true;
                        s.query.clear();
                        s.results.clear();
                        cx.notify();
                    });
                })))
                .into_any_element();
        }

        let query: SharedString = if state.query.is_empty() {
            "Type a username…".into()
        } else {
            format!("{}▏", state.query).into()
        };
        let hint = if state.query.trim().chars().count() < 2 {
            Some("Type at least 2 characters".to_string())
        } else if let Some(e) = &state.search_error {
            Some(format!("Search failed: {e}"))
        } else if state.searching && state.results.is_empty() {
            Some("Searching…".into())
        } else if state.results.is_empty() {
            Some("No players found".into())
        } else if state.results.len() >= crate::net::api::SEARCH_LIMIT {
            Some(format!(
                "Showing the first {}; type more to narrow it down",
                crate::net::api::SEARCH_LIMIT
            ))
        } else {
            None
        };
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(status_color(HealthStatus::Warning))
                            .text_color(if state.query.is_empty() {
                                theme.secondary
                            } else {
                                theme.text
                            })
                            .child(query),
                    )
                    .child(button("cancel", "Cancel", theme).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.state.update(cx, |s, cx| {
                                s.picking = false;
                                cx.notify();
                            });
                        },
                    ))),
            )
            .children(hint.map(|h| div().text_xs().text_color(theme.secondary).child(h)))
            .child(
                div()
                    .id("results")
                    .flex()
                    .flex_col()
                    .max_h(px(22.0 * MAX_RESULTS_SHOWN as f32))
                    .overflow_y_scroll()
                    .children(
                        state
                            .results
                            .iter()
                            .cloned()
                            .enumerate()
                            .map(|(i, player)| {
                                // Already tracked: bold (picking it does nothing).
                                let selected = state.character(&player.entity_id).is_some();
                                let label = format!(
                                    "{}{}",
                                    player.username,
                                    if player.signed_in == Some(true) {
                                        "  •"
                                    } else {
                                        ""
                                    }
                                );
                                div()
                                    .id(("result", i))
                                    .h(px(22.0))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(theme.hover))
                                    .when(selected, |d| d.font_weight(FontWeight::SEMIBOLD))
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let player = player.clone();
                                        this.state.update(cx, |s, cx| s.add_character(player, cx));
                                    }))
                            }),
                    ),
            )
            .into_any_element()
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance());
        let state = self.state.read(cx);
        let cards: Vec<gpui::AnyElement> = state
            .characters
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let muted = state.settings.character(&m.user_id).is_some_and(|c| c.muted);
                let floating = state.floating.contains_key(&m.user_id);
                self.card(i, m, muted, floating, theme).into_any_element()
            })
            .collect();
        let empty = cards.is_empty();
        let mut body: Vec<gpui::AnyElement> = vec![
            self.picker(state, theme, cx).into_any_element(),
            div().flex().flex_wrap().gap_3().children(cards).into_any_element(),
        ];
        if empty {
            body.push(
                div().text_color(theme.secondary).child("No characters yet. Add one to start tracking.").into_any_element(),
            );
        }
        body.push(divider(theme).into_any_element());
        body.push(self.sounds(&state.settings.sounds, theme).into_any_element());
        if let Some(release) = &state.update {
            body.push(divider(theme).into_any_element());
            body.push(update_row(release, theme).into_any_element());
        }
        body.push(divider(theme).into_any_element());
        body.push(
            div()
                .flex()
                .child(div().flex_1())
                .child(button("quit", "Quit", theme).on_click(|_, _, cx: &mut App| cx.quit()))
                .into_any_element(),
        );

        div()
            .id("main")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .overflow_y_scroll()
            .bg(theme.bg)
            .text_size(px(13.0))
            .text_color(theme.text)
            .child(div().flex().flex_col().gap_3().p_4().children(body))
    }
}

/// A 14px checkbox (green when on).
fn checkbox(on: bool, theme: Theme) -> impl IntoElement {
    let green = status_color(HealthStatus::Healthy);
    div()
        .size(px(14.0))
        .flex_none()
        .rounded_sm()
        .border_1()
        .border_color(if on { green } else { theme.border })
        .when(on, |d| d.bg(green))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.0))
        .text_color(rgb(0xffffff))
        .child(if on { "✓" } else { "" })
}

/// A small header button that shows an on/off state (mute, floating).
fn toggle_button(id: (&'static str, usize), label: &'static str, on: bool, theme: Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_1p5()
        .rounded_md()
        .border_1()
        .border_color(if on { theme.text } else { theme.border })
        .text_xs()
        .when(on, |d| d.bg(theme.hover).font_weight(FontWeight::MEDIUM))
        .cursor_pointer()
        .hover(move |s| s.bg(theme.hover))
        .child(label)
}
