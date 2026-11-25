//! Networking on a background Tokio runtime. The UI sends [`Command`]s and
//! receives [`Outgoing`] messages; per-character events carry the user id so
//! stale ones (after switching character) can be dropped. Everything comes
//! from the relay (`relay.bitcraftsync.app`): websocket feeds per region
//! (`relay.rs`) and its HTTP API (`api.rs`: player search, region list).

pub mod api;
pub mod relay;

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::core::types::{CraftInfo, FoodBuffDesc, Gather, PassiveCraft, Plant, PlayerSummary, Position, RawBuff, TradeOffer};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum RelayState {
    Off,
    Connecting,
    /// Initial subscription received; data is current.
    Live,
    /// Connection dropped or failed; retrying with backoff.
    Disconnected,
    /// Refused (still loading, or region not mirrored); retried.
    Unavailable(String),
}

#[derive(Debug)]
pub enum NetEvent {
    /// The player's rows below are sent when they change (every region
    /// update re-sends its data). The first XP list is a snapshot.
    Experience(Vec<(i64, i64)>),
    Stamina(f64),
    Satiation(f64),
    Relay(RelayState),
    /// Every followed region delivered its rows (sent once per character).
    RelayLoaded,
    RelayCrafts(Vec<CraftInfo>),
    RelayPassiveCrafts(Vec<PassiveCraft>),
    RelayPlants(Vec<Plant>),
    /// The player's started buffs (`active_buff_state`).
    RelayBuffs(Vec<RawBuff>),
    /// Food buff types (`buff_desc`).
    FoodBuffDescs(HashMap<i64, FoodBuffDesc>),
    /// The player's own rows (region the player is in). None: no region
    /// has them.
    RelayPlayer(Option<RelayPlayer>),
    /// Open trade offers to the character (all regions), on change.
    TradeOffers(Vec<TradeOffer>),
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
    /// `player_username_state.username`.
    pub username: Option<String>,
    /// `character_stats_state.values` (1 = max stamina, 19 = max satiation).
    pub stats: Option<Vec<f64>>,
    /// `health_state.health` (max = stats value 0).
    pub health: Option<f64>,
    /// `mobile_entity_state` location (x, z), for "moved recently".
    pub position: Option<Position>,
    /// The resource last hit (`extract_outcome_state` target).
    pub gather: Option<Gather>,
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
    Search(String),
}

/// How often to re-read the relay's region list (`/health`); regions are
/// only added, rarely change, and the relay asks not to poll tightly.
const REGIONS_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Retry after `/health` failed (nothing is followed until it answers).
const REGIONS_RETRY: Duration = Duration::from_secs(15);
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

async fn manager(mut commands: mpsc::UnboundedReceiver<Command>, out: async_channel::Sender<Outgoing>) {
    tokio::spawn(check_updates(out.clone()));
    // Held so the tracked character's task runs until replaced.
    let mut _current: Option<AbortOnDrop> = None;
    while let Some(command) = commands.recv().await {
        match command {
            // Replacing the old one aborts it (and its relay feeds).
            Command::Track(user) => _current = Some(AbortOnDrop(tokio::spawn(track(user, out.clone())))),
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

/// Follows one character: the relay hub, fed with the regions the relay
/// mirrors (its `/health`). Every region is followed because placeables and
/// crafting stations stay in their region while the player travels.
async fn track(user: String, out: async_channel::Sender<Outgoing>) {
    let emit = {
        let user = user.clone();
        move |event: NetEvent| {
            let _ = out.try_send(Outgoing::Event { user: user.clone(), event });
        }
    };
    let (hub, rx) = mpsc::unbounded_channel();
    let _hub_task = AbortOnDrop(tokio::spawn(relay_hub(user, rx, emit)));
    let mut regions: BTreeSet<i64> = BTreeSet::new();
    loop {
        let wait = match api::relay_regions().await {
            Ok(list) => {
                // Only added: a region missing from one /health answer
                // keeps its feed (and last rows).
                regions.extend(list);
                let _ = hub.send(HubCommand::Regions { all: regions.clone() });
                REGIONS_INTERVAL
            }
            Err(e) => {
                eprintln!("relay regions: {e}");
                REGIONS_RETRY
            }
        };
        tokio::time::sleep(wait).await;
    }
}

enum HubCommand {
    /// Regions to follow.
    Regions { all: BTreeSet<i64> },
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
    let mut last_xp: Option<Vec<(i64, i64)>> = None;
    let mut last_stamina: Option<f64> = None;
    let mut last_satiation: Option<f64> = None;
    let mut last_trades: Option<Vec<TradeOffer>> = None;
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
        // The player's own rows (XP, stamina, buffs, stats, online status)
        // are in the region the player is in: the one that has
        // `player_state`. After traveling, the old region can still hold a
        // stale copy while it's reconnecting, so a live region wins.
        let player = data
            .iter()
            .filter(|(_, d)| d.signed_in.is_some())
            .max_by_key(|(r, _)| states.get(r) == Some(&RelayState::Live))
            .map(|(_, d)| d);
        let player_info = player.and_then(|d| {
            Some(RelayPlayer {
                signed_in: d.signed_in?,
                username: d.username.clone(),
                stats: d.stats.clone(),
                health: d.health,
                position: d.position,
                gather: d.gather.clone(),
            })
        });
        if player_info != last_player {
            emit(NetEvent::RelayPlayer(player_info.clone()));
            last_player = player_info;
        }
        if let Some(d) = player {
            if d.experience.is_some() && d.experience != last_xp {
                emit(NetEvent::Experience(d.experience.clone().unwrap_or_default()));
                last_xp = d.experience.clone();
            }
            if let Some(v) = d.stamina.filter(|v| Some(*v) != last_stamina) {
                emit(NetEvent::Stamina(v));
                last_stamina = Some(v);
            }
            if let Some(v) = d.satiation.filter(|v| Some(*v) != last_satiation) {
                emit(NetEvent::Satiation(v));
                last_satiation = Some(v);
            }
            if last_buffs.as_ref() != Some(&d.buffs) {
                emit(NetEvent::RelayBuffs(d.buffs.clone()));
                last_buffs = Some(d.buffs.clone());
            }
        }
        // Sessions live in the region the players are in; merged, by id.
        let mut trades: Vec<TradeOffer> = data.values().flat_map(|d| d.trades.clone()).collect();
        trades.sort_by(|a, b| a.id.cmp(&b.id));
        trades.dedup_by(|a, b| a.id == b.id);
        if last_trades.as_ref() != Some(&trades) {
            emit(NetEvent::TradeOffers(trades.clone()));
            last_trades = Some(trades);
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

/// Worst of the followed regions, ignoring those that refuse for good:
/// any disconnected → Disconnected, all live → Live, else Connecting
/// (also "still loading" refusals).
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
    } else if relevant.iter().any(|s| *s == Some(&RelayState::Disconnected)) {
        RelayState::Disconnected
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
