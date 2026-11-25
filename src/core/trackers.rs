//! Trackers collect data; rules that combine sources live in `ActivityCheck`.
//! Same behavior as the Swift app (see AGENTS.md "Status rules").

use std::collections::{BTreeMap, HashMap, HashSet};

use super::skill::Skill;
use super::status::{HealthStatus, StatusMonitor};
use super::types::{CraftInfo, PassiveCraft, Plant};

// MARK: - Experience

#[derive(Default)]
pub struct ExperienceTracker {
    pub last_gain_at: Option<f64>,
    /// Last gain per skill (attributes craft progress to the player).
    gain_by_skill: HashMap<i64, f64>,
    pub last_gain_amount: i64,
    /// Skill with the largest increase in the latest gain.
    pub last_skill_id: Option<i64>,
    pub total: Option<i64>,
    stacks: HashMap<i64, i64>,
}

impl ExperienceTracker {
    pub fn last_gain_in(&self, skill: i64) -> Option<f64> {
        self.gain_by_skill.get(&skill).copied()
    }

    pub fn last_skill_name(&self) -> Option<String> {
        self.last_skill_id.map(Skill::name_for)
    }

    /// `stacks`: (skill id, quantity). The first list is the relay's
    /// snapshot (baseline, not a gain); the skill is known from the first
    /// gain on.
    pub fn handle(&mut self, stacks: &[(i64, i64)], now: f64) {
        let new: HashMap<i64, i64> = stacks.iter().copied().fold(HashMap::new(), |mut m, (k, v)| {
            let e = m.entry(k).or_insert(v);
            *e = (*e).max(v);
            m
        });
        let gained: i64 = if self.stacks.is_empty() {
            0
        } else {
            let per_skill: Vec<(i64, i64)> = new
                .iter()
                .filter_map(|(skill, q)| {
                    let delta = q - self.stacks.get(skill).copied().unwrap_or(0);
                    (delta > 0).then_some((*skill, delta))
                })
                .collect();
            for (skill, _) in &per_skill {
                self.gain_by_skill.insert(*skill, now);
            }
            let gained = per_skill.iter().map(|(_, d)| d).sum();
            if gained > 0 {
                self.last_gain_amount = gained;
                self.last_skill_id = per_skill.iter().max_by_key(|(_, d)| *d).map(|(s, _)| *s);
            }
            gained
        };
        self.total = Some(new.values().sum());
        self.stacks = new;
        if gained > 0 {
            self.last_gain_at = Some(now);
        }
    }

    pub fn has_gained(&self, window: f64, now: f64) -> bool {
        self.last_gain_at.is_some_and(|t| now - t <= window)
    }
}

// MARK: - Stamina

pub struct StaminaTracker {
    pub stamina: Option<f64>,
    pub max: Option<f64>,
    pub last_regen_at: Option<f64>,
    pub last_drain_at: Option<f64>,
    /// Times stamina went up since it was last spent (the current regen
    /// streak). A single tick between actions isn't resting.
    regen_streak: Vec<f64>,
    /// From the stats API / live `character_stats_state`.
    pub reported_max: Option<f64>,
    samples: Vec<(f64, f64)>,
    max_window: f64,
}

impl Default for StaminaTracker {
    fn default() -> Self {
        Self {
            stamina: None,
            max: None,
            last_regen_at: None,
            last_drain_at: None,
            regen_streak: Vec::new(),
            reported_max: None,
            samples: Vec::new(),
            max_window: 10.0 * 60.0,
        }
    }
}

impl StaminaTracker {
    pub fn fraction(&self) -> Option<f64> {
        let (s, m) = (self.stamina?, self.max?);
        (m > 0.0).then(|| (s / m).clamp(0.0, 1.0))
    }

    /// Only trusted with the relay max (the fallback estimate equals the
    /// current value while regenerating).
    pub fn is_at_least(&self, fraction: f64) -> bool {
        match (self.stamina, self.reported_max) {
            (Some(s), Some(m)) => s >= m * fraction - 0.01,
            _ => false,
        }
    }

    pub fn handle(&mut self, value: f64, now: f64) {
        if let Some(old) = self.stamina {
            if value > old {
                self.last_regen_at = Some(now);
                self.regen_streak.push(now);
            } else if value < old {
                self.last_drain_at = Some(now);
                self.regen_streak.clear();
            }
        }
        self.stamina = Some(value);
        self.samples.push((now, value));
        self.update_max(now);
    }

    /// Went up within `window` and hasn't been spent since.
    pub fn is_regenerating(&self, window: f64, now: f64) -> bool {
        let Some(regen) = self.last_regen_at else { return false };
        now - regen <= window && self.last_drain_at.is_none_or(|drain| regen > drain)
    }

    /// Stamina increases after `time` without being spent since.
    pub fn regen_ticks_since(&self, time: f64) -> usize {
        self.regen_streak.iter().filter(|t| **t > time).count()
    }

    pub fn set_reported_max(&mut self, value: Option<f64>, now: f64) {
        self.reported_max = value.filter(|v| *v > 0.0);
        self.update_max(now);
    }

    pub fn tick(&mut self, now: f64) {
        self.update_max(now);
    }

    fn update_max(&mut self, now: f64) {
        let window = self.max_window;
        self.samples.retain(|(t, _)| now - t <= window);
        self.max = match self.reported_max {
            // The API can lag behind gear changes; never show over 100%.
            Some(m) => Some(m.max(self.stamina.unwrap_or(0.0))),
            // Updates stop at full, so include the current value.
            None => self.samples.iter().map(|(_, v)| *v).chain(self.stamina).reduce(f64::max),
        };
    }
}

// MARK: - Satiation

pub struct SatiationTracker {
    pub satiation: Option<f64>,
    pub max: f64,
    pub alert_threshold: f64,
}

impl Default for SatiationTracker {
    fn default() -> Self {
        Self { satiation: None, max: 100.0, alert_threshold: 15.0 }
    }
}

impl SatiationTracker {
    pub const SOURCE: &'static str = "Satiation";

    pub fn fraction(&self) -> Option<f64> {
        let s = self.satiation?;
        (self.max > 0.0).then(|| (s / self.max).clamp(0.0, 1.0))
    }

    pub fn is_low(&self) -> bool {
        self.satiation.is_some_and(|s| s <= self.alert_threshold)
    }

    pub fn set_reported_max(&mut self, value: Option<f64>) {
        if let Some(v) = value.filter(|v| *v > 0.0) {
            self.max = v;
        }
    }

    pub fn report_current(&self, monitor: &mut StatusMonitor) {
        let Some(s) = self.satiation else { return };
        if s <= self.alert_threshold {
            monitor.report(Self::SOURCE, HealthStatus::Alert, format!("Low satiation ({})", s as i64));
        } else {
            monitor.report(Self::SOURCE, HealthStatus::Healthy, format!("Satiation {}", s as i64));
        }
    }
}

// MARK: - Crafts

#[derive(Clone, Debug)]
pub struct Craft {
    pub id: String,
    pub name: String,
    pub building: Option<String>,
    pub progress: i64,
    pub total: i64,
    /// Any progress (the owner or someone else working on it).
    pub last_progress_at: Option<f64>,
    pub kind: Option<String>,
    pub tier: Option<i64>,
    pub skill: Option<i64>,
    /// Last progress made by the player (see [`CraftTracker::mark_worked`]).
    pub worked_at: Option<f64>,
}

impl Craft {
    pub fn fraction(&self) -> f64 {
        if self.total > 0 { (self.progress as f64 / self.total as f64).min(1.0) } else { 0.0 }
    }

    pub fn is_complete(&self) -> bool {
        self.progress >= self.total
    }

    /// Rounded down so it never shows 100% early.
    pub fn percent_string(&self) -> String {
        format!("{}%", (self.fraction() * 100.0).floor() as i64)
    }
}

pub struct CraftTracker {
    pub crafts: BTreeMap<String, Craft>,
    /// Last craft the player worked on (their own progress, not someone
    /// else's on it); kept after completion (in case its row disappears).
    /// Cleared if it disappears unfinished (cancelled).
    pub last_worked: Option<Craft>,
    pub active_window: f64,
    completion_reported: HashSet<String>,
    /// Crafts that just completed, for notifications.
    completed: Vec<Craft>,
}

impl Default for CraftTracker {
    fn default() -> Self {
        Self {
            crafts: BTreeMap::new(),
            last_worked: None,
            active_window: 30.0,
            completion_reported: HashSet::new(),
            completed: Vec::new(),
        }
    }
}

impl CraftTracker {
    /// Most recently worked on first.
    pub fn sorted(&self) -> Vec<Craft> {
        let mut crafts: Vec<Craft> = self.crafts.values().cloned().collect();
        crafts.sort_by(|a, b| {
            b.last_progress_at
                .unwrap_or(0.0)
                .total_cmp(&a.last_progress_at.unwrap_or(0.0))
                .then(b.id.cmp(&a.id))
        });
        crafts
    }

    /// The craft the player is working on: their own progress in the last
    /// 30s (someone else progressing it doesn't count).
    pub fn active(&self, now: f64) -> Option<Craft> {
        let craft = self.crafts.values().filter(|c| c.worked_at.is_some()).max_by(|a, b| a.worked_at.unwrap_or(0.0).total_cmp(&b.worked_at.unwrap_or(0.0)))?;
        let last = craft.worked_at?;
        (now - last <= self.active_window && !craft.is_complete()).then(|| craft.clone())
    }

    /// (id, skill, time) of each craft whose latest progress isn't
    /// attributed to the player (yet): the model checks it against XP.
    pub fn unattributed(&self) -> Vec<(String, Option<i64>, f64)> {
        self.crafts
            .values()
            .filter_map(|c| {
                let t = c.last_progress_at?;
                (c.worked_at != Some(t)).then(|| (c.id.clone(), c.skill, t))
            })
            .collect()
    }

    /// The player made this craft's latest progress.
    pub fn mark_worked(&mut self, id: &str) {
        let Some(craft) = self.crafts.get_mut(id) else { return };
        craft.worked_at = craft.last_progress_at;
        self.last_worked = Some(craft.clone());
    }

    pub fn take_completed(&mut self) -> Vec<Craft> {
        std::mem::take(&mut self.completed)
    }

    fn report_completion(&mut self, craft: &Craft) {
        if craft.is_complete() && self.completion_reported.insert(craft.id.clone()) {
            self.completed.push(craft.clone());
        }
    }

    /// Full craft list (relay). Finished crafts stay listed until
    /// collected (the relay keeps their row; BitJita's REST used to drop
    /// them, so they used to be dropped here too). A craft already
    /// finished when first seen isn't news (no "Craft finished").
    pub fn update(&mut self, list: &[CraftInfo]) {
        let mut seen = HashSet::new();
        for info in list {
            seen.insert(info.id.clone());
            let is_new = !self.crafts.contains_key(&info.id);
            let entry = self.crafts.entry(info.id.clone()).or_insert_with(|| Craft {
                id: info.id.clone(),
                name: info.name.clone(),
                building: info.building.clone(),
                progress: info.progress,
                total: info.total,
                last_progress_at: None,
                kind: info.kind.clone(),
                tier: info.tier,
                skill: info.skill,
                worked_at: None,
            });
            entry.name = info.name.clone();
            entry.skill = info.skill;
            entry.kind = info.kind.clone();
            entry.tier = info.tier;
            entry.building = info.building.clone();
            entry.total = info.total;
            // Live updates are fresher; never move backwards.
            entry.progress = entry.progress.max(info.progress);
            if info.completed {
                entry.progress = entry.progress.max(entry.total);
            }
            let craft = entry.clone();
            if !craft.is_complete() {
                continue;
            }
            if is_new {
                self.completion_reported.insert(craft.id.clone());
            } else {
                self.report_completion(&craft);
            }
            if self.last_worked.as_ref().is_some_and(|l| l.id == craft.id) {
                self.last_worked = Some(craft);
            }
        }
        // Gone: collected (or cancelled).
        self.crafts.retain(|id, _| seen.contains(id));
        self.completion_reported.retain(|id| seen.contains(id));
        if self.last_worked.as_ref().is_some_and(|l| !l.is_complete() && !seen.contains(&l.id)) {
            self.last_worked = None;
        }
    }

    pub fn handle_progress(&mut self, entity_id: &str, progress: i64, now: f64) {
        let Some(craft) = self.crafts.get_mut(entity_id) else { return };
        // Progress only goes up: ignore anything not newer (a list re-sent
        // with the same value isn't progress).
        if progress <= craft.progress {
            return;
        }
        craft.progress = progress;
        craft.last_progress_at = Some(now);
        let craft = craft.clone();
        // Not necessarily the player's progress (`mark_worked`), but the
        // last worked craft shows the current percent.
        if let Some(last) = self.last_worked.as_mut().filter(|l| l.id == craft.id) {
            last.progress = craft.progress;
        }
        self.report_completion(&craft);
    }
}

// MARK: - Plants

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    NeedsFertilizer,
    NeedsWater,
    Growing,
    Grown,
}

#[derive(Clone, Debug, Default)]
pub struct TierSummary {
    pub tier: i64,
    pub total: usize,
    pub needs_fertilizer: usize,
    pub needs_water: usize,
    pub growing: usize,
    pub grown: usize,
    pub growth_ends: Vec<f64>,
    pub is_estimate: bool,
}

impl TierSummary {
    pub fn is_all_grown(&self) -> bool {
        self.total > 0 && self.grown == self.total
    }

    pub fn all_grown_at(&self) -> Option<f64> {
        self.growth_ends.iter().copied().reduce(f64::max)
    }
}

#[derive(Default)]
pub struct PlantTracker {
    pub tiers: Vec<TierSummary>,
    /// Last list (for badges).
    pub plants: Vec<Plant>,
    has_loaded: bool,
    finished: Vec<TierSummary>,
}

impl PlantTracker {
    pub fn stage(plant: &Plant) -> Option<Stage> {
        let name = plant.name.as_str();
        if name.starts_with("Grown ") {
            Some(Stage::Grown)
        } else if name.starts_with("Growing ") {
            Some(Stage::Growing)
        } else if name.starts_with("Fertilized Planted ") {
            Some(Stage::NeedsWater)
        } else if name.starts_with("Planted ") {
            Some(Stage::NeedsFertilizer)
        } else {
            None
        }
    }

    pub fn growing_tiers(&self) -> Vec<TierSummary> {
        self.tiers.iter().filter(|t| t.growing > 0).cloned().collect()
    }

    pub fn take_finished(&mut self) -> Vec<TierSummary> {
        std::mem::take(&mut self.finished)
    }

    /// "T2 grown; T1 needs water", or None.
    pub fn summary(&self) -> Option<String> {
        let list = |f: &dyn Fn(&TierSummary) -> bool| {
            self.tiers.iter().filter(|t| f(t)).map(|t| format!("T{}", t.tier)).collect::<Vec<_>>().join(", ")
        };
        let mut parts = Vec::new();
        let grown = list(&|t| t.is_all_grown());
        let fert = list(&|t| t.needs_fertilizer > 0);
        let water = list(&|t| t.needs_water > 0);
        if !grown.is_empty() {
            parts.push(format!("{grown} grown"));
        }
        if !fert.is_empty() {
            parts.push(format!("{fert} needs fertilizer"));
        }
        if !water.is_empty() {
            parts.push(format!("{water} needs water"));
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }

    /// The first list sets the baseline (no notifications).
    pub fn update(&mut self, plants: &[Plant]) {
        self.plants = plants.to_vec();
        let mut by_tier: BTreeMap<i64, TierSummary> = BTreeMap::new();
        for plant in plants {
            let Some(stage) = Self::stage(plant) else { continue };
            let s = by_tier.entry(plant.tier).or_insert_with(|| TierSummary { tier: plant.tier, ..Default::default() });
            s.total += 1;
            match stage {
                Stage::NeedsFertilizer => s.needs_fertilizer += 1,
                Stage::NeedsWater => s.needs_water += 1,
                Stage::Grown => s.grown += 1,
                Stage::Growing => {
                    s.growing += 1;
                    if let Some(end) = plant.reported_end {
                        s.growth_ends.push(end);
                    } else if let (Some(d), Some(start)) = (plant.growth_duration, plant.started_growing_at) {
                        s.growth_ends.push(start + d);
                        s.is_estimate = true;
                    }
                }
            }
        }
        let previous: HashMap<i64, TierSummary> = self.tiers.drain(..).map(|t| (t.tier, t)).collect();
        self.tiers = by_tier.into_values().collect();
        if self.has_loaded {
            for tier in &self.tiers {
                if let Some(before) = previous.get(&tier.tier)
                    && before.growing > 0
                    && tier.growing == 0
                    && tier.grown > before.grown
                {
                    self.finished.push(tier.clone());
                }
            }
        }
        self.has_loaded = true;
    }
}

// MARK: - Passive crafts

#[derive(Clone, Debug)]
pub struct PassiveGroup {
    pub building: String,
    pub name: String,
    pub running: usize,
    pub ready: usize,
    pub all_done_at: Option<f64>,
    pub has_unknown_end: bool,
}

#[derive(Default)]
pub struct PassiveCraftTracker {
    pub crafts: Vec<PassiveCraft>,
    pub has_loaded: bool,
    finished_ids: HashSet<String>,
    finished: Vec<PassiveCraft>,
}

impl PassiveCraftTracker {
    /// Crafts already finished in the first list aren't news.
    pub fn update(&mut self, crafts: Vec<PassiveCraft>, now: f64) {
        self.crafts = crafts;
        if !self.has_loaded {
            // Already finished before we started watching: don't notify.
            self.finished_ids.extend(self.crafts.iter().filter(|c| c.is_finished(now)).map(|c| c.id.clone()));
            self.has_loaded = true;
        }
        let ids: HashSet<&String> = self.crafts.iter().map(|c| &c.id).collect();
        self.finished_ids.retain(|id| ids.contains(id));
        self.tick(now);
    }

    pub fn tick(&mut self, now: f64) {
        let newly: Vec<PassiveCraft> = self
            .crafts
            .iter()
            .filter(|c| c.is_finished(now) && !self.finished_ids.contains(&c.id))
            .cloned()
            .collect();
        self.finished_ids.extend(newly.iter().map(|c| c.id.clone()));
        self.finished.extend(newly);
    }

    pub fn take_finished(&mut self) -> Vec<PassiveCraft> {
        std::mem::take(&mut self.finished)
    }

    /// Grouped by building + item; running groups first.
    pub fn groups(&self, now: f64) -> Vec<PassiveGroup> {
        let mut groups: BTreeMap<(String, String), PassiveGroup> = BTreeMap::new();
        for craft in &self.crafts {
            let g = groups.entry((craft.building.clone(), craft.name.clone())).or_insert_with(|| PassiveGroup {
                building: craft.building.clone(),
                name: craft.name.clone(),
                running: 0,
                ready: 0,
                all_done_at: None,
                has_unknown_end: false,
            });
            if craft.is_finished(now) {
                g.ready += 1;
            } else {
                g.running += 1;
                match craft.ends_at() {
                    Some(end) => g.all_done_at = Some(g.all_done_at.map_or(end, |a| a.max(end))),
                    None => g.has_unknown_end = true,
                }
            }
        }
        let mut list: Vec<PassiveGroup> = groups.into_values().collect();
        list.sort_by_key(|g| (g.running == 0, g.building.clone(), g.name.clone()));
        list
    }

    /// "Weave {0}" + "Rough Cloth Strip" → "Rough Cloth Strip".
    pub fn display_name(recipe: Option<&str>, item: Option<&str>) -> String {
        if let Some(item) = item {
            return item.to_string();
        }
        match recipe {
            Some(r) => r.replace(" {0}", "").replace("{0}", ""),
            None => "Passive craft".to_string(),
        }
    }
}

// MARK: - Activity

/// Is the character doing something useful? See AGENTS.md "Status rules".
pub struct ActivityCheck {
    pub experience_window: f64,
    pub regen_window: f64,
    pub craft_xp_tolerance: f64,
    /// Moved this recently → "Active".
    pub move_window: f64,
    /// Stamina increases (without spending) after the last XP that mean
    /// the player stopped to rest.
    pub rest_ticks: usize,
}

impl Default for ActivityCheck {
    fn default() -> Self {
        Self { experience_window: 20.0, regen_window: 10.0, craft_xp_tolerance: 5.0, move_window: 10.0, rest_ticks: 2 }
    }
}

impl ActivityCheck {
    pub const SOURCE: &'static str = "Activity";
    /// Message while the character is moving around.
    pub const ACTIVE: &'static str = "Active";

    /// Idle messages (warning, orange; user: red is for food/satiation/
    /// connection problems). Going idle still notifies, see `Model`.
    pub fn is_idle(message: &str) -> bool {
        message.starts_with("Idle") || message.starts_with("Craft idle")
    }

    /// Stamina fraction at which the player can continue, per skill.
    fn ready_fraction(skill: Option<i64>) -> f64 {
        match skill.and_then(Skill::from_id) {
            Some(Skill::Fishing) => 0.2,
            _ => 1.0,
        }
    }

    pub fn evaluate(
        &self,
        xp: &ExperienceTracker,
        stamina: &StaminaTracker,
        last_craft: Option<&Craft>,
        moved_at: Option<f64>,
        monitor: &mut StatusMonitor,
        now: f64,
    ) {
        let ready = Self::ready_fraction(xp.last_skill_id);
        // Resting since the last XP = stamina kept going up without being
        // spent. A single tick between actions (passive / food regen while
        // crafting) isn't resting; counting it showed "Craft idle" while
        // crafting.
        let regen_since_xp = match (stamina.last_regen_at, xp.last_gain_at) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(_), Some(gain)) => stamina.regen_ticks_since(gain) >= self.rest_ticks,
        };
        let gained = xp.has_gained(self.experience_window, now) && !regen_since_xp;
        let craft = last_craft.filter(|c| {
            let Some(progress) = c.last_progress_at else { return false };
            xp.last_gain_at.is_none_or(|gain| progress >= gain - self.craft_xp_tolerance)
        });
        let idle_craft = craft.map(|c| format!("Craft idle ({})", c.percent_string()));
        let src = Self::SOURCE;

        // Moving around (relay position changed in the last 10s): the
        // player is at the keyboard, so nothing else matters (user request).
        // XP after the last move means the player started working: show
        // that right away instead of "Active" (user request).
        let gained_since_move = gained && moved_at.is_none_or(|m| xp.last_gain_at.is_some_and(|g| g >= m));
        if moved_at.is_some_and(|t| now - t <= self.move_window) && !gained_since_move {
            monitor.report(src, HealthStatus::Healthy, Self::ACTIVE);
        } else if craft.is_some_and(|c| c.is_complete()) {
            monitor.report(src, HealthStatus::Warning, "Craft finished");
        } else if gained {
            // Before "Resting": XP newer than the last regen means work
            // resumed, even before the stamina spend arrives (user request:
            // switch immediately). Regen after the XP clears `gained`.
            monitor.report(src, HealthStatus::Healthy, "Gaining experience");
        } else if stamina.is_at_least(ready) {
            let msg = idle_craft.unwrap_or_else(|| {
                if ready >= 1.0 {
                    "Idle: stamina full".into()
                } else {
                    format!("Idle: stamina above {}%", (ready * 100.0) as i64)
                }
            });
            monitor.report(src, HealthStatus::Warning, msg);
        } else if stamina.is_regenerating(self.regen_window, now) {
            monitor.report(src, HealthStatus::Warning, "Resting");
        } else if let Some(msg) = idle_craft {
            monitor.report(src, HealthStatus::Warning, msg);
        } else if xp.last_gain_at.is_none() {
            monitor.report(src, HealthStatus::Warning, "No XP or regen yet");
        } else {
            monitor.report(src, HealthStatus::Warning, "Idle: no XP, no regen");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regen_means_resting_and_full_means_idle() {
        let mut xp = ExperienceTracker::default();
        let mut stamina = StaminaTracker::default();
        let mut monitor = StatusMonitor::default();
        stamina.set_reported_max(Some(100.0), 0.0);
        xp.handle(&[(2, 10)], 0.0);
        xp.handle(&[(2, 20)], 1.0);
        stamina.handle(50.0, 1.0);
        stamina.handle(51.0, 2.0);
        // One tick isn't resting yet (could be regen between actions).
        ActivityCheck::default().evaluate(&xp, &stamina, None, None, &mut monitor, 2.0);
        assert_eq!(monitor.sorted_reports()[0].message, "Gaining experience");
        stamina.handle(52.0, 3.0);
        ActivityCheck::default().evaluate(&xp, &stamina, None, None, &mut monitor, 3.0);
        assert_eq!(monitor.sorted_reports()[0].message, "Resting");
        stamina.handle(100.0, 4.0);
        ActivityCheck::default().evaluate(&xp, &stamina, None, None, &mut monitor, 4.0);
        assert_eq!(monitor.sorted_reports()[0].message, "Idle: stamina full");
    }

    #[test]
    fn regen_tick_between_craft_actions_is_not_craft_idle() {
        let mut xp = ExperienceTracker::default();
        let mut stamina = StaminaTracker::default();
        let mut monitor = StatusMonitor::default();
        let check = ActivityCheck::default();
        stamina.set_reported_max(Some(100.0), 0.0);
        stamina.handle(100.0, 0.0);
        xp.handle(&[(2, 10)], 0.0);
        let craft = |t: f64| Craft {
            id: "1".into(),
            name: "Plank".into(),
            building: None,
            progress: 5,
            total: 100,
            last_progress_at: Some(t),
            kind: None,
            tier: None,
            skill: None,
            worked_at: Some(t),
        };
        // Action: spend + XP, then regen tops stamina back up to full.
        stamina.handle(98.0, 1.0);
        xp.handle(&[(2, 20)], 1.0);
        stamina.handle(100.0, 1.5);
        check.evaluate(&xp, &stamina, Some(&craft(1.0)), None, &mut monitor, 1.6);
        assert_eq!(monitor.sorted_reports()[0].message, "Gaining experience");
        // Next action's spend arrives before its XP.
        stamina.handle(98.0, 2.0);
        check.evaluate(&xp, &stamina, Some(&craft(2.0)), None, &mut monitor, 2.1);
        assert_eq!(monitor.sorted_reports()[0].message, "Gaining experience");
    }

    #[test]
    fn xp_after_regen_is_gaining_immediately() {
        let mut xp = ExperienceTracker::default();
        let mut stamina = StaminaTracker::default();
        let mut monitor = StatusMonitor::default();
        stamina.set_reported_max(Some(100.0), 0.0);
        xp.handle(&[(2, 10)], 0.0);
        stamina.handle(50.0, 1.0);
        stamina.handle(51.0, 2.0);
        stamina.handle(52.0, 2.5);
        let check = ActivityCheck::default();
        check.evaluate(&xp, &stamina, None, None, &mut monitor, 2.0);
        assert_eq!(monitor.sorted_reports()[0].message, "Resting");
        // XP arrives before the stamina spend: no longer resting.
        xp.handle(&[(2, 20)], 3.0);
        check.evaluate(&xp, &stamina, None, None, &mut monitor, 3.0);
        assert_eq!(monitor.sorted_reports()[0].message, "Gaining experience");
    }

    #[test]
    fn xp_after_moving_is_gaining() {
        let mut xp = ExperienceTracker::default();
        let mut stamina = StaminaTracker::default();
        let mut monitor = StatusMonitor::default();
        stamina.set_reported_max(Some(100.0), 0.0);
        stamina.handle(100.0, 0.0);
        xp.handle(&[(2, 10)], 0.0);
        xp.handle(&[(2, 20)], 6.0);
        ActivityCheck::default().evaluate(&xp, &stamina, None, Some(5.0), &mut monitor, 7.0);
        assert_eq!(monitor.sorted_reports()[0].message, "Gaining experience");
    }

    #[test]
    fn xp_snapshot_is_not_a_gain() {
        let mut xp = ExperienceTracker::default();
        xp.handle(&[(2, 10), (12, 5)], 0.0);
        assert_eq!((xp.last_gain_at, xp.last_skill_id, xp.total), (None, None, Some(15)));
        xp.handle(&[(2, 10), (12, 9)], 1.0);
        assert_eq!((xp.last_gain_at, xp.last_skill_id, xp.last_gain_amount), (Some(1.0), Some(12), 4));
    }

    #[test]
    fn finished_crafts_stay_until_collected() {
        let info = |id: &str, progress| CraftInfo {
            id: id.into(),
            name: "Thread".into(),
            building: None,
            progress,
            total: 100,
            completed: progress >= 100,
            kind: None,
            tier: None,
            skill: None,
        };
        let mut crafts = CraftTracker::default();
        // Already finished when first seen: listed, but not news.
        crafts.update(&[info("1", 100), info("2", 40)]);
        assert_eq!(crafts.sorted().len(), 2);
        assert!(crafts.take_completed().is_empty());
        // Finishing while tracked: news once, and still listed.
        crafts.update(&[info("1", 100), info("2", 100)]);
        assert_eq!(crafts.take_completed().len(), 1);
        crafts.update(&[info("1", 100), info("2", 100)]);
        assert!(crafts.take_completed().is_empty());
        assert!(crafts.sorted().iter().all(|c| c.is_complete()));
        // Collected: gone.
        crafts.update(&[info("2", 100)]);
        assert_eq!(crafts.sorted().len(), 1);
    }

    #[test]
    fn craft_progress_never_goes_back() {
        let mut crafts = CraftTracker::default();
        crafts.update(&[CraftInfo {
            id: "1".into(),
            name: "Plank".into(),
            building: None,
            progress: 10,
            total: 100,
            completed: false,
            kind: None,
            tier: None,
            skill: None,
        }]);
        crafts.handle_progress("1", 12, 5.0);
        // Someone else's progress (not marked): not the player's activity.
        assert!(crafts.active(5.0).is_none() && crafts.last_worked.is_none());
        crafts.mark_worked("1");
        assert_eq!(crafts.active(6.0).map(|c| c.id), Some("1".into()));
        // An older value (or the same one again) isn't progress.
        crafts.handle_progress("1", 11, 6.0);
        crafts.handle_progress("1", 12, 7.0);
        let craft = &crafts.sorted()[0];
        assert_eq!((craft.progress, craft.last_progress_at), (12, Some(5.0)));
    }

    #[test]
    fn moving_is_active() {
        let xp = ExperienceTracker::default();
        let mut stamina = StaminaTracker::default();
        let mut monitor = StatusMonitor::default();
        stamina.set_reported_max(Some(100.0), 0.0);
        stamina.handle(100.0, 0.0);
        let check = ActivityCheck::default();
        check.evaluate(&xp, &stamina, None, Some(5.0), &mut monitor, 12.0);
        assert_eq!(monitor.sorted_reports()[0].message, "Active");
        check.evaluate(&xp, &stamina, None, Some(5.0), &mut monitor, 16.0);
        assert_eq!(monitor.sorted_reports()[0].message, "Idle: stamina full");
    }

    #[test]
    fn fishing_is_ready_at_20_percent() {
        let mut xp = ExperienceTracker::default();
        let mut stamina = StaminaTracker::default();
        let mut monitor = StatusMonitor::default();
        stamina.set_reported_max(Some(100.0), 0.0);
        xp.handle(&[(12, 10)], 0.0);
        xp.handle(&[(12, 42)], 1.0);
        stamina.handle(30.0, 1.0);
        ActivityCheck::default().evaluate(&xp, &stamina, None, None, &mut monitor, 40.0);
        let report = &monitor.sorted_reports()[0];
        assert_eq!((report.status, report.message.as_str()), (HealthStatus::Warning, "Idle: stamina above 20%"));
    }

    #[test]
    fn finished_craft_is_warning() {
        let xp = ExperienceTracker::default();
        let stamina = StaminaTracker::default();
        let mut monitor = StatusMonitor::default();
        let craft = Craft {
            id: "1".into(),
            name: "Plank".into(),
            building: None,
            progress: 10,
            total: 10,
            last_progress_at: Some(5.0),
            kind: None,
            tier: None,
            skill: None,
            worked_at: Some(5.0),
        };
        ActivityCheck::default().evaluate(&xp, &stamina, Some(&craft), None, &mut monitor, 6.0);
        let report = &monitor.sorted_reports()[0];
        assert_eq!((report.status, report.message.as_str()), (HealthStatus::Warning, "Craft finished"));
    }

    #[test]
    fn plant_stages_and_summary() {
        let plant = |name: &str, tier| Plant {
            name: name.into(),
            tier,
            reported_end: None,
            started_growing_at: None,
            growth_duration: None,
        };
        let mut tracker = PlantTracker::default();
        tracker.update(&[plant("Grown Starbulb", 1), plant("Planted Embergrain", 2), plant("Sift", 1)]);
        assert_eq!(tracker.summary().as_deref(), Some("T1 grown; T2 needs fertilizer"));
    }
}
