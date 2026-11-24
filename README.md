<h1 align="center">dota2-rust-bot</h1>

<p align="center">
  Explore live Dota 2 matches and tournament history in a Telegram bot and Mini App.
</p>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/-Rust-000000?style=flat-square&logo=rust&logoColor=white" />
  <img alt="Tokio" src="https://img.shields.io/badge/-Tokio-2C3E50?style=flat-square" />
  <img alt="Telegram" src="https://img.shields.io/badge/-Telegram-26A5E4?style=flat-square&logo=telegram&logoColor=white" />
  <img alt="Docker" src="https://img.shields.io/badge/-Docker-2496ED?style=flat-square&logo=docker&logoColor=white" />
</p>

## Features

- **Live:** browse live matches from OpenDota's `premium` league category.
- **Tournament archive:** explore available BLAST, ESL, The International, and DreamLeague seasons.
- **Series and maps:** open a series to browse its maps individually, then view each map's details and draft.
- **Team and hero visuals:** show team logos and hero portraits when source data is available; fall back to team initials when a logo is missing.
- **Telegram bot:** use `/live` or `/app` to open the Mini App, and `/recent` to view recent matches.
- **Single Rust process:** the bot, API, and embedded Mini App run together; no separate frontend build is needed.

OpenDota data may not include every team logo or match draft. The live `premium` category is OpenDota's classification and is not an official, universal Tier 1 designation.

## Data sources

| Data | Source | Notes |
|---|---|---|
| Live and tournament matches | [OpenDota API](https://www.opendota.com/api-keys) | Match availability depends on OpenDota |
| Team logos | OpenDota team metadata | Displayed when a `logo_url` is available |
| Hero portraits | Dota 2 assets | Loaded by the Mini App |

## Getting started

You need the Rust toolchain and a Telegram bot token.

1. Create your local environment file:

   ```powershell
   Copy-Item .env.example .env
   ```

2. Set `TELEGRAM_BOT_TOKEN` in `.env`. The default database is SQLite; PostgreSQL is also supported through `DATABASE_URL`.

3. Run the bot:

   ```powershell
   cargo run
   ```

To launch the Mini App from Telegram, set `WEB_APP_URL` to a publicly accessible HTTPS URL and restart the bot. Telegram does not allow a Mini App to launch from `localhost` or plain HTTP. For local testing, expose the app through an HTTPS tunnel or reverse proxy.

To run with Docker Compose:

```powershell
docker compose up --build
```

The Compose service is named `dota2-bot-rust`. SQLite data is stored in `./data`; keep this directory to preserve Telegram polling state.

## Configuration

| Variable | Purpose | Default |
|---|---|---|
| `TELEGRAM_BOT_TOKEN` | Telegram bot token (required) | — |
| `DATABASE_URL` | SQLite or PostgreSQL connection URL | Set in `.env.example` |
| `OPEN_DOTA_BASE_URL` | OpenDota API base URL | `https://api.opendota.com/api` |
| `HTTP_TIMEOUT_SECS` | Outbound HTTP timeout | `10` |
| `WEB_APP_URL` | Public HTTPS URL used to open the Mini App | Empty |
| `WEB_BIND` | Axum listen address | `0.0.0.0:8080` |
| `WEB_PORT` | Host port published by Docker Compose | `8080` |
| `RUST_LOG` | Rust tracing filter | `info` |

## API

| Endpoint | Description |
|---|---|
| `GET /` | Serves the embedded Mini App |
| `GET /api/matches` | Returns live matches |
| `GET /api/matches/{match_id}` | Returns map details, lineups, and picks/bans |
| `GET /api/tournaments` | Returns available tournament seasons |
| `GET /api/tournaments/{league_id}/matches` | Returns the selected tournament's match history |
| `GET /health` | Health check (`204 No Content`) |

Mini App API requests require `Authorization: tma <initData>`. The server validates Telegram's signature and rejects data older than 24 hours. API requests from an ordinary browser are intentionally unavailable without valid Telegram `initData`.

Axum listens without TLS by default. To use Telegram Mini Apps, put it behind an HTTPS reverse proxy and set `WEB_APP_URL` to that public HTTPS address. A reverse proxy in another Compose container can forward to `http://dota2-bot-rust:8080`.

## Project structure

```text
src/
├── assets/       Embedded Mini App
├── domain/       Match and tournament models
├── providers/    OpenDota API client
├── repositories/ Database access
├── services/     Match services
├── telegram/     Bot commands and polling
├── config.rs     Environment configuration
├── main.rs       Application entry point
└── web.rs        Mini App and API routes
migrations/       SQLite and PostgreSQL migrations
```

## Checks

```powershell
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```

## Built with

[Rust](https://www.rust-lang.org/) · [Tokio](https://tokio.rs/) · [Axum](https://github.com/tokio-rs/axum) · [Teloxide](https://github.com/teloxide/teloxide) · [SQLx](https://github.com/launchbadge/sqlx) · [OpenDota](https://www.opendota.com/)

## Graphify for development

Graphify is an optional local code-navigation tool; it is not required to run the app. On Windows, install the CLI for VS Code Copilot and build the local code graph:

```powershell
python -m pip install --user "graphifyy[sql]"
graphify vscode install
graphify update . --no-cluster
```

`graphify vscode install` adds project instructions at `.github/copilot-instructions.md` and registers the skill for VS Code Copilot. The local graph is generated without LLM requests.

