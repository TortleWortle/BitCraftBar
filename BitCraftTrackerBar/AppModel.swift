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
    /// Live passive crafts + plants via BitJita's SpacetimeDB relay. While
    /// live, the REST poll for those is ignored (it lags behind).
    let relay = RelayFeed()
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

    /// True while starting up (or after switching character) until the
    /// websocket is connected and the first round of REST requests is done,
    /// so the UI doesn't jump around as data trickles in. Capped by
    /// `loadingTimeout` so it can never get stuck.
    private(set) var isLoading = true
    static let loadingTimeout: TimeInterval = 20
    @ObservationIgnored private var loadingStartedAt = Date()
    @ObservationIgnored private var initialRestDone = false

    /// The character is signed out of BitCraft. Activity/satiation rules are
    /// paused (being idle offline isn't a problem) and the UI shows "Offline".
    /// Based on BitJita's `signedIn` (polled), overridden by live events: any
    /// within `liveEventWindow` means online, since `signedIn` can lag.
    private(set) var isOffline = false
    static let liveEventWindow: TimeInterval = 30
    static let playerSource = "Player"
    /// Signed in but no live event for this long on a connection at least
    /// this old → the feed is stale; reconnect (see `refreshPlayer`).
    static let staleFeedWindow: TimeInterval = 60
    @ObservationIgnored private var reportedSignedIn: Bool?
    @ObservationIgnored private var lastLiveEventAt: Date?

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
        stamina = StaminaTracker()
        satiation = SatiationTracker(monitor: monitor)
        userId = UserDefaults.standard.string(forKey: Self.userIdKey) ?? Self.defaultUserId
        username = UserDefaults.standard.string(forKey: Self.usernameKey)

        client.onEvent = { [weak self] channel, data in
            self?.route(channel: channel, data: data)
        }
        relay.onPassiveCrafts = { [weak self] crafts in
            guard let self else { return }
            self.passiveCrafts.update(crafts: crafts, baseline: self.switchSource(\.passiveFromRelay, toRelay: true))
        }
        relay.onPlants = { [weak self] plants in
            self?.applyPlants(plants, fromRelay: true)
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
        relay.stop()
        plantsFromRelay = nil
        passiveFromRelay = nil
        reportedSignedIn = nil
        lastLiveEventAt = nil
        isOffline = false
        startLoading()
        evaluate()
        connect()
        startRestRefresh()
    }

    private func startLoading() {
        isLoading = true
        loadingStartedAt = .now
        initialRestDone = false
    }

    private func updateLoading() {
        guard isLoading else { return }
        let ready = initialRestDone && client.state == .connected
        let timedOut = Date().timeIntervalSince(loadingStartedAt) >= Self.loadingTimeout
        if ready || timedOut {
            isLoading = false
            // Re-evaluate now that data is in, before the UI shows it.
            evaluate()
        }
    }

    private func setUpNotifications() {
        // Error state: a source turning red from a known non-red state. The
        // first report of a source (e.g. "no XP yet" at launch) doesn't count.
        monitor.onChange = { [weak self] previous, report in
            // Nothing during loading: startup states aren't news.
            guard let self, !self.isLoading, report.status == .alert,
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
            BitJitaChannel.characterStats(userId),
        ])
    }

    private func route(channel: String, data: Data) {
        lastLiveEventAt = .now
        do {
            if channel == BitJitaChannel.experience(userId) {
                experience.handle(try decoder.decode(EventEnvelope<ExperienceState>.self, from: data).data)
            } else if channel == BitJitaChannel.stamina(userId) {
                stamina.handle(try decoder.decode(EventEnvelope<StaminaState>.self, from: data).data)
            } else if channel == BitJitaChannel.satiation(userId) {
                satiation.handle(try decoder.decode(EventEnvelope<SatiationState>.self, from: data).data)
            } else if channel == BitJitaChannel.characterStats(userId) {
                let stats = try decoder.decode(EventEnvelope<CharacterStatsState>.self, from: data).data
                if let max = stats.maxStamina { stamina.setReportedMax(max) }
                satiation.setReportedMax(stats.maxSatiation)
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
                self.updateLoading()
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
                // Strong reference only for the round, not during the sleep.
                if let self { await self.refreshRound(userId: userId) } else { return }
                try? await Task.sleep(for: .seconds(60))
            }
        }
    }

    /// One round of REST requests, in parallel, so a round takes as long as
    /// the slowest request, not the sum. Initial loading only waits for the
    /// core requests: passive crafts can take ~10s when BitJita's cache is
    /// cold, so the panel shows a placeholder for them instead.
    private func refreshRound(userId: String) async {
        async let passive: Void = refreshPassiveCrafts(userId: userId)
        await withTaskGroup(of: Void.self) { group in
            group.addTask { await self.refreshCrafts(userId: userId) }
            group.addTask { await self.refreshStats(userId: userId) }
            group.addTask { await self.refreshPlaceables(userId: userId) }
            group.addTask { await self.refreshPlayer(userId: userId) }
        }
        if !Task.isCancelled, !initialRestDone {
            // Core round done (successful or not): loading can end.
            initialRestDone = true
            updateLoading()
        }
        await passive
    }

    private func refreshCrafts(userId: String) async {
        do {
            let response = try await BitJitaAPI.crafts(userId: userId)
            guard !Task.isCancelled else { return }
            let ids = crafts.update(from: response)
            client.subscribe(ids.map { BitJitaChannel.craft($0) })
        } catch {
            print("Failed to fetch crafts: \(error)")
        }
    }

    private func refreshPlayer(userId: String) async {
        do {
            let player = try await BitJitaAPI.player(id: userId)
            guard !Task.isCancelled, self.userId == userId else { return }
            reportedSignedIn = player.signedIn
            if let region = player.regionId {
                relay.start(userId: userId, region: region)
            }
            reconnectIfFeedStale()
            if username != player.username { setUsername(player.username) }
            evaluate()
        } catch {
            print("Failed to fetch player: \(error)")
        }
    }

    /// BitJita can stop delivering events on a connection that otherwise
    /// stays healthy (pings answered), e.g. after an outage on their side;
    /// a fresh connection gets them again. Satiation updates ~1/s while
    /// signed in, so silence while signed in means the feed is stale.
    private func reconnectIfFeedStale() {
        guard reportedSignedIn == true, let connectedAt = client.connectedAt else { return }
        let now = Date()
        let lastEvent = max(lastLiveEventAt ?? .distantPast, connectedAt)
        guard now.timeIntervalSince(lastEvent) >= Self.staleFeedWindow else { return }
        print("Signed in but no live events for \(Int(now.timeIntervalSince(lastEvent)))s; reconnecting")
        client.reconnect()
    }

    private func refreshStats(userId: String) async {
        do {
            let stats = try await BitJitaAPI.stats(userId: userId)
            guard !Task.isCancelled else { return }
            stamina.setReportedMax(stats.maxStamina)
            satiation.setReportedMax(stats.maxSatiation)
        } catch {
            print("Failed to fetch stats: \(error)")
        }
    }

    private func refreshPlaceables(userId: String) async {
        do {
            let placeables = try await BitJitaAPI.placeables(userId: userId)
            guard !Task.isCancelled, !relay.isLive else { return }
            applyPlants(placeables.instances.map(PlantTracker.Plant.init), fromRelay: false)
        } catch {
            print("Failed to fetch placeables: \(error)")
        }
    }

    /// Which source fed plants / passive crafts last (true = relay).
    /// Switching (REST ↔ relay) is a fresh baseline: the lists can differ
    /// (REST lags), which would otherwise look like crafts finishing.
    @ObservationIgnored private var plantsFromRelay: Bool?
    @ObservationIgnored private var passiveFromRelay: Bool?

    /// Records the source; true if it changed.
    private func switchSource(_ slot: ReferenceWritableKeyPath<AppModel, Bool?>, toRelay: Bool) -> Bool {
        let changed = self[keyPath: slot].map { $0 != toRelay } ?? false
        self[keyPath: slot] = toRelay
        return changed
    }

    /// Growing plants without a reported end time: look up the crop's growth
    /// duration once (catalog), for an estimate; then apply.
    private func applyPlants(_ list: [PlantTracker.Plant], fromRelay: Bool) {
        plants.update(plants: list, baseline: switchSource(\.plantsFromRelay, toRelay: fromRelay))
        let missing = plants.missingDurations(in: list)
        guard !missing.isEmpty else { return }
        Task { [weak self] in
            var found = false
            for id in missing {
                if let duration = try? await BitJitaAPI.placeable(id: id).growthDuration {
                    self?.plants.setGrowthDuration(duration, for: id)
                    found = true
                }
            }
            if found { self?.plants.update(plants: list) }
        }
    }

    private func refreshPassiveCrafts(userId: String) async {
        // Fed live by the relay; skip the (slow) request.
        guard !relay.isLive else { return }
        do {
            let passive = try await BitJitaAPI.passiveCrafts(userId: userId)
            guard !Task.isCancelled, !relay.isLive else { return }
            // Duration per recipe: looked up once via the crafted item.
            for missing in passiveCrafts.missingDurations(in: passive) {
                if let item = try? await BitJitaAPI.item(id: missing.itemId),
                   let recipe = item.craftingRecipes.first(where: { $0.id == missing.recipeId }) {
                    passiveCrafts.setDuration(recipe.timeRequirement, forRecipe: missing.recipeId)
                }
            }
            passiveCrafts.update(from: passive, baseline: switchSource(\.passiveFromRelay, toRelay: false))
        } catch {
            print("Failed to fetch passive crafts: \(error)")
        }
    }

    private func evaluate() {
        updateOnline()
        if isOffline {
            // Cleared, not reported: coming back online then is a "first
            // report", so it never triggers an error notification.
            monitor.clear(ActivityCheck.source)
            monitor.clear(SatiationTracker.source)
            monitor.report(Self.playerSource, .alert, "Offline")
        } else {
            // Reported as healthy (not cleared) so going offline is a
            // transition to red and notifies.
            monitor.report(Self.playerSource, .healthy, "Online")
            activity.evaluate(experience: experience, stamina: stamina, lastCraft: crafts.lastWorkedCraft, monitor: monitor)
            satiation.reportCurrent()
        }
        updateConnectionReport()
    }

    private func updateOnline() {
        let liveRecently = lastLiveEventAt.map { Date().timeIntervalSince($0) <= Self.liveEventWindow } ?? false
        let offline = !liveRecently && reportedSignedIn == false
        if offline != isOffline { isOffline = offline }
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
