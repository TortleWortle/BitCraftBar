//! BitJita REST API (`https://bitjita.com/api/`): player search and
//! `players/<id>` (username, regions). See AGENTS.md "Data sources".

use std::sync::OnceLock;

use anyhow::{Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::core::types::PlayerSummary;

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
