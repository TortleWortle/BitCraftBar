import Foundation
import Observation

/// Tracks satiation and reports `.alert` to the status monitor when it drops
/// to `alertThreshold` or below (likely means no food buff is active).
@Observable
final class SatiationTracker {
    static let source = "Satiation"

    let alertThreshold: Double
    private(set) var satiation: Double?

    @ObservationIgnored private let monitor: StatusMonitor

    init(monitor: StatusMonitor, alertThreshold: Double = 15) {
        self.monitor = monitor
        self.alertThreshold = alertThreshold
    }

    func reset() {
        satiation = nil
        monitor.clear(Self.source)
    }

    func handle(_ state: SatiationState) {
        satiation = state.satiation
        if state.satiation <= alertThreshold {
            monitor.report(Self.source, .alert, "Low satiation (\(Int(state.satiation)))")
        } else {
            monitor.report(Self.source, .healthy, "Satiation \(Int(state.satiation))")
        }
    }
}
