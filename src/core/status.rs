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
