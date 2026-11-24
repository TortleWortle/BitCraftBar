import Foundation
import Observation

/// Tracks passive crafts (tanning tubs, kilns, smelters, looms, ...).
///
/// BitJita only reports a start `timestamp` and a `status` per craft; the end
/// time is computed as start + the recipe's `timeRequirement` (looked up once
/// per recipe via the crafted item). A craft counts as finished when BitJita
/// says `complete` or when its computed end time has passed.
@Observable
final class PassiveCraftTracker {
    struct Craft: Identifiable {
        let id: String
        let name: String
        let building: String
        let recipeId: Int
        let itemId: Int?
        let startedAt: Date?
        let reportedComplete: Bool
        var duration: TimeInterval?

        var endsAt: Date? {
            guard let startedAt, let duration else { return nil }
            return startedAt.addingTimeInterval(duration)
        }

        func isFinished(at now: Date) -> Bool {
            reportedComplete || (endsAt.map { $0 <= now } ?? false)
        }
    }

    /// Crafts of the same item in the same building, for display.
    struct Group: Identifiable {
        let building: String
        let name: String
        var running = 0
        var ready = 0
        /// When the last running craft in the group finishes, if known.
        var allDoneAt: Date?
        /// True if some running craft has no known end time.
        var hasUnknownEnd = false

        var id: String { building + "|" + name }
    }

    private(set) var crafts: [Craft] = []

    /// Called with all crafts that finished since the last check (batched so
    /// a building full of crafts finishing together is one notification).
    @ObservationIgnored var onFinished: (([Craft]) -> Void)?

    @ObservationIgnored private var durations: [Int: TimeInterval] = [:]
    /// Ids already finished (or finished when first seen), so each finish is
    /// reported once and pre-existing finished crafts never are.
    @ObservationIgnored private var finishedIds: Set<String> = []
    /// False until the first fetch was applied (for a loading placeholder).
    private(set) var hasLoaded = false

    func reset() {
        crafts = []
        finishedIds = []
        hasLoaded = false
    }

    func groups(now: Date) -> [Group] {
        var byKey: [String: Group] = [:]
        for craft in crafts {
            let key = craft.building + "|" + craft.name
            var group = byKey[key] ?? Group(building: craft.building, name: craft.name)
            if craft.isFinished(at: now) {
                group.ready += 1
            } else {
                group.running += 1
                if let end = craft.endsAt {
                    group.allDoneAt = max(group.allDoneAt ?? end, end)
                } else {
                    group.hasUnknownEnd = true
                }
            }
            byKey[key] = group
        }
        // Running first, then by building/name.
        return byKey.values.sorted {
            ($0.running == 0 ? 1 : 0, $0.building, $0.name) < ($1.running == 0 ? 1 : 0, $1.building, $1.name)
        }
    }

    /// Recipes (with an item to look them up by) whose duration isn't known yet.
    func missingDurations(in response: BitJitaAPI.PassiveCraftsResponse) -> [(recipeId: Int, itemId: Int)] {
        var seen: Set<Int> = []
        return response.craftResults.compactMap { result in
            guard durations[result.recipeId] == nil,
                  let itemId = result.craftedItem.first?.itemId,
                  seen.insert(result.recipeId).inserted
            else { return nil }
            return (result.recipeId, itemId)
        }
    }

    func setDuration(_ duration: TimeInterval, forRecipe recipeId: Int) {
        durations[recipeId] = duration
    }

    func update(from response: BitJitaAPI.PassiveCraftsResponse, now: Date = .now) {
        let itemNames = Dictionary(response.items.map { ($0.id, $0.name) }, uniquingKeysWith: { a, _ in a })

        crafts = response.craftResults.map { result in
            let itemId = result.craftedItem.first?.itemId
            let itemName = itemId.flatMap { itemNames[$0] }
            return Craft(
                id: result.entityId,
                name: Self.displayName(recipeName: result.recipeName, itemName: itemName),
                building: result.buildingName ?? "Building",
                recipeId: result.recipeId,
                itemId: itemId,
                startedAt: result.timestamp?.date,
                reportedComplete: result.status?.lowercased() == "complete",
                duration: durations[result.recipeId]
            )
        }

        if !hasLoaded {
            // Already finished before we started watching: don't notify.
            finishedIds.formUnion(crafts.filter { $0.isFinished(at: now) }.map(\.id))
            hasLoaded = true
        }
        // Forget ids that are gone (collected).
        finishedIds.formIntersection(crafts.map(\.id))
        tick(now: now)
    }

    /// Call periodically: reports crafts whose end time passed.
    func tick(now: Date = .now) {
        let newlyFinished = crafts.filter { $0.isFinished(at: now) && !finishedIds.contains($0.id) }
        guard !newlyFinished.isEmpty else { return }
        finishedIds.formUnion(newlyFinished.map(\.id))
        onFinished?(newlyFinished)
    }

    /// "Weave {0}" + "Rough Cloth Strip" → "Rough Cloth Strip". Prefer the
    /// item name; fall back to the recipe name with the placeholder removed.
    static func displayName(recipeName: String?, itemName: String?) -> String {
        if let itemName { return itemName }
        guard let recipeName else { return "Passive craft" }
        return recipeName.replacingOccurrences(of: " {0}", with: "")
            .replacingOccurrences(of: "{0}", with: "")
    }
}
