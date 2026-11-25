//! Info window: a small always-on-top overview (stamina, health, food,
//! current activity/craft, crafts / passive crafts / plants), toggled from
//! the popup. Movable by dragging; position and open state are saved.

use std::time::Duration;

use gpui::{
    AnyElement, App, Bounds, Context, Entity, FontWeight, MouseButton, Pixels, Render, Rgba, SharedString, Subscription, Task,
    Window, WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowKind, WindowOptions, div, point,
    prelude::*, px, size,
};

use crate::core::status::HealthStatus;
use crate::core::trackers::ActivityCheck;
use crate::model::Model;
use crate::core::badges::{self, Badge};
use crate::ui::{AppState, Theme, Tooltip, badge_wrap, bar, dot, duration_string, status_color};

const WIDTH: f32 = 260.0;
const INITIAL_HEIGHT: f32 = 240.0;
const MARGIN: f32 = 16.0;

/// Open or close the info window (remembered for the next launch).
pub fn toggle(state: &Entity<AppState>, cx: &mut App) {
    if let Some(handle) = state.read(cx).info {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        set_open(state, false, cx);
        return;
    }
    open(state, cx);
}

fn set_open(state: &Entity<AppState>, open: bool, cx: &mut App) {
    state.update(cx, |s, cx| {
        if !open {
            s.info = None;
            s.info_height = None;
        }
        if s.model.settings.info_window != open {
            s.model.settings.info_window = open;
            s.model.settings.save();
        }
        cx.notify();
    });
}

pub fn open(state: &Entity<AppState>, cx: &mut App) {
    let origin = match state.read(cx).model.settings.info_origin {
        Some((x, y)) if on_some_display(point(px(x), px(y)), cx) => point(px(x), px(y)),
        // Default: top-right corner of the main display, below the menu bar.
        _ => match cx.primary_display().map(|d| d.bounds()) {
            Some(screen) => point(screen.right() - px(WIDTH + MARGIN), screen.top() + px(40.0)),
            None => point(px(100.0), px(100.0)),
        },
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size: size(px(WIDTH), px(INITIAL_HEIGHT)) })),
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
    match cx.open_window(options, move |window, cx| {
        crate::platform::make_overlay(window);
        cx.new(|cx| InfoWindow::new(st, window, cx))
    }) {
        Ok(handle) => {
            state.update(cx, |s, _| s.info = Some(handle));
            set_open(state, true, cx);
        }
        Err(e) => eprintln!("failed to open info window: {e}"),
    }
}

/// A saved position on a display that's gone (unplugged monitor) would put
/// the window off screen.
fn on_some_display(p: gpui::Point<Pixels>, cx: &App) -> bool {
    cx.displays().iter().any(|d| d.bounds().contains(&p))
}

pub struct InfoWindow {
    state: Entity<AppState>,
    save_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl InfoWindow {
    fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            // Remember where the user moved it (debounced: a drag sends
            // many updates).
            cx.observe_window_bounds(window, |this, window, cx| {
                let origin = window.bounds().origin;
                let origin = (f32::from(origin.x), f32::from(origin.y));
                if this.state.read(cx).model.settings.info_origin == Some(origin) {
                    return;
                }
                let state = this.state.clone();
                this.save_task = Some(cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    let _ = state.update(cx, |s, _| {
                        s.model.settings.info_origin = Some(origin);
                        s.model.settings.save();
                    });
                }));
            }),
        ];
        Self { state, save_task: None, _subscriptions: subscriptions }
    }
}

/// Resize the window to the card's height, keeping its top-left corner.
fn fit(state: &Entity<AppState>, height: Pixels, window: &mut Window, cx: &mut App) {
    let height = height.ceil();
    let current = state.read(cx).info_height.unwrap_or(window.bounds().size.height);
    if (height - current).abs() < px(1.0) {
        return;
    }
    state.update(cx, |s, _| s.info_height = Some(height));
    let frame = Bounds { origin: window.bounds().origin, size: size(px(WIDTH), height) };
    crate::platform::set_window_frame(window, frame, cx);
}

impl Render for InfoWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut theme = Theme::for_appearance(window.appearance());
        theme.bg.a = 0.94;
        let state = self.state.read(cx);
        let m = &state.model;
        let name = m.settings.username.clone().unwrap_or_else(|| "Character".into());
        let overall = if m.is_loading { HealthStatus::Warning } else { m.monitor.overall() };

        let close_state = self.state.clone();
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            // Windows drags via the hit test (HTCAPTION) on the header.
            .window_control_area(WindowControlArea::Drag)
            .child(dot(status_color(overall), 9.0))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
            .child(div().flex_1())
            .child(
                div()
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
                        set_open(&close_state, false, cx);
                    }),
            );

        let mut body: Vec<AnyElement> = vec![header.into_any_element()];
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

        let fit_state = self.state.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_children_prepainted(move |bounds, window, cx| {
                if let Some(card) = bounds.first() {
                    fit(&fit_state, card.size.height, window, cx);
                }
            })
            .child(
                div()
                    .w_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
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
    }
}

/// What the character is doing: the worst issue (or the activity report)
/// and the current craft with its progress.
fn status_line(m: &Model, theme: Theme) -> impl IntoElement {
    let reports = m.monitor.sorted_reports();
    let shown = reports
        .iter()
        .find(|r| r.status != HealthStatus::Healthy)
        .or_else(|| reports.iter().find(|r| r.source == ActivityCheck::SOURCE))
        .cloned();
    let skill = (!m.is_offline).then(|| m.experience.last_skill_name()).flatten();
    let craft = if m.is_offline { None } else { m.active_craft().or_else(|| m.crafts.last_worked.clone()) };
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
                        .child(div().flex_none().child(c.percent_string())),
                )
                .child(div().flex().child(bar(c.fraction(), gpui::rgb(0x0a84ff), theme)))
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

fn bars(m: &Model, theme: Theme) -> impl IntoElement {
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
    let stamina_color = status_color(if m.is_offline { HealthStatus::Alert } else { m.monitor.overall() });
    let health_fraction = match (m.health, m.max_health) {
        (Some(h), Some(max)) if max > 0.0 => h / max,
        _ => 0.0,
    };
    let health_color = if health_fraction > 0.0 && health_fraction < 0.3 { alert } else { gpui::rgb(0xff6482) };
    let (food_fraction, food_color, food_text, food_text_color) = match &m.food {
        None => (0.0, healthy, "–".to_string(), theme.secondary),
        Some(food) => match food.regen_ends_at(m.now) {
            Some(end) => (food.regen_fraction(m.now).unwrap_or(0.0), healthy, duration_string(end - m.now), theme.text),
            None => (0.0, alert, "None".into(), alert),
        },
    };
    // Hover text: the active food buffs, one per line with time left
    // (user request: instead of a summary line under the bar).
    let food_tip: Option<SharedString> = m.food.as_ref().map(|f| {
        let mut buffs: Vec<_> = f.active(m.now).collect();
        buffs.sort_by_key(|b| (!b.is_regen, b.name.clone()));
        buffs.iter().map(|b| format!("{} · {}", b.short_name(), duration_string(b.ends_at - m.now))).collect::<Vec<_>>().join("\n").into()
    }).filter(|t: &SharedString| !t.is_empty());
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
fn summaries(m: &Model, theme: Theme) -> impl IntoElement {
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
        .child(row("Plants", badges::plants(&m.plants.plants), plants_note))
        .child(divider())
        .child(row(
            "Passive",
            passive,
            if m.passive.has_loaded { passive_note } else { Some("Loading…".into()) },
        ))
        .child(divider())
        .child(row("Crafts", badges::crafts(&m.crafts.sorted(), now, m.crafts.active_window), None))
}
