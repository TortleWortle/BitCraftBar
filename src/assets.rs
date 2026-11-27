//! Icons embedded in the binary (`assets/icons`, see its README), served
//! to GPUI's `svg()` element by path ("icons/heart.svg").

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../assets/", $name)) as &[u8])),*]
    };
}

const FILES: &[(&str, &[u8])] = icons![
    "icons/heart.svg",
    "icons/food.svg",
    "icons/compact.svg",
    "icons/expand.svg",
    "icons/skills/2.svg",
    "icons/skills/3.svg",
    "icons/skills/4.svg",
    "icons/skills/5.svg",
    "icons/skills/6.svg",
    "icons/skills/7.svg",
    "icons/skills/8.svg",
    "icons/skills/9.svg",
    "icons/skills/10.svg",
    "icons/skills/11.svg",
    "icons/skills/12.svg",
    "icons/skills/13.svg",
    "icons/skills/14.svg",
    "icons/skills/15.svg",
    "icons/skills/17.svg",
    "icons/skills/18.svg",
    "icons/skills/19.svg",
    "icons/skills/21.svg",
    "icons/skills/22.svg",
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES.iter().find(|(p, _)| *p == path).map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)).collect())
    }
}

/// Icon of a skill (BitCraft's own glyph), None for ids without one.
pub fn skill_icon(skill_id: i64) -> Option<&'static str> {
    let path = format!("icons/skills/{skill_id}.svg");
    FILES.iter().map(|(p, _)| *p).find(|p| *p == path)
}
