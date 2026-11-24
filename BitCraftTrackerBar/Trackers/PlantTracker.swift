import Foundation
import Observation

/// Tracks the player's crops per tier.
///
/// Crop lifecycle (from the BitJita placeable catalog):
/// `Planted …` → (Fertilizer) → `Fertilized Planted …` → (Water) →
/// `Growing …` (timer) → `Grown …`.
/// Data comes from the REST placeables endpoint (polled).
@Observable
final class PlantTracker {
    enum Stage {
        case needsFertilizer
        case needsWater
        case growing
        case grown
    }

    struct TierSummary: Identifiable {
        let tier: Int
        var total = 0
        var needsFertilizer = 0
        var needsWater = 0
        var growing = 0
        var grown = 0
        /// Completion time of each growing plant whose end time is known.
        var growthEnds: [Date] = []
        /// True if any of `growthEnds` is an estimate rather than reported.
        var isEstimate = false

        var id: Int { tier }
        var isAllGrown: Bool { total > 0 && grown == total }
        /// When the last growing plant in this tier is done.
        var allGrownAt: Date? { growthEnds.max() }
    }

    private(set) var tiers: [TierSummary] = []

    /// Called when every growing plant in a tier has finished growing
    /// (between two polls: growing went to 0 and grown went up).
    @ObservationIgnored var onTierFinishedGrowing: ((TierSummary) -> Void)?
    @ObservationIgnored private var hasLoaded = false

    /// Growth duration per placeable id (from the catalog), for estimates when
    /// the instance doesn't report an end time.
    @ObservationIgnored private var growthDurations: [Int: TimeInterval] = [:]

    var growingTiers: [TierSummary] { tiers.filter { $0.growing > 0 } }

    /// Short text for the menu bar, e.g. "T2 grown; T1 needs water", or nil.
    var menuBarSummary: String? {
        var parts: [String] = []
        func tierList(_ tiers: [TierSummary]) -> String {
            tiers.map { "T\($0.tier)" }.joined(separator: ", ")
        }
        let grown = tiers.filter(\.isAllGrown)
        let fertilizer = tiers.filter { $0.needsFertilizer > 0 }
        let water = tiers.filter { $0.needsWater > 0 }
        if !grown.isEmpty { parts.append("\(tierList(grown)) grown") }
        if !fertilizer.isEmpty { parts.append("\(tierList(fertilizer)) needs fertilizer") }
        if !water.isEmpty { parts.append("\(tierList(water)) needs water") }
        return parts.isEmpty ? nil : parts.joined(separator: "; ")
    }

    func reset() {
        tiers = []
        hasLoaded = false
    }

    /// Placeable ids of growing plants with no reported end time and no
    /// cached duration; fetch these from the catalog and pass them to
    /// `setGrowthDuration`.
    func missingDurations(in response: BitJitaAPI.PlaceablesResponse) -> Set<Int> {
        Set(response.instances.compactMap { instance in
            guard Self.stage(of: instance) == .growing,
                  Self.reportedEnd(of: instance) == nil,
                  growthDurations[instance.placeableId] == nil
            else { return nil }
            return instance.placeableId
        })
    }

    func setGrowthDuration(_ duration: TimeInterval, for placeableId: Int) {
        growthDurations[placeableId] = duration
    }

    func update(from response: BitJitaAPI.PlaceablesResponse) {
        var byTier: [Int: TierSummary] = [:]
        for instance in response.instances {
            guard let stage = Self.stage(of: instance) else { continue }
            let tier = instance.placeable.tier
            var summary = byTier[tier] ?? TierSummary(tier: tier)
            summary.total += 1
            switch stage {
            case .needsFertilizer: summary.needsFertilizer += 1
            case .needsWater: summary.needsWater += 1
            case .grown: summary.grown += 1
            case .growing:
                summary.growing += 1
                if let end = Self.reportedEnd(of: instance) {
                    summary.growthEnds.append(end)
                } else if let duration = growthDurations[instance.placeableId],
                          let start = instance.updatedAt?.date {
                    // Estimate: assumes `updatedAt` is when it started growing.
                    summary.growthEnds.append(start.addingTimeInterval(duration))
                    summary.isEstimate = true
                }
            }
            byTier[tier] = summary
        }
        let previous = Dictionary(uniqueKeysWithValues: tiers.map { ($0.tier, $0) })
        tiers = byTier.values.sorted { $0.tier < $1.tier }

        // No notifications for the state found on the first load.
        if hasLoaded {
            for tier in tiers {
                guard let before = previous[tier.tier] else { continue }
                if before.growing > 0, tier.growing == 0, tier.grown > before.grown {
                    onTierFinishedGrowing?(tier)
                }
            }
        }
        hasLoaded = true
    }

    static func reportedEnd(of instance: BitJitaAPI.PlaceablesResponse.Instance) -> Date? {
        (instance.growthEndTimestamp ?? instance.nextStateAt)?.date
    }

    /// Crop stage, or nil if this placeable isn't a crop.
    static func stage(of instance: BitJitaAPI.PlaceablesResponse.Instance) -> Stage? {
        let name = instance.placeable.name
        let verbs = Set((instance.availableInteractions ?? []).map { $0.verbPhrase.lowercased() })

        if name.hasPrefix("Grown ") { return .grown }
        if name.hasPrefix("Growing ") { return .growing }
        // Prefer the available action; fall back to the name.
        if verbs.contains("fertilizer") { return .needsFertilizer }
        if verbs.contains("water") { return .needsWater }
        if name.hasPrefix("Fertilized Planted ") { return .needsWater }
        if name.hasPrefix("Planted ") { return .needsFertilizer }
        return nil
    }
}
