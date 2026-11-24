import Foundation
import Observation

/// Wires the websocket client to the trackers and the status monitor.
@Observable
final class AppModel {
    static let userIdKey = "userId"
    static let usernameKey = "username"
    static let defaultUserId = "648518346633885895"

    let monitor = StatusMonitor()
    let client = BitJitaClient()
    let experience: ExperienceTracker
    let stamina: StaminaTracker
    let satiation: SatiationTracker
    let activity = ActivityCheck()
    let crafts = CraftTracker()
    let plants = PlantTracker()
    let passiveCrafts = PassiveCraftTracker()

    private(set) var userId: String
    /// Display name of the tracked character; looked up if not stored yet.
    private(set) var username: String?
    /// Updated every second so time-based UI (active/paused crafts) re-renders.
    private(set) var now = Date()

    /// The craft currently being worked on.
    var activeCraft: CraftTracker.Craft? {
        crafts.activeCraft(now: now)
    }

    @ObservationIgnored private let notifier = Notifier()
    /// Last error notification per status source, for the cooldown.
    @ObservationIgnored private var lastAlertNotification: [String: Date] = [:]
    static let alertNotificationCooldown: TimeInterval = 5 * 60

    @ObservationIgnored private var tickTask: Task<Void, Never>?
    @ObservationIgnored private var restRefreshTask: Task<Void, Never>?
    @ObservationIgnored private let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }()

    init() {
        experience = ExperienceTracker()
        stamina = StaminaTracker(monitor: monitor)
        satiation = SatiationTracker(monitor: monitor)
        userId = UserDefaults.standard.string(forKey: Self.userIdKey) ?? Self.defaultUserId
        username = UserDefaults.standard.string(forKey: Self.usernameKey)

        client.onEvent = { [weak self] channel, data in
            self?.route(channel: channel, data: data)
        }
        setUpNotifications()
        connect()
        evaluate()
        startTicking()
        startRestRefresh()
        if username == nil { lookUpUsername() }
    }

    /// Switch to tracking another character (picked via username search).
    func selectPlayer(_ player: BitJitaAPI.PlayerSummary) {
        setUsername(player.username)
        guard player.entityId != userId else { return }
        userId = player.entityId
        UserDefaults.standard.set(userId, forKey: Self.userIdKey)
        experience.reset()
        stamina.reset()
        satiation.reset()
        crafts.reset()
        plants.reset()
        passiveCrafts.reset()
        evaluate()
        connect()
        startRestRefresh()
    }

    private func setUpNotifications() {
        // Error state: a source turning red from a known non-red state. The
        // first report of a source (e.g. "no XP yet" at launch) doesn't count.
        monitor.onChange = { [weak self] previous, report in
            guard let self, report.status == .alert,
                  let previous, previous != .alert
            else { return }
            let now = Date()
            if let last = self.lastAlertNotification[report.source],
               now.timeIntervalSince(last) < Self.alertNotificationCooldown {
                return
            }
            self.lastAlertNotification[report.source] = now
            self.notifier.notify(title: "\(self.username ?? "BitCraft"): \(report.source)", body: report.message)
        }

        crafts.onCraftCompleted = { [weak self] craft in
            self?.notifier.notify(title: "Craft finished", body: "\(craft.name) at \(craft.building ?? "station") is done")
        }

        passiveCrafts.onFinished = { [weak self] finished in
            guard let self else { return }
            let groups = Dictionary(grouping: finished) { "\($0.name) at \($0.building)" }
            let body = groups.keys.sorted().map { key in
                let count = groups[key]!.count
                return count > 1 ? "\(count)× \(key)" : key
            }.joined(separator: "\n")
            self.notifier.notify(title: "Passive craft finished", body: body)
        }

        plants.onTierFinishedGrowing = { [weak self] tier in
            self?.notifier.notify(title: "Plants grown", body: "All T\(tier.tier) plants have finished growing (\(tier.grown) ready to harvest)")
        }
    }

    private func setUsername(_ name: String) {
        username = name
        UserDefaults.standard.set(name, forKey: Self.usernameKey)
    }

    private func lookUpUsername() {
        let id = userId
        Task { [weak self] in
            guard let player = try? await BitJitaAPI.player(id: id) else { return }
            guard let self, self.userId == id else { return }
            self.setUsername(player.username)
        }
    }

    private func connect() {
        client.connect(channels: [
            BitJitaChannel.experience(userId),
            BitJitaChannel.stamina(userId),
            BitJitaChannel.satiation(userId),
        ])
    }

    private func route(channel: String, data: Data) {
        do {
            if channel == BitJitaChannel.experience(userId) {
                experience.handle(try decoder.decode(EventEnvelope<ExperienceState>.self, from: data).data)
            } else if channel == BitJitaChannel.stamina(userId) {
                stamina.handle(try decoder.decode(EventEnvelope<StaminaState>.self, from: data).data)
            } else if channel == BitJitaChannel.satiation(userId) {
                satiation.handle(try decoder.decode(EventEnvelope<SatiationState>.self, from: data).data)
            } else if channel.hasPrefix(BitJitaChannel.craftPrefix) {
                crafts.handle(try decoder.decode(EventEnvelope<ProgressiveActionState>.self, from: data).data)
            }
        } catch {
            print("Failed to decode \(channel): \(error)")
        }
        evaluate()
    }

    /// Time-based checks (e.g. "no XP in the last 20s") need re-evaluating even
    /// when no messages arrive.
    private func startTicking() {
        tickTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                guard let self else { return }
                self.now = .now
                self.stamina.tick()
                self.passiveCrafts.tick(now: self.now)
                self.evaluate()
            }
        }
    }

    /// Some data only comes from REST: the craft list (followed live over the
    /// websocket once known), max stamina and placeables (plants). Poll it to
    /// pick up changes.
    private func startRestRefresh() {
        restRefreshTask?.cancel()
        let userId = userId
        restRefreshTask = Task { [weak self] in
            while !Task.isCancelled {
                do {
                    let response = try await BitJitaAPI.crafts(userId: userId)
                    guard let self, !Task.isCancelled else { return }
                    let ids = self.crafts.update(from: response)
                    self.client.subscribe(ids.map { BitJitaChannel.craft($0) })
                } catch {
                    print("Failed to fetch crafts: \(error)")
                }
                do {
                    let stats = try await BitJitaAPI.stats(userId: userId)
                    guard let self, !Task.isCancelled else { return }
                    self.stamina.setReportedMax(stats.maxStamina)
                } catch {
                    print("Failed to fetch stats: \(error)")
                }
                do {
                    let placeables = try await BitJitaAPI.placeables(userId: userId)
                    guard let self, !Task.isCancelled else { return }
                    // Growing plants without a reported end time: look up the
                    // crop's growth duration once, for an estimate.
                    for id in self.plants.missingDurations(in: placeables) {
                        if let duration = try? await BitJitaAPI.placeable(id: id).growthDuration {
                            self.plants.setGrowthDuration(duration, for: id)
                        }
                    }
                    self.plants.update(from: placeables)
                } catch {
                    print("Failed to fetch placeables: \(error)")
                }
                do {
                    let passive = try await BitJitaAPI.passiveCrafts(userId: userId)
                    guard let self, !Task.isCancelled else { return }
                    // Duration per recipe: looked up once via the crafted item.
                    for missing in self.passiveCrafts.missingDurations(in: passive) {
                        if let item = try? await BitJitaAPI.item(id: missing.itemId),
                           let recipe = item.craftingRecipes.first(where: { $0.id == missing.recipeId }) {
                            self.passiveCrafts.setDuration(recipe.timeRequirement, forRecipe: missing.recipeId)
                        }
                    }
                    self.passiveCrafts.update(from: passive)
                } catch {
                    print("Failed to fetch passive crafts: \(error)")
                }
                try? await Task.sleep(for: .seconds(60))
            }
        }
    }

    private func evaluate() {
        activity.evaluate(experience: experience, stamina: stamina, monitor: monitor)
        updateConnectionReport()
    }

    private func updateConnectionReport() {
        switch client.state {
        // Reported as healthy (not cleared) so a later disconnect counts as a
        // transition to red for notifications.
        case .connected: monitor.report("Connection", .healthy, "Connected")
        case .connecting: monitor.report("Connection", .warning, "Connecting…")
        case .disconnected: monitor.report("Connection", .alert, "Disconnected")
        }
    }
}
