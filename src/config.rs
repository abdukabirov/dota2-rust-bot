use anyhow::{Context, Result};
use std::{env, net::SocketAddr, time::Duration};
use url::Url;

#[derive(Clone, Debug)]
pub struct Config {
    pub telegram_bot_token: String,
    pub database_url: String,
    pub opendota_base_url: String,
    pub http_timeout: Duration,
    pub log_filter: String,
    pub web_bind: SocketAddr,
    pub web_app_url: Option<Url>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let token = required("TELEGRAM_BOT_TOKEN")?;
        if token.len() < 20 || token == "replace-me" {
            anyhow::bail!("TELEGRAM_BOT_TOKEN is invalid");
        }
        let database_url = required("DATABASE_URL")?;
        if !database_url.starts_with("sqlite://")
            && !database_url.starts_with("postgres://")
            && !database_url.starts_with("postgresql://")
        {
            anyhow::bail!("DATABASE_URL must use sqlite://, postgres://, or postgresql://");
        }
        let opendota_base_url = env::var("OPEN_DOTA_BASE_URL")
            .unwrap_or_else(|_| "https://api.opendota.com/api".to_owned());
        if !opendota_base_url.starts_with("https://") && !opendota_base_url.starts_with("http://") {
            anyhow::bail!("OPEN_DOTA_BASE_URL must be an absolute HTTP URL");
        }
        let web_bind = env::var("WEB_BIND")
            .unwrap_or_else(|_| "0.0.0.0:8080".to_owned())
            .parse()
            .context("WEB_BIND must be a valid socket address")?;
        let web_app_url = env::var("WEB_APP_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                let url =
                    Url::parse(value.trim()).context("WEB_APP_URL must be a valid absolute URL")?;
                if url.scheme() != "https" || url.host_str().is_none() {
                    anyhow::bail!("WEB_APP_URL must use HTTPS and include a host");
                }
                Ok(url)
            })
            .transpose()?;
        Ok(Self {
            telegram_bot_token: token,
            database_url,
            opendota_base_url,
            http_timeout: seconds("HTTP_TIMEOUT_SECS", 10)?,
            log_filter: env::var("RUST_LOG").unwrap_or_else(|_| "info".to_owned()),
            web_bind,
            web_app_url,
        })
    }
}

fn required(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("{name} must be set"))
}

fn seconds(name: &str, default: u64) -> Result<Duration> {
    let value = env::var(name).unwrap_or_else(|_| default.to_string());
    let seconds = value
        .parse()
        .with_context(|| format!("{name} must be an integer"))?;
    if seconds == 0 {
        anyhow::bail!("{name} must be greater than zero");
    }
    Ok(Duration::from_secs(seconds))
}
