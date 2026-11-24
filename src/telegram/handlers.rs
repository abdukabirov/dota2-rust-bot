use crate::{
    providers::opendota::MatchProvider,
    services::matches::{format_match, MatchService},
};
use anyhow::Result;
use std::time::Duration;
use teloxide::{
    prelude::*,
    types::{ChatId, InlineKeyboardButton, InlineKeyboardMarkup, Message, WebAppInfo},
    RequestError,
};
use tokio::time::sleep;
use url::Url;

const MAX_SEND_ATTEMPTS: usize = 3;

pub async fn message<P: MatchProvider>(
    bot: Bot,
    msg: Message,
    service: MatchService<P>,
    web_app_url: Option<&Url>,
) -> Result<()> {
    let text = msg.text().unwrap_or("");
    let mut parts = text.split_whitespace();
    let command = parts.next().unwrap_or("").split('@').next().unwrap_or("");
    match command {
        "/start" => {
            send_text(&bot, msg.chat.id, help_text()).await?;
        }
        "/help" => {
            send_text(&bot, msg.chat.id, help_text()).await?;
        }
        "/live" | "/app" => {
            if !msg.chat.is_private() {
                send_text(
                    &bot,
                    msg.chat.id,
                    "Откройте личный чат с ботом и отправьте /live, чтобы запустить Mini App.",
                )
                .await?;
            } else if let Some(url) = web_app_url {
                bot.send_message(msg.chat.id, "Dota 2 · Live и архив турниров")
                    .reply_markup(InlineKeyboardMarkup::new(vec![vec![
                        InlineKeyboardButton::web_app(
                            "Открыть матчи",
                            WebAppInfo { url: url.clone() },
                        ),
                    ]]))
                    .await?;
            } else {
                tracing::warn!("WEB_APP_URL is not configured");
                send_text(
                    &bot,
                    msg.chat.id,
                    "Mini App пока не настроен. Укажите публичный HTTPS-адрес в WEB_APP_URL и перезапустите бота.",
                )
                .await?;
            }
        }
        "/recent" => {
            let result = service.recent().await;
            send_result(&bot, msg.chat.id, result).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn send_text(bot: &Bot, chat_id: ChatId, text: impl Into<String>) -> Result<()> {
    let text = text.into();
    let mut retries = 0;
    loop {
        match bot.send_message(chat_id, text.clone()).await {
            Ok(_) => return Ok(()),
            Err(RequestError::RetryAfter(delay)) if retries + 1 < MAX_SEND_ATTEMPTS => {
                retries += 1;
                tracing::warn!(
                    retry_after_seconds = delay.seconds(),
                    attempt = retries,
                    "Telegram rate limit reached; delaying message retry"
                );
                sleep(delay.duration()).await;
            }
            Err(RequestError::Network(error))
                if error.is_connect() && retries + 1 < MAX_SEND_ATTEMPTS =>
            {
                let retry_delay = Duration::from_millis(250 * 2_u64.pow(retries as u32));
                retries += 1;
                tracing::warn!(
                    attempt = retries,
                    retry_in_ms = retry_delay.as_millis(),
                    error = %super::safe_network_error(&error, bot.token()),
                    "Telegram message connection failed; retrying"
                );
                sleep(retry_delay).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn help_text() -> &'static str {
    "Dota 2 Chat Bot\n\nКоманды:\n/live (или /app) — открыть Mini App с live-матчами и архивом турниров\n/recent — последние pro-матчи\n/help — эта справка"
}

async fn send_provider_error(bot: &Bot, chat_id: ChatId, error: anyhow::Error) -> Result<()> {
    tracing::error!(%error, "match provider request failed");
    send_text(
        bot,
        chat_id,
        "Не удалось загрузить матчи. Попробуйте ещё раз через минуту.",
    )
    .await?;
    Ok(())
}

async fn send_result(
    bot: &Bot,
    chat_id: ChatId,
    result: Result<Vec<crate::domain::r#match::Match>>,
) -> Result<()> {
    match result {
        Ok(matches) => send_matches(bot, chat_id, matches).await,
        Err(error) => send_provider_error(bot, chat_id, error).await,
    }
}

async fn send_matches(
    bot: &Bot,
    chat_id: ChatId,
    matches: Vec<crate::domain::r#match::Match>,
) -> Result<()> {
    if matches.is_empty() {
        send_text(bot, chat_id, "Матчей не найдено.").await?;
        return Ok(());
    }
    for value in matches {
        send_text(bot, chat_id, format_match(&value)).await?;
    }
    Ok(())
}
