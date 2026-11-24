import AppKit
import Foundation
import Observation

/// Live passive crafts and plants straight from BitCraft's SpacetimeDB, via
/// BitJita's read-only relay (`relay.bitjita.com`, "BITCONNECT"). No account
/// or BitCraft token needed: the relay hands out its own identities.
///
/// Speaks SpacetimeDB's v1 JSON websocket protocol (`v1.json.spacetimedb`):
/// one `Subscribe` with all SQL queries; the server answers with an
/// `InitialSubscription` (current rows) and then pushes `TransactionUpdate`s
/// (row deletes + inserts). Rows are JSON arrays in column order; sum types
/// (enums/options) are `[tag, payload]`, timestamps `[micros]`.
///
/// Each BitCraft region is its own database (`idc-<region id>`); only some
/// regions are mirrored. Rows are kept in a small store; after each update
/// the feed rebuilds the tracker inputs and calls `onPassiveCrafts` /
/// `onPlants`.
@Observable
final class RelayFeed {
    enum State: Equatable {
        case off
        case connecting
        /// Initial subscription received; data is current.
        case live
        /// The relay refused the subscription. Retried: soon if it says it's
        /// still loading (relay restart), else after `refusedRetry` (e.g.
        /// region not mirrored).
        case unavailable(String)
    }

    static let host = "relay.bitjita.com"
    static let refusedRetry: Duration = .seconds(5 * 60)

    private(set) var state: State = .off
    var isLive: Bool { state == .live }
    private(set) var region: Int?

    @ObservationIgnored var onPassiveCrafts: (([PassiveCraftTracker.Craft]) -> Void)?
    @ObservationIgnored var onPlants: (([PlantTracker.Plant]) -> Void)?

    @ObservationIgnored private var userId: String?
    @ObservationIgnored private var runTask: Task<Void, Never>?
    @ObservationIgnored private var socket: URLSessionWebSocketTask?
    @ObservationIgnored private let session = URLSession(configuration: .default)
    @ObservationIgnored private var wakeObserver: NSObjectProtocol?

    /// Rows per table, keyed by primary key (column 0). Counted, because join
    /// queries can deliver the same row several times (once per match).
    @ObservationIgnored private var tables: [String: [String: (row: [Any], count: Int)]] = [:]
    /// Lookups added after seeing ids in other tables (item and building names).
    @ObservationIgnored private var itemIds: Set<Int> = []
    @ObservationIgnored private var buildingDescIds: Set<Int> = []
    /// When each plant was seen switching to a growing stage (live only).
    @ObservationIgnored private var growingSince: [String: Date] = [:]
    @ObservationIgnored private var growingIds: Set<String> = []

    init() {
        wakeObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.restart() }
        }
    }

    /// Follow `userId`'s passive crafts and plants in `region`. No-op if
    /// already doing exactly that.
    func start(userId: String, region: Int) {
        if self.userId == userId, self.region == region, state != .off { return }
        stop()
        self.userId = userId
        self.region = region
        restart()
    }

    func stop() {
        runTask?.cancel()
        runTask = nil
        socket?.cancel(with: .goingAway, reason: nil)
        socket = nil
        userId = nil
        region = nil
        tables = [:]
        itemIds = []
        buildingDescIds = []
        growingSince = [:]
        growingIds = []
        state = .off
    }

    private func restart() {
        guard userId != nil, region != nil else { return }
        runTask?.cancel()
        socket?.cancel(with: .goingAway, reason: nil)
        state = .connecting
        runTask = Task { [weak self] in await self?.run() }
    }

    // MARK: - Connection

    private func run() async {
        var backoff: Duration = .seconds(1)
        while !Task.isCancelled, let region {
            state = .connecting
            var request = URLRequest(url: URL(string: "wss://\(Self.host)/v1/database/idc-\(region)/subscribe")!)
            request.setValue("v1.json.spacetimedb", forHTTPHeaderField: "Sec-WebSocket-Protocol")
            let socket = session.webSocketTask(with: request)
            socket.maximumMessageSize = 32 << 20
            self.socket = socket
            socket.resume()
            var keepalive: Task<Void, Never>?

            do {
                try await sendSubscribe(on: socket)
                keepalive = Task { await BitJitaClient.keepAlive(socket) }
                while !Task.isCancelled {
                    let message = try await socket.receive()
                    let data: Data
                    switch message {
                    case .string(let text): data = Data(text.utf8)
                    case .data(let bytes): data = bytes
                    @unknown default: continue
                    }
                    if handle(data, socket: socket) { backoff = .seconds(1) }
                    if case .unavailable = state { break }
                }
            } catch {
                // Fall through to reconnect.
            }

            keepalive?.cancel()
            socket.cancel(with: .goingAway, reason: nil)
            guard !Task.isCancelled else { return }
            var wait = backoff
            if case .unavailable(let reason) = state {
                // "module is still loading its initial state from upstream;
                // retry shortly" happens while the relay restarts.
                let transient = reason.contains("loading") || reason.contains("retry")
                wait = transient ? max(backoff, .seconds(5)) : Self.refusedRetry
            } else {
                state = .connecting
            }
            try? await Task.sleep(for: wait)
            backoff = min(backoff * 2, .seconds(30))
        }
    }

    private var queries: [String] {
        guard let userId else { return [] }
        var queries = [
            "SELECT * FROM passive_craft_state WHERE owner_entity_id = \(userId)",
            "SELECT crafting_recipe_desc.* FROM crafting_recipe_desc JOIN passive_craft_state ON crafting_recipe_desc.id = passive_craft_state.recipe_id WHERE passive_craft_state.owner_entity_id = \(userId)",
            "SELECT building_state.* FROM building_state JOIN passive_craft_state ON building_state.entity_id = passive_craft_state.building_entity_id WHERE passive_craft_state.owner_entity_id = \(userId)",
            "SELECT * FROM placeable_state WHERE owner_entity_id = \(userId)",
            "SELECT placeable_desc.* FROM placeable_desc JOIN placeable_state ON placeable_desc.id = placeable_state.placeable_id WHERE placeable_state.owner_entity_id = \(userId)",
            "SELECT growth_state.* FROM growth_state JOIN placeable_state ON growth_state.entity_id = placeable_state.entity_id WHERE placeable_state.owner_entity_id = \(userId)",
        ]
        queries += itemIds.sorted().map { "SELECT * FROM item_desc WHERE id = \($0)" }
        queries += buildingDescIds.sorted().map { "SELECT * FROM building_desc WHERE id = \($0)" }
        return queries
    }

    /// v1 `Subscribe` replaces the whole subscription set, so this is also
    /// how lookups are added (the server re-sends all current rows).
    private func sendSubscribe(on socket: URLSessionWebSocketTask) async throws {
        let message: [String: Any] = ["Subscribe": ["query_strings": queries, "request_id": 1]]
        let data = try JSONSerialization.data(withJSONObject: message)
        try await socket.send(.string(String(decoding: data, as: UTF8.self)))
    }

    /// Returns true for a data message (initial subscription or update).
    private func handle(_ data: Data, socket: URLSessionWebSocketTask) -> Bool {
        guard let message = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return false }

        if let error = message["SubscriptionError"] as? [String: Any] {
            let text = error["error"] as? String ?? "subscription refused"
            print("Relay subscription error: \(text)")
            state = .unavailable(text)
            return false
        }

        let changes: [[String: Any]]
        let isInitial: Bool
        if let initial = message["InitialSubscription"] as? [String: Any] {
            changes = (initial["database_update"] as? [String: Any])?["tables"] as? [[String: Any]] ?? []
            isInitial = true
        } else if let update = message["TransactionUpdate"] as? [String: Any],
                  let committed = (update["status"] as? [String: Any])?["Committed"] as? [String: Any] {
            changes = committed["tables"] as? [[String: Any]] ?? []
            isInitial = false
        } else {
            return false
        }

        apply(changes, isInitial: isInitial)
        state = .live
        publish(isInitial: isInitial)
        addMissingLookups(socket: socket)
        return true
    }

    // MARK: - Row store

    private func apply(_ changes: [[String: Any]], isInitial: Bool) {
        if isInitial {
            // A (re-)subscribe re-sends everything subscribed.
            tables = [:]
        }
        for change in changes {
            guard let name = change["table_name"] as? String else { continue }
            var table = tables[name] ?? [:]
            for update in change["updates"] as? [[String: Any]] ?? [] {
                for row in Self.rows(update["deletes"]) {
                    let key = Self.string(row[0])
                    if let existing = table[key] {
                        table[key] = existing.count > 1 ? (existing.row, existing.count - 1) : nil
                    }
                }
                for row in Self.rows(update["inserts"]) {
                    let key = Self.string(row[0])
                    table[key] = (row, (table[key]?.count ?? 0) + 1)
                }
            }
            tables[name] = table
        }
    }

    private func rows(_ table: String) -> [[Any]] {
        tables[table]?.values.map(\.row) ?? []
    }

    private func lookup(_ table: String, _ key: String) -> [Any]? {
        tables[table]?[key]?.row
    }

    /// Item names (for recipe names like "Weave {0}") and building names
    /// need a second lookup; add queries for ids we haven't asked for yet.
    private func addMissingLookups(socket: URLSessionWebSocketTask) {
        let neededItems = Set(rows("crafting_recipe_desc").compactMap(Self.craftedItemId))
        let neededBuildings = Set(rows("building_state").compactMap { Self.int($0[3]) })
        guard !neededItems.isSubset(of: itemIds) || !neededBuildings.isSubset(of: buildingDescIds) else { return }
        itemIds.formUnion(neededItems)
        buildingDescIds.formUnion(neededBuildings)
        Task { try? await sendSubscribe(on: socket) }
    }

    // MARK: - Tracker inputs

    private func publish(isInitial: Bool) {
        onPassiveCrafts?(passiveCrafts())
        onPlants?(plants(isInitial: isInitial))
    }

    private func passiveCrafts() -> [PassiveCraftTracker.Craft] {
        rows("passive_craft_state").map { row in
            let recipe = lookup("crafting_recipe_desc", Self.string(row[2]))
            let itemName = recipe.flatMap(Self.craftedItemId)
                .flatMap { lookup("item_desc", String($0)) }
                .flatMap { $0[1] as? String }
            let buildingName = lookup("building_state", Self.string(row[3]))
                .flatMap { Self.int($0[3]) }
                .flatMap { lookup("building_desc", String($0)) }
                .flatMap { $0[2] as? String }
            // status: 0 Queued, 1 Processing, 2 Complete.
            let status = Self.tag(row[5])
            return PassiveCraftTracker.Craft(
                id: Self.string(row[0]),
                name: PassiveCraftTracker.displayName(recipeName: recipe?[1] as? String, itemName: itemName),
                building: buildingName ?? "Building",
                recipeId: Self.int(row[2]) ?? 0,
                itemId: recipe.flatMap(Self.craftedItemId),
                // A queued craft hasn't started; its end is unknown until it
                // becomes Processing (pushed live).
                startedAt: status == 0 ? nil : Self.date(row[4]),
                reportedComplete: status == 2,
                duration: recipe.flatMap { Self.double($0[2]) }
            )
        }
    }

    private func plants(isInitial: Bool) -> [PlantTracker.Plant] {
        let now = Date()
        var growing: Set<String> = []
        let plants: [PlantTracker.Plant] = rows("placeable_state").compactMap { row in
            let id = Self.string(row[0])
            guard let placeableId = Self.int(row[2]),
                  let desc = lookup("placeable_desc", String(placeableId)),
                  let name = desc[1] as? String
            else { return nil }
            if name.hasPrefix("Growing ") {
                growing.insert(id)
                // Seen starting to grow (not just growing on first load).
                if !isInitial, !growingIds.contains(id), growingSince[id] == nil {
                    growingSince[id] = now
                }
            } else {
                growingSince[id] = nil
            }
            return PlantTracker.Plant(
                placeableId: placeableId,
                name: name,
                tier: Self.int(desc[3]) ?? 0,
                interactions: [],
                reportedEnd: lookup("growth_state", id).flatMap { Self.date($0[1]) },
                startedGrowingAt: growingSince[id]
            )
        }
        growingIds = growing
        return plants
    }

    // MARK: - Value decoding

    private nonisolated static func rows(_ value: Any?) -> [[Any]] {
        (value as? [Any] ?? []).compactMap { element in
            if let text = element as? String {
                return (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [Any]
            }
            return element as? [Any]
        }
    }

    nonisolated static func string(_ value: Any) -> String {
        if let number = value as? NSNumber { return number.stringValue }
        return "\(value)"
    }

    nonisolated static func int(_ value: Any) -> Int? { (value as? NSNumber)?.intValue }
    nonisolated static func double(_ value: Any) -> Double? { (value as? NSNumber)?.doubleValue }

    /// Sum type `[tag, payload]` → tag.
    nonisolated static func tag(_ value: Any) -> Int? { ((value as? [Any])?.first).flatMap(int) }

    /// Timestamp `[micros since epoch]`.
    nonisolated static func date(_ value: Any) -> Date? {
        guard let micros = ((value as? [Any])?.first).flatMap(double), micros > 0 else { return nil }
        return Date(timeIntervalSince1970: micros / 1_000_000)
    }

    /// `crafting_recipe_desc.crafted_item_stacks[0].item_id`.
    nonisolated static func craftedItemId(_ recipe: [Any]) -> Int? {
        guard recipe.indices.contains(13),
              let stacks = recipe[13] as? [Any],
              let first = stacks.first as? [Any],
              let id = first.first
        else { return nil }
        return int(id)
    }
}
