//! Everything about the character (XP, stamina, satiation, crafts, passive
//! crafts, plants, food, stats, health, position, online status, username)
//! from BitCraft's SpacetimeDB via the community read-only relay
//! `relay.bitcraftsync.app` (one port per region:
//! 3000 + region). No account/token needed. Protocol:
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
use crate::core::types::{CraftInfo, FoodBuffDesc, Gather, PassiveCraft, Plant, Position, RawBuff, TradeOffer, now};

const HOST: &str = "relay.bitcraftsync.app";
/// Each region is its own relay instance on port `PORT_BASE + region`.
const PORT_BASE: i64 = 3000;
/// `buff_type_desc` ids: "Food Regen" (every food) and "Food Buffs" (extras).
const FOOD_REGEN_TYPE: i64 = 1790361536;
const FOOD_BUFFS_TYPE: i64 = 861783812;

/// Refused for another reason than "still loading" (e.g. region not mirrored).
const REFUSED_RETRY: Duration = Duration::from_secs(5 * 60);

/// Rows per table keyed by primary key (`pk_column`), ref-counted because join
/// queries can deliver the same row once per match, and several queries the
/// same row. Fresh per connection; queries add rows when applied and remove
/// them when unsubscribed.
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

    /// Table updates (`[{table_name, updates: [{deletes, inserts}]}]`).
    fn apply(&mut self, changes: &[Value]) {
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

/// Rows are JSON strings of objects with the columns in schema order
/// (subscribe replies) or of arrays (`TransactionUpdate`); both become
/// column arrays.
/// Object order relies on serde_json's `preserve_order`.
fn rows(value: &Value) -> Vec<Vec<Value>> {
    fn columns(v: Value) -> Option<Vec<Value>> {
        match v {
            Value::Array(a) => Some(a),
            Value::Object(o) => Some(o.into_iter().map(|(_, v)| v).collect()),
            _ => None,
        }
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| match v {
            Value::String(s) => serde_json::from_str::<Value>(s).ok().and_then(columns),
            other => columns(other.clone()),
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

/// Timestamp `[micros since epoch]` or
/// `{"__timestamp_micros_since_unix_epoch__": micros}` → seconds.
fn date(v: &Value) -> Option<f64> {
    let micros = match v {
        Value::Array(a) => a.first()?.as_f64()?,
        Value::Object(o) => o.values().next()?.as_f64()?,
        _ => return None,
    };
    (micros > 0.0).then_some(micros / 1e6)
}

/// `crafting_recipe_desc.crafted_item_stacks[0].item_id` (column 13).
fn crafted_item_id(recipe: &[Value]) -> Option<i64> {
    recipe.get(13)?.as_array()?.first()?.as_array()?.first().and_then(int)
}

/// `query_id` of the fixed per-character `SubscribeMulti`; lookups count
/// up from the next one.
const STABLE_QUERY: u32 = 1;

struct Feed {
    user: String,
    store: Store,
    /// Lookup query → its `query_id` (subscribed or awaiting its reply).
    lookups: HashMap<String, u32>,
    /// Lookups sent but not applied yet: data is held until they are, so
    /// names never show half-resolved.
    pending: HashSet<u32>,
    stable_pending: bool,
    next_id: u32,
    /// When each plant was seen switching to a growing stage (live only).
    growing_since: HashMap<String, f64>,
    growing_ids: HashSet<String>,
    /// Last gather target seen alive (kept with health 0 once it's gone).
    last_gather: Option<Gather>,
}

impl Feed {
    fn new(user: String) -> Self {
        Feed {
            user,
            store: Store::default(),
            lookups: HashMap::new(),
            pending: HashSet::new(),
            stable_pending: true,
            next_id: STABLE_QUERY + 1,
            growing_since: HashMap::new(),
            growing_ids: HashSet::new(),
            last_gather: None,
        }
    }

    /// The fixed set: everything filtered by the character. Sent once per
    /// connection (`SubscribeMulti`), never changed.
    fn stable_queries(&self) -> Vec<String> {
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
            // Growth end per plant (exact), and the catalog growth time
            // (fallback estimate while a timer row is missing).
            format!("SELECT placeable_growth_timer.* FROM placeable_growth_timer JOIN placeable_state ON placeable_growth_timer.entity_id = placeable_state.entity_id WHERE placeable_state.owner_entity_id = {u}"),
            format!("SELECT placeable_growth_desc.* FROM placeable_growth_desc JOIN placeable_state ON placeable_growth_desc.placeable_id = placeable_state.placeable_id WHERE placeable_state.owner_entity_id = {u}"),
        ];
        // Food buffs: the character's active buffs + the food buff types'
        // descriptions (names).
        q.push(format!("SELECT * FROM active_buff_state WHERE entity_id = {u}"));
        // Character rows (move with the player, like buffs): stats (max
        // stamina/satiation) and online status.
        q.push(format!("SELECT * FROM character_stats_state WHERE entity_id = {u}"));
        q.push(format!("SELECT * FROM player_state WHERE entity_id = {u}"));
        q.push(format!("SELECT * FROM health_state WHERE entity_id = {u}"));
        q.push(format!("SELECT * FROM mobile_entity_state WHERE entity_id = {u}"));
        q.push(format!("SELECT * FROM experience_state WHERE entity_id = {u}"));
        q.push(format!("SELECT * FROM stamina_state WHERE entity_id = {u}"));
        q.push(format!("SELECT * FROM satiation_state WHERE entity_id = {u}"));
        q.push(format!("SELECT * FROM player_username_state WHERE entity_id = {u}"));
        // Gather target (last resource hit): health, type, in-game timer.
        // The joins follow the target, so they only ever hold its rows.
        q.push(format!("SELECT * FROM extract_outcome_state WHERE entity_id = {u}"));
        for table in ["resource_state", "resource_health_state", "resource_growth_timer"] {
            q.push(format!("SELECT {table}.* FROM {table} JOIN extract_outcome_state ON {table}.entity_id = extract_outcome_state.target_entity_id WHERE extract_outcome_state.entity_id = {u}"));
        }
        // Trades other players start with the character.
        q.push(format!("SELECT * FROM trade_session_state WHERE acceptor_entity_id = {u}"));
        q.push(format!("SELECT * FROM buff_desc WHERE buff_type_id = {FOOD_REGEN_TYPE}"));
        q.push(format!("SELECT * FROM buff_desc WHERE buff_type_id = {FOOD_BUFFS_TYPE}"));
        q
    }

    /// Lookups the current rows need (joins can't chain these): crafted
    /// item and building names for the crafts and passive crafts, and the
    /// gather target's type (`resource_desc`, extraction recipe → skill,
    /// growth recipe → timer duration). Only what's referenced right now,
    /// so it never grows: one gather target at a time.
    fn wanted_lookups(&self) -> BTreeSet<String> {
        let mut q = BTreeSet::new();
        for id in self.store.rows("crafting_recipe_desc").filter_map(|r| crafted_item_id(r)) {
            q.insert(format!("SELECT * FROM item_desc WHERE id = {id}"));
        }
        for id in self.store.rows("building_state").filter_map(|r| r.get(3).and_then(int)) {
            q.insert(format!("SELECT * FROM building_desc WHERE id = {id}"));
        }
        for id in self.store.rows("resource_state").filter_map(|r| r.get(1).and_then(int)) {
            q.insert(format!("SELECT * FROM resource_desc WHERE id = {id}"));
            q.insert(format!("SELECT * FROM extraction_recipe_desc WHERE resource_id = {id}"));
        }
        for id in self.store.rows("resource_growth_timer").filter_map(|r| r.get(3).and_then(int)) {
            q.insert(format!("SELECT * FROM resource_growth_recipe_desc WHERE id = {id}"));
        }
        // Trade partner names.
        for row in self.store.rows("trade_session_state") {
            if let Some(id) = row.get(2).and_then(int) {
                q.insert(format!("SELECT * FROM player_username_state WHERE entity_id = {id}"));
            }
        }
        q
    }

    fn next_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id - 1
    }

    /// Messages that bring the lookups in line with [`Self::wanted_lookups`]:
    /// `SubscribeSingle` per new query, `Unsubscribe` per one no longer
    /// referenced. Nothing else is re-sent.
    fn sync_lookups(&mut self) -> Vec<Message> {
        let wanted = self.wanted_lookups();
        let mut messages = Vec::new();
        let stale: Vec<String> = self.lookups.keys().filter(|q| !wanted.contains(*q)).cloned().collect();
        for query in stale {
            let Some(id) = self.lookups.remove(&query) else { continue };
            // If its reply is still on the way, the unsubscribe reply
            // (sent after it) removes those rows again.
            self.pending.remove(&id);
            let request = self.next_id();
            messages.push(json!({"Unsubscribe": {"request_id": request, "query_id": {"id": id}}}));
        }
        for query in wanted {
            if self.lookups.contains_key(&query) {
                continue;
            }
            let id = self.next_id();
            self.lookups.insert(query.clone(), id);
            self.pending.insert(id);
            messages.push(json!({"SubscribeSingle": {"query": query, "request_id": id, "query_id": {"id": id}}}));
        }
        messages.into_iter().map(|m| Message::text(m.to_string())).collect()
    }

    /// A new connection starts empty: the fixed set, then the lookups
    /// again as its rows come in.
    fn reset_connection(&mut self) {
        self.store = Store::default();
        self.lookups.clear();
        self.pending.clear();
        self.stable_pending = true;
    }

    fn stable_message(&self) -> Message {
        Message::text(json!({"SubscribeMulti": {"query_strings": self.stable_queries(), "request_id": STABLE_QUERY, "query_id": {"id": STABLE_QUERY}}}).to_string())
    }

    /// The resource last hit: `extract_outcome_state` = [entity_id,
    /// target_entity_id, last_timestamp, damage, is_crit] → target's
    /// `resource_state` = [entity_id, resource_id, direction],
    /// `resource_health_state` = [entity_id, health], `resource_desc` =
    /// [id, name, …, max_health (4), …, tier (13), tag (14), …],
    /// `extraction_recipe_desc` (by resource_id, column 1) column 15
    /// `experience_per_progress` = [[skill_id, xp], …], and
    /// `resource_growth_timer` = [scheduled_id, scheduled_at (1 = Time),
    /// entity_id, growth_recipe_id] → `resource_growth_recipe_desc` = [id,
    /// resource_id, time [min, max], …]. A used-up resource's rows are
    /// deleted: while the player's last hit still points at it, the last
    /// seen state is kept with health 0 (that's how it "dies"). None while
    /// its type isn't looked up yet, or for a target this feed never saw
    /// alive (e.g. right after traveling).
    fn gather(&mut self) -> Option<Gather> {
        let outcome = self.store.rows("extract_outcome_state").next()?;
        let target = key(outcome.get(1)?);
        let last_hit_at = outcome.get(2).and_then(date)?;
        match self.live_gather(&target, last_hit_at) {
            Some(g) => {
                self.last_gather = Some(g.clone());
                Some(g)
            }
            None => {
                let last = self.last_gather.as_ref().filter(|g| g.target == target)?;
                Some(Gather { health: Some(0.0), last_hit_at, ..last.clone() })
            }
        }
    }

    fn live_gather(&self, target: &str, last_hit_at: f64) -> Option<Gather> {
        let target = target.to_string();
        let resource_id = self.store.get("resource_state", &target)?.get(1).and_then(int)?;
        let desc = self.store.get("resource_desc", &resource_id.to_string())?;
        let skill = self
            .store
            .rows("extraction_recipe_desc")
            .find(|r| r.get(1).and_then(int) == Some(resource_id))
            .and_then(|r| r.get(15)?.as_array()?.first()?.as_array()?.first().and_then(int));
        let timer = self.store.rows("resource_growth_timer").find(|t| t.get(2).map(key).as_deref() == Some(target.as_str()));
        let ends_at = timer.and_then(|t| {
            let at = t.get(1)?.as_array()?;
            (at.first().and_then(int) == Some(1)).then(|| at.get(1).and_then(date)).flatten()
        });
        let timer_duration = timer
            .and_then(|t| t.get(3).and_then(int))
            .and_then(|id| self.store.get("resource_growth_recipe_desc", &id.to_string()))
            .and_then(|r| r.get(2)?.as_array()?.iter().filter_map(Value::as_f64).reduce(f64::max));
        Some(Gather {
            target: target.clone(),
            name: desc.get(1).and_then(Value::as_str).unwrap_or("Resource").to_string(),
            tier: desc.get(13).and_then(int).filter(|t| *t > 0),
            skill,
            health: self.store.get("resource_health_state", &target).and_then(|h| h.get(1)?.as_f64()),
            max_health: desc.get(4).and_then(Value::as_f64),
            ends_at,
            timer_duration,
            last_hit_at,
        })
    }

    /// Started buffs: `active_buff_state.active_buffs[]` =
    /// `[buff_id, [start (Unix s)], duration (s), values]`; inactive entries
    /// have start 0. Which are food: [`Self::food_descs`].
    fn buffs(&self) -> Vec<RawBuff> {
        let Some(row) = self.store.rows("active_buff_state").next() else { return vec![] };
        row.get(1).and_then(super::parse_buffs).unwrap_or_default()
    }

    /// Only food types are subscribed in `buff_desc` = [id, buff_type_id,
    /// name, …].
    fn food_descs(&self) -> HashMap<i64, FoodBuffDesc> {
        self.store
            .rows("buff_desc")
            .filter_map(|d| {
                let id = d.first().and_then(int)?;
                let name = d.get(2).and_then(Value::as_str).unwrap_or("Food buff").to_string();
                Some((id, FoodBuffDesc { name, is_regen: d.get(1).and_then(int) == Some(FOOD_REGEN_TYPE) }))
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

    /// `mobile_entity_state` = [entity_id, chunk_index, timestamp,
    /// location_x, location_z, destination_x, destination_z, dimension,
    /// is_walking, …]: current position, for movement detection.
    fn position(&self) -> Option<Position> {
        let row = self.store.rows("mobile_entity_state").next()?;
        Some(Position { x: row.get(3).and_then(int)?, z: row.get(4).and_then(int)? })
    }

    /// `experience_state` = [entity_id, experience_stacks [[skill_id,
    /// quantity], …]].
    fn experience(&self) -> Option<Vec<(i64, i64)>> {
        let stacks = self.store.rows("experience_state").next()?.get(1)?.as_array()?;
        Some(stacks.iter().filter_map(|s| Some((int(s.get(0)?)?, int(s.get(1)?)?))).collect())
    }

    /// `stamina_state` = [entity_id, last_stamina_decrease_timestamp,
    /// stamina].
    fn stamina(&self) -> Option<f64> {
        self.store.rows("stamina_state").next()?.get(2)?.as_f64()
    }

    /// `satiation_state` = [entity_id, satiation].
    fn satiation(&self) -> Option<f64> {
        self.store.rows("satiation_state").next()?.get(1)?.as_f64()
    }

    /// `player_username_state` = [entity_id, username].
    /// (By id: trade partners' names are in the same table.)
    fn username(&self) -> Option<String> {
        self.store.get("player_username_state", &self.user)?.get(1)?.as_str().map(str::to_string)
    }

    /// Trades offered to the character: `trade_session_state` = [entity_id,
    /// status (0 sessionOffered, 1 sessionAccepted, 2 initiatorAccepted,
    /// 3 acceptorAccepted, 4 sessionResolved), initiator_entity_id,
    /// acceptor_entity_id, initiator_offer, acceptor_offer, updated_at,
    /// resolution_message]; only status 0. Name via `player_username_state`.
    fn trade_offers(&self) -> Vec<TradeOffer> {
        self.store
            .rows("trade_session_state")
            .filter(|r| r.get(1).and_then(tag) == Some(0))
            .map(|r| TradeOffer {
                id: key(&r[0]),
                from: r
                    .get(2)
                    .and_then(|i| self.store.get("player_username_state", &key(i)))
                    .and_then(|u| u.get(1)?.as_str().map(str::to_string)),
            })
            .collect()
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
                    // `experience_per_progress` (column 12) = [[skill_id, xp], …].
                    skill: recipe.and_then(|r| r.get(12)?.as_array()?.first()?.as_array()?.first().and_then(int)),
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

    /// Plants: `placeable_state` = [entity_id, owner, placeable_id,
    /// direction] → `placeable_desc` = [id, name, tag, tier, …]. Growth end:
    /// `placeable_growth_timer` = [scheduled_id, scheduled_at (sum: 0
    /// Interval, 1 Time), entity_id, growth_recipe_id]. Catalog duration:
    /// `placeable_growth_desc` = [id, placeable_id, time [min, max], …].
    fn plants(&mut self, initial: bool) -> Vec<Plant> {
        let now = now();
        let mut growing = HashSet::new();
        let mut plants = Vec::new();
        let ends: HashMap<String, f64> = self
            .store
            .rows("placeable_growth_timer")
            .filter_map(|t| {
                let at = t.get(1)?.as_array()?;
                if at.first().and_then(int) != Some(1) {
                    return None;
                }
                Some((key(t.get(2)?), at.get(1).and_then(date)?))
            })
            .collect();
        let durations: HashMap<i64, f64> = self
            .store
            .rows("placeable_growth_desc")
            .filter_map(|d| {
                let max = d.get(2)?.as_array()?.iter().filter_map(Value::as_f64).reduce(f64::max)?;
                Some((d.get(1).and_then(int)?, max))
            })
            .collect();
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
                name,
                tier,
                reported_end: ends.get(&id).copied(),
                started_growing_at: self.growing_since.get(&id).copied(),
                growth_duration: durations.get(&placeable_id).copied(),
            });
        }
        self.growing_ids = growing;
        plants
    }
}

/// `item_desc` tag (column 10, "Ingot") and tier (column 9).
fn item_kind(item: Option<&Vec<Value>>) -> (Option<String>, Option<i64>) {
    let Some(item) = item else { return (None, None) };
    (item.get(10).and_then(Value::as_str).map(str::to_string), item.get(9).and_then(int))
}

/// Everything one region's database has for the character, after an update.
#[derive(Default)]
pub struct RegionData {
    pub crafts: Vec<CraftInfo>,
    pub passive: Vec<PassiveCraft>,
    pub plants: Vec<Plant>,
    /// Player's started buffs (meaningful in the player's region only).
    pub buffs: Vec<RawBuff>,
    pub food_descs: HashMap<i64, FoodBuffDesc>,
    /// Character rows; Some only in the region the player is in.
    pub signed_in: Option<bool>,
    pub stats: Option<Vec<f64>>,
    pub health: Option<f64>,
    pub position: Option<Position>,
    pub experience: Option<Vec<(i64, i64)>>,
    pub stamina: Option<f64>,
    pub satiation: Option<f64>,
    pub username: Option<String>,
    pub gather: Option<Gather>,
    pub trades: Vec<TradeOffer>,
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
    let mut feed = Feed::new(user);
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
            // Dropped or couldn't connect: disconnected until the retry.
            Ok(()) => {
                emit(RegionUpdate::State(RelayState::Disconnected));
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
    let port = PORT_BASE + region;
    let url = format!("wss://{HOST}:{port}/v1/database/idc-{region}/subscribe?compression=None");
    let Ok(mut request) = url.into_client_request() else { return Ok(()) };
    request.headers_mut().insert("Sec-WebSocket-Protocol", "v1.json.spacetimedb".parse().expect("header"));
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(64 << 20))
        .max_frame_size(Some(64 << 20));
    let connect = tokio_tungstenite::connect_async_with_config(request, Some(config), false);
    let ws = match timeout(Duration::from_secs(20), connect).await {
        Ok(Ok((ws, _))) => ws,
        // Ports without a mirrored region behind them answer nginx 502:
        // a lasting refusal (retried every 5 min), not a dropped connection.
        Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response))) if response.status().as_u16() == 502 => {
            return Err(format!("region {region} not mirrored (HTTP 502)"));
        }
        _ => return Ok(()),
    };
    let (mut sink, mut stream) = ws.split();
    feed.reset_connection();
    if sink.send(feed.stable_message()).await.is_err() {
        return Ok(());
    }
    let mut ping = interval(Duration::from_secs(15));
    ping.tick().await;
    let mut awaiting_pong: Option<tokio::time::Instant> = None;
    // First data of this connection: plants already growing weren't just
    // seen switching to growing.
    let mut first_data = true;

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
                let kind = message.as_object().and_then(|o| o.keys().next().cloned()).unwrap_or_default();
                let body = &message[kind.as_str()];
                let query_id = body.pointer("/query_id/id").and_then(Value::as_u64).map(|id| id as u32);
                // BITCRAFT_DEBUG: log everything except the frequent updates.
                if std::env::var_os("BITCRAFT_DEBUG").is_some() && kind != "TransactionUpdate" {
                    let tables: Vec<String> = changes(&kind, body)
                        .iter()
                        .map(|t| format!("{}:{}", t["table_name"].as_str().unwrap_or("?"), t["num_rows"]))
                        .collect();
                    let extra = if tables.is_empty() { text.chars().take(300).collect::<String>() } else { String::new() };
                    eprintln!("relay idc-{region}: {kind} {query_id:?} {tables:?} {extra}");
                }
                match kind.as_str() {
                    "SubscriptionError" => {
                        let error = body["error"].as_str().unwrap_or("subscription refused").to_string();
                        match query_id {
                            // A lookup failed: drop it, keep the connection.
                            Some(id) if id != STABLE_QUERY => {
                                eprintln!("relay idc-{region}: lookup {id} refused: {error}");
                                feed.lookups.retain(|_, q| *q != id);
                                feed.pending.remove(&id);
                            }
                            // The fixed set (or the connection) was refused.
                            _ => return Err(error),
                        }
                    }
                    "SubscribeMultiApplied" if query_id == Some(STABLE_QUERY) => feed.stable_pending = false,
                    "SubscribeApplied" => {
                        if let Some(id) = query_id {
                            feed.pending.remove(&id);
                        }
                    }
                    "UnsubscribeApplied" | "TransactionUpdate" => {}
                    _ => continue,
                }
                feed.store.apply(&changes(&kind, body));
                *backoff = Duration::from_secs(1);
                for m in feed.sync_lookups() {
                    if sink.send(m).await.is_err() {
                        return Ok(());
                    }
                }
                // Hold the data until the fixed set and every pending lookup
                // are in, so names never show half-resolved.
                if feed.stable_pending || !feed.pending.is_empty() {
                    continue;
                }
                emit(RegionUpdate::State(RelayState::Live));
                emit(RegionUpdate::Data(RegionData {
                    crafts: feed.crafts(),
                    passive: feed.passive_crafts(),
                    plants: feed.plants(first_data),
                    buffs: feed.buffs(),
                    food_descs: feed.food_descs(),
                    signed_in: feed.signed_in(),
                    stats: feed.stats(),
                    health: feed.health(),
                    position: feed.position(),
                    experience: feed.experience(),
                    stamina: feed.stamina(),
                    satiation: feed.satiation(),
                    username: feed.username(),
                    gather: feed.gather(),
                    trades: feed.trade_offers(),
                }));
                first_data = false;
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

/// The table updates a server message carries: `SubscribeMultiApplied` /
/// `UnsubscribeMultiApplied` → `update.tables`, `SubscribeApplied` /
/// `UnsubscribeApplied` → `rows.table_rows` (one table),
/// `TransactionUpdate` → `status.Committed.tables`.
fn changes(kind: &str, body: &Value) -> Vec<Value> {
    let list = match kind {
        "SubscribeMultiApplied" | "UnsubscribeMultiApplied" => body.pointer("/update/tables"),
        "SubscribeApplied" | "UnsubscribeApplied" => {
            return body.pointer("/rows/table_rows").cloned().into_iter().collect();
        }
        "TransactionUpdate" => body.pointer("/status/Committed/tables"),
        _ => None,
    };
    list.and_then(Value::as_array).cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `TransactionUpdate` table update for `table`.
    fn update(table: &str, deletes: &[&str], inserts: &[&str]) -> Value {
        json!({"table_name": table, "updates": [{"deletes": deletes, "inserts": inserts}]})
    }

    fn sent(messages: Vec<Message>) -> Vec<String> {
        messages
            .into_iter()
            .map(|m| {
                let v: Value = serde_json::from_str(m.to_text().unwrap()).unwrap();
                match v.get("SubscribeSingle") {
                    Some(s) => format!("+{}", s["query"].as_str().unwrap()),
                    None => format!("-{}", v["Unsubscribe"]["query_id"]["id"]),
                }
            })
            .collect()
    }

    #[test]
    fn lookups_follow_the_current_target_only() {
        let mut feed = Feed::new("7".into());
        // Gathering a Young Beech Tree (resource 12).
        feed.store.apply(&[update("resource_state", &[], &["[100, 12, 0]"])]);
        assert_eq!(
            sent(feed.sync_lookups()),
            ["+SELECT * FROM extraction_recipe_desc WHERE resource_id = 12", "+SELECT * FROM resource_desc WHERE id = 12"]
        );
        assert_eq!(feed.pending.len(), 2);
        // Another tree of the same type: nothing to send.
        feed.store.apply(&[update("resource_state", &["[100, 12, 0]"], &["[101, 12, 0]"])]);
        assert!(feed.sync_lookups().is_empty());
        // A different type: the old lookups go, the new ones come.
        feed.store.apply(&[update("resource_state", &["[101, 12, 0]"], &["[102, 19, 0]"])]);
        let mut changes = sent(feed.sync_lookups());
        changes.sort();
        assert_eq!(
            changes,
            [
                "+SELECT * FROM extraction_recipe_desc WHERE resource_id = 19",
                "+SELECT * FROM resource_desc WHERE id = 19",
                "-2",
                "-3"
            ]
        );
        // Used up (rows deleted): nothing tracked anymore.
        feed.store.apply(&[update("resource_state", &["[102, 19, 0]"], &[])]);
        assert_eq!(sent(feed.sync_lookups()).len(), 2);
        assert!(feed.lookups.is_empty() && feed.pending.is_empty());
    }

    #[test]
    fn trade_offers_with_initiator_name() {
        let mut feed = Feed::new("7".into());
        feed.store.apply(&[
            update("trade_session_state", &[], &[
                // Offered by 9 (status 0) and an already accepted one (1).
                "[500, [0, {}], 9, 7, [], [], [1700000000000000], \"\"]",
                "[501, [1, {}], 8, 7, [], [], [1700000000000000], \"\"]",
            ]),
            update("player_username_state", &[], &["[9, \"Trader\"]", "[7, \"Me\"]"]),
        ]);
        assert_eq!(feed.trade_offers(), vec![TradeOffer { id: "500".into(), from: Some("Trader".into()) }]);
        // The character's own name isn't confused with the partner's.
        assert_eq!(feed.username().as_deref(), Some("Me"));
        // Both partners' names are looked up.
        let wanted = feed.wanted_lookups();
        assert!(wanted.contains("SELECT * FROM player_username_state WHERE entity_id = 9"));
        assert!(wanted.contains("SELECT * FROM player_username_state WHERE entity_id = 8"));
    }

    #[test]
    fn subscribe_replies_carry_table_updates() {
        let single = json!({"rows": {"table_id": 1, "table_name": "resource_desc", "table_rows": update("resource_desc", &[], &["[12, \"Young Beech Tree\"]"])}});
        let mut store = Store::default();
        store.apply(&changes("SubscribeApplied", &single));
        assert!(store.get("resource_desc", "12").is_some());
        let gone = json!({"rows": {"table_id": 1, "table_name": "resource_desc", "table_rows": update("resource_desc", &["[12, \"Young Beech Tree\"]"], &[])}});
        store.apply(&changes("UnsubscribeApplied", &gone));
        assert!(store.get("resource_desc", "12").is_none());
        let multi = json!({"update": {"tables": [update("player_state", &[], &["[0, 7, 0, 0, 0, 0, true]"])]}});
        store.apply(&changes("SubscribeMultiApplied", &multi));
        assert!(store.get("player_state", "7").is_some());
    }

    #[test]
    fn rows_from_arrays_and_objects() {
        // Updates: arrays; initial rows: objects in column order.
        let update = json!([
            "[1, 7, [1700000000000000]]",
            "{\"entity_id\":2,\"owner_entity_id\":7,\"timestamp\":{\"__timestamp_micros_since_unix_epoch__\":1700000000000000}}"
        ]);
        let rows = rows(&update);
        assert_eq!(rows[0][0], json!(1));
        assert_eq!(rows[1][0], json!(2));
        assert_eq!(rows[1][1], json!(7));
        assert_eq!(date(&rows[0][2]), Some(1_700_000_000.0));
        assert_eq!(date(&rows[1][2]), Some(1_700_000_000.0));
    }
}
