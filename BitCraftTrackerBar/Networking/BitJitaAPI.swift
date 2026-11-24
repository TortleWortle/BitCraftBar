import Foundation

/// REST endpoints on bitjita.com.
enum BitJitaAPI {
    struct CraftsResponse: Decodable {
        struct CraftResult: Decodable {
            struct CraftedItem: Decodable {
                let itemId: Int
                let quantity: Int

                enum CodingKeys: String, CodingKey {
                    case itemId = "item_id"
                    case quantity
                }
            }

            let entityId: String
            let buildingName: String?
            let progress: Int
            let craftCount: Int
            let actionsRequiredPerItem: Int
            let totalActionsRequired: Int
            let completed: Bool
            let craftedItem: [CraftedItem]
        }

        struct Item: Decodable {
            let id: Int
            let name: String
        }

        let craftResults: [CraftResult]
        let items: [Item]
    }

    /// Character stats; `values` is indexed by stat id.
    struct StatsResponse: Decodable {
        struct Stats: Decodable {
            let values: [Double]
        }

        let stats: Stats

        /// Stat id 1 ("Maximum Stamina" in the equipment endpoint's stat names).
        var maxStamina: Double? {
            stats.values.indices.contains(1) ? stats.values[1] : nil
        }
    }

    /// Player's placeables (crops, sift collectors, ...).
    struct PlaceablesResponse: Decodable {
        struct Placeable: Decodable {
            let name: String
            let tag: String?
            let tier: Int
        }

        struct Interaction: Decodable {
            let verbPhrase: String
        }

        struct Instance: Decodable {
            let entityId: String
            let placeableId: Int
            let placeable: Placeable
            let availableInteractions: [Interaction]?
            let updatedAt: BitJitaDate?
            let nextStateAt: BitJitaDate?
            let growthEndTimestamp: BitJitaDate?
        }

        let instances: [Instance]
    }

    /// One placeable type from the catalog (`placeables/<placeableId>`).
    struct PlaceableDetail: Decodable {
        struct Growth: Decodable {
            /// [min, max] growth time in seconds.
            let time: [Double]
        }

        let growth: [Growth]

        /// Longest growth time in seconds, if this stage grows on a timer.
        var growthDuration: TimeInterval? {
            growth.compactMap { $0.time.max() }.max()
        }
    }

    struct PlayerSummary: Decodable, Identifiable, Equatable {
        let entityId: String
        let username: String
        let signedIn: Bool?

        var id: String { entityId }
    }

    struct PlayerSearchResponse: Decodable {
        let players: [PlayerSummary]
    }

    struct PlayerResponse: Decodable {
        let player: PlayerSummary
    }

    /// Substring search on username (server needs ≥ 2 characters, returns at
    /// most 50, alphabetically). Re-ranked: exact match, then prefix, then rest.
    static func searchPlayers(_ query: String) async throws -> [PlayerSummary] {
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.count >= 2 else { return [] }
        var components = URLComponents()
        components.queryItems = [URLQueryItem(name: "q", value: trimmed)]
        let response: PlayerSearchResponse = try await get("players" + (components.percentEncodedQuery.map { "?" + $0 } ?? ""))
        let lower = trimmed.lowercased()
        func rank(_ player: PlayerSummary) -> Int {
            let name = player.username.lowercased()
            if name == lower { return 0 }
            if name.hasPrefix(lower) { return 1 }
            return 2
        }
        return response.players.sorted { (rank($0), $0.username.lowercased()) < (rank($1), $1.username.lowercased()) }
    }

    static func player(id: String) async throws -> PlayerSummary {
        let response: PlayerResponse = try await get("players/\(id)")
        return response.player
    }

    /// `players/<id>/passive-crafts`: crafts running on their own in a
    /// building (tanning tub, kiln, smelter, loom, ...). No progress or end
    /// time; only a start `timestamp` and a `status` ("complete" when done).
    struct PassiveCraftsResponse: Decodable {
        struct PassiveCraft: Decodable {
            let entityId: String
            let buildingName: String?
            let recipeId: Int
            let recipeName: String?
            let status: String?
            let timestamp: BitJitaDate?
            let craftedItem: [CraftsResponse.CraftResult.CraftedItem]
        }

        let craftResults: [PassiveCraft]
        let items: [CraftsResponse.Item]
    }

    /// `items/<id>`: used for the recipes that craft the item, to get a
    /// passive recipe's duration.
    struct ItemDetail: Decodable {
        struct Recipe: Decodable {
            let id: Int
            /// Seconds. For passive recipes this is the craft duration.
            let timeRequirement: Double
            let isPassive: Bool?
        }

        let craftingRecipes: [Recipe]
    }

    static func passiveCrafts(userId: String) async throws -> PassiveCraftsResponse {
        try await get("players/\(userId)/passive-crafts")
    }

    static func item(id: Int) async throws -> ItemDetail {
        try await get("items/\(id)")
    }

    static func crafts(userId: String) async throws -> CraftsResponse {
        try await get("players/\(userId)/crafts")
    }

    static func stats(userId: String) async throws -> StatsResponse {
        try await get("players/\(userId)/stats")
    }

    static func placeables(userId: String) async throws -> PlaceablesResponse {
        try await get("players/\(userId)/placeables")
    }

    static func placeable(id: Int) async throws -> PlaceableDetail {
        try await get("placeables/\(id)")
    }

    private static func get<T: Decodable>(_ path: String) async throws -> T {
        let url = URL(string: "https://bitjita.com/api/\(path)")!
        let (data, response) = try await URLSession.shared.data(from: url)
        if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
            throw URLError(.badServerResponse)
        }
        return try JSONDecoder().decode(T.self, from: data)
    }
}

/// A timestamp in any of the formats BitJita uses: Postgres-style strings
/// ("2026-11-24 09:03:18.97199+00"), ISO 8601, or epoch seconds/milliseconds
/// (as number or numeric string).
struct BitJitaDate: Decodable {
    let date: Date

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let number = try? container.decode(Double.self) {
            date = Self.fromEpoch(number)
        } else {
            let string = try container.decode(String.self)
            guard let parsed = Self.parse(string) else {
                throw DecodingError.dataCorruptedError(in: container, debugDescription: "Unrecognized date: \(string)")
            }
            date = parsed
        }
    }

    static func fromEpoch(_ value: Double) -> Date {
        // Milliseconds if it's too large to be seconds.
        Date(timeIntervalSince1970: value > 1e11 ? value / 1000 : value)
    }

    static func parse(_ string: String) -> Date? {
        if let number = Double(string) { return fromEpoch(number) }

        // Normalize Postgres style to ISO 8601.
        var iso = string.replacingOccurrences(of: " ", with: "T")
        if let match = iso.range(of: #"[+-]\d{2}$"#, options: .regularExpression) {
            iso.replaceSubrange(match, with: iso[match] + ":00")
        }
        // ISO8601DateFormatter wants exactly 3 fractional digits.
        if let match = iso.range(of: #"\.\d+"#, options: .regularExpression) {
            let digits = iso[match].dropFirst().prefix(3)
            iso.replaceSubrange(match, with: "." + digits.padding(toLength: 3, withPad: "0", startingAt: 0))
        }

        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: iso) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: iso)
    }
}
