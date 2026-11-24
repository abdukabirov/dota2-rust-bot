use crate::{providers::opendota::MatchProvider, services::matches::MatchService};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, Response, StatusCode},
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;
use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::net::TcpListener;

const MINI_APP_HTML: &str = include_str!("assets/mini_app.html");
const MAX_INIT_DATA_BYTES: usize = 8 * 1024;
const MAX_INIT_DATA_AGE_SECS: u64 = 24 * 60 * 60;
const MAX_FUTURE_SKEW_SECS: u64 = 60;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct AppState<P> {
    service: MatchService<P>,
    bot_token: String,
}

pub async fn serve<P: MatchProvider>(
    listener: TcpListener,
    service: MatchService<P>,
    bot_token: String,
) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/api/matches", get(live_matches))
        .route("/api/matches/{match_id}", get(match_details))
        .route("/api/tournaments", get(tournaments))
        .route(
            "/api/tournaments/{league_id}/matches",
            get(tournament_matches),
        )
        .route("/health", get(health))
        .with_state(AppState { service, bot_token });
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Response<Body> {
    let mut response = Html(MINI_APP_HTML).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

async fn health() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn live_matches<P: MatchProvider>(
    State(state): State<AppState<P>>,
    headers: HeaderMap,
) -> Response<Body> {
    if !authorized(&headers, &state.bot_token) {
        return unauthorized();
    }

    match state.service.live().await {
        Ok(matches) => no_store(Json(matches).into_response()),
        Err(error) => {
            tracing::error!(%error, "Mini App could not load live matches");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "Live matches are temporarily unavailable"})),
            )
                .into_response()
        }
    }
}

async fn tournaments<P: MatchProvider>(
    State(state): State<AppState<P>>,
    headers: HeaderMap,
) -> Response<Body> {
    if !authorized(&headers, &state.bot_token) {
        return unauthorized();
    }

    match state.service.tournaments().await {
        Ok(tournaments) => no_store(Json(tournaments).into_response()),
        Err(error) => {
            tracing::error!(%error, "Mini App could not load tournament leagues");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "Tournament archive is temporarily unavailable"})),
            )
                .into_response()
        }
    }
}

async fn match_details<P: MatchProvider>(
    State(state): State<AppState<P>>,
    Path(match_id): Path<i64>,
    headers: HeaderMap,
) -> Response<Body> {
    if !authorized(&headers, &state.bot_token) {
        return unauthorized();
    }

    match state.service.get_match(match_id).await {
        Ok(value) => no_store(Json(value).into_response()),
        Err(error) => {
            tracing::error!(match_id, %error, "Mini App could not load match details");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "Match details are temporarily unavailable"})),
            )
                .into_response()
        }
    }
}

async fn tournament_matches<P: MatchProvider>(
    State(state): State<AppState<P>>,
    Path(league_id): Path<i64>,
    headers: HeaderMap,
) -> Response<Body> {
    if !authorized(&headers, &state.bot_token) {
        return unauthorized();
    }

    match state.service.tournament_matches(league_id).await {
        Ok(matches) => no_store(Json(matches).into_response()),
        Err(error) => {
            tracing::error!(league_id, %error, "Mini App could not load tournament matches");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "Tournament matches are temporarily unavailable"})),
            )
                .into_response()
        }
    }
}

fn authorized(headers: &HeaderMap, bot_token: &str) -> bool {
    let init_data = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("tma "));
    init_data.is_some_and(|value| validate_init_data(value, bot_token, current_timestamp()).is_ok())
}

fn unauthorized() -> Response<Body> {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "Telegram session is missing or invalid"})),
    )
        .into_response()
}

fn no_store(mut response: Response<Body>) -> Response<Body> {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn validate_init_data(init_data: &str, bot_token: &str, now: u64) -> Result<(), InitDataError> {
    if init_data.is_empty() || init_data.len() > MAX_INIT_DATA_BYTES {
        return Err(InitDataError);
    }

    let mut fields = BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(init_data.as_bytes()) {
        if fields
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(InitDataError);
        }
    }

    let supplied_hash = fields.remove("hash").ok_or(InitDataError)?;
    let auth_date = fields
        .get("auth_date")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or(InitDataError)?;
    if auth_date > now.saturating_add(MAX_FUTURE_SKEW_SECS)
        || now.saturating_sub(auth_date) > MAX_INIT_DATA_AGE_SECS
    {
        return Err(InitDataError);
    }

    let supplied_hash = decode_hash(&supplied_hash).ok_or(InitDataError)?;
    let data_check_string = fields
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut secret = HmacSha256::new_from_slice(b"WebAppData").map_err(|_| InitDataError)?;
    secret.update(bot_token.as_bytes());
    let secret_key = secret.finalize().into_bytes();
    let mut verifier = HmacSha256::new_from_slice(&secret_key).map_err(|_| InitDataError)?;
    verifier.update(data_check_string.as_bytes());
    verifier
        .verify_slice(&supplied_hash)
        .map_err(|_| InitDataError)
}

fn decode_hash(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.is_ascii() {
        return None;
    }
    let mut result = [0; 32];
    for (index, pair) in value.as_bytes().chunks(2).enumerate() {
        result[index] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(result)
}

#[derive(Debug)]
struct InitDataError;

#[cfg(test)]
mod tests {
    use super::*;
    use url::form_urlencoded;

    fn signed_init_data(bot_token: &str, auth_date: u64) -> String {
        let fields = BTreeMap::from([
            ("auth_date", auth_date.to_string()),
            ("query_id", "AAEAAAE".to_owned()),
            ("user", r#"{"id":123,"first_name":"Dota"}"#.to_owned()),
        ]);
        let data_check_string = fields
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut secret = HmacSha256::new_from_slice(b"WebAppData").unwrap();
        secret.update(bot_token.as_bytes());
        let secret_key = secret.finalize().into_bytes();
        let mut signer = HmacSha256::new_from_slice(&secret_key).unwrap();
        signer.update(data_check_string.as_bytes());
        let hash = signer
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mut query = form_urlencoded::Serializer::new(String::new());
        for (key, value) in fields {
            query.append_pair(key, &value);
        }
        query.append_pair("hash", &hash);
        query.finish()
    }

    #[test]
    fn accepts_valid_telegram_init_data() {
        let data = signed_init_data("test-bot-token", 1_800_000_000);
        assert!(validate_init_data(&data, "test-bot-token", 1_800_000_001).is_ok());
    }

    #[test]
    fn rejects_tampered_expired_future_and_duplicate_init_data() {
        let valid = signed_init_data("test-bot-token", 1_800_000_000);
        assert!(validate_init_data(&valid, "wrong-token", 1_800_000_001).is_err());
        assert!(validate_init_data(&valid, "test-bot-token", 1_800_000_000 + 86_401).is_err());
        assert!(validate_init_data(&valid, "test-bot-token", 1_800_000_000 - 61).is_err());
        assert!(validate_init_data(
            &format!("{valid}&auth_date=1800000000"),
            "test-bot-token",
            1_800_000_001
        )
        .is_err());
    }

    #[test]
    fn rejects_tampered_malformed_and_duplicate_hashes() {
        let valid = signed_init_data("test-bot-token", 1_800_000_000);
        let (fields, hash) = valid.rsplit_once("&hash=").unwrap();
        let tampered_hash = format!(
            "{}{}",
            if hash.starts_with('0') { "1" } else { "0" },
            &hash[1..]
        );

        assert!(validate_init_data(
            &format!("{fields}&hash={tampered_hash}"),
            "test-bot-token",
            1_800_000_001
        )
        .is_err());
        assert!(validate_init_data(
            &format!("{fields}&hash=not-a-hash"),
            "test-bot-token",
            1_800_000_001
        )
        .is_err());
        assert!(validate_init_data(
            &format!("{valid}&hash={hash}"),
            "test-bot-token",
            1_800_000_001
        )
        .is_err());
    }
}
