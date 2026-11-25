//! BitJita live websocket (`wss://live.bitjita.com/`): subscribe to channels,
//! keepalive ping, reconnect with backoff. See AGENTS.md "Data sources".

use crate::core::types::Position;
use std::collections::HashSet;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::{Instant, interval, timeout};
use tokio_tungstenite::tungstenite::Message;

use super::{ConnState, NetEvent};

pub const URL: &str = "wss://live.bitjita.com/";
const PING_INTERVAL: Duration = Duration::from_secs(15);
const PING_TIMEOUT: Duration = Duration::from_secs(10);

pub enum LiveCommand {
    Subscribe(Vec<String>),
    Reconnect,
}

pub fn experience(user: &str) -> String {
    format!("experience_state:{user}")
}
pub fn stamina(user: &str) -> String {
    format!("stamina_state:{user}")
}
pub fn satiation(user: &str) -> String {
    format!("satiation_state:{user}")
}
pub fn character_stats(user: &str) -> String {
    format!("character_stats_state:{user}")
}
/// Position (~16 events/s while moving; throttled to 1/s in `handle`).
pub fn mobile(user: &str) -> String {
    format!("mobile_entity_state:{user}")
}
pub const CRAFT_PREFIX: &str = "progressive_action_state:";

/// Last position event sent on (when, where), for throttling.
static LAST_POSITION: std::sync::Mutex<Option<(std::time::Instant, (i64, i64))>> = std::sync::Mutex::new(None);

/// Runs until the command channel closes. Sends events via `emit`.
pub async fn run(
    channels: Vec<String>,
    mut commands: mpsc::UnboundedReceiver<LiveCommand>,
    emit: impl Fn(NetEvent),
) {
    let mut channels: HashSet<String> = channels.into_iter().collect();
    let mut backoff = Duration::from_secs(1);
    loop {
        emit(NetEvent::Live(ConnState::Connecting));
        let result = session(&mut channels, &mut commands, &emit, &mut backoff).await;
        match result {
            SessionEnd::Closed => return,
            SessionEnd::Reconnect => continue,
            SessionEnd::Failed => {
                emit(NetEvent::Live(ConnState::Disconnected));
                // Commands that arrive while waiting still count.
                let deadline = Instant::now() + backoff;
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => break,
                        cmd = commands.recv() => match cmd {
                            None => return,
                            Some(LiveCommand::Subscribe(new)) => channels.extend(new),
                            Some(LiveCommand::Reconnect) => break,
                        },
                    }
                }
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

enum SessionEnd {
    Closed,
    Reconnect,
    Failed,
}

async fn session(
    channels: &mut HashSet<String>,
    commands: &mut mpsc::UnboundedReceiver<LiveCommand>,
    emit: &impl Fn(NetEvent),
    backoff: &mut Duration,
) -> SessionEnd {
    let Ok(Ok((ws, _))) = timeout(Duration::from_secs(20), tokio_tungstenite::connect_async(URL)).await else {
        return SessionEnd::Failed;
    };
    let (mut sink, mut stream) = ws.split();
    let subscribe = |list: Vec<&String>| Message::text(json!({"type": "subscribe", "channels": list}).to_string());
    if sink.send(subscribe(channels.iter().collect())).await.is_err() {
        return SessionEnd::Failed;
    }
    emit(NetEvent::Live(ConnState::Connected));
    *backoff = Duration::from_secs(1);

    let mut ping_timer = interval(PING_INTERVAL);
    ping_timer.tick().await;
    let mut awaiting_pong: Option<Instant> = None;

    loop {
        tokio::select! {
            message = stream.next() => match message {
                Some(Ok(Message::Text(text))) => handle(text.as_str(), emit),
                Some(Ok(Message::Binary(bytes))) => handle(&String::from_utf8_lossy(&bytes), emit),
                Some(Ok(Message::Pong(_))) => awaiting_pong = None,
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return SessionEnd::Failed,
            },
            cmd = commands.recv() => match cmd {
                None => return SessionEnd::Closed,
                Some(LiveCommand::Reconnect) => return SessionEnd::Reconnect,
                Some(LiveCommand::Subscribe(new)) => {
                    let added: Vec<String> = new.into_iter().filter(|c| !channels.contains(c)).collect();
                    if !added.is_empty() {
                        channels.extend(added.iter().cloned());
                        if sink.send(subscribe(added.iter().collect())).await.is_err() {
                            return SessionEnd::Failed;
                        }
                    }
                }
            },
            // A socket can die silently (sleep, Wi-Fi switch): ping every
            // 15s; a ping still unanswered at the next tick (> 10s) fails.
            _ = ping_timer.tick() => {
                if awaiting_pong.is_some_and(|sent| sent.elapsed() >= PING_TIMEOUT) {
                    return SessionEnd::Failed;
                }
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return SessionEnd::Failed;
                }
                awaiting_pong.get_or_insert_with(Instant::now);
            },
        }
    }
}

fn handle(text: &str, emit: &impl Fn(NetEvent)) {
    let Ok(message) = serde_json::from_str::<Value>(text) else { return };
    if message["type"] != "event" {
        return;
    }
    let Some(channel) = message["channel"].as_str() else { return };
    let data = &message["data"];
    let kind = channel.split(':').next().unwrap_or_default();
    let event = match kind {
        "experience_state" => NetEvent::Experience(
            data["experience_stacks"]
                .as_array()
                .map(|stacks| {
                    stacks
                        .iter()
                        .filter_map(|s| Some((s["skill_id"].as_i64()?, s["quantity"].as_i64()?)))
                        .collect()
                })
                .unwrap_or_default(),
        ),
        "stamina_state" => match data["stamina"].as_f64() {
            Some(v) => NetEvent::Stamina(v),
            None => return,
        },
        "satiation_state" => match data["satiation"].as_f64() {
            Some(v) => NetEvent::Satiation(v),
            None => return,
        },
        "character_stats_state" => NetEvent::CharacterStats(
            data["values"].as_array().map(|v| v.iter().filter_map(Value::as_f64).collect()).unwrap_or_default(),
        ),
        "mobile_entity_state" => {
            let (Some(x), Some(z)) = (data["location_x"].as_i64(), data["location_z"].as_i64()) else { return };
            // Movement only needs ~1s resolution; don't re-render 16×/s.
            let mut last = LAST_POSITION.lock().unwrap();
            if last.is_some_and(|(at, pos)| pos == (x, z) || at.elapsed().as_secs_f64() < 1.0) {
                return;
            }
            *last = Some((std::time::Instant::now(), (x, z)));
            let at = data["timestamp"].as_f64().filter(|t| *t > 0.0).map(|ms| ms / 1e3);
            NetEvent::Position(Position { x, z, at })
        }
        "progressive_action_state" => {
            let id = match &data["entity_id"] {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => return,
            };
            match data["progress"].as_i64() {
                Some(progress) => NetEvent::CraftProgress { id, progress },
                None => return,
            }
        }
        _ => return,
    };
    emit(event);
}
