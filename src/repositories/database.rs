use anyhow::Result;

use super::{postgres::PostgresRepository, sqlite::SqliteRepository};

#[derive(Clone)]
pub enum DatabaseRepository {
    Sqlite(SqliteRepository),
    Postgres(PostgresRepository),
}

impl DatabaseRepository {
    pub async fn connect(url: &str) -> Result<Self> {
        if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            Ok(Self::Postgres(PostgresRepository::connect(url).await?))
        } else {
            Ok(Self::Sqlite(SqliteRepository::connect(url).await?))
        }
    }

    pub async fn migrate(&self) -> Result<()> {
        match self {
            Self::Sqlite(repository) => repository.migrate().await,
            Self::Postgres(repository) => repository.migrate().await,
        }
    }

    pub async fn set_offset(&self, offset: i64) -> Result<()> {
        match self {
            Self::Sqlite(repository) => repository.set_offset(offset).await,
            Self::Postgres(repository) => repository.set_offset(offset).await,
        }
    }

    pub async fn offset(&self) -> Result<i64> {
        match self {
            Self::Sqlite(repository) => repository.offset().await,
            Self::Postgres(repository) => repository.offset().await,
        }
    }
}
