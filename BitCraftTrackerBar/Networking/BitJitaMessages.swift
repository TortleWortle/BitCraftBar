import Foundation

struct SubscribeMessage: Encodable {
    var type = "subscribe"
    let channels: [String]
}

/// Just enough of an incoming message to route it by channel.
struct IncomingEnvelope: Decodable {
    let type: String
    let channel: String?
}

struct EventEnvelope<Payload: Decodable>: Decodable {
    let channel: String
    let data: Payload
}

/// `experience_state:<entity_id>`
struct ExperienceState: Decodable {
    struct Stack: Decodable {
        let quantity: Int
        let skillId: Int
    }

    let entityId: String
    let experienceStacks: [Stack]
    let timestamp: Int64
}

/// `stamina_state:<entity_id>`
struct StaminaState: Decodable {
    let entityId: String
    let stamina: Double
    let timestamp: Int64
}

/// `satiation_state:<entity_id>`
struct SatiationState: Decodable {
    let entityId: String
    let satiation: Double
    let timestamp: Int64
}

/// `character_stats_state:<entity_id>`: same `values` array as the REST
/// `players/<id>/stats`. Only sent when a stat changes (e.g. gear swap).
struct CharacterStatsState: Decodable {
    let values: [Double]

    /// Stat id 1 = "Maximum Stamina" (see `BitJitaAPI.StatsResponse`).
    var maxStamina: Double? {
        values.indices.contains(1) ? values[1] : nil
    }

    /// Stat id 19 = max satiation.
    var maxSatiation: Double? {
        values.indices.contains(19) ? values[19] : nil
    }
}

/// `progressive_action_state:<craft_entity_id>`, one event per craft action.
struct ProgressiveActionState: Decodable {
    let entityId: String
    let ownerEntityId: String
    let progress: Int
    let craftCount: Int
    let timestamp: Int64
}

enum BitJitaChannel {
    static func experience(_ userId: String) -> String { "experience_state:\(userId)" }
    static func stamina(_ userId: String) -> String { "stamina_state:\(userId)" }
    static func satiation(_ userId: String) -> String { "satiation_state:\(userId)" }
    static func characterStats(_ userId: String) -> String { "character_stats_state:\(userId)" }
    static let craftPrefix = "progressive_action_state:"
    static func craft(_ craftId: String) -> String { craftPrefix + craftId }
}
