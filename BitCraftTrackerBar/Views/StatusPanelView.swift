import SwiftUI

/// The popover shown when clicking the menu bar item.
struct StatusPanelView: View {
    let model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Circle()
                    .fill(model.monitor.overall.color)
                    .frame(width: 12, height: 12)
                Text("BitCraft Tracker")
                    .font(.headline)
                Spacer()
                Text(connectionText)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            VStack(alignment: .leading, spacing: 6) {
                ForEach(model.monitor.sortedReports) { report in
                    HStack(spacing: 8) {
                        Circle()
                            .fill(report.status.color)
                            .frame(width: 8, height: 8)
                        Text(report.source)
                            .fontWeight(.medium)
                        Text(report.message)
                            .foregroundStyle(.secondary)
                    }
                    .font(.callout)
                }
            }

            Divider()

            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 4) {
                GridRow {
                    Text("Stamina").foregroundStyle(.secondary)
                    HStack {
                        ProgressView(value: model.stamina.fraction ?? 0)
                            .tint(model.monitor.overall.color)
                            .frame(minWidth: 40, maxWidth: .infinity)
                        Text(staminaText)
                            .fixedSize()
                    }
                }
                GridRow {
                    Text("Satiation").foregroundStyle(.secondary)
                    Text(model.satiation.satiation.map { String(format: "%.1f", $0) } ?? "–")
                        .foregroundStyle(satiationColor)
                }
                GridRow {
                    Text("Total XP").foregroundStyle(.secondary)
                    Text(model.experience.totalExperience.map { $0.formatted() } ?? "–")
                }
                GridRow {
                    Text("Last skill").foregroundStyle(.secondary)
                    Text(model.experience.lastSkillName ?? "–")
                }
                GridRow {
                    Text("Last gain").foregroundStyle(.secondary)
                    lastGainText
                }
            }
            .font(.callout)
            .monospacedDigit()

            if !model.plants.tiers.isEmpty {
                Divider()
                plantsSection
            }

            if !model.passiveCrafts.crafts.isEmpty {
                Divider()
                passiveCraftsSection
            }

            if !model.crafts.sortedCrafts.isEmpty {
                Divider()
                VStack(alignment: .leading, spacing: 8) {
                    Text("Crafts").font(.subheadline.weight(.semibold))
                    ForEach(model.crafts.sortedCrafts) { craft in
                        craftRow(craft)
                    }
                }
            }

            Divider()

            CharacterPicker(model: model)

            HStack {
                Spacer()
                Button("Quit") { NSApplication.shared.terminate(nil) }
            }
        }
        .padding()
        .frame(width: 300)
    }

    @ViewBuilder
    private var lastGainText: some View {
        if let date = model.experience.lastGainAt {
            HStack(spacing: 4) {
                if model.experience.lastGainAmount > 0 {
                    Text("+\(model.experience.lastGainAmount)")
                }
                Text(date, style: .relative)
                Text("ago")
            }
        } else {
            Text("–")
        }
    }

    private var plantsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Plants").font(.subheadline.weight(.semibold))

            // Growth tracker: only tiers with plants currently growing.
            if !model.plants.growingTiers.isEmpty {
                Grid(alignment: .leading, horizontalSpacing: 8, verticalSpacing: 4) {
                    ForEach(model.plants.growingTiers) { tier in
                        GridRow {
                            Text("T\(tier.tier)").fontWeight(.medium)
                            Text("\(tier.growing) growing")
                                .foregroundStyle(.secondary)
                            Text(timeLeftText(tier))
                                .gridColumnAlignment(.trailing)
                                .frame(maxWidth: .infinity, alignment: .trailing)
                        }
                    }
                }
                .monospacedDigit()
            }

            // Needs attention, listed separately.
            plantNeedRow("Needs fertilizer", .orange) { $0.needsFertilizer }
            plantNeedRow("Needs water", .blue) { $0.needsWater }
            plantNeedRow("Ready to harvest", .green) { $0.grown }
        }
        .font(.callout)
    }

    @ViewBuilder
    private func plantNeedRow(_ label: String, _ color: Color, count: (PlantTracker.TierSummary) -> Int) -> some View {
        let tiers = model.plants.tiers.filter { count($0) > 0 }
        if !tiers.isEmpty {
            HStack(spacing: 6) {
                Circle().fill(color).frame(width: 7, height: 7)
                Text(label)
                Spacer()
                Text(tiers.map { "T\($0.tier) ×\(count($0))" }.joined(separator: ", "))
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
        }
    }

    /// Time until every growing plant in the tier is grown.
    private func timeLeftText(_ tier: PlantTracker.TierSummary) -> String {
        guard let end = tier.allGrownAt else { return "–" }
        let remaining = end.timeIntervalSince(model.now)
        if remaining <= 0 { return "any moment" }
        return (tier.isEstimate ? "~" : "") + Self.durationString(remaining) + " left"
    }

    /// "1h 05m" or "4m 12s".
    static func durationString(_ interval: TimeInterval) -> String {
        let total = max(0, Int(interval.rounded(.up)))
        let h = total / 3600, m = (total % 3600) / 60, s = total % 60
        return h > 0 ? String(format: "%dh %02dm", h, m) : String(format: "%dm %02ds", m, s)
    }

    private var passiveCraftsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Passive crafts").font(.subheadline.weight(.semibold))
            ForEach(model.passiveCrafts.groups(now: model.now)) { group in
                VStack(alignment: .leading, spacing: 1) {
                    HStack {
                        Text(group.name).fontWeight(.medium)
                        Spacer()
                        Text(passiveStatusText(group))
                            .foregroundStyle(group.running == 0 ? HealthStatus.healthy.color : .secondary)
                            .monospacedDigit()
                    }
                    Text(passiveDetailText(group))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .font(.callout)
    }

    /// Right-hand status: time until the group is done, or "Ready".
    private func passiveStatusText(_ group: PassiveCraftTracker.Group) -> String {
        guard group.running > 0 else { return "Ready" }
        guard let end = group.allDoneAt, !group.hasUnknownEnd else { return "Running" }
        let remaining = end.timeIntervalSince(model.now)
        return remaining > 0 ? Self.durationString(remaining) + " left" : "any moment"
    }

    private func passiveDetailText(_ group: PassiveCraftTracker.Group) -> String {
        var parts = [group.building]
        if group.running > 0 { parts.append("\(group.running) running") }
        if group.ready > 0 { parts.append("\(group.ready) ready to collect") }
        return parts.joined(separator: " · ")
    }

    private func craftRow(_ craft: CraftTracker.Craft) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(craft.name).fontWeight(.medium)
                Spacer()
                if let status = craftStatusText(craft) {
                    Text(status).foregroundStyle(.secondary)
                }
                Text(craft.percentString)
                    .monospacedDigit()
            }
            ProgressView(value: craft.fraction)
            Text("\(craft.building ?? "") · \(craft.progress.formatted()) / \(craft.total.formatted())")
                .font(.caption)
                .foregroundStyle(.secondary)
                .monospacedDigit()
        }
        .font(.callout)
    }

    /// "Done" / "Paused", or nil while actively being worked on.
    private func craftStatusText(_ craft: CraftTracker.Craft) -> String? {
        if craft.isComplete { return "Done" }
        guard let last = craft.lastProgressAt,
              model.now.timeIntervalSince(last) <= model.crafts.activeWindow
        else { return "Paused" }
        return nil
    }

    private var satiationColor: Color {
        guard let value = model.satiation.satiation,
              value <= model.satiation.alertThreshold
        else { return .primary }
        return HealthStatus.alert.color
    }

    private var staminaText: String {
        guard let current = model.stamina.stamina else { return "–" }
        let max = model.stamina.maxStamina.map { "\(Int($0))" } ?? "?"
        return "\(Int(current)) / \(max)"
    }

    private var connectionText: String {
        switch model.client.state {
        case .connected: "Connected"
        case .connecting: "Connecting…"
        case .disconnected: "Disconnected"
        }
    }
}
