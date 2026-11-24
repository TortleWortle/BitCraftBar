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
///
/// If the last thing done was a craft (see `lastActionWasCraft`):
/// - it completed → warning "Craft finished" (immediately, even while
///   resting), until something else gains XP,
/// - it still needs progress and the player is idle → alert
///   "Craft idle (N%)" instead of the generic idle messages. Resting still
///   shows "Resting".
struct ActivityCheck {
    static let source = "Activity"

    /// Long enough to bridge gaps between XP ticks while stamina regenerates
    /// mid-activity, so short pauses don't flip the status.
    var experienceWindow: TimeInterval = 20
    var regenWindow: TimeInterval = 10
    /// XP from a craft action arrives around the same time as its progress
    /// event; XP more than this after the last craft progress means the
    /// player moved on to something else (e.g. a new craft not tracked yet).
    var craftXPTolerance: TimeInterval = 5
    /// Stamina fraction at which the player is ready to continue, per skill.
    /// Skills not listed need full stamina.
    var readyFractionBySkill: [Skill: Double] = [
        .fishing: 0.2,
    ]

    func evaluate(experience: ExperienceTracker, stamina: StaminaTracker, lastCraft: CraftTracker.Craft?, monitor: StatusMonitor, now: Date = .now) {
        let ready = readyFraction(for: experience.lastSkillId)
        let regenSinceXP: Bool = {
            guard let regen = stamina.lastRegenAt else { return false }
            guard let gain = experience.lastGainAt else { return true }
            return regen > gain
        }()
        let gainedXP = experience.hasGained(within: experienceWindow, now: now) && !regenSinceXP
        let craft = lastCraft.flatMap { lastActionWasCraft($0, experience: experience) ? $0 : nil }
        let idleCraftMessage = craft.map { "Craft idle (\($0.percentString))" }

        if craft?.isComplete == true {
            monitor.report(Self.source, .warning, "Craft finished")
        } else if !gainedXP && stamina.isAtLeast(ready) {
            monitor.report(Self.source, .alert, idleCraftMessage ?? (ready >= 1
                ? "Idle: stamina full"
                : "Idle: stamina above \(Int(ready * 100))%"))
        } else if stamina.isRegenerating(within: regenWindow, now: now) {
            monitor.report(Self.source, .warning, "Resting")
        } else if gainedXP {
            monitor.report(Self.source, .healthy, "Gaining experience")
        } else if let idleCraftMessage {
            monitor.report(Self.source, .alert, idleCraftMessage)
        } else if experience.lastGainAt == nil {
            monitor.report(Self.source, .alert, "No XP or regen yet")
        } else {
            monitor.report(Self.source, .alert, "Idle: no XP, no regen")
        }
    }

    /// The craft's last progress is at least as recent as the last XP gain
    /// (within `craftXPTolerance`).
    private func lastActionWasCraft(_ craft: CraftTracker.Craft, experience: ExperienceTracker) -> Bool {
        guard let progressAt = craft.lastProgressAt else { return false }
        guard let gainAt = experience.lastGainAt else { return true }
        return progressAt >= gainAt.addingTimeInterval(-craftXPTolerance)
    }

    private func readyFraction(for skillId: Int?) -> Double {
        skillId.flatMap(Skill.init(rawValue:)).flatMap { readyFractionBySkill[$0] } ?? 1
    }
}
