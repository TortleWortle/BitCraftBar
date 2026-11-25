//! Networking on a background Tokio runtime. The UI sends [`Command`]s and
//! receives [`Outgoing`] messages; per-character events carry the user id so
//! stale ones (after switching character) can be dropped.

pub mod api;
pub mod live;
pub mod relay;

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::core::trackers::PassiveCraftTracker;
use crate::core::types::{CraftInfo, FoodBuffDesc, PassiveCraft, Plant, PlayerSummary, Position, RawBuff};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RelayState {
    Off,
    Connecting,
    /// Initial subscription received; data is current.
    Live,
    /// Refused (still loading, or region not mirrored); retried.
    Unavailable(String),
}

#[derive(Debug)]
pub enum NetEvent {
    Live(ConnState),
    Experience(Vec<(i64, i64)>),
    Stamina(f64),
    Satiation(f64),
    /// `character_stats_state` values (same indices as REST stats).
    CharacterStats(Vec<f64>),
    /// BitJita live `mobile_entity_state` location (x, z), throttled.
    Position(Position),
    CraftProgress { id: String, progress: i64 },
    Crafts(Vec<CraftInfo>),
    Stats(Vec<f64>),
    Player(PlayerSummary),
    /// REST (only sent while the relay isn't live).
    Plants(Vec<Plant>),
    PassiveCrafts(Vec<PassiveCraft>),
    /// First round of core REST requests finished (for the loading state).
    InitialRestDone,
    Relay(RelayState),
    RelayPassiveCrafts(Vec<PassiveCraft>),
    RelayPlants(Vec<Plant>),
    /// The player's started buffs (`active_buff_state`), from the relay
    /// (sent when they change) or BitJita live (`LiveBuffs`, usually first).
    RelayBuffs(Vec<RawBuff>),
    LiveBuffs(Vec<RawBuff>),
    /// Food buff types from the relay's `buff_desc` (cached by the model).
    FoodBuffDescs(HashMap<i64, FoodBuffDesc>),
    RelayCrafts(Vec<CraftInfo>),
    /// The player's own rows (region the player is in). None: not found in
    /// a live region (relay down / still loading) → use REST.
    RelayPlayer(Option<RelayPlayer>),
}

/// `active_buff_state.active_buffs` → started buffs. Accepts the relay's
/// column arrays (`[buff_id, [start], duration, values]`) and objects with
/// column names (BitJita live). Timestamps may be wrapped (`[n]`,
/// `{"…": n}`) and in s, ms or µs. None if `list` isn't an array.
pub fn parse_buffs(list: &Value) -> Option<Vec<RawBuff>> {
    fn number(v: &Value) -> Option<f64> {
        match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.parse().ok(),
            Value::Array(a) => a.first().and_then(number),
            Value::Object(o) => o.values().find_map(number),
            _ => None,
        }
    }
    fn field<'a>(o: &'a serde_json::Map<String, Value>, names: &[&str]) -> Option<&'a Value> {
        names.iter().find_map(|n| o.get(*n))
    }
    let buffs = list
        .as_array()?
        .iter()
        .filter_map(|b| {
            let (id, start, duration) = match b {
                Value::Array(a) => (a.first()?, a.get(1)?, a.get(2)?),
                Value::Object(o) => (
                    field(o, &["buff_id", "id"])?,
                    field(o, &["buff_start_timestamp", "start_timestamp", "start"])?,
                    field(o, &["buff_duration", "duration"])?,
                ),
                _ => return None,
            };
            let start = number(start)?;
            let start = if start > 1e14 { start / 1e6 } else if start > 1e11 { start / 1e3 } else { start };
            // Inactive entries have start 0.
            (start > 0.0).then_some(RawBuff { id: number(id)? as i64, start, duration: number(duration)? })
        })
        .collect();
    Some(buffs)
}

#[derive(Clone, Debug, PartialEq)]
pub struct RelayPlayer {
    pub signed_in: bool,
    /// `character_stats_state.values` (1 = max stamina, 19 = max satiation).
    pub stats: Option<Vec<f64>>,
    /// `health_state.health` (max = stats value 0).
    pub health: Option<f64>,
    /// `mobile_entity_state` location (x, z), for "moved recently".
    pub position: Option<Position>,
}

pub enum Outgoing {
    Event { user: String, event: NetEvent },
    SearchResults { query: String, result: Result<Vec<PlayerSummary>, String> },
    /// A GitHub release newer than this build (update check).
    UpdateAvailable(api::Release),
}

pub enum Command {
    /// Start tracking a character (stops the previous one).
    Track(String),
    Subscribe(Vec<String>),
    ReconnectLive,
    Search(String),
}

pub const POLL_INTERVAL: Duration = Duration::from_secs(60);
/// How often to look for a new release (also once at startup).
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);

/// Starts the network thread; messages for the UI go to `out`.
pub fn start(out: async_channel::Sender<Outgoing>) -> mpsc::UnboundedSender<Command> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("network".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(manager(rx, out));
        })
        .expect("network thread");
    tx
}

#[derive(Default)]
struct Caches {
    /// Catalog growth duration per placeable id.
    growth: HashMap<i64, Option<f64>>,
    /// Passive recipe duration per recipe id.
    recipes: HashMap<i64, Option<f64>>,
}

struct Tracking {
    live: mpsc::UnboundedSender<live::LiveCommand>,
    tasks: Vec<JoinHandle<()>>,
}

async fn manager(mut commands: mpsc::UnboundedReceiver<Command>, out: async_channel::Sender<Outgoing>) {
    let caches = Arc::new(Mutex::new(Caches::default()));
    tokio::spawn(check_updates(out.clone()));
    let mut current: Option<Tracking> = None;
    while let Some(command) = commands.recv().await {
        match command {
            Command::Track(user) => {
                if let Some(old) = current.take() {
                    old.tasks.iter().for_each(JoinHandle::abort);
                }
                current = Some(track(user, out.clone(), caches.clone()));
            }
            Command::Subscribe(channels) => {
                if let Some(t) = &current {
                    let _ = t.live.send(live::LiveCommand::Subscribe(channels));
                }
            }
            Command::ReconnectLive => {
                if let Some(t) = &current {
                    let _ = t.live.send(live::LiveCommand::Reconnect);
                }
            }
            Command::Search(query) => {
                let out = out.clone();
                tokio::spawn(async move {
                    let result = api::search_players(&query).await.map_err(|e| e.to_string());
                    let _ = out.send(Outgoing::SearchResults { query, result }).await;
                });
            }
        }
    }
}

/// Startup + every 12h: tell the UI when GitHub has a newer release.
/// Errors (offline, no releases yet, rate limit) are ignored.
async fn check_updates(out: async_channel::Sender<Outgoing>) {
    loop {
        match api::latest_release().await {
            Ok(release) if crate::core::version::is_newer(&release.tag_name, env!("CARGO_PKG_VERSION")) => {
                let _ = out.send(Outgoing::UpdateAvailable(release)).await;
            }
            Ok(_) => {}
            Err(e) => {
                if std::env::var_os("BITCRAFT_DEBUG").is_some() {
                    eprintln!("update check: {e}");
                }
            }
        }
        tokio::time::sleep(UPDATE_CHECK_INTERVAL).await;
    }
}

fn track(user: String, out: async_channel::Sender<Outgoing>, caches: Arc<Mutex<Caches>>) -> Tracking {
    let emitter = {
        let user = user.clone();
        let out = out.clone();
        move |event: NetEvent| {
            let _ = out.try_send(Outgoing::Event { user: user.clone(), event });
        }
    };
    let relay_live = Arc::new(AtomicBool::new(false));

    let (live_tx, live_rx) = mpsc::unbounded_channel();
    let channels = vec![
        live::experience(&user),
        live::stamina(&user),
        live::satiation(&user),
        live::character_stats(&user),
        live::mobile(&user),
        live::buffs(&user),
    ];
    let live_task = tokio::spawn(live::run(channels, live_rx, emitter.clone()));

    let poll_task = tokio::spawn(poll(user, emitter, relay_live, caches));
    Tracking { live: live_tx, tasks: vec![live_task, poll_task] }
}

/// REST every 60s, requests of a round in parallel. Also starts the relay
/// once the player's region is known.
async fn poll(user: String, emit: impl Fn(NetEvent) + Clone + Send + Sync + 'static, relay_live: Arc<AtomicBool>, caches: Arc<Mutex<Caches>>) {
    // Aborted with this task (character switch).
    let (hub, hub_task) = start_relay_hub(user.clone(), emit.clone(), relay_live.clone(), caches.clone());
    let _hub_task = AbortOnDrop(hub_task);
    let mut first = true;
    loop {
        let passive = tokio::spawn(poll_passive(user.clone(), emit.clone(), relay_live.clone(), caches.clone()));
        let (crafts, stats, plants, player) = tokio::join!(
            poll_crafts(&user, &relay_live),
            api::stats(&user),
            poll_plants(&user, &relay_live, &caches),
            api::player(&user),
        );
        match crafts {
            Ok(Some(c)) => emit(NetEvent::Crafts(c)),
            Ok(None) => {}
            Err(e) => eprintln!("crafts: {e}"),
        }
        match stats {
            Ok(s) => emit(NetEvent::Stats(s)),
            Err(e) => eprintln!("stats: {e}"),
        }
        match plants {
            Ok(Some(p)) => emit(NetEvent::Plants(p)),
            Ok(None) => {}
            Err(e) => eprintln!("placeables: {e}"),
        }
        match player {
            Ok(p) => {
                // Placeables and crafting stations stay in their region while
                // the player travels (buffs move with the player), so follow
                // every mirrored region + the player's + their claims'.
                let mut regions: BTreeSet<i64> = MIRRORED_REGIONS.into_iter().collect();
                regions.extend(p.region_id);
                regions.extend(p.claims.iter().filter_map(|c| c.region_id));
                let _ = hub.send(HubCommand::Regions { all: regions });
                emit(NetEvent::Player(p));
            }
            Err(e) => eprintln!("player: {e}"),
        }
        if first {
            // Loading doesn't wait for passive crafts (can take ~10s).
            emit(NetEvent::InitialRestDone);
            first = false;
        }
        let _ = passive.await;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// None while the relay is live (it has crafts too, and BitJita's REST
/// sometimes answers 500).
async fn poll_crafts(user: &str, relay_live: &AtomicBool) -> anyhow::Result<Option<Vec<CraftInfo>>> {
    if relay_live.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let crafts = api::crafts(user).await?;
    Ok((!relay_live.load(Ordering::Relaxed)).then_some(crafts))
}

/// None while the relay is live (its data is fresher).
async fn poll_plants(user: &str, relay_live: &AtomicBool, caches: &Arc<Mutex<Caches>>) -> anyhow::Result<Option<Vec<Plant>>> {
    if relay_live.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let plants = api::placeables(user).await?;
    if relay_live.load(Ordering::Relaxed) {
        return Ok(None);
    }
    Ok(Some(with_growth_durations(plants, caches).await))
}

async fn poll_passive(user: String, emit: impl Fn(NetEvent), relay_live: Arc<AtomicBool>, caches: Arc<Mutex<Caches>>) {
    if relay_live.load(Ordering::Relaxed) {
        return;
    }
    let response = match api::passive_crafts(&user).await {
        Ok(r) => r,
        Err(e) => return eprintln!("passive crafts: {e}"),
    };
    let mut crafts = Vec::new();
    for c in &response.craft_results {
        let cached = caches.lock().unwrap().recipes.get(&c.recipe_id).copied();
        let duration = match (cached, c.item_id()) {
            (Some(d), _) => d,
            (None, Some(item)) => {
                let d = api::recipe_duration(item, c.recipe_id).await.ok().flatten();
                caches.lock().unwrap().recipes.insert(c.recipe_id, d);
                d
            }
            (None, None) => None,
        };
        let (kind, tier) = c.item_id().map_or((None, None), |i| response.item_kind(i));
        crafts.push(PassiveCraft {
            kind,
            tier,
            id: c.entity_id.clone(),
            name: PassiveCraftTracker::display_name(
                c.recipe_name.as_deref(),
                c.item_id().and_then(|i| response.item_name(i)),
            ),
            building: c.building_name.clone().unwrap_or_else(|| "Building".into()),
            started_at: c.timestamp.as_ref().and_then(api::parse_date),
            reported_complete: c.status.as_deref().is_some_and(|s| s.eq_ignore_ascii_case("complete")),
            duration,
        });
    }
    if !relay_live.load(Ordering::Relaxed) {
        emit(NetEvent::PassiveCrafts(crafts));
    }
}

/// Fill in the catalog growth duration for growing plants with a start time
/// (for the "~ time left" estimate). Looked up once per placeable type.
async fn with_growth_durations(mut plants: Vec<Plant>, caches: &Arc<Mutex<Caches>>) -> Vec<Plant> {
    for plant in &mut plants {
        if !plant.name.starts_with("Growing ") || plant.started_growing_at.is_none() || plant.reported_end.is_some() {
            continue;
        }
        let cached = caches.lock().unwrap().growth.get(&plant.placeable_id).copied();
        plant.growth_duration = match cached {
            Some(d) => d,
            None => {
                let d = api::growth_duration(plant.placeable_id).await.ok().flatten();
                caches.lock().unwrap().growth.insert(plant.placeable_id, d);
                d
            }
        };
    }
    plants
}

/// Regions the relay mirrors (its `/health`; others answer 502, retried every
/// 5 min).
const MIRRORED_REGIONS: [i64; 9] = [7, 8, 9, 12, 13, 14, 17, 18, 19];

enum HubCommand {
    /// Regions to follow.
    Regions { all: BTreeSet<i64> },
}

fn start_relay_hub(
    user: String,
    emit: impl Fn(NetEvent) + Send + Sync + 'static,
    relay_live: Arc<AtomicBool>,
    caches: Arc<Mutex<Caches>>,
) -> (mpsc::UnboundedSender<HubCommand>, JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (tx, tokio::spawn(relay_hub(user, rx, emit, relay_live, caches)))
}

/// One relay feed per region; merges their data. The relay only counts as
/// live (and replaces REST for crafts/plants/passive crafts) when every
/// followed region is live, so a region still loading never hides data.
/// Regions that refuse for good (not mirrored) are ignored.
async fn relay_hub(
    user: String,
    mut commands: mpsc::UnboundedReceiver<HubCommand>,
    emit: impl Fn(NetEvent),
    relay_live: Arc<AtomicBool>,
    caches: Arc<Mutex<Caches>>,
) {
    let (tx, mut updates) = mpsc::unbounded_channel::<(i64, relay::RegionUpdate)>();
    let mut feeds: HashMap<i64, AbortOnDrop> = HashMap::new();
    let mut states: HashMap<i64, RelayState> = HashMap::new();
    let mut data: HashMap<i64, relay::RegionData> = HashMap::new();
    let mut last_state: Option<RelayState> = None;
    let mut last_player: Option<RelayPlayer> = None;
    let mut last_buffs: Option<Vec<RawBuff>> = None;
    let mut last_descs: HashMap<i64, FoodBuffDesc> = HashMap::new();
    loop {
        let mut data_changed = false;
        tokio::select! {
            command = commands.recv() => match command {
                None => return,
                Some(HubCommand::Regions { all }) => {
                    feeds.retain(|r, _| all.contains(r));
                    states.retain(|r, _| all.contains(r));
                    data.retain(|r, _| all.contains(r));
                    for region in all {
                        if feeds.contains_key(&region) {
                            continue;
                        }
                        let tx = tx.clone();
                        let task = tokio::spawn(relay::run(user.clone(), region, move |update| {
                            let _ = tx.send((region, update));
                        }));
                        feeds.insert(region, AbortOnDrop(task));
                    }
                    data_changed = true;
                }
            },
            Some((region, update)) = updates.recv() => {
                if !feeds.contains_key(&region) {
                    continue;
                }
                match update {
                    relay::RegionUpdate::State(state) => {
                        states.insert(region, state);
                    }
                    relay::RegionUpdate::Data(d) => {
                        data.insert(region, d);
                        data_changed = true;
                    }
                }
            },
        }

        let overall = overall_state(&feeds, &states);
        let live = overall == RelayState::Live;
        relay_live.store(live, Ordering::Relaxed);
        if last_state.as_ref() != Some(&overall) {
            emit(NetEvent::Relay(overall.clone()));
            // Just went live: send the merged lists right away.
            data_changed |= live;
            last_state = Some(overall);
        }
        if live && data_changed {
            emit(NetEvent::RelayCrafts(data.values().flat_map(|d| d.crafts.clone()).collect()));
            emit(NetEvent::RelayPassiveCrafts(data.values().flat_map(|d| d.passive.clone()).collect()));
            let plants = data.values().flat_map(|d| d.plants.clone()).collect();
            emit(NetEvent::RelayPlants(with_growth_durations(plants, &caches).await));
        }
        // The player's own rows (buffs, stats, online status) are in the
        // region the player is in: the live region that has `player_state`.
        let player = data
            .iter()
            .find(|(r, d)| d.signed_in.is_some() && states.get(r) == Some(&RelayState::Live))
            .map(|(_, d)| d);
        let player_info = player.and_then(|d| Some(RelayPlayer { signed_in: d.signed_in?, stats: d.stats.clone(), health: d.health, position: d.position }));
        if player_info != last_player {
            emit(NetEvent::RelayPlayer(player_info.clone()));
            last_player = player_info;
        }
        // Only on change: every region update re-sends its data, and a
        // repeat of older relay buffs would undo newer BitJita live ones.
        if let Some(d) = player
            && last_buffs.as_ref() != Some(&d.buffs)
        {
            emit(NetEvent::RelayBuffs(d.buffs.clone()));
            last_buffs = Some(d.buffs.clone());
        }
        let descs: HashMap<i64, FoodBuffDesc> = data.values().flat_map(|d| d.food_descs.clone()).collect();
        if !descs.is_empty() && descs != last_descs {
            emit(NetEvent::FoodBuffDescs(descs.clone()));
            last_descs = descs;
        }
    }
}

fn overall_state(feeds: &HashMap<i64, AbortOnDrop>, states: &HashMap<i64, RelayState>) -> RelayState {
    if feeds.is_empty() {
        return RelayState::Off;
    }
    let relevant: Vec<Option<&RelayState>> = feeds
        .keys()
        .map(|r| states.get(r))
        .filter(|s| !matches!(s, Some(RelayState::Unavailable(reason)) if !relay::is_transient(reason)))
        .collect();
    if relevant.is_empty() {
        RelayState::Unavailable("no mirrored region".into())
    } else if relevant.iter().all(|s| *s == Some(&RelayState::Live)) {
        RelayState::Live
    } else {
        RelayState::Connecting
    }
}

struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_relay_and_live_buffs() {
        // Relay columns; inactive entry (start 0) dropped.
        let relay = json!([[7, [1_700_000_000], 1800, []], [8, [0], 900, []]]);
        assert_eq!(parse_buffs(&relay), Some(vec![RawBuff { id: 7, start: 1_700_000_000.0, duration: 1800.0 }]));
        // Named fields, wrapped µs timestamp, string id.
        let live = json!([{"buff_id": "7", "buff_start_timestamp": {"__timestamp_micros_since_unix_epoch__": 1_700_000_000_000_000_i64}, "buff_duration": 1800}]);
        assert_eq!(parse_buffs(&live), Some(vec![RawBuff { id: 7, start: 1_700_000_000.0, duration: 1800.0 }]));
        assert_eq!(parse_buffs(&Value::Null), None);
    }
}
