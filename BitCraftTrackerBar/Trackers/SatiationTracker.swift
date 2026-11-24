import Foundation
import Observation

/// Tracks satiation and reports `.alert` to the status monitor when it drops
/// to `alertThreshold` or below (likely means no food buff is active).
@Observable
final class SatiationTracker {
    static let source = "Satiation"

    let alertThreshold: Double
    private(set) var satiation: Double?
    /// From the stats API; 100 until known (BitJita's own default).
    private(set) var maxSatiation: Double = 100

    /// 0...1, or nil before any data.
    var fraction: Double? {
        guard let satiation, maxSatiation > 0 else { return nil }
        return min(1, max(0, satiation / maxSatiation))
    }

    @ObservationIgnored private let monitor: StatusMonitor

    init(monitor: StatusMonitor, alertThreshold: Double = 15) {
        self.monitor = monitor
        self.alertThreshold = alertThreshold
    }

    func setReportedMax(_ value: Double?) {
        guard let value, value > 0, value != maxSatiation else { return }
        maxSatiation = value
    }

    func reset() {
        satiation = nil
        maxSatiation = 100
        monitor.clear(Self.source)
    }

    func handle(_ state: SatiationState) {
        satiation = state.satiation
        reportCurrent()
    }

    /// (Re-)report the last known value, e.g. after the report was cleared
    /// while the player was offline.
    func reportCurrent() {
        guard let satiation else { return }
        if satiation <= alertThreshold {
            monitor.report(Self.source, .alert, "Low satiation (\(Int(satiation)))")
        } else {
            monitor.report(Self.source, .healthy, "Satiation \(Int(satiation))")
        }
    }
}
