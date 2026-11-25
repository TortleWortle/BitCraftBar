//! Data passed from the network layer to the trackers. Times are Unix
//! seconds (`f64`), see [`now`].

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// One active (non-passive) craft (relay `progressive_action_state`).
#[derive(Clone, Debug)]
pub struct CraftInfo {
    pub id: String,
    pub name: String,
    pub building: Option<String>,
    pub progress: i64,
    pub total: i64,
    pub completed: bool,
    /// Crafted item's tag ("Ingot", "Rope") and tier, for badges.
    pub kind: Option<String>,
    pub tier: Option<i64>,
}

/// One passive craft (relay `passive_craft_state`).
#[derive(Clone, Debug)]
pub struct PassiveCraft {
    pub id: String,
    pub name: String,
    pub building: String,
    /// None while queued (not started) or unknown.
    pub started_at: Option<f64>,
    pub reported_complete: bool,
    /// Recipe duration in seconds, if known.
    pub duration: Option<f64>,
    /// Crafted item's tag and tier, for badges.
    pub kind: Option<String>,
    pub tier: Option<i64>,
}

impl PassiveCraft {
    pub fn ends_at(&self) -> Option<f64> {
        Some(self.started_at? + self.duration?)
    }

    pub fn is_finished(&self, now: f64) -> bool {
        self.reported_complete || self.ends_at().is_some_and(|end| end <= now)
    }
}

/// One placeable (relay `placeable_state`); only crops matter.
#[derive(Clone, Debug)]
pub struct Plant {
    pub name: String,
    pub tier: i64,
    /// Growth end (relay `placeable_growth_timer`).
    pub reported_end: Option<f64>,
    /// When it started growing, if known (for the estimate).
    pub started_growing_at: Option<f64>,
    /// Catalog growth duration (max), if looked up.
    pub growth_duration: Option<f64>,
}

/// Character location (relay `mobile_entity_state`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    pub x: i64,
    pub z: i64,
}

/// One active food buff (relay `active_buff_state` + `buff_desc`).
#[derive(Clone, Debug, PartialEq)]
pub struct FoodBuff {
    /// e.g. "Level 3 Food Regen", "Level 2 Deluxe Action Speed".
    pub name: String,
    /// Buff type "Food Regen" (the stamina/health regen every food gives);
    /// otherwise "Food Buffs" (extra effects of better food).
    pub is_regen: bool,
    pub ends_at: f64,
    /// Full length in seconds (for the bar).
    pub duration: f64,
}

/// The character's active food buffs. (Food names were dropped: several
/// foods give the same buffs and the names are long; the user preferred a
/// summary of the buffs.)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoodState {
    pub buffs: Vec<FoodBuff>,
}

impl FoodBuff {
    /// "Level 3 Food Regen" → "Regen 3", "Level 2 Deluxe Action Speed" →
    /// "Deluxe Action Speed 2"; other names ("Mining Pie") unchanged.
    pub fn short_name(&self) -> String {
        let Some(rest) = self.name.strip_prefix("Level ") else { return self.name.clone() };
        let Some((level, what)) = rest.split_once(' ') else { return self.name.clone() };
        let what = if what == "Food Regen" { "Regen" } else { what };
        format!("{what} {level}")
    }
}

/// One started entry of the relay's `active_buff_state.active_buffs`;
/// which buffs are food comes from [`FoodBuffDesc`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawBuff {
    pub id: i64,
    /// Unix seconds.
    pub start: f64,
    /// Seconds.
    pub duration: f64,
}

/// A food buff type (relay `buff_desc` rows of the two food buff types).
/// Cached in settings: the live channel only has buff ids, and the relay
/// can be down for a long time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FoodBuffDesc {
    pub name: String,
    pub is_regen: bool,
}

impl FoodState {
    /// Food buffs among `buffs` (ids found in `descs`).
    pub fn from_raw(buffs: &[RawBuff], descs: &HashMap<i64, FoodBuffDesc>) -> Self {
        let buffs = buffs
            .iter()
            .filter_map(|b| {
                let desc = descs.get(&b.id)?;
                Some(FoodBuff { name: desc.name.clone(), is_regen: desc.is_regen, ends_at: b.start + b.duration, duration: b.duration })
            })
            .collect();
        Self { buffs }
    }

    /// When the food regen runs out, if it's active.
    pub fn regen_ends_at(&self, now: f64) -> Option<f64> {
        self.buffs.iter().filter(|b| b.is_regen && b.ends_at > now).map(|b| b.ends_at).reduce(f64::max)
    }

    /// Share of the food regen left (0...1), for the bar.
    pub fn regen_fraction(&self, now: f64) -> Option<f64> {
        let buff = self.buffs.iter().filter(|b| b.is_regen && b.ends_at > now).max_by(|a, b| a.ends_at.total_cmp(&b.ends_at))?;
        (buff.duration > 0.0).then(|| ((buff.ends_at - now) / buff.duration).clamp(0.0, 1.0))
    }

    pub fn active(&self, now: f64) -> impl Iterator<Item = &FoodBuff> {
        self.buffs.iter().filter(move |b| b.ends_at > now)
    }

    /// Active buffs, regen first: "Regen 3 · Deluxe Action Speed 2".
    pub fn summary(&self, now: f64) -> String {
        let mut buffs: Vec<&FoodBuff> = self.active(now).collect();
        buffs.sort_by_key(|b| (!b.is_regen, b.name.clone()));
        buffs.iter().map(|b| b.short_name()).collect::<Vec<_>>().join(" · ")
    }
}

/// A relay `/player?name=` search result.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PlayerSummary {
    pub entity_id: String,
    pub username: String,
    pub signed_in: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relay_player_search() {
        let json = r#"[{"entity_id":"648518346633885895","last_active_timestamp":1795629491,"region":14,"signed_in":true,"username":"Tortle"}]"#;
        let players: Vec<PlayerSummary> = serde_json::from_str(json).unwrap();
        assert_eq!(players, vec![PlayerSummary { entity_id: "648518346633885895".into(), username: "Tortle".into(), signed_in: Some(true) }]);
    }

    #[test]
    fn food_regen_and_label() {
        let buff = |name: &str, is_regen, ends_at| FoodBuff { name: name.into(), is_regen, ends_at, duration: 100.0 };
        let food = FoodState {
            buffs: vec![
                buff("Level 2 Deluxe Action Speed", false, 200.0),
                buff("Level 3 Food Regen", true, 100.0),
                buff("Mining Pie", false, 200.0),
            ],
        };
        assert_eq!(food.regen_ends_at(50.0), Some(100.0));
        assert_eq!(food.regen_fraction(50.0), Some(0.5));
        assert_eq!(food.summary(50.0), "Regen 3 · Deluxe Action Speed 2 · Mining Pie");
        // The extra buffs alone don't count as food regen.
        assert_eq!(food.regen_ends_at(150.0), None);
        assert_eq!(food.summary(150.0), "Deluxe Action Speed 2 · Mining Pie");
    }
}

/// All timers: "1h 05m", "4m", or "42s" (seconds only under a minute;
/// user preference).
pub fn duration_string(seconds: f64) -> String {
    let total = seconds.max(0.0).ceil() as i64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        // Round up so it never shows "0m" with time left.
        match m + (s > 0) as i64 {
            60 => "1h 00m".to_string(),
            rounded => format!("{rounded}m"),
        }
    } else {
        format!("{s}s")
    }
}
