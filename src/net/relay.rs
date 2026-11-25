//! Live passive crafts + plants from BitCraft's SpacetimeDB via BitJita's
//! read-only relay (`relay.bitjita.com`). No account/token needed. Protocol:
//! SpacetimeDB v1 JSON (`v1.json.spacetimedb`). See AGENTS.md "SpacetimeDB
//! relay" for tables, column indices and quirks.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::time::{interval, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::RelayState;
use crate::core::trackers::PassiveCraftTracker;
use crate::core::types::{CraftInfo, FoodBuff, FoodState, PassiveCraft, Plant, PlayerAction, now};

const HOST: &str = "relay.bitjita.com";
/// `buff_type_desc` ids: "Food Regen" (every food) and "Food Buffs" (extras).
const FOOD_REGEN_TYPE: i64 = 1790361536;
const FOOD_BUFFS_TYPE: i64 = 861783812;

/// Refused for another reason than "still loading" (e.g. region not mirrored).
const REFUSED_RETRY: Duration = Duration::from_secs(5 * 60);

/// Rows per table keyed by primary key (`pk_column`), ref-counted because join
/// queries can deliver the same row once per match.
#[derive(Default)]
struct Store {
    tables: HashMap<String, HashMap<String, (Vec<Value>, usize)>>,
}

impl Store {
    fn rows(&self, table: &str) -> impl Iterator<Item = &Vec<Value>> {
        self.tables.get(table).into_iter().flat_map(|t| t.values().map(|(row, _)| row))
    }

    fn get(&self, table: &str, key: &str) -> Option<&Vec<Value>> {
        self.tables.get(table)?.get(key).map(|(row, _)| row)
    }

    fn apply(&mut self, changes: &[Value], initial: bool) {
        if initial {
            // A (re-)subscribe re-sends everything subscribed.
            self.tables.clear();
        }
        for change in changes {
            let Some(name) = change["table_name"].as_str() else { continue };
            let table = self.tables.entry(name.to_string()).or_default();
            for update in change["updates"].as_array().into_iter().flatten() {
                let pk = pk_column(name);
                for row in rows(&update["deletes"]) {
                    let key = key(&row[pk]);
                    if let Some((_, count)) = table.get_mut(&key) {
                        if *count > 1 {
                            *count -= 1;
                        } else {
                            table.remove(&key);
                        }
                    }
                }
                for row in rows(&update["inserts"]) {
                    let key = key(&row[pk]);
                    let count = table.get(&key).map_or(0, |(_, c)| *c);
                    table.insert(key, (row, count + 1));
                }
            }
        }
    }
}

/// Primary key column per table (from the schema; 0 for all used tables
/// except `player_state`, whose column 0 is the teleport location).
fn pk_column(table: &str) -> usize {
    if table == "player_state" { 1 } else { 0 }
}

/// Rows are JSON strings of arrays (or arrays).
fn rows(value: &Value) -> Vec<Vec<Value>> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| match v {
            Value::String(s) => serde_json::from_str::<Vec<Value>>(s).ok(),
            Value::Array(a) => Some(a.clone()),
            _ => None,
        })
        .collect()
}

fn key(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn int(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_u64().map(|u| u as i64))
}

/// Sum type `[tag, payload]` → tag.
fn tag(v: &Value) -> Option<i64> {
    v.as_array()?.first().and_then(int)
}

/// Timestamp `[micros since epoch]` → seconds.
fn date(v: &Value) -> Option<f64> {
    let micros = v.as_array()?.first()?.as_f64()?;
    (micros > 0.0).then_some(micros / 1e6)
}

/// `crafting_recipe_desc.crafted_item_stacks[0].item_id` (column 13).
fn crafted_item_id(recipe: &[Value]) -> Option<i64> {
    recipe.get(13)?.as_array()?.first()?.as_array()?.first().and_then(int)
}

struct Feed {
    user: String,
    store: Store,
    item_ids: BTreeSet<i64>,
    building_desc_ids: BTreeSet<i64>,
    /// When each plant was seen switching to a growing stage (live only).
    growing_since: HashMap<String, f64>,
    growing_ids: HashSet<String>,
}

impl Feed {
    fn queries(&self) -> Vec<String> {
        let u = &self.user;
        let mut q = vec![
            format!("SELECT * FROM passive_craft_state WHERE owner_entity_id = {u}"),
            format!("SELECT crafting_recipe_desc.* FROM crafting_recipe_desc JOIN passive_craft_state ON crafting_recipe_desc.id = passive_craft_state.recipe_id WHERE passive_craft_state.owner_entity_id = {u}"),
            format!("SELECT building_state.* FROM building_state JOIN passive_craft_state ON building_state.entity_id = passive_craft_state.building_entity_id WHERE passive_craft_state.owner_entity_id = {u}"),
            // Active crafts (progress, recipe, count, building).
            format!("SELECT * FROM progressive_action_state WHERE owner_entity_id = {u}"),
            format!("SELECT crafting_recipe_desc.* FROM crafting_recipe_desc JOIN progressive_action_state ON crafting_recipe_desc.id = progressive_action_state.recipe_id WHERE progressive_action_state.owner_entity_id = {u}"),
            format!("SELECT building_state.* FROM building_state JOIN progressive_action_state ON building_state.entity_id = progressive_action_state.building_entity_id WHERE progressive_action_state.owner_entity_id = {u}"),
            format!("SELECT * FROM placeable_state WHERE owner_entity_id = {u}"),
            format!("SELECT placeable_desc.* FROM placeable_desc JOIN placeable_state ON placeable_desc.id = placeable_state.placeable_id WHERE placeable_state.owner_entity_id = {u}"),
            format!("SELECT growth_state.* FROM growth_state JOIN placeable_state ON growth_state.entity_id = placeable_state.entity_id WHERE placeable_state.owner_entity_id = {u}"),
        ];
        // Food buffs: the character's active buffs + the food buff types'
        // descriptions (names).
        q.push(format!("SELECT * FROM active_buff_state WHERE entity_id = {u}"));
        // Character rows (move with the player, like buffs): stats (max
        // stamina/satiation) and online status.
        q.push(format!("SELECT * FROM character_stats_state WHERE entity_id = {u}"));
        q.push(format!("SELECT * FROM player_state WHERE entity_id = {u}"));
        q.push(format!("SELECT * FROM health_state WHERE entity_id = {u}"));
        q.push(format!("SELECT * FROM player_action_state WHERE entity_id = {u}"));
        q.push(format!("SELECT * FROM buff_desc WHERE buff_type_id = {FOOD_REGEN_TYPE}"));
        q.push(format!("SELECT * FROM buff_desc WHERE buff_type_id = {FOOD_BUFFS_TYPE}"));
        q.extend(self.item_ids.iter().map(|id| format!("SELECT * FROM item_desc WHERE id = {id}")));
        q.extend(self.building_desc_ids.iter().map(|id| format!("SELECT * FROM building_desc WHERE id = {id}")));
        q
    }

    /// v1 `Subscribe` replaces the whole set; also how lookups are added.
    fn subscribe_message(&self) -> Message {
        Message::text(json!({"Subscribe": {"query_strings": self.queries(), "request_id": 1}}).to_string())
    }

    /// Item names (recipe "Weave {0}") and building names need a second
    /// lookup. True if new ids were added (→ re-subscribe).
    fn add_missing_lookups(&mut self) -> bool {
        let items: BTreeSet<i64> = self.store.rows("crafting_recipe_desc").filter_map(|r| crafted_item_id(r)).collect();
        let buildings: BTreeSet<i64> =
            self.store.rows("building_state").filter_map(|r| r.get(3).and_then(int)).collect();
        if items.is_subset(&self.item_ids) && buildings.is_subset(&self.building_desc_ids) {
            return false;
        }
        self.item_ids.extend(items);
        self.building_desc_ids.extend(buildings);
        true
    }

    /// Active food buffs: `active_buff_state.active_buffs[]` =
    /// `[buff_id, [start (Unix s)], duration (s), values]`; inactive entries
    /// have start 0. Only food types are subscribed in `buff_desc`, so a
    /// match there means it's a food buff.
    fn food_buffs(&self) -> Vec<(i64, FoodBuff)> {
        let now = now();
        let Some(row) = self.store.rows("active_buff_state").next() else { return vec![] };
        row.get(1)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|buff| {
                let buff = buff.as_array()?;
                let id = buff.first().and_then(int)?;
                let start = buff.get(1)?.as_array()?.first()?.as_f64()?;
                let duration = buff.get(2)?.as_f64()?;
                let desc = self.store.get("buff_desc", &id.to_string())?;
                let ends_at = start + duration;
                (start > 0.0 && ends_at > now).then(|| {
                    (
                        id,
                        FoodBuff {
                            name: desc.get(2).and_then(Value::as_str).unwrap_or("Food buff").to_string(),
                            is_regen: desc.get(1).and_then(int) == Some(FOOD_REGEN_TYPE),
                            ends_at,
                            duration,
                        },
                    )
                })
            })
            .collect()
    }

    /// `player_state` = [teleport_location, entity_id, time_played,
    /// session_start_timestamp, time_signed_in, sign_in_timestamp,
    /// signed_in, traveler_tasks_expiration] (pk is column 1, but the
    /// filtered query returns at most one row). None: the player isn't in
    /// this region.
    fn signed_in(&self) -> Option<bool> {
        self.store.rows("player_state").next()?.get(6)?.as_bool()
    }

    /// `character_stats_state` = [entity_id, values]; same `values` as REST
    /// `players/<id>/stats` (1 = max stamina, 19 = max satiation).
    fn stats(&self) -> Option<Vec<f64>> {
        let values = self.store.rows("character_stats_state").next()?.get(1)?.as_array()?;
        Some(values.iter().filter_map(Value::as_f64).collect())
    }

    /// `health_state` = [entity_id, last_health_decrease_timestamp, health,
    /// died_timestamp]; max health is stats value 0.
    fn health(&self) -> Option<f64> {
        self.store.rows("health_state").next()?.get(2)?.as_f64()
    }

    /// `player_action_state` = [auto_id, chunk_index, entity_id, start_time,
    /// duration, target, recipe_id, action_type, layer (Base/UpperBody),
    /// last_action_result, client_cancel, was_consumed, …]. The Base layer
    /// row with a real action. start_time/duration are plain u64s; the unit
    /// is detected from start_time's magnitude (and applied to both).
    fn action(&self) -> Option<PlayerAction> {
        self.store
            .rows("player_action_state")
            .filter(|r| r.get(8).and_then(tag) == Some(0))
            .find_map(|r| {
                let kind = *ACTION_TYPES.get(r.get(7).and_then(tag)? as usize)?;
                let start = r.get(3)?.as_f64()?;
                let scale = if start > 1e14 { 1e6 } else if start > 1e11 { 1e3 } else { 1.0 };
                Some(PlayerAction { kind, start: start / scale, duration: r.get(4)?.as_f64()? / scale })
            })
    }

    fn food(&self) -> FoodState {
        FoodState { buffs: self.food_buffs().into_iter().map(|(_, b)| b).collect() }
    }

    /// Building name via `building_state` → `building_desc`.
    fn building_name(&self, building_entity: &Value) -> Option<String> {
        self.store
            .get("building_state", &key(building_entity))
            .and_then(|b| b.get(3).and_then(int))
            .and_then(|id| self.store.get("building_desc", &id.to_string()))
            .and_then(|d| d.get(2)?.as_str().map(str::to_string))
    }

    /// Active crafts: `progressive_action_state` = [entity_id,
    /// building_entity_id, function_type, progress, recipe_id, craft_count,
    /// last_crit_outcome, owner, lock_expiration, preparation]. Total =
    /// recipe `actions_required` (column 14) × craft_count (matches REST
    /// `totalActionsRequired`).
    fn crafts(&self) -> Vec<CraftInfo> {
        self.store
            .rows("progressive_action_state")
            .map(|row| {
                let recipe = self.store.get("crafting_recipe_desc", &key(&row[4]));
                let item = recipe
                    .and_then(|r| crafted_item_id(r))
                    .and_then(|id| self.store.get("item_desc", &id.to_string()));
                let item_name = item.and_then(|r| r.get(1)?.as_str());
                let (kind, tier) = item_kind(item);
                let progress = row.get(3).and_then(int).unwrap_or(0);
                let count = row.get(5).and_then(int).unwrap_or(1);
                let per_item = recipe.and_then(|r| r.get(14).and_then(int)).unwrap_or(0);
                let total = per_item * count;
                CraftInfo {
                    id: key(&row[0]),
                    name: PassiveCraftTracker::display_name(recipe.and_then(|r| r.get(1)?.as_str()), item_name),
                    building: self.building_name(&row[1]),
                    progress,
                    total,
                    completed: progress >= total,
                    kind,
                    tier,
                }
            })
            // Recipe not loaded yet: total unknown (0 would look finished).
            .filter(|c| c.total > 0)
            .collect()
    }

    fn passive_crafts(&self) -> Vec<PassiveCraft> {
        self.store
            .rows("passive_craft_state")
            .map(|row| {
                let recipe = self.store.get("crafting_recipe_desc", &key(&row[2]));
                let item_id = recipe.and_then(|r| crafted_item_id(r));
                let item = item_id.and_then(|id| self.store.get("item_desc", &id.to_string()));
                let item_name = item.and_then(|r| r.get(1)?.as_str());
                let (kind, tier) = item_kind(item);
                let building = self.building_name(&row[3]);
                // status: 0 Queued, 1 Processing, 2 Complete.
                let status = row.get(5).and_then(tag);
                PassiveCraft {
                    id: key(&row[0]),
                    name: PassiveCraftTracker::display_name(recipe.and_then(|r| r.get(1)?.as_str()), item_name),
                    building: building.unwrap_or_else(|| "Building".into()),
                    // Queued: not started; end unknown until Processing.
                    started_at: if status == Some(0) { None } else { row.get(4).and_then(date) },
                    reported_complete: status == Some(2),
                    duration: recipe.and_then(|r| r.get(2)?.as_f64()),
                    kind,
                    tier,
                }
            })
            .collect()
    }

    fn plants(&mut self, initial: bool) -> Vec<Plant> {
        let now = now();
        let mut growing = HashSet::new();
        let mut plants = Vec::new();
        let rows: Vec<Vec<Value>> = self.store.rows("placeable_state").cloned().collect();
        for row in rows {
            let id = key(&row[0]);
            let Some(placeable_id) = row.get(2).and_then(int) else { continue };
            let Some(desc) = self.store.get("placeable_desc", &placeable_id.to_string()) else { continue };
            let Some(name) = desc.get(1).and_then(Value::as_str).map(str::to_string) else { continue };
            let tier = desc.get(3).and_then(int).unwrap_or(0);
            if name.starts_with("Growing ") {
                growing.insert(id.clone());
                if !initial && !self.growing_ids.contains(&id) {
                    self.growing_since.entry(id.clone()).or_insert(now);
                }
            } else {
                self.growing_since.remove(&id);
            }
            plants.push(Plant {
                placeable_id,
                name,
                tier,
                interactions: vec![],
                reported_end: self.store.get("growth_state", &id).and_then(|g| g.get(1).and_then(date)),
                started_growing_at: self.growing_since.get(&id).copied(),
                growth_duration: None,
            });
        }
        self.growing_ids = growing;
        plants
    }
}

/// `player_action_state.action_type` variants, in schema order.
const ACTION_TYPES: [&str; 34] = [
    "None", "Attack", "DestroyPaving", "StationaryEmote", "Extract", "PaveTile", "SpawnCargo", "Build",
    "Deconstruct", "RepairBuilding", "ResupplyClaim", "CargoPickUp", "Terraform", "DeployDeployable",
    "StoreDeployable", "Sleep", "Teleport", "Death", "Climb", "UseItem", "Craft", "ConvertItems", "PlayerMove",
    "DeployableMove", "ResupplyEmpireNode", "SetHome", "UseElevator", "MobileEmote", "PlacePillarShaping",
    "DestroyPillarShaping", "AbilityCustom", "Prospect", "PlacePlaceable", "InteractPlaceable",
];

/// `item_desc` tag (column 10, "Ingot") and tier (column 9).
fn item_kind(item: Option<&Vec<Value>>) -> (Option<String>, Option<i64>) {
    let Some(item) = item else { return (None, None) };
    (item.get(10).and_then(Value::as_str).map(str::to_string), item.get(9).and_then(int))
}

/// Follows `user`'s passive crafts and plants in `region` until cancelled.
/// Everything one region's database has for the character, after an update.
#[derive(Default)]
pub struct RegionData {
    pub crafts: Vec<CraftInfo>,
    pub passive: Vec<PassiveCraft>,
    pub plants: Vec<Plant>,
    pub food: FoodState,
    /// Character rows; Some only in the region the player is in.
    pub signed_in: Option<bool>,
    pub stats: Option<Vec<f64>>,
    pub health: Option<f64>,
    pub action: Option<PlayerAction>,
}

pub enum RegionUpdate {
    State(RelayState),
    Data(RegionData),
}

/// "module is still loading its initial state from upstream; retry shortly"
/// (relay restarting) vs a lasting refusal (e.g. region not mirrored).
pub fn is_transient(reason: &str) -> bool {
    reason.contains("loading") || reason.contains("retry")
}

/// Follows `user`'s rows in one region's database until cancelled.
pub async fn run(user: String, region: i64, emit: impl Fn(RegionUpdate)) {
    let mut feed = Feed {
        user,
        store: Store::default(),
        item_ids: BTreeSet::new(),
        building_desc_ids: BTreeSet::new(),
        growing_since: HashMap::new(),
        growing_ids: HashSet::new(),
    };
    let mut backoff = Duration::from_secs(1);
    loop {
        emit(RegionUpdate::State(RelayState::Connecting));
        let wait = match session(&mut feed, region, &emit, &mut backoff).await {
            Err(reason) => {
                // "module is still loading its initial state from upstream;
                // retry shortly" happens while the relay restarts.
                let transient = is_transient(&reason);
                emit(RegionUpdate::State(RelayState::Unavailable(reason)));
                if transient { backoff.max(Duration::from_secs(5)) } else { REFUSED_RETRY }
            }
            Ok(()) => {
                emit(RegionUpdate::State(RelayState::Connecting));
                backoff
            }
        };
        tokio::time::sleep(wait).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

/// Ok(()) = connection dropped (retry); Err = subscription refused.
async fn session(
    feed: &mut Feed,
    region: i64,
    emit: &impl Fn(RegionUpdate),
    backoff: &mut Duration,
) -> Result<(), String> {
    let url = format!("wss://{HOST}/v1/database/idc-{region}/subscribe");
    let Ok(mut request) = url.into_client_request() else { return Ok(()) };
    request.headers_mut().insert("Sec-WebSocket-Protocol", "v1.json.spacetimedb".parse().expect("header"));
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(64 << 20))
        .max_frame_size(Some(64 << 20));
    let connect = tokio_tungstenite::connect_async_with_config(request, Some(config), false);
    let Ok(Ok((ws, _))) = timeout(Duration::from_secs(20), connect).await else { return Ok(()) };
    let (mut sink, mut stream) = ws.split();
    if sink.send(feed.subscribe_message()).await.is_err() {
        return Ok(());
    }
    let mut ping = interval(Duration::from_secs(15));
    ping.tick().await;
    let mut awaiting_pong: Option<tokio::time::Instant> = None;

    loop {
        tokio::select! {
            message = stream.next() => {
                let text = match message {
                    Some(Ok(Message::Text(t))) => t.to_string(),
                    Some(Ok(Message::Binary(b))) => String::from_utf8_lossy(&b).into_owned(),
                    Some(Ok(Message::Pong(_))) => { awaiting_pong = None; continue }
                    Some(Ok(_)) => continue,
                    Some(Err(_)) | None => return Ok(()),
                };
                let Ok(message) = serde_json::from_str::<Value>(&text) else { continue };
                // BITCRAFT_DEBUG: log everything except the frequent updates.
                if std::env::var_os("BITCRAFT_DEBUG").is_some() && message.get("TransactionUpdate").is_none() {
                    let kind = message.as_object().and_then(|o| o.keys().next().cloned()).unwrap_or_default();
                    let tables: Vec<String> = message
                        .pointer("/InitialSubscription/database_update/tables")
                        .or_else(|| message.pointer("/TransactionUpdate/status/Committed/tables"))
                        .and_then(Value::as_array)
                        .map(|t| t.iter().map(|t| format!("{}:{}", t["table_name"].as_str().unwrap_or("?"), t["num_rows"])).collect())
                        .unwrap_or_default();
                    let status = message.pointer("/TransactionUpdate/status").and_then(|s| s.as_object()).and_then(|o| o.keys().next().cloned()).unwrap_or_default();
                    eprintln!("relay idc-{region}: {kind} {status} {tables:?} {}", if tables.is_empty() { text.chars().take(300).collect::<String>() } else { String::new() });
                }
                if let Some(error) = message.get("SubscriptionError") {
                    return Err(error["error"].as_str().unwrap_or("subscription refused").to_string());
                }
                let (changes, initial) = if let Some(init) = message.get("InitialSubscription") {
                    (init["database_update"]["tables"].clone(), true)
                } else if let Some(committed) = message.get("TransactionUpdate").and_then(|u| u["status"].get("Committed")) {
                    (committed["tables"].clone(), false)
                } else {
                    continue;
                };
                feed.store.apply(changes.as_array().map(Vec::as_slice).unwrap_or_default(), initial);
                *backoff = Duration::from_secs(1);
                emit(RegionUpdate::State(RelayState::Live));
                emit(RegionUpdate::Data(RegionData {
                    crafts: feed.crafts(),
                    passive: feed.passive_crafts(),
                    plants: feed.plants(initial),
                    food: feed.food(),
                    signed_in: feed.signed_in(),
                    stats: feed.stats(),
                    health: feed.health(),
                    action: feed.action(),
                }));
                if feed.add_missing_lookups() && sink.send(feed.subscribe_message()).await.is_err() {
                    return Ok(());
                }
            },
            _ = ping.tick() => {
                if awaiting_pong.is_some_and(|sent| sent.elapsed() >= Duration::from_secs(10)) {
                    return Ok(());
                }
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return Ok(());
                }
                awaiting_pong.get_or_insert_with(tokio::time::Instant::now);
            },
        }
    }
}
