//! Short "type + tier" badges for plants, passive crafts and crafts
//! ("Grain T1", "Ingot T3"), grouped with a count and colored by state.

use std::collections::BTreeMap;

use crate::core::trackers::{Craft, PlantTracker, Stage};
use crate::core::types::{PassiveCraft, Plant};

/// Order = display order (most urgent first).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BadgeState {
    /// Red: the player has to do something (plant needs fertilizer/water).
    Action,
    /// Orange: running (growing, passive craft processing, craft worked on).
    Processing,
    /// Gray: waiting (paused craft, queued passive craft).
    Idle,
    /// Green: done (grown, ready to collect, craft complete).
    Finished,
}

/// Per-item detail for the hover text (user request: idle → percent done,
/// red → what's needed, orange → time left).
#[derive(Clone, Debug, PartialEq)]
pub enum Detail {
    /// Fraction done (0–1).
    Percent(f64),
    /// What the player has to do ("fertilizer", "water").
    Need(&'static str),
    /// Unix seconds it ends at (None: unknown), and whether it's estimated.
    EndsAt(Option<f64>, bool),
    /// Finished / waiting text ("Ready to harvest", "Queued").
    Note(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    /// Item tag / crop ("Ingot", "Grain").
    pub kind: String,
    pub tier: Option<i64>,
    pub count: usize,
    pub state: BadgeState,
    pub details: Vec<Detail>,
    /// Fraction done (0–1) of each item where it's known (craft progress,
    /// time elapsed); the compact chips' tiny bars.
    pub progress: Vec<f64>,
}

impl Badge {
    /// "Grain T1" or "Grain T1 ×6".
    pub fn text(&self) -> String {
        let label = match self.tier {
            Some(t) => format!("{} T{t}", self.kind),
            None => self.kind.clone(),
        };
        with_count(label, self.count)
    }

    /// Something in this group finishes within `within` seconds (plants
    /// growing, passive crafts running): the badge pulses.
    pub fn ends_soon(&self, now: f64, within: f64) -> bool {
        self.state == BadgeState::Processing
            && self.details.iter().any(|d| matches!(d, Detail::EndsAt(Some(end), _) if *end > now && *end - now <= within))
    }

    /// Without the tier, for rows already grouped by tier: "Grain ×6".
    pub fn short_text(&self) -> String {
        with_count(self.kind.clone(), self.count)
    }

    /// Hover text: percents ("42% done", "80%, 42% done"), needs ("Needs
    /// water", "3 need fertilizer · 1 needs water"), time left ("4m left",
    /// "next 4m · all 12m"), or a note ("Ready to harvest").
    pub fn tooltip(&self, now: f64) -> String {
        let mut parts: Vec<String> = Vec::new();

        let mut percents: Vec<f64> = self.details.iter().filter_map(|d| match d { Detail::Percent(f) => Some(*f), _ => None }).collect();
        if !percents.is_empty() {
            percents.sort_by(|a, b| b.total_cmp(a));
            let shown: Vec<String> = percents.iter().take(5).map(|f| format!("{}%", (f * 100.0).floor() as i64)).collect();
            let more = if percents.len() > 5 { format!(" (+{})", percents.len() - 5) } else { String::new() };
            parts.push(format!("{} done{more}", shown.join(", ")));
        }

        let mut needs: BTreeMap<&str, usize> = BTreeMap::new();
        for d in &self.details {
            if let Detail::Need(n) = d {
                *needs.entry(n).or_default() += 1;
            }
        }
        if self.count == 1 && needs.len() == 1 {
            parts.push(format!("Needs {}", needs.keys().next().unwrap()));
        } else {
            parts.extend(needs.iter().map(|(n, c)| format!("{c} need{} {n}", if *c == 1 { "s" } else { "" })));
        }

        let ends: Vec<(Option<f64>, bool)> = self.details.iter().filter_map(|d| match d { Detail::EndsAt(e, est) => Some((*e, *est)), _ => None }).collect();
        if !ends.is_empty() {
            let known: Vec<f64> = ends.iter().filter_map(|(e, _)| *e).collect();
            let est = if ends.iter().any(|(e, est)| e.is_some() && *est) { "~" } else { "" };
            let left = |end: f64| if end <= now { "any moment".to_string() } else { format!("{est}{}", crate::core::types::duration_string(end - now)) };
            let unknown = ends.len() - known.len();
            match (known.iter().copied().reduce(f64::min), known.iter().copied().reduce(f64::max)) {
                (Some(first), Some(last)) if known.len() == 1 || last - first < 1.0 => parts.push(format!("{} left", left(last))),
                (Some(first), Some(last)) => parts.push(format!("next {} · all {}", left(first), left(last))),
                _ => {}
            }
            if unknown > 0 {
                parts.push(if known.is_empty() { "Time left unknown".into() } else { format!("{unknown} unknown") });
            }
        }

        let mut notes: Vec<&str> = self.details.iter().filter_map(|d| match d { Detail::Note(n) => Some(*n), _ => None }).collect();
        notes.dedup();
        parts.extend(notes.into_iter().map(str::to_string));
        parts.join(" · ")
    }
}

/// Compact floating window (user request: "tiny lists"): one chip per
/// tier and state ("T1" in the state's color); the kinds go in the hover
/// text.
#[derive(Clone, Debug, PartialEq)]
pub struct TierChip {
    pub tier: Option<i64>,
    pub state: BadgeState,
    /// The badges merged into it (one per kind).
    pub members: Vec<Badge>,
}

impl TierChip {
    /// "T1", or "–" without a tier.
    pub fn text(&self) -> String {
        self.tier.map_or("–".to_string(), |t| format!("T{t}"))
    }

    /// One line per kind: "Grain ×6 · 4m left".
    pub fn tooltip(&self, now: f64) -> String {
        self.members
            .iter()
            .map(|b| {
                let tip = b.tooltip(now);
                if tip.is_empty() { b.text() } else { format!("{} · {tip}", b.text()) }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn ends_soon(&self, now: f64, within: f64) -> bool {
        self.members.iter().any(|b| b.ends_soon(now, within))
    }

    /// The tiny bar under the chip: the least done item, so the bar is
    /// full when the whole chip is (like "all grown in"). None when no
    /// item has a known progress (done, needs action, queued).
    pub fn progress(&self) -> Option<f64> {
        self.members.iter().flat_map(|b| b.progress.iter().copied()).reduce(f64::min)
    }
}

/// Badges merged per (tier, state): by tier (untiered last), then most
/// urgent first.
pub fn tier_chips(list: &[Badge]) -> Vec<TierChip> {
    // Key: (untiered, tier, state), so untiered sorts last.
    type Key = (bool, Option<i64>, BadgeState);
    let mut groups: BTreeMap<Key, Vec<Badge>> = BTreeMap::new();
    for b in list {
        groups.entry((b.tier.is_none(), b.tier, b.state)).or_default().push(b.clone());
    }
    groups.into_iter().map(|((_, tier, state), members)| TierChip { tier, state, members }).collect()
}

fn with_count(label: String, count: usize) -> String {
    if count > 1 { format!("{label} ×{count}") } else { label }
}

/// Group equal (kind, tier, state), most urgent first, then by kind/tier.
/// One item: label, state, hover detail, fraction done if known.
pub type Item = ((String, Option<i64>), BadgeState, Detail, Option<f64>);

pub fn group(items: impl IntoIterator<Item = Item>) -> Vec<Badge> {
    // (state, kind, tier) → (details, known fractions done).
    type Groups = BTreeMap<(BadgeState, String, Option<i64>), (Vec<Detail>, Vec<f64>)>;
    let mut groups = Groups::new();
    for ((kind, tier), state, detail, progress) in items {
        let (details, fractions) = groups.entry((state, kind, tier)).or_default();
        details.push(detail);
        fractions.extend(progress.map(|f| f.clamp(0.0, 1.0)));
    }
    groups
        .into_iter()
        .map(|((state, kind, tier), (details, progress))| Badge { kind, tier, count: details.len(), state, details, progress })
        .collect()
}

/// The soonest end of anything still running (plants growing, passive
/// crafts), and whether it's an estimate.
pub fn next_end(list: &[Badge], now: f64) -> Option<(f64, bool)> {
    list.iter()
        .flat_map(|b| &b.details)
        .filter_map(|d| match d {
            Detail::EndsAt(Some(end), estimate) if *end > now => Some((*end, *estimate)),
            _ => None,
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
}

/// Fraction of `start..end` passed at `now`.
fn elapsed(start: f64, end: f64, now: f64) -> Option<f64> {
    (end > start).then(|| (now - start) / (end - start))
}

/// Rows for display: one per tier (ascending, untiered last) when the list
/// has more than one tier (user request: sub-sections per tier), else a
/// single row with the tier in each badge. `Some(tier)` = row heading.
pub fn by_tier(list: &[Badge]) -> Vec<(Option<Option<i64>>, Vec<Badge>)> {
    let tiers: std::collections::BTreeSet<Option<i64>> = list.iter().map(|b| b.tier).collect();
    if tiers.len() <= 1 {
        return if list.is_empty() { vec![] } else { vec![(None, list.to_vec())] };
    }
    let mut ordered: Vec<Option<i64>> = tiers.into_iter().collect();
    // None sorts first in a BTreeSet; show untiered last.
    ordered.sort_by_key(|t| (t.is_none(), *t));
    ordered
        .into_iter()
        .map(|t| (Some(t), list.iter().filter(|b| b.tier == t).cloned().collect()))
        .collect()
}

/// ("Ingot", 3); falls back to `name` without a tag; tiers ≤ 0 dropped.
pub fn label(kind: Option<&str>, tier: Option<i64>, name: &str) -> (String, Option<i64>) {
    (kind.filter(|k| !k.is_empty()).unwrap_or(name).to_string(), tier.filter(|t| *t > 0))
}

/// Crop type from the placeable name: "Growing Basic Embergrain" → "Grain".
/// All crops are `<stage> <quality> <crop>` (quality = tier); the three
/// crops get short names, anything else keeps its own last word.
pub fn plant_kind(name: &str) -> String {
    let crop = name.rsplit(' ').next().unwrap_or(name);
    match crop {
        "Embergrain" => "Grain",
        "Starbulb" => "Bulbs",
        "Wispweave" => "Fiber",
        other => other,
    }
    .to_string()
}

pub fn plants(plants: &[Plant], now: f64) -> Vec<Badge> {
    group(plants.iter().filter_map(|p| {
        let (state, detail, progress) = match PlantTracker::stage(p)? {
            Stage::NeedsFertilizer => (BadgeState::Action, Detail::Need("fertilizer"), None),
            Stage::NeedsWater => (BadgeState::Action, Detail::Need("water"), None),
            Stage::Growing => {
                // Same rule as the tier summary: reported end, else estimate.
                let end = match (p.reported_end, p.growth_duration, p.started_growing_at) {
                    (Some(end), _, _) => Detail::EndsAt(Some(end), false),
                    (None, Some(d), Some(start)) => Detail::EndsAt(Some(start + d), true),
                    _ => Detail::EndsAt(None, false),
                };
                // Seen start → end when known; else the catalog duration
                // (its max: the real growth time is random between min
                // and max, so this can start partly filled).
                let progress = match (p.started_growing_at, p.reported_end, p.growth_duration) {
                    (Some(start), Some(end), _) => elapsed(start, end, now),
                    (_, Some(end), Some(d)) => elapsed(end - d, end, now),
                    (Some(start), None, Some(d)) => elapsed(start, start + d, now),
                    _ => None,
                };
                (BadgeState::Processing, end, progress)
            }
            Stage::Grown => (BadgeState::Finished, Detail::Note("Ready to harvest"), None),
        };
        Some((label(Some(&plant_kind(&p.name)), Some(p.tier), &p.name), state, detail, progress))
    }))
}

pub fn passive(crafts: &[PassiveCraft], now: f64) -> Vec<Badge> {
    group(crafts.iter().map(|c| {
        let (state, detail, progress) = if c.is_finished(now) {
            (BadgeState::Finished, Detail::Note("Ready to collect"), None)
        } else if c.started_at.is_none() {
            // Queued: not started, so 0% (no end time yet).
            (BadgeState::Idle, Detail::Note("Queued"), None)
        } else {
            let progress = c.started_at.zip(c.ends_at()).and_then(|(start, end)| elapsed(start, end, now));
            (BadgeState::Processing, Detail::EndsAt(c.ends_at(), true), progress)
        };
        (label(c.kind.as_deref(), c.tier, &c.name), state, detail, progress)
    }))
}

/// `active_window`: a craft that progressed this recently is being worked on.
pub fn crafts(crafts: &[Craft], now: f64, active_window: f64) -> Vec<Badge> {
    group(crafts.iter().map(|c| {
        // Active crafts have no time left (rate-based ETAs were removed as
        // too inaccurate), so orange shows the percent too.
        let state = if c.is_complete() {
            BadgeState::Finished
        } else if c.last_progress_at.is_some_and(|t| now - t <= active_window) {
            BadgeState::Processing
        } else {
            BadgeState::Idle
        };
        let (detail, progress) =
            if state == BadgeState::Finished { (Detail::Note("Done"), None) } else { (Detail::Percent(c.fraction()), Some(c.fraction())) };
        (label(c.kind.as_deref(), c.tier, &c.name), state, detail, progress)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plant(name: &str, tier: i64) -> Plant {
        Plant {
            name: name.into(),
            tier,
            reported_end: None,
            started_growing_at: None,
            growth_duration: None,
        }
    }

    #[test]
    fn plant_badges() {
        let list = [
            plant("Growing Basic Embergrain", 1),
            plant("Growing Basic Embergrain", 1),
            plant("Planted Infused Starbulb", 3),
            plant("Grown Simple Wispweave", 2),
            plant("Some Sift Collector", 1),
        ];
        let texts: Vec<(String, BadgeState)> = plants(&list, 0.0).iter().map(|b| (b.text(), b.state)).collect();
        assert_eq!(
            texts,
            vec![
                ("Bulbs T3".into(), BadgeState::Action),
                ("Grain T1 ×2".into(), BadgeState::Processing),
                ("Fiber T2".into(), BadgeState::Finished),
            ]
        );
    }

    #[test]
    fn tooltips() {
        let mut growing = plant("Growing Basic Embergrain", 1);
        growing.started_growing_at = Some(0.0);
        growing.growth_duration = Some(300.0);
        let mut later = growing.clone();
        later.started_growing_at = Some(600.0);
        let list = [growing, later, plant("Planted Basic Embergrain", 1), plant("Fertilized Planted Basic Embergrain", 1)];
        let b = plants(&list, 0.0);
        assert_eq!(b[0].tooltip(0.0), "1 needs fertilizer · 1 needs water");
        assert_eq!(b[1].tooltip(60.0), "next ~4m · all ~14m");
        assert!(!b[1].ends_soon(60.0, 30.0));
        assert!(b[1].ends_soon(280.0, 30.0));
        let single = plants(&[plant("Planted Basic Starbulb", 2)], 0.0);
        assert_eq!(single[0].tooltip(0.0), "Needs fertilizer");
    }

    #[test]
    fn chips() {
        let list = [
            plant("Grown Basic Embergrain", 1),
            plant("Growing Basic Starbulb", 2),
            plant("Planted Basic Starbulb", 1),
            plant("Grown Basic Starbulb", 1),
        ];
        let chips = tier_chips(&plants(&list, 0.0));
        let got: Vec<(String, BadgeState, usize)> = chips.iter().map(|c| (c.text(), c.state, c.members.len())).collect();
        assert_eq!(
            got,
            vec![
                ("T1".into(), BadgeState::Action, 1),
                ("T1".into(), BadgeState::Finished, 2),
                ("T2".into(), BadgeState::Processing, 1),
            ]
        );
        assert_eq!(chips[1].tooltip(0.0), "Bulbs T1 · Ready to harvest\nGrain T1 · Ready to harvest");
        assert_eq!(chips[1].progress(), None);
    }

    #[test]
    fn chip_progress() {
        // Timer end + catalog duration; seen start + end; nothing known.
        let mut a = plant("Growing Basic Embergrain", 1);
        a.reported_end = Some(100.0);
        a.growth_duration = Some(100.0);
        let mut b = a.clone();
        b.started_growing_at = Some(40.0);
        let c = plant("Growing Basic Embergrain", 1);
        let progress = |list: &[Plant], now: f64| tier_chips(&plants(list, now))[0].progress();
        assert_eq!(progress(&[a.clone()], 50.0), Some(0.5));
        assert_eq!(progress(&[b.clone()], 70.0), Some(0.5));
        // The least done item; unknown ones don't count.
        assert_eq!(progress(&[a, b, c.clone()], 70.0), Some(0.5));
        assert_eq!(progress(&[c], 70.0), None);
    }

    #[test]
    fn next_plant_end() {
        let growing = |end: f64| Plant { reported_end: Some(end), ..plant("Growing Basic Embergrain", 1) };
        let list = plants(&[growing(500.0), growing(300.0), growing(50.0), plant("Grown Basic Embergrain", 2)], 100.0);
        // Already past (any moment) doesn't count.
        assert_eq!(next_end(&list, 100.0), Some((300.0, false)));
        assert_eq!(next_end(&plants(&[plant("Planted Basic Starbulb", 1)], 0.0), 0.0), None);
    }

    #[test]
    fn labels() {
        assert_eq!(label(Some("Ingot"), Some(3), "Simple Ingot"), ("Ingot".into(), Some(3)));
        assert_eq!(label(None, Some(0), "Rope"), ("Rope".into(), None));
        let rows = by_tier(&plants(&[plant("Planted Basic Starbulb", 1), plant("Grown Simple Embergrain", 2)], 0.0));
        assert_eq!(rows.iter().map(|(t, b)| (*t, b.len())).collect::<Vec<_>>(), vec![(Some(Some(1)), 1), (Some(Some(2)), 1)]);
    }
}
