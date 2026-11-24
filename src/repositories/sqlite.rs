use anyhow::Result;
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

#[derive(Clone)]
pub struct SqliteRepository {
    pool: SqlitePool,
}

impl SqliteRepository {
    pub async fn connect(url: &str) -> Result<Self> {
        let url = if url.starts_with("sqlite://") && !url.contains('?') {
            format!("{url}?mode=rwc")
        } else {
            url.to_owned()
        };
        Ok(Self {
            pool: SqlitePoolOptions::new()
                .max_connections(5)
                .connect(&url)
                .await?,
        })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    pub async fn set_offset(&self, offset: i64) -> Result<()> {
        sqlx::query("INSERT INTO update_state(key, value) VALUES('offset', ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
            .bind(offset.to_string()).execute(&self.pool).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn stores_telegram_update_offset() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("test.db");
        std::fs::File::create(&database_path).unwrap();
        let repo = SqliteRepository::connect(&format!("sqlite://{}", database_path.display()))
            .await
            .unwrap();
        repo.migrate().await.unwrap();
        let removed_tables = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
             AND name IN ('subscriptions', 'notification_deliveries', 'widget_messages')",
        )
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert_eq!(removed_tables, 0);
        repo.set_offset(9).await.unwrap();
        assert_eq!(repo.offset().await.unwrap(), 9);
    }
}
