//! Wires network events into the trackers and status rules (port of the
//! Swift `AppModel`). No GPUI here: `main.rs` drives it and drains its
//! outputs (commands for the network, notifications).

use std::collections::HashMap;

use crate::core::status::{HealthStatus, StatusMonitor};
use crate::core::trackers::{
    ActivityCheck, Craft, CraftTracker, ExperienceTracker, PassiveCraftTracker, PlantTracker, SatiationTracker,
    StaminaTracker,
};
use crate::core::types::{FoodState, PlayerSummary, now};
use crate::net::{Command, ConnState, NetEvent, RelayPlayer, RelayState, live};
use crate::settings::Settings;

pub const LOADING_TIMEOUT: f64 = 20.0;
/// Any live event within this window means online (`signedIn` can lag).
pub const LIVE_EVENT_WINDOW: f64 = 30.0;
/// Signed in but no live event for this long → stale feed, reconnect.
pub const STALE_FEED_WINDOW: f64 = 60.0;
pub const ALERT_COOLDOWN: f64 = 5.0 * 60.0;
const PLAYER_SOURCE: &str = "Player";
const CONNECTION_SOURCE: &str = "Connection";
const FOOD_SOURCE: &str = "Food";

pub struct Notification {
    pub title: String,
    pub body: String,
}

pub struct Model {
    pub settings: Settings,
    pub monitor: StatusMonitor,
    pub experience: ExperienceTracker,
    pub stamina: StaminaTracker,
    pub satiation: SatiationTracker,
    pub crafts: CraftTracker,
    pub plants: PlantTracker,
    pub passive: PassiveCraftTracker,
    /// Food buffs from the relay; None until the relay delivered them (no
    /// "Food" report then: REST buff data is unreliable).
    pub food: Option<FoodState>,
    activity: ActivityCheck,

    pub live_state: ConnState,
    pub relay_state: RelayState,
    pub now: f64,
    pub is_loading: bool,
    pub is_offline: bool,

    loading_started_at: f64,
    initial_rest_done: bool,
    connected_at: Option<f64>,
    reported_signed_in: Option<bool>,
    /// From the relay (`player_state` / `character_stats_state`); while
    /// Some, it replaces REST `signedIn` and REST stats (both lag).
    relay_player: Option<RelayPlayer>,
    last_live_event_at: Option<f64>,
    last_alert_at: HashMap<String, f64>,

    commands: Vec<Command>,
    notifications: Vec<Notification>,
    /// Source of the last plants / passive crafts list (Some(true) = relay).
    plants_from_relay: Option<bool>,
    passive_from_relay: Option<bool>,
}

#[derive(Clone, Copy)]
enum Feed {
    Plants,
    Passive,
}

impl Model {
    pub fn new(settings: Settings) -> Self {
        let now = now();
        let mut model = Self {
            settings,
            monitor: StatusMonitor::default(),
            experience: ExperienceTracker::default(),
            stamina: StaminaTracker::default(),
            satiation: SatiationTracker::default(),
            crafts: CraftTracker::default(),
            plants: PlantTracker::default(),
            passive: PassiveCraftTracker::default(),
            food: None,
            activity: ActivityCheck::default(),
            live_state: ConnState::Connecting,
            relay_state: RelayState::Off,
            now,
            is_loading: true,
            is_offline: false,
            loading_started_at: now,
            initial_rest_done: false,
            connected_at: None,
            reported_signed_in: None,
            relay_player: None,
            last_live_event_at: None,
            last_alert_at: HashMap::new(),
            commands: Vec::new(),
            notifications: Vec::new(),
            plants_from_relay: None,
            passive_from_relay: None,
        };
        model.commands.push(Command::Track(model.settings.user_id.clone()));
        model.evaluate();
        model
    }

    pub fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }

    pub fn take_notifications(&mut self) -> Vec<Notification> {
        std::mem::take(&mut self.notifications)
    }

    pub fn active_craft(&self) -> Option<Craft> {
        self.crafts.active(self.now)
    }

    /// Switch to another character (from the search).
    pub fn select_player(&mut self, player: &PlayerSummary) {
        self.settings.username = Some(player.username.clone());
        if player.entity_id != self.settings.user_id {
            self.settings.user_id = player.entity_id.clone();
            self.experience = ExperienceTracker::default();
            self.stamina = StaminaTracker::default();
            self.satiation = SatiationTracker::default();
            self.crafts = CraftTracker::default();
            self.plants = PlantTracker::default();
            self.passive = PassiveCraftTracker::default();
            self.food = None;
            self.monitor = StatusMonitor::default();
            self.relay_state = RelayState::Off;
            self.live_state = ConnState::Connecting;
            self.reported_signed_in = None;
            self.relay_player = None;
            self.last_live_event_at = None;
            self.connected_at = None;
            self.is_offline = false;
            self.plants_from_relay = None;
            self.passive_from_relay = None;
            self.is_loading = true;
            self.loading_started_at = now();
            self.initial_rest_done = false;
            self.commands.push(Command::Track(self.settings.user_id.clone()));
        }
        self.settings.save();
        self.evaluate();
    }

    /// Events for another character (before a switch) are dropped.
    pub fn handle(&mut self, user: &str, event: NetEvent) {
        if user != self.settings.user_id {
            return;
        }
        let now = now();
        self.now = now;
        match event {
            NetEvent::Live(state) => {
                self.live_state = state;
                self.connected_at = (state == ConnState::Connected).then_some(now);
            }
            NetEvent::Experience(stacks) => {
                self.last_live_event_at = Some(now);
                self.experience.handle(&stacks, now);
            }
            NetEvent::Stamina(v) => {
                self.last_live_event_at = Some(now);
                self.stamina.handle(v, now);
            }
            NetEvent::Satiation(v) => {
                self.last_live_event_at = Some(now);
                self.satiation.satiation = Some(v);
            }
            NetEvent::CharacterStats(values) => {
                self.last_live_event_at = Some(now);
                self.apply_stats(&values);
            }
            NetEvent::CraftProgress { id, progress } => {
                self.last_live_event_at = Some(now);
                self.crafts.handle_progress(&id, progress, now);
            }
            NetEvent::Crafts(list) => {
                let ids = self.crafts.update(&list);
                self.commands.push(Command::Subscribe(ids.iter().map(|id| format!("{}{id}", live::CRAFT_PREFIX)).collect()));
            }
            NetEvent::RelayCrafts(list) => {
                // The relay pushes progress live: record it as progress
                // (sets "last progressed") before merging the list.
                for craft in &list {
                    self.crafts.handle_progress(&craft.id, craft.progress, now);
                }
                let ids = self.crafts.update(&list);
                // Keep the BitJita live channels too (fallback if the relay drops).
                self.commands.push(Command::Subscribe(ids.iter().map(|id| format!("{}{id}", live::CRAFT_PREFIX)).collect()));
            }
            // REST stats lag behind gear/buff changes; the relay's are live.
            NetEvent::Stats(values) if self.relay_player.as_ref().is_none_or(|p| p.stats.is_none()) => {
                self.apply_stats(&values)
            }
            NetEvent::Stats(_) => {}
            NetEvent::RelayPlayer(player) => {
                if let Some(values) = player.as_ref().and_then(|p| p.stats.as_ref()) {
                    self.apply_stats(values);
                }
                self.relay_player = player;
            }
            NetEvent::Player(player) => {
                self.reported_signed_in = player.signed_in;
                if self.settings.username.as_deref() != Some(player.username.as_str()) {
                    self.settings.username = Some(player.username.clone());
                    self.settings.save();
                }
                self.reconnect_if_feed_stale(now);
            }
            // Switching source (REST ↔ relay) is a fresh baseline: their lists
            // differ (REST can lag), which would look like crafts finishing.
            NetEvent::Plants(plants) => {
                let baseline = self.switch_source(false, Feed::Plants);
                self.plants.update(&plants, baseline);
            }
            NetEvent::RelayPlants(plants) => {
                let baseline = self.switch_source(true, Feed::Plants);
                self.plants.update(&plants, baseline);
            }
            NetEvent::PassiveCrafts(crafts) => {
                let baseline = self.switch_source(false, Feed::Passive);
                self.passive.update(crafts, now, baseline);
            }
            NetEvent::RelayPassiveCrafts(crafts) => {
                let baseline = self.switch_source(true, Feed::Passive);
                self.passive.update(crafts, now, baseline);
            }
            NetEvent::InitialRestDone => self.initial_rest_done = true,
            NetEvent::Relay(state) => self.relay_state = state,
            NetEvent::RelayFood(food) => self.food = Some(food),
        }
        self.collect_notifications();
        self.update_loading();
        self.evaluate();
    }

    /// Every second: time-based rules, loading timeout, passive end times.
    pub fn tick(&mut self) {
        self.now = now();
        self.stamina.tick(self.now);
        self.passive.tick(self.now);
        self.collect_notifications();
        self.update_loading();
        self.evaluate();
    }

    /// Records which source fed `feed` last; true if it changed.
    fn switch_source(&mut self, relay: bool, feed: Feed) -> bool {
        let slot = match feed {
            Feed::Plants => &mut self.plants_from_relay,
            Feed::Passive => &mut self.passive_from_relay,
        };
        let changed = slot.is_some_and(|was| was != relay);
        *slot = Some(relay);
        changed
    }

    /// Food: healthy while a food regen buff runs, alert "No food" when it
    /// ran out (also a notification, via the alert transition).
    fn report_food(&mut self) {
        let Some(food) = &self.food else { return };
        match food.regen_ends_at(self.now) {
            Some(end) => {
                let left = crate::ui::duration_string(end - self.now);
                self.monitor.report(FOOD_SOURCE, HealthStatus::Healthy, format!("{left} left"));
            }
            None => self.monitor.report(FOOD_SOURCE, HealthStatus::Alert, "No food"),
        }
    }

    fn apply_stats(&mut self, values: &[f64]) {
        self.stamina.set_reported_max(values.get(1).copied(), self.now);
        self.satiation.set_reported_max(values.get(19).copied());
    }

    fn update_loading(&mut self) {
        if !self.is_loading {
            return;
        }
        let ready = self.initial_rest_done && self.live_state == ConnState::Connected;
        if ready || self.now - self.loading_started_at >= LOADING_TIMEOUT {
            self.is_loading = false;
            self.evaluate();
            // Startup states aren't news.
            self.monitor.take_changes();
        }
    }

    /// BitJita can stop delivering events on a healthy-looking connection;
    /// satiation updates ~1/s while signed in, so silence means stale.
    /// Relay `player_state.signed_in` when available, else REST `signedIn`.
    fn signed_in(&self) -> Option<bool> {
        self.relay_player.as_ref().map(|p| p.signed_in).or(self.reported_signed_in)
    }

    fn reconnect_if_feed_stale(&mut self, now: f64) {
        let (Some(true), Some(connected_at)) = (self.signed_in(), self.connected_at) else { return };
        let last = self.last_live_event_at.unwrap_or(0.0).max(connected_at);
        if now - last >= STALE_FEED_WINDOW {
            eprintln!("Signed in but no live events for {}s; reconnecting", (now - last) as i64);
            self.commands.push(Command::ReconnectLive);
        }
    }

    fn evaluate(&mut self) {
        self.is_offline = match &self.relay_player {
            // The game's own `signed_in`: live and authoritative.
            Some(player) => !player.signed_in,
            // REST `signedIn` lags: any live event in the last 30s wins.
            None => {
                let live_recently = self.last_live_event_at.is_some_and(|t| self.now - t <= LIVE_EVENT_WINDOW);
                !live_recently && self.reported_signed_in == Some(false)
            }
        };
        if self.is_offline {
            // Cleared: coming back online is a first report → no notification.
            self.monitor.clear(ActivityCheck::SOURCE);
            self.monitor.clear(SatiationTracker::SOURCE);
            self.monitor.clear(FOOD_SOURCE);
            self.monitor.report(PLAYER_SOURCE, HealthStatus::Alert, "Player offline");
        } else {
            self.monitor.report(PLAYER_SOURCE, HealthStatus::Healthy, "Online");
            self.activity.evaluate(&self.experience, &self.stamina, self.crafts.last_worked.as_ref(), &mut self.monitor, self.now);
            self.satiation.report_current(&mut self.monitor);
            self.report_food();
        }
        match self.live_state {
            ConnState::Connected => self.monitor.report(CONNECTION_SOURCE, HealthStatus::Healthy, "Connected"),
            ConnState::Connecting => self.monitor.report(CONNECTION_SOURCE, HealthStatus::Warning, "Connecting…"),
            ConnState::Disconnected => self.monitor.report(CONNECTION_SOURCE, HealthStatus::Alert, "Disconnected"),
        }
        self.collect_alert_notifications();
    }

    /// A source turning red from a known non-red state, or the activity
    /// going idle (orange) from a known non-idle state; 5 min cooldown.
    fn collect_alert_notifications(&mut self) {
        let changes = self.monitor.take_changes();
        if self.is_loading {
            return;
        }
        for (previous, report) in changes {
            let Some(previous) = previous else { continue };
            let turned_red = report.status == HealthStatus::Alert && previous.status != HealthStatus::Alert;
            let went_idle = report.source == ActivityCheck::SOURCE
                && ActivityCheck::is_idle(&report.message)
                && !ActivityCheck::is_idle(&previous.message);
            if !turned_red && !went_idle {
                continue;
            }
            if self.last_alert_at.get(&report.source).is_some_and(|t| self.now - t < ALERT_COOLDOWN) {
                continue;
            }
            self.last_alert_at.insert(report.source.clone(), self.now);
            let who = self.settings.username.clone().unwrap_or_else(|| "BitCraft".into());
            self.notifications.push(Notification { title: format!("{who}: {}", report.source), body: report.message });
        }
    }

    fn collect_notifications(&mut self) {
        for craft in self.crafts.take_completed() {
            self.notifications.push(Notification {
                title: "Craft finished".into(),
                body: format!("{} at {} is done", craft.name, craft.building.as_deref().unwrap_or("station")),
            });
        }
        let finished = self.passive.take_finished();
        if !finished.is_empty() {
            let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
            for c in &finished {
                *counts.entry(format!("{} at {}", c.name, c.building)).or_default() += 1;
            }
            let body = counts
                .into_iter()
                .map(|(k, n)| if n > 1 { format!("{n}× {k}") } else { k })
                .collect::<Vec<_>>()
                .join("\n");
            self.notifications.push(Notification { title: "Passive craft finished".into(), body });
        }
        for tier in self.plants.take_finished() {
            self.notifications.push(Notification {
                title: "Plants grown".into(),
                body: format!("All T{} plants have finished growing ({} ready to harvest)", tier.tier, tier.grown),
            });
        }
        if self.is_loading {
            self.notifications.clear();
        }
    }

    /// Menu bar / tooltip text: "Fishing 22% · Resting",
    /// "T2 grown · Player offline", "Loading…".
    pub fn label_text(&self) -> String {
        if self.is_loading {
            return "Loading…".into();
        }
        let issues = self.monitor.active_issues().into_iter().map(|r| r.message);
        if self.is_offline {
            // No skill/craft (stale), but plants keep growing offline.
            return self.plants.summary().into_iter().chain(issues).collect::<Vec<_>>().join(" · ");
        }
        let mut head = self.experience.last_skill_name().unwrap_or_default();
        if let Some(craft) = self.active_craft() {
            if !head.is_empty() {
                head.push(' ');
            }
            head.push_str(&craft.percent_string());
        }
        if let Some(plants) = self.plants.summary() {
            if !head.is_empty() {
                head.push_str(" — ");
            }
            head.push_str(&plants);
        }
        std::iter::once(head).filter(|h| !h.is_empty()).chain(issues).collect::<Vec<_>>().join(" · ")
    }
}
