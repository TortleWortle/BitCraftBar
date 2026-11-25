//! HTTP: the relay's JSON API (`https://relay.bitcraftsync.app`: player
//! search, mirrored regions) and the GitHub update check. See AGENTS.md
//! "Data sources".

use std::sync::OnceLock;

use anyhow::{Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::core::types::PlayerSummary;

const RELAY: &str = "https://relay.bitcraftsync.app";
/// Search results shown (the relay returns every match, e.g. 400+ for "to").
pub const SEARCH_LIMIT: usize = 50;

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("BitCraftTracker/0.1")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("http client")
    })
}

async fn get<T: DeserializeOwned>(url: reqwest::Url) -> Result<T> {
    let response = client().get(url.clone()).send().await?;
    if !response.status().is_success() {
        bail!("GET {}: {}", url.path(), response.status());
    }
    Ok(response.json().await?)
}

/// `GET /player?name=` (case-insensitive substring, all regions, no
/// limit). Re-ranked: exact, prefix, rest (each alphabetical), then cut to
/// [`SEARCH_LIMIT`].
pub async fn search_players(query: &str) -> Result<Vec<PlayerSummary>> {
    let q = query.trim();
    if q.chars().count() < 2 {
        return Ok(vec![]);
    }
    let mut url = reqwest::Url::parse(&format!("{RELAY}/player"))?;
    url.query_pairs_mut().append_pair("name", q);
    let mut players: Vec<PlayerSummary> = get(url).await?;
    let lower = q.to_lowercase();
    players.sort_by_cached_key(|p| {
        let name = p.username.to_lowercase();
        let rank = if name == lower {
            0
        } else if name.starts_with(&lower) {
            1
        } else {
            2
        };
        (rank, name)
    });
    players.truncate(SEARCH_LIMIT);
    Ok(players)
}

#[derive(Deserialize)]
struct Health {
    sources: std::collections::HashMap<String, serde::de::IgnoredAny>,
}

/// Mirrored regions from `/health` (sources `idc-<N>`; `global` has no
/// player rows).
pub async fn relay_regions() -> Result<Vec<i64>> {
    let health: Health = get(reqwest::Url::parse(&format!("{RELAY}/health"))?).await?;
    Ok(health.sources.keys().filter_map(|k| k.strip_prefix("idc-")?.parse().ok()).collect())
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
