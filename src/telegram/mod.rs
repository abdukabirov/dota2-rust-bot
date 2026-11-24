pub mod handlers;

use crate::{
    providers::opendota::MatchProvider, repositories::database::DatabaseRepository,
    services::matches::MatchService,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::time::Duration;
use teloxide::{prelude::*, types::Message, ApiError, RequestError};
use url::Url;

pub async fn run<P: MatchProvider>(
    bot: Bot,
    service: MatchService<P>,
    repository: DatabaseRepository,
    web_app_url: Option<Url>,
) -> Result<()> {
    let mut attempt = 0_u32;
    let identity = loop {
        let result = tokio::select! {
            result = bot.get_me() => result,
            _ = tokio::signal::ctrl_c() => return Ok(()),
        };
        match result {
            Ok(identity) => break identity,
            Err(RequestError::Api(ApiError::InvalidToken)) => {
                anyhow::bail!("Telegram rejected TELEGRAM_BOT_TOKEN (invalid or revoked)")
            }
            Err(error) => {
                let retry_delay = match &error {
                    RequestError::RetryAfter(delay) => delay.duration(),
                    _ => Duration::from_secs(1_u64 << attempt.min(5)).min(Duration::from_secs(30)),
                };
                attempt = attempt.saturating_add(1);
                tracing::warn!(
                    attempt,
                    retry_in_seconds = retry_delay.as_secs(),
                    error = %safe_request_error(error, bot.token()),
                    "Telegram getMe failed; retrying"
                );
                tokio::select! {
                    _ = tokio::time::sleep(retry_delay) => {},
                    _ = tokio::signal::ctrl_c() => return Ok(()),
                }
            }
        }
    };
    tracing::info!(
        bot_id = identity.id.0,
        username = identity.username.as_deref().unwrap_or("<no username>"),
        "Telegram polling started"
    );
    let client = reqwest::Client::new();
    remove_webhook(&bot, &client).await;
    let mut offset = repository.offset().await?;
    let endpoint = format!("https://api.telegram.org/bot{}/getUpdates", bot.token());
    let mut conflict_attempts = 0_u32;

    loop {
        let response = tokio::select! {
            result = client            .post(&endpoint)
            .json(&json!({
                "offset": offset,
                "limit": 100,
                "timeout": 30,
                "allowed_updates": vec!["message"],
            }))
            .send() => match result {
                Ok(response) => response,
                Err(_) => {
                    tracing::warn!("Telegram getUpdates network error; retrying");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            },
            _ = tokio::signal::ctrl_c() => break,
        };
        let status = response.status();
        if status == reqwest::StatusCode::CONFLICT {
            retry_after_conflict(&bot, &client, &mut conflict_attempts).await;
            continue;
        }
        if !status.is_success() {
            anyhow::bail!("Telegram getUpdates returned HTTP status {status}");
        }
        let response = response
            .json::<Value>()
            .await
            .map_err(|_| anyhow::anyhow!("Telegram getUpdates returned invalid JSON"))?;
        if !response.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            let description = response
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("unknown Telegram error");
            if response.get("error_code").and_then(Value::as_u64) == Some(409) {
                retry_after_conflict(&bot, &client, &mut conflict_attempts).await;
                continue;
            }
            anyhow::bail!("Telegram getUpdates returned an unsuccessful response: {description}");
        }

        conflict_attempts = 0;
        let updates = response
            .get("result")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        tracing::info!(count = updates.len(), "Telegram updates received");
        for raw in updates {
            let update_id = raw
                .get("update_id")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow::anyhow!("Telegram update has no valid update_id"))?;
            let next_offset = update_id
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("Telegram update_id overflow"))?;
            let update_keys = raw
                .as_object()
                .map(|value| value.keys().cloned().collect::<Vec<_>>());
            tracing::info!(update_id, keys = ?update_keys, "Telegram update received");

            if let Some(message) = raw.get("message") {
                match serde_json::from_value::<Message>(message.clone()) {
                    Ok(message) => {
                        tracing::info!("Telegram message received");
                        if let Err(error) = handlers::message(
                            bot.clone(),
                            message,
                            service.clone(),
                            web_app_url.as_ref(),
                        )
                        .await
                        {
                            tracing::error!(%error, "message handler failed");
                        }
                    }
                    Err(error) => tracing::warn!(%error, "ignoring invalid Telegram message"),
                }
            } else {
                tracing::debug!(update_id, keys = ?update_keys, "Ignoring unsupported Telegram update");
            }

            offset = i64::try_from(next_offset)?;
            repository.set_offset(offset).await?;
        }
    }
    Ok(())
}

pub(super) fn safe_network_error<E>(error: &E, token: &str) -> String
where
    E: std::error::Error + ?Sized,
{
    let mut details = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        details.push_str(": ");
        details.push_str(&cause.to_string());
        source = std::error::Error::source(cause);
    }
    details.replace(token, "[redacted]")
}

fn safe_request_error(error: RequestError, token: &str) -> String {
    match error {
        RequestError::Api(error) => error.to_string(),
        RequestError::Network(error) => safe_network_error(&error, token),
        RequestError::RetryAfter(delay) => {
            format!("rate limited; retry after {} seconds", delay.seconds())
        }
        RequestError::MigrateToChatId(_) => "chat migration response".to_owned(),
        RequestError::InvalidJson { source, .. } => format!("invalid JSON response: {source}"),
        RequestError::Io(error) => format!("I/O error: {error}"),
    }
}

async fn remove_webhook(bot: &Bot, client: &reqwest::Client) -> bool {
    let endpoint = format!("https://api.telegram.org/bot{}/deleteWebhook", bot.token());
    for attempt in 0..3 {
        let result = match client
            .post(&endpoint)
            .json(&json!({"drop_pending_updates": false}))
            .send()
            .await
        {
            Ok(response) => response.json::<Value>().await.ok(),
            Err(_) => None,
        };
        if result
            .as_ref()
            .and_then(|value| value.get("ok"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            tracing::info!("Telegram webhook removed; long polling is enabled");
            return true;
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_secs(2_u64.pow(attempt))).await;
        }
    }
    tracing::warn!("Could not remove the Telegram webhook; polling will retry");
    false
}

async fn retry_after_conflict(bot: &Bot, client: &reqwest::Client, attempts: &mut u32) {
    *attempts = (*attempts).saturating_add(1);
    if *attempts == 1 {
        tracing::warn!(
            "Telegram getUpdates conflict detected; removing any webhook and retrying polling"
        );
        remove_webhook(bot, client).await;
    } else {
        tracing::warn!(
            attempt = *attempts,
            "Telegram polling is busy in another process; retrying"
        );
    }
    let delay = 2_u64.saturating_pow((*attempts).min(5));
    tokio::time::sleep(Duration::from_secs(delay.min(30))).await;
}
