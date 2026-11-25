//! Wires network events into the trackers and status rules (port of the
//! Swift `AppModel`). No GPUI here: `main.rs` drives it and drains its
//! outputs (commands for the network, notifications).

use std::collections::HashMap;

use crate::core::status::{HealthStatus, StatusMonitor};
use crate::core::trackers::{
    ActivityCheck, Craft, CraftTracker, ExperienceTracker, PassiveCraftTracker, PlantTracker, SatiationTracker,
    StaminaTracker,
};
use crate::core::types::{FoodBuffDesc, FoodState, PlayerSummary, Position, RawBuff, now};
use crate::sound::Sound;
use crate::net::{Command, ConnState, NetEvent, RelayPlayer, RelayState};
use crate::settings::Settings;

pub const LOADING_TIMEOUT: f64 = 20.0;
/// Signed in but no live event for this long → stale feed, reconnect.
pub const STALE_FEED_WINDOW: f64 = 60.0;
pub const ALERT_COOLDOWN: f64 = 5.0 * 60.0;
const PLAYER_SOURCE: &str = "Player";
const CONNECTION_SOURCE: &str = "Connection";
const RELAY_SOURCE: &str = "Relay";
const FOOD_SOURCE: &str = "Food";

/// Stamina fraction above which "idle" plays the idle sound.
pub const IDLE_SOUND_STAMINA: f64 = 0.9;

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
    /// "Food" report then).
    pub food: Option<FoodState>,
    /// Started buffs, turned into `food` with the food buff types.
    buffs: Option<Vec<RawBuff>>,
    food_descs: HashMap<i64, FoodBuffDesc>,
    /// Relay only (`health_state`, max = stats value 0).
    pub health: Option<f64>,
    pub max_health: Option<f64>,
    sounds: Vec<Sound>,
    was_idle_full: bool,
    /// Relay position and when it last changed ("Active" while moving).
    position: Option<Position>,
    pub moved_at: Option<f64>,
    /// Activity went idle straight from "Active" (moving): no idle alerts.
    idle_after_moving: bool,
    activity: ActivityCheck,

    pub live_state: ConnState,
    pub relay_state: RelayState,
    pub now: f64,
    pub is_loading: bool,
    pub is_offline: bool,

    loading_started_at: f64,
    /// Every relay region delivered its rows.
    relay_loaded: bool,
    connected_at: Option<f64>,
    /// The player's relay rows (`player_state`, `character_stats_state`, …).
    relay_player: Option<RelayPlayer>,
    last_live_event_at: Option<f64>,
    last_alert_at: HashMap<String, f64>,

    commands: Vec<Command>,
    notifications: Vec<Notification>,
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
            buffs: None,
            food_descs: HashMap::new(),
            health: None,
            max_health: None,
            sounds: Vec::new(),
            was_idle_full: false,
            position: None,
            moved_at: None,
            idle_after_moving: false,
            activity: ActivityCheck::default(),
            live_state: ConnState::Connecting,
            relay_state: RelayState::Off,
            now,
            is_loading: true,
            is_offline: false,
            loading_started_at: now,
            relay_loaded: false,
            connected_at: None,
            relay_player: None,
            last_live_event_at: None,
            last_alert_at: HashMap::new(),
            commands: Vec::new(),
            notifications: Vec::new(),
        };
        model.commands.push(Command::Track(model.settings.user_id.clone()));
        model.evaluate();
        model
    }

    pub fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }

    /// Sound events since the last call (played by `AppState::flush` if
    /// enabled in the settings).
    pub fn take_sounds(&mut self) -> Vec<Sound> {
        std::mem::take(&mut self.sounds)
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
            self.buffs = None;
            self.health = None;
            self.max_health = None;
            self.position = None;
            self.moved_at = None;
            self.idle_after_moving = false;
            self.monitor = StatusMonitor::default();
            self.relay_state = RelayState::Off;
            self.live_state = ConnState::Connecting;
            self.relay_player = None;
            self.last_live_event_at = None;
            self.connected_at = None;
            self.is_offline = false;
            self.is_loading = true;
            self.loading_started_at = now();
            self.relay_loaded = false;
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
            NetEvent::RelayCrafts(list) => {
                // The relay pushes progress live: record it as progress
                // (sets "last progressed") before merging the list.
                for craft in &list {
                    self.crafts.handle_progress(&craft.id, craft.progress, now);
                }
                self.crafts.update(&list);
            }
            NetEvent::RelayPlayer(player) => {
                if let Some(values) = player.as_ref().and_then(|p| p.stats.as_ref()) {
                    self.apply_stats(values);
                }
                self.health = player.as_ref().and_then(|p| p.health);
                if let Some(position) = player.as_ref().and_then(|p| p.position) {
                    self.update_position(position, now);
                }
                self.relay_player = player;
            }
            NetEvent::Player(player) => {
                if self.settings.username.as_deref() != Some(player.username.as_str()) {
                    self.settings.username = Some(player.username.clone());
                    self.settings.save();
                }
                self.reconnect_if_feed_stale(now);
            }
            NetEvent::RelayPlants(plants) => self.plants.update(&plants),
            NetEvent::RelayPassiveCrafts(crafts) => self.passive.update(crafts, now),
            NetEvent::Relay(state) => self.relay_state = state,
            NetEvent::RelayLoaded => self.relay_loaded = true,
            NetEvent::RelayBuffs(buffs) => {
                self.buffs = Some(buffs);
                self.update_food();
            }
            NetEvent::FoodBuffDescs(descs) => {
                self.food_descs = descs;
                self.update_food();
            }
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

    /// Food from the last buffs; unknown (None) until buffs and the food
    /// buff types are both known (no types → everything looks like "No food").
    fn update_food(&mut self) {
        let descs = &self.food_descs;
        self.food = self.buffs.as_ref().filter(|_| !descs.is_empty()).map(|b| FoodState::from_raw(b, descs));
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
        if values.is_empty() {
            return;
        }
        self.stamina.set_reported_max(values.get(1).copied(), self.now);
        self.satiation.set_reported_max(values.get(19).copied());
        self.max_health = values.first().copied().filter(|v| *v > 0.0);
    }

    fn update_loading(&mut self) {
        if !self.is_loading {
            return;
        }
        let ready = self.relay_loaded && self.live_state == ConnState::Connected;
        if ready || self.now - self.loading_started_at >= LOADING_TIMEOUT {
            self.is_loading = false;
            self.evaluate();
            // Startup states aren't news.
            self.monitor.take_changes();
        }
    }

    /// BitJita can stop delivering events on a healthy-looking connection;
    /// satiation updates ~1/s while signed in, so silence means stale.
    /// Online status: relay `player_state.signed_in`.
    fn reconnect_if_feed_stale(&mut self, now: f64) {
        let (Some(true), Some(connected_at)) = (self.relay_player.as_ref().map(|p| p.signed_in), self.connected_at) else { return };
        let last = self.last_live_event_at.unwrap_or(0.0).max(connected_at);
        if now - last >= STALE_FEED_WINDOW {
            eprintln!("Signed in but no live events for {}s; reconnecting", (now - last) as i64);
            self.commands.push(Command::ReconnectLive);
        }
    }

    fn evaluate(&mut self) {
        // The game's own `signed_in` (relay); unknown counts as online.
        self.is_offline = self.relay_player.as_ref().is_some_and(|p| !p.signed_in);
        if self.is_offline {
            // Cleared: coming back online is a first report → no notification.
            self.monitor.clear(ActivityCheck::SOURCE);
            self.monitor.clear(SatiationTracker::SOURCE);
            self.monitor.clear(FOOD_SOURCE);
            self.monitor.report(PLAYER_SOURCE, HealthStatus::Alert, "Player offline");
        } else {
            self.monitor.report(PLAYER_SOURCE, HealthStatus::Healthy, "Online");
            self.activity.evaluate(&self.experience, &self.stamina, self.crafts.last_worked.as_ref(), self.moved_at, &mut self.monitor, self.now);
            self.satiation.report_current(&mut self.monitor);
            self.report_food();
        }
        match self.live_state {
            ConnState::Connected => self.monitor.report(CONNECTION_SOURCE, HealthStatus::Healthy, "Connected"),
            ConnState::Connecting => self.monitor.report(CONNECTION_SOURCE, HealthStatus::Warning, "Connecting…"),
            ConnState::Disconnected => self.monitor.report(CONNECTION_SOURCE, HealthStatus::Alert, "Disconnected"),
        }
        // Single source for everything but XP/stamina/satiation: an outage
        // means stale data, so it's always shown.
        match self.relay_state {
            RelayState::Live => self.monitor.report(RELAY_SOURCE, HealthStatus::Healthy, "Relay live"),
            RelayState::Off | RelayState::Connecting => self.monitor.report(RELAY_SOURCE, HealthStatus::Warning, "Relay connecting…"),
            RelayState::Unavailable(_) => self.monitor.report(RELAY_SOURCE, HealthStatus::Alert, "Relay unavailable"),
        }
        self.collect_alert_notifications();
        self.check_idle_full();
    }

    /// Relay position; a change = moved ("Active").
    fn update_position(&mut self, position: Position, now: f64) {
        if self.position.is_some_and(|p| p != position) {
            self.moved_at = Some(now);
        }
        self.position = Some(position);
    }

    /// Idle (orange idle message) with stamina above 90%: once per time it
    /// becomes true (resets when working again or stamina drops).
    fn check_idle_full(&mut self) {
        let idle = self.monitor.get(ActivityCheck::SOURCE).is_some_and(|r| ActivityCheck::is_idle(&r.message));
        let full = self.stamina.fraction().is_some_and(|f| f > IDLE_SOUND_STAMINA);
        let now_idle_full = idle && full && !self.is_offline && !self.is_loading;
        if now_idle_full && !self.was_idle_full && !self.idle_after_moving {
            self.sounds.push(Sound::IdleFull);
        }
        self.was_idle_full = now_idle_full;
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
            // Idle right after moving: the player stopped the action by
            // walking away (user request: not alert-worthy). Stays quiet
            // until the activity is something other than idle again.
            if report.source == ActivityCheck::SOURCE {
                if ActivityCheck::is_idle(&report.message) {
                    if previous.message == ActivityCheck::ACTIVE {
                        self.idle_after_moving = true;
                    }
                } else {
                    self.idle_after_moving = false;
                }
            }
            let turned_red = report.status == HealthStatus::Alert && previous.status != HealthStatus::Alert;
            let went_idle = report.source == ActivityCheck::SOURCE
                && ActivityCheck::is_idle(&report.message)
                && !ActivityCheck::is_idle(&previous.message)
                && !self.idle_after_moving;
            if !turned_red && !went_idle {
                continue;
            }
            if self.last_alert_at.get(&report.source).is_some_and(|t| self.now - t < ALERT_COOLDOWN) {
                continue;
            }
            self.last_alert_at.insert(report.source.clone(), self.now);
            if turned_red && report.source == FOOD_SOURCE {
                self.sounds.push(Sound::OutOfFood);
            }
            let who = self.settings.username.clone().unwrap_or_else(|| "BitCraft".into());
            self.notifications.push(Notification { title: format!("{who}: {}", report.source), body: report.message });
        }
    }

    fn collect_notifications(&mut self) {
        let completed = self.crafts.take_completed();
        if !completed.is_empty() && !self.is_loading {
            self.sounds.push(Sound::CraftFinished);
        }
        for craft in completed {
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
