use crate::domain::r#match::{DraftAction, Match, Team, Tournament};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{de::Deserializer, Deserialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::time::sleep;
use tokio::{
    sync::{Mutex, OnceCell},
    task::JoinSet,
};

#[async_trait]
pub trait MatchProvider: Clone + Send + Sync + 'static {
    async fn live_matches(&self) -> Result<Vec<Match>>;
    async fn recent_matches(&self) -> Result<Vec<Match>>;
    async fn get_match(&self, id: i64) -> Result<Match>;
    async fn tournaments(&self) -> Result<Vec<Tournament>>;
    async fn tournament_matches(&self, league_id: i64) -> Result<Vec<Match>>;
}

type TeamCache = Arc<Mutex<HashMap<i64, Arc<OnceCell<Option<ApiTeam>>>>>>;

#[derive(Clone)]
pub struct OpenDotaClient {
    client: Client,
    base_url: String,
    hero_names: Arc<OnceCell<HashMap<u32, ApiHero>>>,
    leagues: Arc<OnceCell<HashMap<i64, ApiLeague>>>,
    teams: TeamCache,
}

impl OpenDotaClient {
    pub fn new(base_url: String, timeout: Duration) -> Result<Self> {
        Ok(Self {
            client: Client::builder().timeout(timeout).build()?,
            base_url: base_url.trim_end_matches('/').to_owned(),
            hero_names: Arc::new(OnceCell::new()),
            leagues: Arc::new(OnceCell::new()),
            teams: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let mut last_error = None;
        for attempt in 0..3 {
            match self
                .client
                .get(format!("{}{path}", self.base_url))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    return response
                        .json()
                        .await
                        .with_context(|| format!("OpenDota {path} returned an invalid response"))
                }
                Ok(response) => {
                    last_error = Some(anyhow::anyhow!(
                        "OpenDota {path} returned HTTP status {}",
                        response.status()
                    ))
                }
                Err(error) => last_error = Some(error.into()),
            }
            if attempt < 2 {
                sleep(Duration::from_millis(250 * 2_u64.pow(attempt))).await;
            }
        }
        Err(last_error.expect("retry always records an error"))
    }

    async fn league_map(&self) -> Result<&HashMap<i64, ApiLeague>> {
        self.leagues
            .get_or_try_init(|| async {
                Ok::<_, anyhow::Error>(
                    self.get::<Vec<ApiLeague>>("/leagues")
                        .await?
                        .into_iter()
                        .map(|league| (league.league_id, league))
                        .collect(),
                )
            })
            .await
    }
}

#[async_trait]
impl MatchProvider for OpenDotaClient {
    async fn live_matches(&self) -> Result<Vec<Match>> {
        let mut values = self.get::<Vec<ApiMatch>>("/live").await?;
        let leagues = self.league_map().await?;
        retain_premium_leagues(&mut values, leagues);
        Ok(self.enrich_matches(values).await)
    }

    async fn recent_matches(&self) -> Result<Vec<Match>> {
        Ok(self
            .get::<Vec<ApiMatch>>("/proMatches?limit=20")
            .await?
            .into_iter()
            .map(Match::from)
            .collect())
    }

    async fn get_match(&self, id: i64) -> Result<Match> {
        let value = self.get::<ApiMatch>(&format!("/matches/{id}")).await?;
        self.enrich_matches(vec![value])
            .await
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("OpenDota returned no match for {id}"))
    }

    async fn tournaments(&self) -> Result<Vec<Tournament>> {
        let leagues = self.league_map().await?;
        let mut tournaments = leagues
            .values()
            .filter_map(|league| {
                Some(Tournament {
                    id: league.league_id,
                    name: league.name.clone(),
                    series: tournament_series(&league.name)?.to_owned(),
                })
            })
            .collect::<Vec<_>>();
        tournaments.sort_by(|left, right| {
            left.series
                .cmp(&right.series)
                .then_with(|| right.id.cmp(&left.id))
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(tournaments)
    }

    async fn tournament_matches(&self, league_id: i64) -> Result<Vec<Match>> {
        let tournament = self
            .tournaments()
            .await?
            .into_iter()
            .find(|tournament| tournament.id == league_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown tournament league id {league_id}"))?;
        let mut values = self
            .get::<Vec<ApiMatch>>(&format!("/leagues/{league_id}/matches"))
            .await?;
        for value in &mut values {
            value.league_name = Some(tournament.name.clone());
        }
        let mut matches = self.enrich_matches(values).await;
        matches.sort_by(|left, right| {
            right
                .started_at
                .cmp(&left.started_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(matches)
    }
}

fn tournament_series(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    if name.contains("blast") {
        Some("BLAST")
    } else if name.contains("esl") {
        Some("ESL")
    } else if name.contains("international") {
        Some("The International")
    } else if name.contains("dreamleague") {
        Some("DreamLeague")
    } else {
        None
    }
}

impl OpenDotaClient {
    async fn team_details(&self, team_id: i64) -> Option<ApiTeam> {
        if team_id <= 0 {
            return None;
        }
        let entry = {
            let mut teams = self.teams.lock().await;
            teams
                .entry(team_id)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        entry
            .get_or_init(|| async {
                match self.get::<ApiTeam>(&format!("/teams/{team_id}")).await {
                    Ok(team) => Some(team),
                    Err(error) => {
                        tracing::warn!(team_id, %error, "could not load team metadata");
                        None
                    }
                }
            })
            .await
            .clone()
    }

    async fn load_team_details(&self, values: &[ApiMatch]) -> HashMap<i64, ApiTeam> {
        let mut team_ids = values
            .iter()
            .flat_map(|value| [value.radiant_team_id, value.dire_team_id])
            .flatten()
            .filter(|id| *id > 0)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        team_ids.sort_unstable();

        let mut tasks = JoinSet::new();
        let mut pending = team_ids.into_iter();
        let mut team_details = HashMap::new();
        loop {
            while tasks.len() < 8 {
                let Some(team_id) = pending.next() else {
                    break;
                };
                let client = self.clone();
                tasks.spawn(async move { (team_id, client.team_details(team_id).await) });
            }
            let Some(result) = tasks.join_next().await else {
                break;
            };
            match result {
                Ok((team_id, Some(team))) => {
                    team_details.insert(team_id, team);
                }
                Ok((_team_id, None)) => {}
                Err(error) => tracing::warn!(%error, "team metadata task failed"),
            }
        }
        team_details
    }

    async fn enrich_matches(&self, mut values: Vec<ApiMatch>) -> Vec<Match> {
        let hero_names = self
            .hero_names
            .get_or_init(|| async {
                match self.get::<Vec<ApiHero>>("/heroes").await {
                    Ok(heroes) => heroes.into_iter().map(|hero| (hero.id, hero)).collect(),
                    Err(error) => {
                        tracing::warn!(%error, "could not load Dota hero metadata");
                        HashMap::new()
                    }
                }
            })
            .await;
        let team_details = self.load_team_details(&values).await;

        for value in &mut values {
            for player in &mut value.players {
                if player.hero_name.as_deref().is_none_or(str::is_empty) {
                    player.hero_name = hero_names
                        .get(&player.hero_id)
                        .and_then(valid_localized_hero_name);
                }
            }
            if value
                .radiant_name
                .as_deref()
                .is_none_or(|name| name.trim().is_empty())
            {
                value.radiant_name = value
                    .radiant_team_id
                    .and_then(|id| team_details.get(&id))
                    .and_then(|team| team.name.clone());
            }
            if value
                .dire_name
                .as_deref()
                .is_none_or(|name| name.trim().is_empty())
            {
                value.dire_name = value
                    .dire_team_id
                    .and_then(|id| team_details.get(&id))
                    .and_then(|team| team.name.clone());
            }
        }

        let mut matches = values.into_iter().map(Match::from).collect::<Vec<_>>();
        for value in &mut matches {
            let radiant_team_id = value.radiant.team_id;
            let dire_team_id = value.dire.team_id;
            apply_team_metadata(
                &mut value.radiant,
                radiant_team_id.and_then(|id| team_details.get(&id)),
            );
            apply_team_metadata(
                &mut value.dire,
                dire_team_id.and_then(|id| team_details.get(&id)),
            );
            for action in &mut value.draft {
                if let Some(hero) = hero_names.get(&action.hero_id) {
                    if action.hero_name.starts_with("Hero #") || action.hero_name.is_empty() {
                        if let Some(localized_name) = valid_localized_hero_name(hero) {
                            action.hero_name = localized_name.clone();
                        }
                    }
                    action.hero_image_url = hero_portrait_url(&hero.name);
                }
            }
        }
        matches
    }
}

#[derive(Debug, Deserialize)]
struct ApiMatch {
    #[serde(deserialize_with = "deserialize_i64")]
    match_id: i64,
    #[serde(default)]
    series_id: Option<i64>,
    #[serde(default)]
    series_type: Option<u8>,
    #[serde(default, alias = "team_name_radiant", alias = "radiant_team_name")]
    radiant_name: Option<String>,
    #[serde(default, alias = "team_name_dire", alias = "dire_team_name")]
    dire_name: Option<String>,
    #[serde(default, alias = "team_id_radiant")]
    radiant_team_id: Option<i64>,
    #[serde(default, alias = "team_id_dire")]
    dire_team_id: Option<i64>,
    #[serde(default)]
    radiant_score: u16,
    #[serde(default)]
    dire_score: u16,
    #[serde(default)]
    league_name: Option<String>,
    #[serde(default)]
    league_id: Option<i64>,
    #[serde(default)]
    start_time: Option<i64>,
    #[serde(default)]
    radiant_win: Option<bool>,
    #[serde(default)]
    picks_bans: Option<Vec<ApiPickBan>>,
    #[serde(default)]
    players: Vec<ApiLivePlayer>,
}

#[derive(Debug, Deserialize)]
struct ApiLeague {
    #[serde(rename = "leagueid")]
    league_id: i64,
    tier: Option<String>,
    name: String,
}

fn retain_premium_leagues(matches: &mut Vec<ApiMatch>, leagues: &HashMap<i64, ApiLeague>) {
    matches.retain_mut(|value| {
        let Some(league) = value.league_id.and_then(|id| leagues.get(&id)) else {
            return false;
        };
        if league.tier.as_deref() != Some("premium") {
            return false;
        }
        value.league_name = Some(league.name.clone());
        true
    });
}

#[derive(Debug, Deserialize)]
struct ApiLivePlayer {
    hero_id: u32,
    #[serde(default)]
    team: Option<u8>,
    #[serde(default)]
    player_slot: Option<u8>,
    #[serde(default)]
    hero_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiHero {
    id: u32,
    #[serde(default)]
    localized_name: String,
    #[serde(default)]
    name: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiTeam {
    #[serde(default, deserialize_with = "deserialize_optional_i64")]
    team_id: Option<i64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    logo_url: Option<String>,
}

fn hero_portrait_url(name: &str) -> Option<String> {
    let hero = name.strip_prefix("npc_dota_hero_")?;
    if hero.is_empty()
        || hero.starts_with('_')
        || hero.ends_with('_')
        || hero.contains("__")
        || !hero
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return None;
    }
    Some(format!(
        "https://cdn.cloudflare.steamstatic.com/apps/dota2/images/dota_react/heroes/{hero}.png"
    ))
}

fn valid_localized_hero_name(hero: &ApiHero) -> Option<String> {
    let name = hero.localized_name.trim();
    if name.is_empty() || name.chars().any(char::is_control) || name.starts_with("Hero #") {
        None
    } else {
        Some(name.to_owned())
    }
}

fn safe_team_logo_url(url: Option<&String>) -> Option<String> {
    let url = url?;
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() == "https" && parsed.host().is_some() {
        Some(url.clone())
    } else {
        None
    }
}

fn apply_team_metadata(team: &mut Team, metadata: Option<&ApiTeam>) {
    let Some(metadata) = metadata else {
        return;
    };
    if team.team_id.is_none_or(|id| id <= 0) {
        team.team_id = metadata.team_id.filter(|id| *id > 0);
    }
    if let Some(name) = metadata
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        if team.name.trim().is_empty()
            || team.name.starts_with("Team #")
            || team.name.contains("название команды не указано")
        {
            team.name = name.to_owned();
        }
    }
    team.logo_url = safe_team_logo_url(metadata.logo_url.as_ref());
}

fn deserialize_optional_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumberOrString {
        Number(i64),
        String(String),
        Null,
    }

    match NumberOrString::deserialize(deserializer)? {
        NumberOrString::Number(value) => Ok(Some(value)),
        NumberOrString::String(value) => value.parse().map(Some).map_err(serde::de::Error::custom),
        NumberOrString::Null => Ok(None),
    }
}

fn deserialize_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumberOrString {
        Number(i64),
        String(String),
    }

    match NumberOrString::deserialize(deserializer)? {
        NumberOrString::Number(value) => Ok(value),
        NumberOrString::String(value) => value.parse().map_err(serde::de::Error::custom),
    }
}

#[derive(Debug, Deserialize)]
struct ApiPickBan {
    hero_id: u32,
    is_pick: bool,
    team: u8,
    #[serde(default)]
    hero_name: Option<String>,
}

impl From<ApiMatch> for Match {
    fn from(value: ApiMatch) -> Self {
        let started_at = value
            .start_time
            .and_then(|time| DateTime::<Utc>::from_timestamp(time, 0));
        let mut draft: Vec<DraftAction> = value
            .picks_bans
            .unwrap_or_default()
            .into_iter()
            .map(|item| DraftAction {
                hero_id: item.hero_id,
                hero_name: item
                    .hero_name
                    .unwrap_or_else(|| format!("Hero #{}", item.hero_id)),
                hero_image_url: None,
                is_pick: item.is_pick,
                is_radiant: item.team == 0,
            })
            .collect();
        if draft.is_empty() {
            draft = value
                .players
                .into_iter()
                .map(|player| DraftAction {
                    hero_id: player.hero_id,
                    hero_name: player
                        .hero_name
                        .unwrap_or_else(|| format!("Hero #{}", player.hero_id)),
                    hero_image_url: None,
                    is_pick: true,
                    is_radiant: player
                        .team
                        .map(|team| team == 0)
                        .or_else(|| player.player_slot.map(|slot| slot < 128))
                        .unwrap_or_default(),
                })
                .collect();
        }
        Self {
            id: value.match_id,
            series_id: value.series_id,
            series_type: value.series_type,
            radiant: Team {
                name: team_name(value.radiant_name, value.radiant_team_id, "Radiant"),
                score: value.radiant_score,
                team_id: value.radiant_team_id,
                logo_url: None,
            },
            dire: Team {
                name: team_name(value.dire_name, value.dire_team_id, "Dire"),
                score: value.dire_score,
                team_id: value.dire_team_id,
                logo_url: None,
            },
            radiant_win: value.radiant_win,
            tournament: value.league_name,
            started_at,
            finished: value.radiant_win.is_some(),
            draft,
        }
    }
}

fn team_name(name: Option<String>, team_id: Option<i64>, fallback: &str) -> String {
    name.filter(|name| !name.trim().is_empty())
        .or_else(|| team_id.filter(|id| *id > 0).map(|id| format!("Team #{id}")))
        .unwrap_or_else(|| fallback.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn converts_open_dota_match() {
        let result = Match::from(ApiMatch {
            match_id: 7,
            series_id: Some(12),
            series_type: Some(2),
            radiant_name: Some("A".into()),
            dire_name: Some("B".into()),
            radiant_team_id: Some(11),
            dire_team_id: None,
            radiant_score: 10,
            dire_score: 8,
            league_name: Some("Cup".into()),
            league_id: None,
            start_time: Some(1_700_000_000),
            radiant_win: None,
            picks_bans: Some(vec![ApiPickBan {
                hero_id: 1,
                is_pick: true,
                team: 1,
                hero_name: None,
            }]),
            players: Vec::new(),
        });
        assert_eq!(result.id, 7);
        assert_eq!(result.draft[0].hero_name, "Hero #1");
        assert_eq!(result.radiant.name, "A");
        assert_eq!(result.series_id, Some(12));
        assert_eq!(result.series_type, Some(2));
        assert!(!result.finished);
    }

    #[test]
    fn uses_team_id_when_team_name_is_empty() {
        assert_eq!(team_name(Some(" ".into()), Some(42), "Radiant"), "Team #42");
        assert_eq!(team_name(None, Some(0), "Radiant"), "Radiant");
    }

    #[test]
    fn groups_supported_tournament_series() {
        assert_eq!(tournament_series("BLAST Slam IX"), Some("BLAST"));
        assert_eq!(tournament_series("ESL One Birmingham 2026"), Some("ESL"));
        assert_eq!(
            tournament_series("The International 2025"),
            Some("The International")
        );
        assert_eq!(
            tournament_series("DreamLeague Season 29"),
            Some("DreamLeague")
        );
        assert_eq!(tournament_series("Community Cup"), None);
    }

    #[test]
    fn builds_safe_hero_portrait_urls_from_open_dota_names() {
        assert_eq!(
            hero_portrait_url("npc_dota_hero_anti_mage").as_deref(),
            Some("https://cdn.cloudflare.steamstatic.com/apps/dota2/images/dota_react/heroes/anti_mage.png")
        );
        assert!(hero_portrait_url("https://example.com/image.png").is_none());
        assert!(hero_portrait_url("npc_dota_hero_").is_none());
        assert!(hero_portrait_url("npc_dota_hero__nevermore").is_none());
        assert!(hero_portrait_url("npc_dota_hero_Nevermore").is_none());
        assert!(hero_portrait_url("npc_dota_hero_nevermore/evil").is_none());
        assert!(hero_portrait_url("npc_dota_hero_nevermore.png").is_none());
    }

    #[test]
    fn maps_team_metadata_and_rejects_invalid_logo_urls() {
        let mut team = Team {
            name: "Team #42".into(),
            score: 0,
            team_id: Some(42),
            logo_url: None,
        };
        let metadata: ApiTeam = serde_json::from_str(
            r#"{
                "team_id": "42",
                "name": "OG",
                "logo_url": "https://cdn.example.com/og.png"
            }"#,
        )
        .expect("team metadata should deserialize");

        apply_team_metadata(&mut team, Some(&metadata));

        assert_eq!(team.team_id, Some(42));
        assert_eq!(team.name, "OG");
        assert_eq!(
            team.logo_url.as_deref(),
            Some("https://cdn.example.com/og.png")
        );
        assert!(safe_team_logo_url(Some(&"http://cdn.example.com/og.png".into())).is_none());

        let mut invalid = Team::default();
        let metadata: ApiTeam = serde_json::from_str(
            r#"{
                "team_id": 99,
                "name": "Team 99",
                "logo_url": "javascript:alert(1)"
            }"#,
        )
        .expect("team metadata with a string logo should deserialize");
        apply_team_metadata(&mut invalid, Some(&metadata));

        assert_eq!(invalid.team_id, Some(99));
        assert_eq!(invalid.name, "Team 99");
        assert!(invalid.logo_url.is_none());
    }

    #[test]
    fn accepts_live_api_match_shape() {
        let value: ApiMatch = serde_json::from_str(
            r#"{
                "match_id": "9092298032",
                "team_name_radiant": "Radiant",
                "team_name_dire": "Dire",
                "radiant_score": 49,
                "dire_score": 34,
                "players": [
                    {"account_id": 123, "hero_id": 76, "team": 0, "team_slot": 1}
                ]
            }"#,
        )
        .expect("live match shape should deserialize");

        assert_eq!(value.match_id, 9_092_298_032);
        assert_eq!(value.radiant_name.as_deref(), Some("Radiant"));
        assert_eq!(value.dire_name.as_deref(), Some("Dire"));
        assert_eq!(value.players.len(), 1);
        assert_eq!(value.players[0].hero_id, 76);
    }

    #[test]
    fn accepts_tournament_history_match_shape() {
        let value: ApiMatch = serde_json::from_str(
            r#"{
                "match_id": 5469223685,
                "radiant_win": true,
                "series_id": 454530,
                "series_type": 2,
                "start_time": 1592072311,
                "leagueid": 11979,
                "radiant_score": 47,
                "dire_score": 18,
                "radiant_team_id": 1838315,
                "radiant_team_name": "Team Radiant",
                "dire_team_id": 2586976,
                "dire_team_name": "Team Dire"
            }"#,
        )
        .expect("tournament match shape should deserialize");
        let converted = Match::from(value);

        assert_eq!(converted.id, 5_469_223_685);
        assert_eq!(converted.radiant.name, "Team Radiant");
        assert_eq!(converted.dire.name, "Team Dire");
        assert_eq!(converted.series_id, Some(454530));
        assert_eq!(converted.series_type, Some(2));
        assert_eq!(converted.radiant_win, Some(true));
        assert!(converted.finished);
    }

    #[test]
    fn accepts_match_detail_player_slots() {
        let value: ApiMatch = serde_json::from_str(
            r#"{
                "match_id": 9092303974,
                "picks_bans": [
                    {"hero_id": 1, "is_pick": true, "team": 0, "order": 0},
                    {"hero_id": 2, "is_pick": false, "team": 1, "order": 1}
                ],
                "players": [
                    {"hero_id": 76, "player_slot": 0},
                    {"hero_id": 7, "player_slot": 128}
                ]
            }"#,
        )
        .expect("match detail shape should deserialize");

        let converted = Match::from(value);
        assert_eq!(converted.draft.len(), 2);
        assert!(converted.draft[0].is_pick);
        assert!(converted.draft[0].is_radiant);
        assert!(!converted.draft[1].is_pick);
        assert!(!converted.draft[1].is_radiant);
    }

    #[test]
    fn falls_back_to_players_when_match_detail_has_no_draft() {
        let value: ApiMatch = serde_json::from_str(
            r#"{
                "match_id": 9092303974,
                "players": [
                    {"hero_id": 76, "player_slot": 0},
                    {"hero_id": 7, "player_slot": 128}
                ]
            }"#,
        )
        .expect("match detail shape should deserialize");

        let converted = Match::from(value);
        assert_eq!(converted.draft.len(), 2);
        assert!(converted.draft[0].is_radiant);
        assert!(!converted.draft[1].is_radiant);
    }

    #[test]
    fn filters_live_matches_to_premium_leagues_and_adds_tournament_name() {
        let mut matches: Vec<ApiMatch> = serde_json::from_str(
            r#"[
                {"match_id": 1, "league_id": 10},
                {"match_id": 2, "league_id": 20},
                {"match_id": 3, "league_id": 30},
                {"match_id": 4, "league_id": 0},
                {"match_id": 5}
            ]"#,
        )
        .expect("live matches should deserialize");
        let leagues: Vec<ApiLeague> = serde_json::from_str(
            r#"[
                {"leagueid": 10, "tier": "premium", "name": "Major"},
                {"leagueid": 20, "tier": "professional", "name": "Pro League"},
                {"leagueid": 30, "tier": "excluded", "name": "Community Cup"}
            ]"#,
        )
        .expect("league metadata should deserialize");
        let leagues = leagues
            .into_iter()
            .map(|league| (league.league_id, league))
            .collect();

        retain_premium_leagues(&mut matches, &leagues);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].match_id, 1);
        assert_eq!(matches[0].league_name.as_deref(), Some("Major"));
    }

    #[test]
    fn accepts_leagues_without_a_tier() {
        let leagues: Vec<ApiLeague> = serde_json::from_str(
            r#"[
                {"leagueid": 10, "tier": "premium", "name": "Major"},
                {"leagueid": 20, "tier": null, "name": "Unclassified"},
                {"leagueid": 30, "name": "Legacy League"}
            ]"#,
        )
        .expect("league catalog should allow missing or null tiers");

        assert_eq!(leagues[0].tier.as_deref(), Some("premium"));
        assert_eq!(leagues[1].tier, None);
        assert_eq!(leagues[2].tier, None);
    }

    #[test]
    fn converts_live_player_heroes_into_picks() {
        let value = Match::from(ApiMatch {
            match_id: 9,
            series_id: None,
            series_type: None,
            radiant_name: None,
            dire_name: None,
            radiant_team_id: Some(11),
            dire_team_id: Some(22),
            radiant_score: 0,
            dire_score: 0,
            league_name: None,
            league_id: None,
            start_time: None,
            radiant_win: None,
            picks_bans: None,
            players: vec![
                ApiLivePlayer {
                    hero_id: 5,
                    team: Some(0),
                    player_slot: None,
                    hero_name: Some("Crystal Maiden".into()),
                },
                ApiLivePlayer {
                    hero_id: 7,
                    team: Some(1),
                    player_slot: None,
                    hero_name: Some("Earthshaker".into()),
                },
            ],
        });

        assert_eq!(value.radiant.name, "Team #11");
        assert_eq!(value.dire.name, "Team #22");
        assert_eq!(value.draft.len(), 2);
        assert_eq!(value.draft[0].hero_name, "Crystal Maiden");
        assert!(value.draft[0].is_radiant);
        assert!(!value.draft[1].is_radiant);
    }
}
