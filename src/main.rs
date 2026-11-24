mod config;
mod domain;
mod providers;
mod repositories;
mod services;
mod telegram;
mod web;

use anyhow::Result;
use config::Config;
use repositories::database::DatabaseRepository;
use services::matches::MatchService;
use teloxide::prelude::*;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&config.log_filter))
        .init();
    let repository = DatabaseRepository::connect(&config.database_url).await?;
    repository.migrate().await?;
    let provider =
        providers::opendota::OpenDotaClient::new(config.opendota_base_url, config.http_timeout)?;
    let service = MatchService::new(provider);
    let bot = Bot::new(config.telegram_bot_token.clone());
    let listener = tokio::net::TcpListener::bind(config.web_bind).await?;
    tracing::info!(address = %listener.local_addr()?, "Mini App server listening");
    let web_server = tokio::spawn(web::serve(
        listener,
        service.clone(),
        config.telegram_bot_token.clone(),
    ));
    let bot_result = tokio::select! {
        result = telegram::run(bot, service, repository, config.web_app_url) => result,
        result = web_server => {
            result??;
            anyhow::bail!("Mini App server stopped unexpectedly");
        }
    };
    bot_result
}
