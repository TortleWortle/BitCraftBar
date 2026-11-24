//! Networking on a background Tokio runtime. The UI sends [`Command`]s and
//! receives [`Outgoing`] messages; per-character events carry the user id so
//! stale ones (after switching character) can be dropped.

pub mod api;
pub mod live;
pub mod relay;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::core::trackers::PassiveCraftTracker;
use crate::core::types::{CraftInfo, PassiveCraft, Plant, PlayerSummary};

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
    // Aborted when this task is (character switch).
    let mut relay: Option<(i64, AbortOnDrop)> = None;
    let mut first = true;
    loop {
        let passive = tokio::spawn(poll_passive(user.clone(), emit.clone(), relay_live.clone(), caches.clone()));
        let (crafts, stats, plants, player) = tokio::join!(
            api::crafts(&user),
            api::stats(&user),
            poll_plants(&user, &relay_live, &caches),
            api::player(&user),
        );
        match crafts {
            Ok(c) => emit(NetEvent::Crafts(c)),
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
                if let Some(region) = p.region_id
                    && relay.as_ref().is_none_or(|(r, _)| *r != region)
                {
                    drop(relay.take()); // aborts the old region's relay
                    relay_live.store(false, Ordering::Relaxed);
                    let task = start_relay(user.clone(), region, emit.clone(), relay_live.clone(), caches.clone());
                    relay = Some((region, AbortOnDrop(task)));
                }
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
        crafts.push(PassiveCraft {
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

fn start_relay(
    user: String,
    region: i64,
    emit: impl Fn(NetEvent) + Send + Sync + 'static,
    relay_live: Arc<AtomicBool>,
    caches: Arc<Mutex<Caches>>,
) -> JoinHandle<()> {
    // Relay events pass through here: track liveness, add growth durations.
    let (tx, mut rx) = mpsc::unbounded_channel::<NetEvent>();
    let forward = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                NetEvent::Relay(state) => {
                    relay_live.store(state == RelayState::Live, Ordering::Relaxed);
                    emit(NetEvent::Relay(state));
                }
                NetEvent::RelayPlants(plants) => emit(NetEvent::RelayPlants(with_growth_durations(plants, &caches).await)),
                other => emit(other),
            }
        }
    });
    tokio::spawn(async move {
        let _forward = AbortOnDrop(forward);
        relay::run(user, region, move |e| {
            let _ = tx.send(e);
        })
        .await;
    })
}

struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
