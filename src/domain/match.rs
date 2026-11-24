use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Team {
    pub name: String,
    pub score: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DraftAction {
    pub hero_id: u32,
    pub hero_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hero_image_url: Option<String>,
    pub is_pick: bool,
    pub is_radiant: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Match {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_type: Option<u8>,
    pub radiant: Team,
    pub dire: Team,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radiant_win: Option<bool>,
    pub tournament: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished: bool,
    pub draft: Vec<DraftAction>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tournament {
    pub id: i64,
    pub name: String,
    pub series: String,
}
