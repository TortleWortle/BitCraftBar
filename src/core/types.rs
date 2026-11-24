//! Data passed from the network layer to the trackers. Times are Unix
//! seconds (`f64`), see [`now`].

use serde::Deserialize;

pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// One active (non-passive) craft from REST `players/<id>/crafts`.
#[derive(Clone, Debug)]
pub struct CraftInfo {
    pub id: String,
    pub name: String,
    pub building: Option<String>,
    pub progress: i64,
    pub total: i64,
    pub completed: bool,
}

/// One passive craft (relay or REST).
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
}

impl PassiveCraft {
    pub fn ends_at(&self) -> Option<f64> {
        Some(self.started_at? + self.duration?)
    }

    pub fn is_finished(&self, now: f64) -> bool {
        self.reported_complete || self.ends_at().is_some_and(|end| end <= now)
    }
}

/// One placeable (relay or REST); only crops matter.
#[derive(Clone, Debug)]
pub struct Plant {
    pub placeable_id: i64,
    pub name: String,
    pub tier: i64,
    /// Lowercased verb phrases of the available interactions (REST only).
    pub interactions: Vec<String>,
    pub reported_end: Option<f64>,
    /// When it started growing, if known (for the estimate).
    pub started_growing_at: Option<f64>,
    /// Catalog growth duration (max), if looked up.
    pub growth_duration: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlayerSummary {
    pub entity_id: String,
    pub username: String,
    pub signed_in: Option<bool>,
    pub region_id: Option<i64>,
}
