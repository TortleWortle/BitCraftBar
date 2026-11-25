//! Persisted settings (`<config dir>/bitcraft-tracker/settings.json`).

use serde::{Deserialize, Serialize};

/// The original user's character; change it with the character search.
pub const DEFAULT_USER_ID: &str = "648518346633885895";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    /// Entity id: what all channels/endpoints use.
    pub user_id: String,
    /// Display only; looked up from `players/<id>`.
    pub username: Option<String>,
    /// Info window open (reopened at launch).
    #[serde(default)]
    pub info_window: bool,
    /// Info window position (top-left, logical screen coordinates), kept
    /// when the user moves it.
    #[serde(default)]
    pub info_origin: Option<(f32, f32)>,
    #[serde(default)]
    pub sounds: SoundSettings,
}

/// Which events play a sound, and how loud (settings in the popup).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundSettings {
    pub craft_finished: bool,
    pub out_of_food: bool,
    pub idle_full: bool,
    /// 0–1.
    pub volume: f32,
}

impl Default for SoundSettings {
    fn default() -> Self {
        Self { craft_finished: false, out_of_food: false, idle_full: false, volume: 0.5 }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self { user_id: DEFAULT_USER_ID.into(), username: None, info_window: false, info_origin: None, sounds: SoundSettings::default() }
    }
}

impl Settings {
    fn path() -> Option<std::path::PathBuf> {
        Some(dirs::config_dir()?.join("bitcraft-tracker").join("settings.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
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
