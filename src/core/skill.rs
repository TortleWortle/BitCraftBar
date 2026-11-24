//! BitCraft skills; value = skill id (from bitjita.com/api/players/<id> `skillMap`).

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Skill {
    Any = 1,
    Forestry = 2,
    Carpentry = 3,
    Masonry = 4,
    Mining = 5,
    Smithing = 6,
    Scholar = 7,
    Leatherworking = 8,
    Hunting = 9,
    Tailoring = 10,
    Farming = 11,
    Fishing = 12,
    Cooking = 13,
    Foraging = 14,
    Construction = 15,
    Taming = 17,
    Slayer = 18,
    Merchanting = 19,
    Sailing = 21,
    HexiteGathering = 22,
}

impl Skill {
    pub const ALL: [Skill; 20] = [
        Skill::Any, Skill::Forestry, Skill::Carpentry, Skill::Masonry, Skill::Mining,
        Skill::Smithing, Skill::Scholar, Skill::Leatherworking, Skill::Hunting,
        Skill::Tailoring, Skill::Farming, Skill::Fishing, Skill::Cooking, Skill::Foraging,
        Skill::Construction, Skill::Taming, Skill::Slayer, Skill::Merchanting, Skill::Sailing,
        Skill::HexiteGathering,
    ];

    pub fn from_id(id: i64) -> Option<Skill> {
        Self::ALL.into_iter().find(|s| *s as i64 == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            Skill::Any => "Any",
            Skill::Forestry => "Forestry",
            Skill::Carpentry => "Carpentry",
            Skill::Masonry => "Masonry",
            Skill::Mining => "Mining",
            Skill::Smithing => "Smithing",
            Skill::Scholar => "Scholar",
            Skill::Leatherworking => "Leatherworking",
            Skill::Hunting => "Hunting",
            Skill::Tailoring => "Tailoring",
            Skill::Farming => "Farming",
            Skill::Fishing => "Fishing",
            Skill::Cooking => "Cooking",
            Skill::Foraging => "Foraging",
            Skill::Construction => "Construction",
            Skill::Taming => "Taming",
            Skill::Slayer => "Slayer",
            Skill::Merchanting => "Merchanting",
            Skill::Sailing => "Sailing",
            Skill::HexiteGathering => "Hexite Gathering",
        }
    }

    /// Display name for any id, including unknown ones.
    pub fn name_for(id: i64) -> String {
        Skill::from_id(id).map(|s| s.name().to_string()).unwrap_or_else(|| format!("Skill {id}"))
    }
}
