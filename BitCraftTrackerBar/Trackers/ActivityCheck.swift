import Foundation

/// Is the character doing something useful?
/// XP only counts if gained within `experienceWindow` *and* after stamina last
/// regenerated: XP while regenerating is improbable, so once regen starts the
/// earlier XP no longer means "active".
/// - Stamina "ready", no counting XP → alert (nothing left to wait for).
///   Ready means full, or a lower fraction for skills listed in
///   `readyFractionBySkill` (e.g. fishing rarely needs full stamina).
/// - Stamina regenerating → warning ("Resting"), immediately
/// - Counting XP → healthy
/// - No XP, no regen → alert (no food active)
struct ActivityCheck {
    static let source = "Activity"

    /// Long enough to bridge gaps between XP ticks while stamina regenerates
    /// mid-activity, so short pauses don't flip the status.
    var experienceWindow: TimeInterval = 20
    var regenWindow: TimeInterval = 10
    /// Stamina fraction at which the player is ready to continue, per skill.
    /// Skills not listed need full stamina.
    var readyFractionBySkill: [Skill: Double] = [
        .fishing: 0.2,
    ]

    func evaluate(experience: ExperienceTracker, stamina: StaminaTracker, monitor: StatusMonitor, now: Date = .now) {
        let ready = readyFraction(for: experience.lastSkillId)
        let regenSinceXP: Bool = {
            guard let regen = stamina.lastRegenAt else { return false }
            guard let gain = experience.lastGainAt else { return true }
            return regen > gain
        }()
        let gainedXP = experience.hasGained(within: experienceWindow, now: now) && !regenSinceXP

        if !gainedXP && stamina.isAtLeast(ready) {
            monitor.report(Self.source, .alert, ready >= 1
                ? "Idle: stamina full"
                : "Idle: stamina above \(Int(ready * 100))%")
        } else if stamina.isRegenerating(within: regenWindow, now: now) {
            monitor.report(Self.source, .warning, "Resting")
        } else if gainedXP {
            monitor.report(Self.source, .healthy, "Gaining experience")
        } else if experience.lastGainAt == nil {
            monitor.report(Self.source, .alert, "No XP or regen yet")
        } else {
            monitor.report(Self.source, .alert, "Idle: no XP, no regen")
        }
    }

    private func readyFraction(for skillId: Int?) -> Double {
        skillId.flatMap(Skill.init(rawValue:)).flatMap { readyFractionBySkill[$0] } ?? 1
    }
}
