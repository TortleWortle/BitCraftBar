import Foundation

/// BitCraft skills, raw value = skill id.
/// Source: bitjita.com/api/players/<id> `skillMap`.
enum Skill: Int, CaseIterable {
    case any = 1
    case forestry = 2
    case carpentry = 3
    case masonry = 4
    case mining = 5
    case smithing = 6
    case scholar = 7
    case leatherworking = 8
    case hunting = 9
    case tailoring = 10
    case farming = 11
    case fishing = 12
    case cooking = 13
    case foraging = 14
    case construction = 15
    case taming = 17
    case slayer = 18
    case merchanting = 19
    case sailing = 21
    case hexiteGathering = 22

    var name: String {
        switch self {
        case .any: "Any"
        case .forestry: "Forestry"
        case .carpentry: "Carpentry"
        case .masonry: "Masonry"
        case .mining: "Mining"
        case .smithing: "Smithing"
        case .scholar: "Scholar"
        case .leatherworking: "Leatherworking"
        case .hunting: "Hunting"
        case .tailoring: "Tailoring"
        case .farming: "Farming"
        case .fishing: "Fishing"
        case .cooking: "Cooking"
        case .foraging: "Foraging"
        case .construction: "Construction"
        case .taming: "Taming"
        case .slayer: "Slayer"
        case .merchanting: "Merchanting"
        case .sailing: "Sailing"
        case .hexiteGathering: "Hexite Gathering"
        }
    }

    /// Display name for any id, including ones not (yet) in this enum.
    static func name(for id: Int) -> String {
        Skill(rawValue: id)?.name ?? "Skill \(id)"
    }
}
