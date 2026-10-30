use serde_json::{Value, json};
use sqlx::Row;

use super::{Storage, StorageError};

impl Storage {
    pub async fn publication_state(
        &self,
        destination: &str,
        channel: &str,
        revision: &str,
        image_id: &str,
        state: (&str, Option<&str>),
    ) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO publications(destination, channel, revision, image_id, status, error) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(destination, channel, revision, image_id) DO UPDATE SET status=excluded.status, error=excluded.error, updated_at=CURRENT_TIMESTAMP")
            .bind(destination).bind(channel).bind(revision).bind(image_id).bind(state.0).bind(state.1)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn record_upload(
        &self,
        destination: &str,
        key: &str,
        checksum: &str,
        size: usize,
    ) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO uploaded_objects(destination, object_key, sha256, size_bytes) VALUES (?, ?, ?, ?) ON CONFLICT(destination, object_key) DO UPDATE SET sha256=excluded.sha256, size_bytes=excluded.size_bytes, verified_at=CURRENT_TIMESTAMP")
            .bind(destination).bind(key).bind(checksum).bind(size as i64).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn record_publication_head(
        &self,
        destination: &str,
        channel: &str,
        revision: &str,
        image_id: &str,
    ) -> Result<(), StorageError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("INSERT INTO publication_heads(destination, channel, revision, image_id) VALUES (?, ?, ?, ?) ON CONFLICT(destination, channel) DO UPDATE SET revision=excluded.revision, image_id=excluded.image_id, updated_at=CURRENT_TIMESTAMP")
            .bind(destination).bind(channel).bind(revision).bind(image_id).execute(&mut *transaction).await?;
        sqlx::query("UPDATE publications SET status='published', error=NULL, updated_at=CURRENT_TIMESTAMP WHERE destination=? AND channel=? AND revision=? AND image_id=?")
            .bind(destination).bind(channel).bind(revision).bind(image_id).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn publication_status(&self) -> Result<Value, StorageError> {
        let rows = sqlx::query("SELECT destination, channel, revision, image_id, status, error, updated_at FROM publications ORDER BY updated_at DESC, revision DESC LIMIT 50").fetch_all(&self.pool).await?;
        let mut publications = Vec::new();
        for row in rows {
            publications.push(json!({
                "destination": row.try_get::<String, _>("destination")?,
                "channel": row.try_get::<String, _>("channel")?,
                "revision": row.try_get::<String, _>("revision")?,
                "image_id": row.try_get::<String, _>("image_id")?,
                "status": row.try_get::<String, _>("status")?,
                "error": row.try_get::<Option<String>, _>("error")?,
                "updated_at": row.try_get::<String, _>("updated_at")?,
            }));
        }
        Ok(json!({"publications": publications}))
    }
}
