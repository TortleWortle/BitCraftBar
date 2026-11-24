import Foundation
import Observation

/// Tracks stamina, when it last regenerated/drained, and max stamina.
/// Purely a data source; status rules live in the checks (see `ActivityCheck`).
@Observable
final class StaminaTracker {
    /// Fallback when max stamina hasn't been loaded from the API: estimate it
    /// as the highest value seen within this window.
    let maxWindow: TimeInterval

    private(set) var stamina: Double?
    private(set) var maxStamina: Double?
    private(set) var lastRegenAt: Date?
    /// When stamina last went down (spent on an action).
    private(set) var lastDrainAt: Date?
    /// Max stamina as reported by the BitJita stats API.
    private(set) var reportedMax: Double?

    /// 0...1, or nil before any data.
    var fraction: Double? {
        guard let stamina, let maxStamina, maxStamina > 0 else { return nil }
        return min(1, max(0, stamina / maxStamina))
    }

    /// True when stamina is at least `fraction` of the max reported by the
    /// API. Unknown (false) without an API max: the fallback estimate always
    /// equals the current value while regenerating, so it can't be trusted.
    func isAtLeast(_ fraction: Double) -> Bool {
        guard let stamina, let reportedMax else { return false }
        return stamina >= reportedMax * fraction - 0.01
    }

    var isFull: Bool { isAtLeast(1) }

    @ObservationIgnored private var samples: [(date: Date, value: Double)] = []

    init(maxWindow: TimeInterval = 10 * 60) {
        self.maxWindow = maxWindow
    }

    func reset() {
        stamina = nil
        maxStamina = nil
        reportedMax = nil
        lastRegenAt = nil
        lastDrainAt = nil
        samples = []
    }

    func handle(_ state: StaminaState, at now: Date = .now) {
        if let stamina, state.stamina > stamina {
            lastRegenAt = now
        } else if let stamina, state.stamina < stamina {
            lastDrainAt = now
        }
        stamina = state.stamina
        samples.append((now, state.stamina))
        updateMax(now: now)
    }

    func hasRegenerated(within window: TimeInterval, now: Date = .now) -> Bool {
        guard let lastRegenAt else { return false }
        return now.timeIntervalSince(lastRegenAt) <= window
    }

    /// Stamina is currently regenerating: it went up within `window` and
    /// hasn't been spent since (a drop ends regenerating immediately).
    func isRegenerating(within window: TimeInterval, now: Date = .now) -> Bool {
        guard hasRegenerated(within: window, now: now), let lastRegenAt else { return false }
        return lastDrainAt.map { lastRegenAt > $0 } ?? true
    }

    func setReportedMax(_ value: Double?, now: Date = .now) {
        reportedMax = value.flatMap { $0 > 0 ? $0 : nil }
        updateMax(now: now)
    }

    /// Call periodically so old samples age out of the max estimate.
    func tick(now: Date = .now) {
        updateMax(now: now)
    }

    private func updateMax(now: Date) {
        samples.removeAll { now.timeIntervalSince($0.date) > maxWindow }
        // The server stops sending updates once stamina is full, so always
        // include the current value even if its sample has aged out.
        let observed = samples.map(\.value) + (stamina.map { [$0] } ?? [])
        let newMax: Double?
        if let reportedMax {
            // The stats API can lag behind gear changes; never show over 100%.
            newMax = max(reportedMax, stamina ?? 0)
        } else {
            newMax = observed.max()
        }
        if newMax != maxStamina { maxStamina = newMax }
    }
}
