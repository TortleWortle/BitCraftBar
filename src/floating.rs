//! Floating window: a small always-on-top overview (stamina, health, food,
//! current activity/craft, crafts / passive crafts / plants), one per
//! character, toggled from its card in the main window. Movable by
//! dragging; position and open state are saved per character.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, ClickEvent, Context, DispatchPhase, Entity, FontWeight, MouseButton,
    MouseExitEvent, MouseMoveEvent, Pixels, Render,
    Rgba, SharedString, Subscription, Task, Window, WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowKind,
    WindowOptions, div, point, prelude::*, pulsating_between, px, size, svg,
};

use crate::core::status::{HealthStatus, compact_food_status, compact_health_status, food_rgb, health_rgb, stamina_rgb};
use crate::core::status::Report;
use crate::core::trackers::{ActivityCheck, Craft, XpDrop};
use crate::model::Model;
use crate::core::badges::{self, Badge, TierChip};
use crate::core::types::{rough_duration_string, short_duration_string};
use crate::ui::{AppState, PULSE_WITHIN, Theme, Tooltip, badge_wrap, bar, dot, duration_string, status_color};

const WIDTH: f32 = 260.0;
/// Compact layout: stamina and the current craft side by side.
const COMPACT_WIDTH: f32 = 300.0;
/// Craft bar color (floating window, main window card).
const CRAFT_RGB: u32 = 0x0a84ff;
/// Compact header XP drops: how long one floats up, how many show at once,
/// row height.
const XP_DROP_SECONDS: f64 = 2.4;
const XP_DROPS_SHOWN: usize = 2;
const XP_DROP_ROW: f32 = 16.0;

fn width(compact: bool) -> f32 {
    if compact { COMPACT_WIDTH } else { WIDTH }
}
const INITIAL_HEIGHT: f32 = 240.0;
const MARGIN: f32 = 16.0;

/// Open or close a character's floating window (remembered for the next
/// launch).
pub fn toggle(state: &Entity<AppState>, user_id: &str, cx: &mut App) {
    if let Some(handle) = state.read(cx).floating.get(user_id).copied() {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        set_open(state, user_id, false, cx);
        return;
    }
    open(state, user_id, cx);
}

/// Close a character's floating window without saving it as closed (the
/// character is being removed).
pub fn close(state: &Entity<AppState>, user_id: &str, cx: &mut App) {
    if let Some(handle) = state.read(cx).floating.get(user_id).copied() {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

fn set_open(state: &Entity<AppState>, user_id: &str, open: bool, cx: &mut App) {
    state.update(cx, |s, cx| {
        if !open {
            s.floating.remove(user_id);
            s.floating_size.remove(user_id);
        }
        if let Some(c) = s.settings.character_mut(user_id)
            && c.floating_window != open
        {
            c.floating_window = open;
            s.settings.save();
        }
        cx.notify();
    });
}

/// Switch a character's floating window between the normal and the
/// compact layout (saved); `fit` resizes it on the next frame.
fn set_compact(state: &Entity<AppState>, user_id: &str, compact: bool, cx: &mut App) {
    state.update(cx, |s, cx| {
        if let Some(c) = s.settings.character_mut(user_id)
            && c.floating_compact != compact
        {
            c.floating_compact = compact;
            s.settings.save();
        }
        cx.notify();
    });
}

pub fn open(state: &Entity<AppState>, user_id: &str, cx: &mut App) {
    let (saved, index, compact) = {
        let s = state.read(cx);
        let index = s.settings.characters.iter().position(|c| c.user_id == user_id).unwrap_or(0);
        let c = s.settings.character(user_id);
        (c.and_then(|c| c.floating_origin), index, c.is_some_and(|c| c.floating_compact))
    };
    let origin = match saved {
        Some((x, y)) if on_some_display(point(px(x), px(y)), cx) => point(px(x), px(y)),
        // Default: top-right corner of the main display, below the menu
        // bar; one column further left per character so they don't stack.
        _ => match cx.primary_display().map(|d| d.bounds()) {
            Some(screen) => point(screen.right() - px((WIDTH + MARGIN) * (index as f32 + 1.0)), screen.top() + px(40.0)),
            None => point(px(100.0), px(100.0)),
        },
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(crate::platform::open_bounds(Bounds {
            origin,
            size: size(px(width(compact)), px(INITIAL_HEIGHT)),
        }))),
        titlebar: None,
        // Never take focus from the game.
        focus: false,
        show: true,
        // macOS: a non-activating panel at popup level, on all Spaces and
        // next to fullscreen apps. Windows: made topmost below.
        kind: WindowKind::PopUp,
        is_movable: true,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("bitcraft-tracker".into()),
        ..Default::default()
    };
    let st = state.clone();
    let id = user_id.to_string();
    match cx.open_window(options, move |window, cx| {
        crate::platform::make_overlay(window);
        cx.new(|cx| FloatingWindow::new(st, id, window, cx))
    }) {
        Ok(handle) => {
            state.update(cx, |s, _| {
                s.floating.insert(user_id.to_string(), handle);
            });
            set_open(state, user_id, true, cx);
        }
        Err(e) => eprintln!("failed to open floating window: {e}"),
    }
}

/// A saved position on a display that's gone (unplugged monitor) would put
/// the window off screen.
fn on_some_display(p: gpui::Point<Pixels>, cx: &App) -> bool {
    cx.displays().iter().any(|d| d.bounds().contains(&p))
}

pub struct FloatingWindow {
    state: Entity<AppState>,
    /// The character it shows.
    user_id: String,
    save_task: Option<Task<()>>,
    /// Mouse over the card: the compact layout shows its buttons.
    hovered: bool,
    _subscriptions: Vec<Subscription>,
}

impl FloatingWindow {
    fn new(state: Entity<AppState>, user_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            // Remember where the user moved it (debounced: a drag sends
            // many updates).
            cx.observe_window_bounds(window, |this, window, cx| {
                let origin = window.bounds().origin;
                let origin = (f32::from(origin.x), f32::from(origin.y));
                let saved = this.state.read(cx).settings.character(&this.user_id).and_then(|c| c.floating_origin);
                if saved == Some(origin) {
                    return;
                }
                let state = this.state.clone();
                let user_id = this.user_id.clone();
                this.save_task = Some(cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    let _ = state.update(cx, |s, _| {
                        if let Some(c) = s.settings.character_mut(&user_id) {
                            c.floating_origin = Some(origin);
                            s.settings.save();
                        }
                    });
                }));
            }),
        ];
        Self { state, user_id, save_task: None, hovered: false, _subscriptions: subscriptions }
    }
}

/// Resize the window to the card's height and the layout's width,
/// keeping its top-left corner.
fn fit(state: &Entity<AppState>, user_id: &str, width: Pixels, height: Pixels, window: &mut Window, cx: &mut App) {
    let wanted = size(width, height.ceil());
    let current = state.read(cx).floating_size.get(user_id).copied().unwrap_or(window.bounds().size);
    if (wanted.height - current.height).abs() < px(1.0) && (wanted.width - current.width).abs() < px(1.0) {
        return;
    }
    state.update(cx, |s, _| {
        s.floating_size.insert(user_id.to_string(), wanted);
    });
    let frame = Bounds { origin: window.bounds().origin, size: wanted };
    crate::platform::set_window_frame(window, frame, cx);
}

/// Small icon button (compact toggle). Doesn't start a window drag.
fn icon_button(
    id: &'static str,
    icon: &'static str,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .p_1()
        .rounded_sm()
        .cursor_pointer()
        .hover(move |s| s.bg(theme.hover))
        .child(svg().path(icon).size(px(11.0)).text_color(theme.secondary))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
}

impl Render for FloatingWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut theme = Theme::for_appearance(window.appearance());
        theme.bg.a = 0.94;
        let state = self.state.read(cx);
        // Removed character: the window is closing.
        let Some(m) = state.character(&self.user_id) else { return div().into_any_element() };
        let name = m.name();
        let overall = if m.is_loading { HealthStatus::Warning } else { m.monitor.overall() };
        let compact = state.settings.character(&self.user_id).is_some_and(|c| c.floating_compact);

        let toggle_state = self.state.clone();
        let toggle_id = self.user_id.clone();
        let toggle = icon_button(
            "compact",
            if compact { "icons/expand.svg" } else { "icons/compact.svg" },
            theme,
            move |_, _, cx| set_compact(&toggle_state, &toggle_id, !compact, cx),
        );
        let close_state = self.state.clone();
        let close_id = self.user_id.clone();
        let close = div()
            .id("close")
            .px_1()
            .rounded_sm()
            .cursor_pointer()
            .text_color(theme.secondary)
            .hover(move |s| s.bg(theme.hover))
            .child("✕")
            // Don't start a window drag from the close button.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // Close through the handler's own window: `toggle` goes
            // through the window handle, which can't be updated
            // while this window is already being updated (the
            // close failed silently, the state said closed, and the
            // next click opened a second window).
            .on_click(move |_, window, cx| {
                window.remove_window();
                set_open(&close_state, &close_id, false, cx);
            });

        let body: Vec<AnyElement> = if compact {
            // Buttons only while the mouse is over the card (user wasn't
            // sure where they'd go; this keeps the compact card clean).
            let buttons = self.hovered.then(|| div().flex().items_center().child(toggle).child(close));
            compact_body(m, overall, name, buttons, theme)
        } else {
            let header = div()
                .flex()
                .items_center()
                .gap_2()
                // Windows drags via the hit test (HTCAPTION) on the header.
                .window_control_area(WindowControlArea::Drag)
                .child(dot(status_color(overall), 9.0))
                .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_weight(FontWeight::SEMIBOLD).child(name))
                .child(div().flex_1())
                // XP drops here too (user request), same as compact.
                .when(!m.is_loading && !m.is_offline, |d| d.child(xp_drops(m, theme)))
                .child(toggle)
                .child(close);
            normal_body(m, header, theme)
        };

        let fit_state = self.state.clone();
        let fit_id = self.user_id.clone();
        let fit_width = px(width(compact));
        let this = cx.entity().downgrade();
        // Hover tracking for the compact buttons. Not `on_hover`: GPUI
        // clears that only on a mouse move, and leaving the window sends
        // an exit event instead, so the buttons would stay visible.
        let hover_tracker = gpui::canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let set = |this: &gpui::WeakEntity<FloatingWindow>, hovered: bool, cx: &mut App| {
                    let _ = this.update(cx, |this, cx| {
                        if this.hovered != hovered {
                            this.hovered = hovered;
                            cx.notify();
                        }
                    });
                };
                let moved = this.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        let inside = Bounds { origin: point(px(0.0), px(0.0)), size: window.bounds().size }.contains(&e.position);
                        set(&moved, inside, cx);
                    }
                });
                let exited = this.clone();
                window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        set(&exited, false, cx);
                    }
                });
            },
        )
        .absolute()
        .size_0();
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_children_prepainted(move |bounds, window, cx| {
                if let Some(card) = bounds.first() {
                    fit(&fit_state, &fit_id, fit_width, card.size.height, window, cx);
                }
            })
            .child(
                div()
                    .w_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .when(compact, |d| d.gap_1p5().p_2p5())
                    .when(!compact, |d| d.gap_2().p_3())
                    .bg(theme.bg)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_lg()
                    .text_size(px(12.0))
                    .text_color(theme.text)
                    .cursor_move()
                    // Drag anywhere to move (macOS / Linux).
                    .on_mouse_down(MouseButton::Left, |_, window, _| crate::platform::start_window_drag(window))
                    .children(body),
            )
            .child(hover_tracker)
            .into_any_element()
    }
}

/// The normal layout: status line, bars, badge rows.
fn normal_body(m: &Model, header: impl IntoElement, theme: Theme) -> Vec<AnyElement> {
    let mut body: Vec<AnyElement> = vec![header.into_any_element()];
    {
        if m.is_loading {
            body.push(div().text_color(theme.secondary).child("Loading…").into_any_element());
        } else {
            body.push(status_line(m, theme).into_any_element());
            // Offline: stamina/health/food are stale and don't matter.
            if !m.is_offline {
                body.push(bars(m, theme).into_any_element());
            }
            body.push(div().h(px(1.0)).w_full().bg(theme.border).into_any_element());
            body.push(summaries(m, theme).into_any_element());
        }
    }
    body
}

/// What the character is doing: the worst issue (or the activity report)
/// and the current craft with its progress.
/// The worst issue, or the Activity report when all is well.
fn shown_report(m: &Model) -> Option<Report> {
    let reports = m.monitor.sorted_reports();
    reports
        .iter()
        .find(|r| r.status != HealthStatus::Healthy)
        .or_else(|| reports.iter().find(|r| r.source == ActivityCheck::SOURCE))
        .cloned()
}

/// What the player is working on: the gather target while it's the latest
/// thing they did, else the current (or last worked on) craft. Nothing
/// while offline.
enum Work {
    Craft(Craft),
    /// name, (fraction, "39%" / time left), "1,155 / 3,000" for health.
    Gather(String, (f64, String), Option<String>),
}

fn current_work(m: &Model) -> Option<Work> {
    if let Some(g) = m.active_gather()
        && let Some(bar) = g.bar(m.now)
    {
        let inside = match (g.shows_timer(), g.health, g.max_health) {
            (false, Some(h), Some(max)) => {
                Some(format!("{} / {}", crate::ui::group_thousands(h as i64), crate::ui::group_thousands(max as i64)))
            }
            _ => None,
        };
        return Some(Work::Gather(g.name.clone(), bar, inside));
    }
    if m.is_offline {
        return None;
    }
    m.active_craft().or_else(|| m.crafts.last_worked.clone()).map(Work::Craft)
}

pub(crate) fn status_line(m: &Model, theme: Theme) -> impl IntoElement {
    let now = m.now;
    let shown = shown_report(m);
    let skill = (!m.is_offline).then(|| m.experience.last_skill_name()).flatten();
    let (craft, gather) = match current_work(m) {
        Some(Work::Craft(c)) => (Some(c), None),
        Some(Work::Gather(name, bar, inside)) => (None, Some((name, bar, inside))),
        None => (None, None),
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .children(shown.map(|r| {
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(dot(status_color(r.status), 7.0))
                .child(div().font_weight(FontWeight::MEDIUM).child(r.message))
                .child(div().flex_1())
                .children(skill.map(|s| div().text_color(theme.secondary).child(s)))
        }))
        .children(craft.map(|c| {
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().flex_1().child(c.name.clone()))
                        // Rough time left (user request): "65% · 12m".
                        .child(div().flex_none().child(match c.eta_left(now) {
                            Some(left) => format!("{} · {}", c.percent_string(), rough_duration_string(left)),
                            None => c.percent_string(),
                        })),
                )
                .child(div().flex().child(bar(c.fraction(), gpui::rgb(CRAFT_RGB), theme)))
        }))
        .children(gather.map(|(name, (fraction, text), inside)| {
            // Value inside the bar like stamina/health/food (user request):
            // "health / max", or a fishing spot's time left; the percent
            // stays next to the name.
            let (right, inside) = match inside {
                Some(inside) => (Some(text), inside),
                None => (None, text),
            };
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().flex_1().child(name))
                        .children(right.map(|r| div().flex_none().child(r))),
                )
                .child(div().flex().child(labeled_bar(fraction, gpui::rgb(crate::ui::GATHER_RGB), inside, theme.text, theme)))
        }))
}

/// A taller bar with its value ("212 / 262", "41m") centered inside
/// (user request). The fill is slightly translucent so the text stays
/// readable on both the fill and the empty track, in light and dark mode.
fn labeled_bar(fraction: f64, color: Rgba, text: String, text_color: Rgba, theme: Theme) -> impl IntoElement {
    let mut fill = color;
    fill.a = 0.7;
    div()
        .flex_1()
        .relative()
        .h(px(16.0))
        .rounded_md()
        .overflow_hidden()
        .bg(theme.track)
        .child(div().absolute().top_0().left_0().h_full().bg(fill).w(gpui::relative(fraction.clamp(0.0, 1.0) as f32)))
        .child(
            div()
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(text_color)
                .child(text),
        )
}

/// Hover text: the active food buffs, one per line with time left (user
/// request: instead of a summary line under the bar).
fn food_tooltip(m: &Model) -> Option<SharedString> {
    m.food
        .as_ref()
        .map(|f| {
            let mut buffs: Vec<_> = f.active(m.now).collect();
            buffs.sort_by_key(|b| (!b.is_regen, b.name.clone()));
            buffs.iter().map(|b| format!("{} · {}", b.short_name(), duration_string(b.ends_at - m.now))).collect::<Vec<_>>().join("\n")
        })
        .filter(|t| !t.is_empty())
        .map(SharedString::from)
}

fn tooltip(text: impl Into<SharedString>, theme: Theme) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tooltip { text: text.clone(), theme }).into()
}

pub(crate) fn bars(m: &Model, theme: Theme) -> impl IntoElement {
    let row = move |label: &str, fraction: f64, color: Rgba, text: String, text_color: Rgba| {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().w(px(50.0)).flex_none().text_color(theme.secondary).child(label.to_string()))
            .child(labeled_bar(fraction, color, text, text_color, theme))
    };
    let fmt = |current: Option<f64>, max: Option<f64>| match current {
        Some(c) => format!("{} / {}", c as i64, max.map_or("?".into(), |x| (x as i64).to_string())),
        None => "–".into(),
    };
    let healthy = status_color(HealthStatus::Healthy);
    let alert = status_color(HealthStatus::Alert);
    let stamina_color = gpui::rgb(stamina_rgb(m.stamina.fraction().unwrap_or(1.0)));
    let health_fraction = match (m.health, m.max_health) {
        (Some(h), Some(max)) if max > 0.0 => h / max,
        _ => 0.0,
    };
    let health_color = gpui::rgb(health_rgb(health_fraction));
    let (food_fraction, food_color, food_text, food_text_color) = match &m.food {
        None => (0.0, healthy, "–".to_string(), theme.secondary),
        Some(food) => match food.regen_ends_at(m.now) {
            Some(end) => (food.regen_fraction(m.now).unwrap_or(0.0), gpui::rgb(food_rgb(end - m.now)), duration_string(end - m.now), theme.text),
            None => (1.0, alert, "None".into(), alert),
        },
    };
    let food_tip = food_tooltip(m);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(row("Stamina", m.stamina.fraction().unwrap_or(0.0), stamina_color, fmt(m.stamina.stamina, m.stamina.max), theme.text))
        .child(row("Health", health_fraction, health_color, fmt(m.health, m.max_health), theme.text))
        .child(
            row("Food", food_fraction, food_color, food_text, food_text_color)
                .id("food")
                .when_some(food_tip, |d, tip| d.tooltip(move |_, cx| cx.new(|_| Tooltip { text: tip.clone(), theme }).into())),
        )
}

/// One row each for plants, passive crafts and crafts: "type + tier"
/// badges (red = needs action, orange = running, gray = idle, green =
/// done), plus when the next one finishes.
pub(crate) fn summaries(m: &Model, theme: Theme) -> impl IntoElement {
    let now = m.now;
    // Label column on the left; with several tiers, the tier headings go
    // in that column under the label (user request: keeps the badges
    // aligned with each other).
    let row = move |label: &'static str, list: Vec<Badge>, note: Option<String>| {
        let label_cell = move |text: String, indent: bool| {
            div()
                .w(px(50.0))
                .flex_none()
                // Tier headings: left-aligned with the label above.
                .when(indent, |d| d.text_xs().font_weight(FontWeight::SEMIBOLD))
                .text_color(theme.secondary)
                .child(text)
        };
        let note_el = note.map(|n| div().text_xs().text_color(theme.secondary).child(n));
        let rows = badges::by_tier(&list);
        let grouped = rows.first().is_some_and(|(heading, _)| heading.is_some());
        // Tiers: a bit more space than the 4px between wrapped badge lines.
        let mut out = div().flex().flex_col().gap_2();
        if list.is_empty() {
            out = out.child(
                div()
                    .flex()
                    .gap_2()
                    .child(label_cell(label.to_string(), false))
                    .child(div().flex_1().flex().flex_col().gap_0p5().child(div().text_color(theme.secondary).child("None")).children(note_el)),
            );
        } else if !grouped {
            out = out.child(
                div()
                    .flex()
                    .gap_2()
                    .child(label_cell(label.to_string(), false))
                    .child(div().flex_1().flex().flex_col().gap_0p5().child(badge_wrap(label, 0, &list, false, now, theme)).children(note_el)),
            );
        } else {
            out = out.child(
                div()
                    .flex()
                    .gap_2()
                    .child(label_cell(label.to_string(), false))
                    .child(div().flex_1().children(note_el)),
            );
            for (r, (heading, items)) in rows.into_iter().enumerate() {
                let text = heading.flatten().map_or("–".to_string(), |t| format!("T{t}"));
                out = out.child(
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(label_cell(text, true).pt_0p5())
                        .child(badge_wrap(label, r, &items, true, now, theme)),
                );
            }
        }
        out
    };

    // Plants: time until the first growing tier is fully grown.
    let plants_note = m
        .plants
        .growing_tiers()
        .iter()
        .filter_map(|t| t.all_grown_at().map(|end| (end, t.is_estimate)))
        .filter(|(end, _)| *end > now)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(end, estimate)| format!("next grown in {}{}", if estimate { "~" } else { "" }, duration_string(end - now)));

    // Passive crafts: time until the next one finishes.
    let passive_note = m
        .passive
        .crafts
        .iter()
        .filter(|c| !c.is_finished(now))
        .filter_map(|c| c.ends_at())
        .reduce(f64::min)
        .map(|end| format!("next done in {}", duration_string(end - now)));

    let divider = move || div().h(px(1.0)).w_full().bg(theme.border);
    let passive = if m.passive.has_loaded { badges::passive(&m.passive.crafts, now) } else { vec![] };
    div()
        .flex()
        .flex_col()
        // Plants / Passive / Crafts separated by dividers (8px either side).
        .gap_2()
        .child(row("Plants", badges::plants(&m.plants.plants, m.now), plants_note))
        .child(divider())
        .child(row(
            "Passive",
            passive,
            if m.passive.has_loaded { passive_note } else { Some("Loading…".into()) },
        ))
        .child(divider())
        .child(row("Crafts", badges::crafts(&m.crafts.sorted(), now, m.crafts.active_window), None))
}

// MARK: - Compact layout

/// The compact layout (user sketch): header with the last XP drop (skill
/// icon), health and food icons; stamina and the current work side by
/// side; tiny per-tier chips for plants, crafts and passive crafts.
fn compact_body(m: &Model, overall: HealthStatus, name: String, buttons: Option<impl IntoElement>, theme: Theme) -> Vec<AnyElement> {
    let shown = shown_report(m);
    // Hover on the status dot or the name (user request): every issue (or
    // the activity).
    let issues: Vec<String> = m
        .monitor
        .sorted_reports()
        .iter()
        .filter(|r| r.status != HealthStatus::Healthy)
        .map(|r| r.message.clone())
        .collect();
    let dot_tip = if issues.is_empty() { shown.as_ref().map(|r| r.message.clone()).unwrap_or_default() } else { issues.join("\n") };

    let live = !m.is_loading && !m.is_offline;
    let header = div()
        .flex()
        .items_center()
        .gap_2()
        // Windows drags via the hit test (HTCAPTION) on the header.
        .window_control_area(WindowControlArea::Drag)
        .child(
            div().flex_1().min_w_0().flex().child(
                div()
                    .id("status")
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(dot(status_color(overall), 10.0))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .when(!dot_tip.is_empty(), |d| d.tooltip(tooltip(dot_tip, theme))),
            ),
        )
        .children(buttons)
        .when(live, |d| d.child(xp_drops(m, theme)).child(health_icon(m, theme)).child(food_icon(m, theme)));

    let mut body: Vec<AnyElement> = vec![header.into_any_element()];
    if m.is_loading {
        body.push(div().text_color(theme.secondary).child("Loading…").into_any_element());
        return body;
    }
    if m.is_offline {
        // Stamina and the current work are stale; the dot is red.
        body.extend(shown.map(|r| message_row(r, theme).into_any_element()));
    } else {
        body.push(compact_bars(m, shown, theme).into_any_element());
    }
    body.push(div().h(px(1.0)).w_full().bg(theme.border).into_any_element());
    body.push(compact_lists(m, theme).into_any_element());
    body
}

fn message_row(r: Report, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .min_w_0()
        .child(dot(status_color(r.status), 7.0))
        .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(theme.text).child(r.message))
}

/// XP gains like the game shows them (user request): skill icon + "32
/// EXP", rising and fading out, at most `XP_DROPS_SHOWN` at once (the game
/// shows 3; 2 fit the compact header). The area keeps its size so the
/// header doesn't jump.
fn xp_drops(m: &Model, theme: Theme) -> impl IntoElement {
    let drops: Vec<XpDrop> = m
        .experience
        .drops
        .iter()
        .rev()
        .filter(|d| m.now - d.at < XP_DROP_SECONDS)
        .take(XP_DROPS_SHOWN)
        .cloned()
        .collect();
    div().relative().w(px(84.0)).h(px(32.0)).flex_none().overflow_hidden().children(drops.into_iter().map(move |d| {
        let row = div()
            .absolute()
            .right_0()
            .h(px(XP_DROP_ROW))
            .flex()
            .items_center()
            .gap_1()
            .children(crate::assets::skill_icon(d.skill_id).map(|icon| svg().path(icon).size(px(13.0)).text_color(theme.text)))
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text)
                    .child(format!("{} EXP", crate::ui::group_thousands(d.amount))),
            );
        // Starts at the bottom of the area and rises past its top; fades
        // in quickly and out over the last 40%. The animation runs once per
        // drop id; the drop is dropped from the list once it's over.
        row.with_animation(
            ("xp-drop", d.id as usize),
            Animation::new(Duration::from_secs_f64(XP_DROP_SECONDS)),
            |row, t| {
                let opacity = if t < 0.1 { t / 0.1 } else if t > 0.6 { (1.0 - t) / 0.4 } else { 1.0 };
                row.top(px(32.0 - XP_DROP_ROW - 22.0 * t)).opacity(opacity)
            },
        )
    }))
}

/// Icon colored by state, with hover text.
fn state_icon(id: &'static str, icon: &'static str, color: Rgba, tip: String, theme: Theme) -> impl IntoElement {
    div()
        .id(id)
        .h(px(22.0))
        .flex()
        .items_center()
        .child(svg().path(icon).size(px(17.0)).text_color(color))
        .tooltip(tooltip(tip, theme))
}

/// Heart (user request): red under 20% health, orange under 70%, else
/// green; gray until health is known.
fn health_icon(m: &Model, theme: Theme) -> impl IntoElement {
    let (color, tip) = match (m.health, m.max_health) {
        (Some(h), Some(max)) if max > 0.0 => {
            (status_color(compact_health_status(h / max)), format!("Health {} / {}", h as i64, max as i64))
        }
        _ => (theme.secondary, "Health unknown".to_string()),
    };
    state_icon("health", "icons/heart.svg", color, tip, theme)
}

/// Food (user request): red without a food regen buff, orange in its last
/// 15 seconds, else green; gray until the relay delivered buffs.
fn food_icon(m: &Model, theme: Theme) -> impl IntoElement {
    let (color, tip) = match &m.food {
        None => (theme.secondary, "Food unknown".to_string()),
        Some(food) => {
            let left = food.regen_ends_at(m.now).map(|end| end - m.now);
            let tip = match left {
                None => "No food".to_string(),
                Some(_) => food_tooltip(m).map(|t| t.to_string()).unwrap_or_default(),
            };
            (status_color(compact_food_status(left)), tip)
        }
    };
    state_icon("food-icon", "icons/food.svg", color, tip, theme)
}

/// Stamina and the current work side by side; the status message when
/// there's no work.
fn compact_bars(m: &Model, shown: Option<Report>, theme: Theme) -> impl IntoElement {
    let cell = move |left: String, left_color: Rgba, right: Option<String>, bar: AnyElement| {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(
                div()
                    .flex()
                    .gap_1()
                    .text_xs()
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(left_color).child(left))
                    .children(right.map(|r| div().flex_none().child(r))),
            )
            .child(div().flex().child(bar))
    };
    let stamina = cell(
        "Stamina".into(),
        theme.secondary,
        None,
        labeled_bar(
            m.stamina.fraction().unwrap_or(0.0),
            gpui::rgb(stamina_rgb(m.stamina.fraction().unwrap_or(1.0))),
            match m.stamina.stamina {
                Some(c) => format!("{} / {}", c as i64, m.stamina.max.map_or("?".into(), |x| (x as i64).to_string())),
                None => "–".into(),
            },
            theme.text,
            theme,
        )
        .into_any_element(),
    );
    let work = match current_work(m) {
        Some(Work::Craft(c)) => {
            // Rough time left inside the bar (user request).
            let left = c.eta_left(m.now).map(|s| format!("{} left", rough_duration_string(s))).unwrap_or_default();
            let bar = labeled_bar(c.fraction(), gpui::rgb(CRAFT_RGB), left, theme.text, theme).into_any_element();
            cell(c.name.clone(), theme.text, Some(c.percent_string()), bar).into_any_element()
        }
        Some(Work::Gather(name, (fraction, text), inside)) => {
            // Same as the normal layout: "health / max" inside, the
            // percent next to the name; a fishing timer inside.
            let (right, inside) = match inside {
                Some(inside) => (Some(text), inside),
                None => (None, text),
            };
            let bar = labeled_bar(fraction, gpui::rgb(crate::ui::GATHER_RGB), inside, theme.text, theme).into_any_element();
            cell(name, theme.text, right, bar).into_any_element()
        }
        None => div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_end()
            .children(shown.map(|r| message_row(r, theme)))
            .into_any_element(),
    };
    div().flex().gap_2().child(stamina).child(work)
}

/// Plants | Crafts | Passive columns of per-tier chips.
fn compact_lists(m: &Model, theme: Theme) -> impl IntoElement {
    let now = m.now;
    let column = move |id: &'static str, label: &'static str, note: Option<String>, chips: Option<Vec<TierChip>>| {
        let content = match chips {
            None => div().text_xs().text_color(theme.secondary).child("…").into_any_element(),
            Some(chips) if chips.is_empty() => div().text_xs().text_color(theme.secondary).child("–").into_any_element(),
            Some(chips) => div()
                .flex()
                .flex_wrap()
                .gap_1()
                .children(chips.iter().enumerate().map(|(i, c)| chip(c, (id, i), now, theme)))
                .into_any_element(),
        };
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap_1()
                    .text_xs()
                    .text_color(theme.secondary)
                    .child(label)
                    .children(note.map(|n| div().font_weight(FontWeight::MEDIUM).text_color(theme.text).child(n))),
            )
            .child(content)
    };
    let divider = move || div().w(px(1.0)).flex_none().bg(theme.border);
    let passive = m.passive.has_loaded.then(|| badges::tier_chips(&badges::passive(&m.passive.crafts, now)));
    let plants = badges::plants(&m.plants.plants, now);
    // Time until the next plant is grown (user request), one unit: "5m".
    let plants_note = badges::next_end(&plants, now)
        .map(|(end, estimate)| format!("{}{}", if estimate { "~" } else { "" }, short_duration_string(end - now)));
    div()
        .flex()
        .gap_2()
        .child(column("chip-plants", "Plants", plants_note, Some(badges::tier_chips(&plants))))
        .child(divider())
        .child(column(
            "chip-crafts",
            "Crafts",
            None,
            Some(badges::tier_chips(&badges::crafts(&m.crafts.sorted(), now, m.crafts.active_window))),
        ))
        .child(divider())
        .child(column("chip-passive", "Passive", None, passive))
}

/// "T1" in the state's color (badge colors), pulsing like the badges
/// when something in it finishes soon.
fn chip(c: &TierChip, id: (&'static str, usize), now: f64, theme: Theme) -> AnyElement {
    let color = crate::ui::badge_color(c.state);
    let mut tint = color;
    tint.a = 0.25;
    // Tiny progress bar along the bottom edge (user request): crafts'
    // progress, plants' / passive crafts' time elapsed.
    let progress = c.progress();
    let el = div()
        .id(id)
        .relative()
        .overflow_hidden()
        .px_1p5()
        .rounded_md()
        .bg(tint)
        .border_1()
        .border_color(color)
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text)
        .child(c.text())
        .children(progress.map(|f| {
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .h(px(2.0))
                .w(gpui::relative(f.clamp(0.0, 1.0) as f32))
                .bg(color)
        }))
        .tooltip(tooltip(c.tooltip(now), theme));
    if c.ends_soon(now, PULSE_WITHIN) {
        el.with_animation(
            (SharedString::from(format!("pulse-{}", id.0)), id.1),
            Animation::new(Duration::from_millis(1400)).repeat().with_easing(pulsating_between(0.35, 1.0)),
            |el, t| el.opacity(t),
        )
        .into_any_element()
    } else {
        el.into_any_element()
    }
}
