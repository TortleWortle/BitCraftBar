//! Persisted settings (`<config dir>/bitcraft-tracker/settings.json`).

use serde::{Deserialize, Serialize};

/// The original user's character; change it with the character search.
pub const DEFAULT_USER_ID: &str = "648518346633885895";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    /// Tracked characters, in the main window's grid order.
    #[serde(default)]
    pub characters: Vec<CharacterSettings>,
    #[serde(default)]
    pub sounds: SoundSettings,
}

/// One tracked character.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CharacterSettings {
    /// Entity id: what all relay queries use.
    pub user_id: String,
    /// Display only; refreshed from the relay's `player_username_state`.
    pub username: Option<String>,
    /// No sounds for this character (notifications still show).
    #[serde(default)]
    pub muted: bool,
    /// Its floating window is open (reopened at launch).
    #[serde(default)]
    pub floating_window: bool,
    /// Its floating window's position (top-left, logical screen
    /// coordinates), kept when the user moves it.
    #[serde(default)]
    pub floating_origin: Option<(f32, f32)>,
}

/// The single-character format (before multiple characters): read once
/// and turned into one `CharacterSettings`.
#[derive(Default, Deserialize)]
struct Legacy {
    user_id: Option<String>,
    username: Option<String>,
    #[serde(default, alias = "info_window")]
    floating_window: bool,
    #[serde(default, alias = "info_origin")]
    floating_origin: Option<(f32, f32)>,
}

/// Which events play a sound, and how loud (settings in the main window).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundSettings {
    pub craft_finished: bool,
    pub out_of_food: bool,
    pub idle_full: bool,
    pub resource_depleted: bool,
    pub trade_request: bool,
    /// 0–1.
    pub volume: f32,
}

impl Default for SoundSettings {
    fn default() -> Self {
        Self { craft_finished: false, out_of_food: false, idle_full: false, resource_depleted: false, trade_request: false, volume: 0.5 }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            characters: vec![CharacterSettings { user_id: DEFAULT_USER_ID.into(), ..Default::default() }],
            sounds: SoundSettings::default(),
        }
    }
}

impl Settings {
    fn path() -> Option<std::path::PathBuf> {
        Some(dirs::config_dir()?.join("bitcraft-tracker").join("settings.json"))
    }

    pub fn load() -> Self {
        Self::path().and_then(|p| std::fs::read(p).ok()).and_then(|bytes| Self::parse(&bytes)).unwrap_or_default()
    }

    /// Current format, or the single-character one (migrated).
    fn parse(bytes: &[u8]) -> Option<Self> {
        let mut settings: Self = serde_json::from_slice(bytes).ok()?;
        if settings.characters.is_empty() {
            let legacy: Legacy = serde_json::from_slice(bytes).unwrap_or_default();
            if let Some(user_id) = legacy.user_id {
                settings.characters.push(CharacterSettings {
                    user_id,
                    username: legacy.username,
                    muted: false,
                    floating_window: legacy.floating_window,
                    floating_origin: legacy.floating_origin,
                });
            }
        }
        Some(settings)
    }

    pub fn character(&self, user_id: &str) -> Option<&CharacterSettings> {
        self.characters.iter().find(|c| c.user_id == user_id)
    }

    pub fn character_mut(&mut self, user_id: &str) -> Option<&mut CharacterSettings> {
        self.characters.iter_mut().find(|c| c.user_id == user_id)
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_single_character() {
        let old = br#"{"user_id":"42","username":"Bob","info_window":true,"info_origin":[10.0,20.0],"sounds":{"volume":0.3}}"#;
        let s = Settings::parse(old).unwrap();
        assert_eq!(s.characters.len(), 1);
        let c = &s.characters[0];
        assert_eq!((c.user_id.as_str(), c.username.as_deref()), ("42", Some("Bob")));
        assert!(c.floating_window && !c.muted);
        assert_eq!(c.floating_origin, Some((10.0, 20.0)));
        assert_eq!(s.sounds.volume, 0.3);
    }

    #[test]
    fn keeps_an_emptied_list() {
        // Everyone removed: stays empty (no default character coming back).
        let s = Settings::parse(br#"{"characters":[],"sounds":{}}"#).unwrap();
        assert!(s.characters.is_empty());
    }
}
