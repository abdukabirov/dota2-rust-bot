use anyhow::Result;
use sqlx::{postgres::PgPoolOptions, PgPool};

#[derive(Clone)]
pub struct PostgresRepository {
    pool: PgPool,
}

impl PostgresRepository {
    pub async fn connect(url: &str) -> Result<Self> {
        Ok(Self {
            pool: PgPoolOptions::new()
                .max_connections(10)
                .connect(url)
                .await?,
        })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations/postgres")
            .run(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_offset(&self, offset: i64) -> Result<()> {
        sqlx::query(
            "INSERT INTO update_state(key, value) VALUES('offset', $1) \
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        )
        .bind(offset.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn offset(&self) -> Result<i64> {
        Ok(
            sqlx::query_scalar::<_, String>("SELECT value FROM update_state WHERE key='offset'")
                .fetch_optional(&self.pool)
                .await?
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
        )
    }
}
