//! Networking on a background Tokio runtime. The UI sends [`Command`]s and
//! receives [`Outgoing`] messages; per-character events carry the user id so
//! stale ones (after switching character) can be dropped.

pub mod api;
pub mod live;
pub mod relay;

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

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
    /// BitJita `players/<id>` (username).
    Player(PlayerSummary),
    Relay(RelayState),
    /// Every followed region delivered its rows (sent once per character).
    RelayLoaded,
    RelayCrafts(Vec<CraftInfo>),
    RelayPassiveCrafts(Vec<PassiveCraft>),
    RelayPlants(Vec<Plant>),
    /// The player's started buffs (`active_buff_state`), sent on change.
    RelayBuffs(Vec<RawBuff>),
    /// Food buff types (`buff_desc`).
    FoodBuffDescs(HashMap<i64, FoodBuffDesc>),
    /// The player's own rows (region the player is in). None: no region
    /// has them.
    RelayPlayer(Option<RelayPlayer>),
}

/// Relay `active_buff_state.active_buffs` (`[buff_id, [start (Unix s)],
/// duration (s), values]`, start possibly `{"…": n}`) → started buffs.
/// None if `list` isn't an array.
pub fn parse_buffs(list: &Value) -> Option<Vec<RawBuff>> {
    fn number(v: &Value) -> Option<f64> {
        match v {
            Value::Number(n) => n.as_f64(),
            Value::Array(a) => a.first().and_then(number),
            Value::Object(o) => o.values().find_map(number),
            _ => None,
        }
    }
    let buffs = list
        .as_array()?
        .iter()
        .filter_map(|b| {
            let b = b.as_array()?;
            let start = number(b.get(1)?)?;
            // Inactive entries have start 0.
            (start > 0.0).then_some(RawBuff { id: number(b.first()?)? as i64, start, duration: number(b.get(2)?)? })
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

struct Tracking {
    live: mpsc::UnboundedSender<live::LiveCommand>,
    tasks: Vec<JoinHandle<()>>,
}

async fn manager(mut commands: mpsc::UnboundedReceiver<Command>, out: async_channel::Sender<Outgoing>) {
    tokio::spawn(check_updates(out.clone()));
    let mut current: Option<Tracking> = None;
    while let Some(command) = commands.recv().await {
        match command {
            Command::Track(user) => {
                if let Some(old) = current.take() {
                    old.tasks.iter().for_each(JoinHandle::abort);
                }
                current = Some(track(user, out.clone()));
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

fn track(user: String, out: async_channel::Sender<Outgoing>) -> Tracking {
    let emitter = {
        let user = user.clone();
        let out = out.clone();
        move |event: NetEvent| {
            let _ = out.try_send(Outgoing::Event { user: user.clone(), event });
        }
    };

    let (live_tx, live_rx) = mpsc::unbounded_channel();
    let channels = vec![
        live::experience(&user),
        live::stamina(&user),
        live::satiation(&user),
    ];
    let live_task = tokio::spawn(live::run(channels, live_rx, emitter.clone()));

    let poll_task = tokio::spawn(poll(user, emitter));
    Tracking { live: live_tx, tasks: vec![live_task, poll_task] }
}

/// Starts the relay on the mirrored regions right away, then polls
/// BitJita `players/<id>` every 60s for the username and any extra regions
/// (the player's and their claims').
async fn poll(user: String, emit: impl Fn(NetEvent) + Clone + Send + Sync + 'static) {
    // Aborted with this task (character switch).
    let (hub, hub_task) = start_relay_hub(user.clone(), emit.clone());
    let _hub_task = AbortOnDrop(hub_task);
    let mut regions: BTreeSet<i64> = MIRRORED_REGIONS.into_iter().collect();
    let _ = hub.send(HubCommand::Regions { all: regions.clone() });
    loop {
        match api::player(&user).await {
            Ok(p) => {
                // Placeables and crafting stations stay in their region while
                // the player travels (buffs move with the player).
                regions.extend(p.region_id);
                regions.extend(p.claims.iter().filter_map(|c| c.region_id));
                let _ = hub.send(HubCommand::Regions { all: regions.clone() });
                emit(NetEvent::Player(p));
            }
            Err(e) => eprintln!("player: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Regions the relay mirrors (its `/health`; others answer 502, retried every
/// 5 min).
const MIRRORED_REGIONS: [i64; 9] = [7, 8, 9, 12, 13, 14, 17, 18, 19];

enum HubCommand {
    /// Regions to follow.
    Regions { all: BTreeSet<i64> },
}

fn start_relay_hub(user: String, emit: impl Fn(NetEvent) + Send + Sync + 'static) -> (mpsc::UnboundedSender<HubCommand>, JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (tx, tokio::spawn(relay_hub(user, rx, emit)))
}

/// One relay feed per region; merges their data. Lists are sent once every
/// followed region delivered its rows at least once (so a region still
/// loading never looks like missing crafts/plants), then on every change.
/// A region that's reconnecting keeps its last rows. Regions that refuse
/// for good (not mirrored) are ignored.
async fn relay_hub(user: String, mut commands: mpsc::UnboundedReceiver<HubCommand>, emit: impl Fn(NetEvent)) {
    let (tx, mut updates) = mpsc::unbounded_channel::<(i64, relay::RegionUpdate)>();
    let mut feeds: HashMap<i64, AbortOnDrop> = HashMap::new();
    let mut states: HashMap<i64, RelayState> = HashMap::new();
    let mut data: HashMap<i64, relay::RegionData> = HashMap::new();
    let mut last_state: Option<RelayState> = None;
    let mut last_player: Option<RelayPlayer> = None;
    let mut last_buffs: Option<Vec<RawBuff>> = None;
    let mut last_descs: HashMap<i64, FoodBuffDesc> = HashMap::new();
    let mut loaded = false;
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
        if last_state.as_ref() != Some(&overall) {
            emit(NetEvent::Relay(overall.clone()));
            last_state = Some(overall);
        }
        if !all_delivered(&feeds, &states, &data) {
            continue;
        }
        if data_changed || !loaded {
            emit(NetEvent::RelayCrafts(data.values().flat_map(|d| d.crafts.clone()).collect()));
            emit(NetEvent::RelayPassiveCrafts(data.values().flat_map(|d| d.passive.clone()).collect()));
            emit(NetEvent::RelayPlants(data.values().flat_map(|d| d.plants.clone()).collect()));
        }
        // The player's own rows (buffs, stats, online status) are in the
        // region the player is in: the one that has `player_state`. After
        // traveling, the old region can still hold a stale copy while it's
        // reconnecting, so a live region wins.
        let player = data
            .iter()
            .filter(|(_, d)| d.signed_in.is_some())
            .max_by_key(|(r, _)| states.get(r) == Some(&RelayState::Live))
            .map(|(_, d)| d);
        let player_info = player.and_then(|d| Some(RelayPlayer { signed_in: d.signed_in?, stats: d.stats.clone(), health: d.health, position: d.position }));
        if player_info != last_player {
            emit(NetEvent::RelayPlayer(player_info.clone()));
            last_player = player_info;
        }
        // Only on change: every region update re-sends its data.
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
        if !loaded {
            emit(NetEvent::RelayLoaded);
            loaded = true;
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

/// Every followed region (except those refusing for good) has sent its
/// rows at least once.
fn all_delivered(feeds: &HashMap<i64, AbortOnDrop>, states: &HashMap<i64, RelayState>, data: &HashMap<i64, relay::RegionData>) -> bool {
    !data.is_empty()
        && feeds.keys().all(|r| {
            data.contains_key(r) || matches!(states.get(r), Some(RelayState::Unavailable(reason)) if !relay::is_transient(reason))
        })
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
    fn parses_relay_buffs() {
        // Inactive entry (start 0) dropped; wrapped start accepted.
        let relay = json!([[7, [1_700_000_000], 1800, []], [8, [0], 900, []], [9, {"t": 1_700_000_100}, 900, []]]);
        assert_eq!(
            parse_buffs(&relay),
            Some(vec![RawBuff { id: 7, start: 1_700_000_000.0, duration: 1800.0 }, RawBuff { id: 9, start: 1_700_000_100.0, duration: 900.0 }])
        );
        assert_eq!(parse_buffs(&Value::Null), None);
    }
}
