//! Status reports from independent sources; the overall status is the worst.
//! Sources don't know about each other (see AGENTS.md, "Status is decoupled
//! from data").

use std::collections::BTreeMap;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HealthStatus {
    Healthy,
    Warning,
    Alert,
}

impl HealthStatus {
    /// RGB used everywhere (tray icon, popup). Green / orange / red.
    pub fn rgb(self) -> u32 {
        match self {
            HealthStatus::Healthy => 0x34c759,
            HealthStatus::Warning => 0xff9500,
            HealthStatus::Alert => 0xff3b30,
        }
    }
}

/// Stamina bar color for a fraction of max stamina (user request): green
/// above 30%, fading to orange at 20%, then to red at 5% and below. Color
/// stops are the status colors, blended linearly between them.
pub fn stamina_rgb(fraction: f64) -> u32 {
    const STOPS: [(f64, u32); 3] = [(0.05, 0xff3b30), (0.20, 0xff9500), (0.30, 0x34c759)];
    let f = fraction.clamp(0.0, 1.0);
    if f <= STOPS[0].0 {
        return STOPS[0].1;
    }
    for pair in STOPS.windows(2) {
        let ((lo, lo_rgb), (hi, hi_rgb)) = (pair[0], pair[1]);
        if f <= hi {
            return blend(lo_rgb, hi_rgb, (f - lo) / (hi - lo));
        }
    }
    STOPS[STOPS.len() - 1].1
}

/// Food bar color (user request): orange under 5 minutes left, else green.
/// No food at all is red (the caller shows a full red bar, so it's visible).
pub fn food_rgb(time_left: f64) -> u32 {
    if time_left < 5.0 * 60.0 { HealthStatus::Warning.rgb() } else { HealthStatus::Healthy.rgb() }
}

/// Health bar color (user request): red at 15% or lower, orange at 50% or
/// lower, else the usual pink. Unknown health (0) keeps the pink.
pub fn health_rgb(fraction: f64) -> u32 {
    match fraction {
        f if f <= 0.0 => 0xff6482,
        f if f <= 0.15 => HealthStatus::Alert.rgb(),
        f if f <= 0.50 => HealthStatus::Warning.rgb(),
        _ => 0xff6482,
    }
}

/// Linear blend of two RGB colors per channel; `t` = 0 → `a`, 1 → `b`.
fn blend(a: u32, b: u32, t: f64) -> u32 {
    let channel = |shift: u32| {
        let (ca, cb) = (((a >> shift) & 0xff) as f64, ((b >> shift) & 0xff) as f64);
        ((ca + (cb - ca) * t).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub source: String,
    pub status: HealthStatus,
    pub message: String,
}

/// A source changed: (previous report, new report). `previous` is None the
/// first time a source reports (or after `clear`).
pub type Change = (Option<Report>, Report);

#[derive(Default)]
pub struct StatusMonitor {
    reports: BTreeMap<String, Report>,
    /// Changes since the last `take_changes`, for notifications.
    changes: Vec<Change>,
}

impl StatusMonitor {
    pub fn report(&mut self, source: &str, status: HealthStatus, message: impl Into<String>) {
        let new = Report { source: source.to_string(), status, message: message.into() };
        let previous = self.reports.get(source).cloned();
        if previous.as_ref() != Some(&new) {
            self.reports.insert(source.to_string(), new.clone());
            self.changes.push((previous, new));
        }
    }

    pub fn get(&self, source: &str) -> Option<&Report> {
        self.reports.get(source)
    }

    pub fn clear(&mut self, source: &str) {
        self.reports.remove(source);
    }

    pub fn take_changes(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.changes)
    }

    /// Worst status; healthy when nothing reported.
    pub fn overall(&self) -> HealthStatus {
        self.reports.values().map(|r| r.status).max().unwrap_or(HealthStatus::Healthy)
    }

    /// Most severe first, then by source.
    pub fn sorted_reports(&self) -> Vec<Report> {
        let mut reports: Vec<Report> = self.reports.values().cloned().collect();
        reports.sort_by(|a, b| b.status.cmp(&a.status).then(a.source.cmp(&b.source)));
        reports
    }

    /// Non-healthy reports, most severe first.
    pub fn active_issues(&self) -> Vec<Report> {
        self.sorted_reports().into_iter().filter(|r| r.status != HealthStatus::Healthy).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamina_color_stops() {
        assert_eq!(stamina_rgb(1.0), HealthStatus::Healthy.rgb());
        assert_eq!(stamina_rgb(0.30), HealthStatus::Healthy.rgb());
        assert_eq!(stamina_rgb(0.20), HealthStatus::Warning.rgb());
        assert_eq!(stamina_rgb(0.05), HealthStatus::Alert.rgb());
        assert_eq!(stamina_rgb(0.0), HealthStatus::Alert.rgb());
        // In between: a blend, neither stop.
        let mid = stamina_rgb(0.125);
        assert_ne!(mid, HealthStatus::Warning.rgb());
        assert_ne!(mid, HealthStatus::Alert.rgb());
        assert_eq!(mid >> 16, 0xff);
    }

    #[test]
    fn food_and_health_colors() {
        assert_eq!(food_rgb(4.0 * 60.0), HealthStatus::Warning.rgb());
        assert_eq!(food_rgb(5.0 * 60.0), HealthStatus::Healthy.rgb());
        assert_eq!(health_rgb(1.0), 0xff6482);
        assert_eq!(health_rgb(0.50), HealthStatus::Warning.rgb());
        assert_eq!(health_rgb(0.15), HealthStatus::Alert.rgb());
        assert_eq!(health_rgb(0.0), 0xff6482);
    }
}
