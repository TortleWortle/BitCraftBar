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
use crate::core::types::{CraftInfo, FoodState, PassiveCraft, Plant, PlayerSummary};

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
    RelayFood(FoodState),
    RelayCrafts(Vec<CraftInfo>),
    /// The player's own rows (region the player is in). None: not found in
    /// a live region (relay down / still loading) → use REST.
    RelayPlayer(Option<RelayPlayer>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RelayPlayer {
    pub signed_in: bool,
    /// `character_stats_state.values` (1 = max stamina, 19 = max satiation).
    pub stats: Option<Vec<f64>>,
    /// `health_state.health` (max = stats value 0).
    pub health: Option<f64>,
}

pub enum Outgoing {
    Event { user: String, event: NetEvent },
    SearchResults { query: String, result: Result<Vec<PlayerSummary>, String> },
}

pub enum Command {
    /// Start tracking a character (stops the previous one).
    Track(String),
    Subscribe(Vec<String>),
    ReconnectLive,
    Search(String),
}

pub const POLL_INTERVAL: Duration = Duration::from_secs(60);

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
    let channels = vec![live::experience(&user), live::stamina(&user), live::satiation(&user), live::character_stats(&user)];
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

/// Regions BitJita's relay mirrors (observed; others refuse, retried every
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
        let player_info = player.and_then(|d| Some(RelayPlayer { signed_in: d.signed_in?, stats: d.stats.clone(), health: d.health }));
        if player_info != last_player {
            emit(NetEvent::RelayPlayer(player_info.clone()));
            last_player = player_info;
        }
        if data_changed && let Some(d) = player {
            emit(NetEvent::RelayFood(d.food.clone()));
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
