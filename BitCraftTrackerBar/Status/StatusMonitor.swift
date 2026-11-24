import Foundation
import Observation

/// Collects status reports from any number of independent sources
/// (experience, stamina, connection, ...) and exposes the worst one as the
/// overall status. Sources don't know about each other; each one just
/// reports its own condition under its own key.
@Observable
final class StatusMonitor {
    struct Report: Identifiable, Equatable {
        let source: String
        var status: HealthStatus
        var message: String

        var id: String { source }
    }

    private(set) var reports: [String: Report] = [:]

    /// Called whenever a source's report changes: (previous status, new report).
    /// `previous` is nil the first time a source reports (or after `clear`).
    @ObservationIgnored var onChange: ((_ previous: HealthStatus?, _ report: Report) -> Void)?

    /// The most severe status across all sources. Healthy when nothing has reported.
    var overall: HealthStatus {
        reports.values.map(\.status).max() ?? .healthy
    }

    /// Reports sorted with the most severe first, for display.
    var sortedReports: [Report] {
        reports.values.sorted {
            $0.status != $1.status ? $0.status > $1.status : $0.source < $1.source
        }
    }

    /// Non-healthy reports, most severe first.
    var activeIssues: [Report] {
        sortedReports.filter { $0.status != .healthy }
    }

    func report(_ source: String, _ status: HealthStatus, _ message: String) {
        let new = Report(source: source, status: status, message: message)
        let previous = reports[source]
        if previous != new {
            reports[source] = new
            onChange?(previous?.status, new)
        }
    }

    func clear(_ source: String) {
        reports[source] = nil
    }
}
