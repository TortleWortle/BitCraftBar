import Foundation
import Observation

/// Tracks experience updates. Purely a data source; status rules live in
/// the checks (see `ActivityCheck`).
@Observable
final class ExperienceTracker {
    private(set) var lastGainAt: Date?
    private(set) var lastGainAmount = 0
    /// Skill that received the most experience in the latest gain.
    private(set) var lastSkillId: Int?
    private(set) var totalExperience: Int?

    var lastSkillName: String? { lastSkillId.map { Skill.name(for: $0) } }

    @ObservationIgnored private var stacks: [Int: Int] = [:]

    func reset() {
        stacks = [:]
        lastGainAt = nil
        lastGainAmount = 0
        lastSkillId = nil
        totalExperience = nil
    }

    func handle(_ state: ExperienceState, at now: Date = .now) {
        let newStacks = Dictionary(
            state.experienceStacks.map { ($0.skillId, $0.quantity) },
            uniquingKeysWith: max
        )

        // The server sends no snapshot on subscribe, only updates, so the
        // first update we see is itself a sign that experience changed.
        let gained: Int
        if stacks.isEmpty {
            gained = 1
            lastGainAmount = 0
        } else {
            var perSkill: [Int: Int] = [:]
            for (skill, quantity) in newStacks {
                let delta = quantity - (stacks[skill] ?? 0)
                if delta > 0 { perSkill[skill] = delta }
            }
            gained = perSkill.values.reduce(0, +)
            if gained > 0 {
                lastGainAmount = gained
                lastSkillId = perSkill.max { $0.value < $1.value }?.key
            }
        }

        stacks = newStacks
        totalExperience = newStacks.values.reduce(0, +)
        if gained > 0 { lastGainAt = now }
    }

    func hasGained(within window: TimeInterval, now: Date = .now) -> Bool {
        guard let lastGainAt else { return false }
        return now.timeIntervalSince(lastGainAt) <= window
    }
}
