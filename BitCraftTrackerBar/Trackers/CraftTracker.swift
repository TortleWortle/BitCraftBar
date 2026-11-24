import Foundation
import Observation

/// Tracks the player's crafts and their progress. Craft list comes from the
/// REST API; live progress from `progressive_action_state:<craft id>`.
@Observable
final class CraftTracker {
    struct Craft: Identifiable {
        let id: String
        var name: String
        var building: String?
        var progress: Int
        var total: Int
        /// When progress last changed.
        var lastProgressAt: Date?

        var fraction: Double { total > 0 ? min(1, Double(progress) / Double(total)) : 0 }
        var isComplete: Bool { progress >= total }

        /// "42%". Rounded down so it never shows 100% before the craft is done.
        var percentString: String { "\(Int((fraction * 100).rounded(.down)))%" }
    }

    /// A craft counts as active if it progressed within this long.
    let activeWindow: TimeInterval

    private(set) var crafts: [String: Craft] = [:]
    /// The craft that last made live progress. Kept after it completes (REST
    /// drops completed crafts from `crafts`), for the "craft finished/idle"
    /// status. Cleared if it disappears without completing (cancelled).
    private(set) var lastWorkedCraft: Craft?

    /// Called once when a tracked craft reaches 100%.
    @ObservationIgnored var onCraftCompleted: ((Craft) -> Void)?
    @ObservationIgnored private var completionReported: Set<String> = []

    init(activeWindow: TimeInterval = 30) {
        self.activeWindow = activeWindow
    }

    /// Crafts sorted with the most recently worked on first.
    var sortedCrafts: [Craft] {
        crafts.values.sorted {
            ($0.lastProgressAt ?? .distantPast, $0.id) > ($1.lastProgressAt ?? .distantPast, $1.id)
        }
    }

    /// The craft currently being worked on, if any.
    func activeCraft(now: Date = .now) -> Craft? {
        guard let craft = sortedCrafts.first,
              let last = craft.lastProgressAt,
              now.timeIntervalSince(last) <= activeWindow,
              !craft.isComplete
        else { return nil }
        return craft
    }

    func reset() {
        crafts = [:]
        lastWorkedCraft = nil
        completionReported = []
    }

    private func reportCompletionIfNeeded(_ craft: Craft) {
        guard craft.isComplete, !completionReported.contains(craft.id) else { return }
        completionReported.insert(craft.id)
        onCraftCompleted?(craft)
    }

    /// Merge the craft list from the REST API. Returns ids of incomplete crafts.
    @discardableResult
    func update(from response: BitJitaAPI.CraftsResponse) -> [String] {
        let itemNames = Dictionary(response.items.map { ($0.id, $0.name) }, uniquingKeysWith: { a, _ in a })
        var seen: Set<String> = []

        // Crafts we were tracking that REST now reports as completed (in case
        // the live update that hit 100% was missed).
        for result in response.craftResults where result.completed {
            if var craft = crafts[result.entityId] {
                craft.progress = max(craft.progress, craft.total)
                reportCompletionIfNeeded(craft)
                if lastWorkedCraft?.id == craft.id { lastWorkedCraft = craft }
            }
        }

        for result in response.craftResults where !result.completed {
            seen.insert(result.entityId)
            let name = result.craftedItem.first.flatMap { itemNames[$0.itemId] } ?? "Craft"
            if var existing = crafts[result.entityId] {
                existing.name = name
                existing.building = result.buildingName
                existing.total = result.totalActionsRequired
                // Live updates are fresher than REST; never move backwards.
                existing.progress = max(existing.progress, result.progress)
                crafts[result.entityId] = existing
            } else {
                crafts[result.entityId] = Craft(
                    id: result.entityId,
                    name: name,
                    building: result.buildingName,
                    progress: result.progress,
                    total: result.totalActionsRequired
                )
            }
        }

        // Drop crafts that were collected or cancelled.
        for id in crafts.keys where !seen.contains(id) {
            crafts[id] = nil
        }
        if let last = lastWorkedCraft, !last.isComplete, !seen.contains(last.id) {
            lastWorkedCraft = nil
        }
        return Array(seen)
    }

    func handle(_ state: ProgressiveActionState, at now: Date = .now) {
        guard var craft = crafts[state.entityId] else { return }
        guard state.progress != craft.progress else { return }

        craft.progress = state.progress
        craft.lastProgressAt = now
        crafts[state.entityId] = craft
        lastWorkedCraft = craft
        reportCompletionIfNeeded(craft)
    }
}
