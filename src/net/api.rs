//! BitJita REST API (`https://bitjita.com/api/`). See AGENTS.md "Data sources".

use std::sync::OnceLock;

use anyhow::{Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::core::types::{CraftInfo, Plant, PlayerSummary};

const BASE: &str = "https://bitjita.com/api/";

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            // Some default library user agents get a 403; use a plain one.
            .user_agent("BitCraftTracker/0.1")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("http client")
    })
}

async fn get<T: DeserializeOwned>(path: &str) -> Result<T> {
    let response = client().get(format!("{BASE}{path}")).send().await?;
    if !response.status().is_success() {
        bail!("GET {path}: {}", response.status());
    }
    Ok(response.json().await?)
}

/// BitJita timestamps: Postgres-style strings ("2026-11-24 09:03:18.97199+00"),
/// ISO 8601, or epoch seconds/milliseconds (number or numeric string).
pub fn parse_date(value: &Value) -> Option<f64> {
    fn epoch(v: f64) -> f64 {
        if v > 1e11 { v / 1000.0 } else { v }
    }
    match value {
        Value::Number(n) => n.as_f64().map(epoch),
        Value::String(s) => {
            if let Ok(n) = s.parse::<f64>() {
                return Some(epoch(n));
            }
            let mut iso = s.replacen(' ', "T", 1);
            // "+00" → "+00:00"
            if let Some(pos) = iso.rfind(['+', '-'])
                && pos > 10
                && iso.len() - pos == 3
            {
                iso.push_str(":00");
            }
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&iso) {
                return Some(dt.timestamp_micros() as f64 / 1e6);
            }
            chrono::NaiveDateTime::parse_from_str(&iso, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|dt| dt.and_utc().timestamp_micros() as f64 / 1e6)
        }
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CraftedItem {
    #[serde(rename = "item_id")]
    item_id: i64,
}

#[derive(Deserialize)]
struct Item {
    id: i64,
    name: String,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    tier: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CraftResult {
    entity_id: String,
    building_name: Option<String>,
    progress: i64,
    total_actions_required: i64,
    completed: bool,
    crafted_item: Vec<CraftedItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CraftsResponse {
    craft_results: Vec<CraftResult>,
    items: Vec<Item>,
}

pub async fn crafts(user: &str) -> Result<Vec<CraftInfo>> {
    let r: CraftsResponse = get(&format!("players/{user}/crafts")).await?;
    Ok(r.craft_results
        .into_iter()
        .map(|c| {
            let item = c.crafted_item.first().and_then(|i| r.items.iter().find(|it| it.id == i.item_id));
            CraftInfo {
            name: item.map(|it| it.name.clone()).unwrap_or_else(|| "Craft".into()),
            kind: item.and_then(|it| it.tag.clone()),
            tier: item.and_then(|it| it.tier),
            id: c.entity_id,
            building: c.building_name,
            progress: c.progress,
            total: c.total_actions_required,
            completed: c.completed,
            }
        })
        .collect())
}

#[derive(Deserialize)]
struct StatsResponse {
    stats: Stats,
}

#[derive(Deserialize)]
struct Stats {
    values: Vec<f64>,
}

/// `stats.values`, indexed by stat id (1 = max stamina, 19 = max satiation).
pub async fn stats(user: &str) -> Result<Vec<f64>> {
    let r: StatsResponse = get(&format!("players/{user}/stats")).await?;
    Ok(r.stats.values)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Instance {
    placeable_id: i64,
    placeable: Placeable,
    available_interactions: Option<Vec<Interaction>>,
    updated_at: Option<Value>,
    next_state_at: Option<Value>,
    growth_end_timestamp: Option<Value>,
}

#[derive(Deserialize)]
struct Placeable {
    name: String,
    tier: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Interaction {
    verb_phrase: String,
}

#[derive(Deserialize)]
struct PlaceablesResponse {
    instances: Vec<Instance>,
}

/// Placeables (crops and others; non-crops are ignored by the tracker).
/// `started_growing_at` assumes `updatedAt` is when growing started.
pub async fn placeables(user: &str) -> Result<Vec<Plant>> {
    let r: PlaceablesResponse = get(&format!("players/{user}/placeables")).await?;
    Ok(r.instances
        .into_iter()
        .map(|i| Plant {
            placeable_id: i.placeable_id,
            name: i.placeable.name,
            tier: i.placeable.tier,
            interactions: i
                .available_interactions
                .unwrap_or_default()
                .into_iter()
                .map(|v| v.verb_phrase.to_lowercase())
                .collect(),
            reported_end: i
                .growth_end_timestamp
                .as_ref()
                .and_then(parse_date)
                .or_else(|| i.next_state_at.as_ref().and_then(parse_date)),
            started_growing_at: i.updated_at.as_ref().and_then(parse_date),
            growth_duration: None,
        })
        .collect())
}

#[derive(Deserialize)]
struct PlaceableDetail {
    growth: Vec<Growth>,
}

#[derive(Deserialize)]
struct Growth {
    time: Vec<f64>,
}

/// Longest catalog growth time (seconds) of a placeable type, if it grows.
pub async fn growth_duration(placeable_id: i64) -> Result<Option<f64>> {
    let r: PlaceableDetail = get(&format!("placeables/{placeable_id}")).await?;
    Ok(r.growth.iter().flat_map(|g| g.time.iter().copied()).reduce(f64::max))
}

#[derive(Deserialize)]
struct PlayerResponse {
    player: PlayerSummary,
}

pub async fn player(id: &str) -> Result<PlayerSummary> {
    let r: PlayerResponse = get(&format!("players/{id}")).await?;
    Ok(r.player)
}

#[derive(Deserialize)]
struct SearchResponse {
    players: Vec<PlayerSummary>,
}

/// Substring search (≥ 2 chars, max 50, alphabetical). Re-ranked: exact,
/// prefix, rest.
pub async fn search_players(query: &str) -> Result<Vec<PlayerSummary>> {
    let q = query.trim();
    if q.chars().count() < 2 {
        return Ok(vec![]);
    }
    let mut url = reqwest::Url::parse(&format!("{BASE}players"))?;
    url.query_pairs_mut().append_pair("q", q);
    let response = client().get(url).send().await?;
    if !response.status().is_success() {
        bail!("search: {}", response.status());
    }
    let r: SearchResponse = response.json().await?;
    let lower = q.to_lowercase();
    let mut players = r.players;
    players.sort_by_key(|p| {
        let name = p.username.to_lowercase();
        if name == lower {
            0
        } else if name.starts_with(&lower) {
            1
        } else {
            2
        }
    });
    Ok(players)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassiveCraftResult {
    pub entity_id: String,
    pub building_name: Option<String>,
    pub recipe_id: i64,
    pub recipe_name: Option<String>,
    pub status: Option<String>,
    pub timestamp: Option<Value>,
    crafted_item: Vec<CraftedItem>,
}

impl PassiveCraftResult {
    pub fn item_id(&self) -> Option<i64> {
        self.crafted_item.first().map(|i| i.item_id)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassiveCraftsResponse {
    pub craft_results: Vec<PassiveCraftResult>,
    items: Vec<Item>,
}

impl PassiveCraftsResponse {
    pub fn item_name(&self, id: i64) -> Option<&str> {
        self.items.iter().find(|i| i.id == id).map(|i| i.name.as_str())
    }

    /// Item tag and tier.
    pub fn item_kind(&self, id: i64) -> (Option<String>, Option<i64>) {
        self.items.iter().find(|i| i.id == id).map_or((None, None), |i| (i.tag.clone(), i.tier))
    }
}

pub async fn passive_crafts(user: &str) -> Result<PassiveCraftsResponse> {
    get(&format!("players/{user}/passive-crafts")).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemDetail {
    crafting_recipes: Vec<Recipe>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Recipe {
    id: i64,
    time_requirement: f64,
}

/// Duration (seconds) of `recipe_id`, looked up via the crafted item.
pub async fn recipe_duration(item_id: i64, recipe_id: i64) -> Result<Option<f64>> {
    let r: ItemDetail = get(&format!("items/{item_id}")).await?;
    Ok(r.crafting_recipes.into_iter().find(|c| c.id == recipe_id).map(|c| c.time_requirement))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bitjita_dates() {
        let pg = parse_date(&Value::String("2026-11-24 09:03:18.5+00".into())).unwrap();
        let iso = parse_date(&Value::String("2026-11-24T09:03:18.5Z".into())).unwrap();
        assert!((pg - iso).abs() < 1e-3);
        assert_eq!(parse_date(&serde_json::json!(1795546305000i64)), Some(1795546305.0));
    }
}

/// A published GitHub release (update check).
#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
}

/// Latest published (non-draft, non-prerelease) release of this app on
/// GitHub. Repository from Cargo.toml `repository`.
pub async fn latest_release() -> Result<Release> {
    let repo = env!("CARGO_PKG_REPOSITORY").trim_end_matches('/');
    let Some(path) = repo.strip_prefix("https://github.com/") else { bail!("repository isn't on GitHub: {repo}") };
    let response = client()
        .get(format!("https://api.github.com/repos/{path}/releases/latest"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    if !response.status().is_success() {
        bail!("latest release: {}", response.status());
    }
    Ok(response.json().await?)
}
