use std::{
    fmt, fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

use crate::domain::{FlightCandidate, SelectedFlight};

mod publication;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
static LEASE_COUNTER: AtomicU64 = AtomicU64::new(1);

pub struct Storage {
    pool: SqlitePool,
}

#[derive(Clone, Debug)]
pub struct StoredArtifact {
    pub kind: String,
    pub path: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub renderer_version: String,
    pub input_hash: String,
}

#[derive(Debug)]
pub struct StatusRecord {
    pub date: String,
    pub status: String,
    pub stage: String,
    pub attempt_count: i64,
    pub error: Option<String>,
    pub display_number: Option<String>,
    pub selected: Option<FlightCandidate>,
    pub score_total: Option<i64>,
}

impl Storage {
    pub async fn state(&self, key: &str) -> Result<Option<String>, StorageError> {
        Ok(
            sqlx::query_scalar("SELECT value FROM runtime_state WHERE key = ?")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn set_state(&self, key: &str, value: &str) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO runtime_state (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
            .bind(key).bind(value).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn connect(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(StorageError::Io)?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn begin_run(&self, date: &str) -> Result<String, StorageError> {
        let lease_token = new_lease_token();
        let result = sqlx::query(
            "INSERT INTO pipeline_runs (run_date, status, stage, lease_until, lease_token)
             VALUES (?, 'running', 'started', datetime('now', '+2 hours'), ?)
             ON CONFLICT(run_date) DO UPDATE SET
               status = 'running', stage = 'started',
               attempt_count = attempt_count + 1,
               started_at = CURRENT_TIMESTAMP, completed_at = NULL, error = NULL,
               lease_until = datetime('now', '+2 hours'), lease_token = excluded.lease_token
             WHERE pipeline_runs.status != 'running'
                OR pipeline_runs.lease_until < CURRENT_TIMESTAMP",
        )
        .bind(date)
        .bind(&lease_token)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::RunAlreadyActive(date.to_owned()));
        }
        Ok(lease_token)
    }

    pub async fn set_stage(
        &self,
        date: &str,
        lease_token: &str,
        stage: &str,
    ) -> Result<(), StorageError> {
        let result = sqlx::query(
            "UPDATE pipeline_runs
             SET stage = ?, lease_until = datetime('now', '+2 hours')
             WHERE run_date = ? AND lease_token = ? AND status = 'running'",
        )
        .bind(stage)
        .bind(date)
        .bind(lease_token)
        .execute(&self.pool)
        .await?;
        ensure_lease(result.rows_affected(), date)
    }

    pub async fn complete_run(&self, date: &str, lease_token: &str) -> Result<(), StorageError> {
        let result = sqlx::query(
            "UPDATE pipeline_runs
             SET status = 'succeeded', stage = 'completed', completed_at = CURRENT_TIMESTAMP,
                 error = NULL, lease_until = NULL
             WHERE run_date = ? AND lease_token = ? AND status = 'running'",
        )
        .bind(date)
        .bind(lease_token)
        .execute(&self.pool)
        .await?;
        ensure_lease(result.rows_affected(), date)
    }

    pub async fn fail_run(
        &self,
        date: &str,
        lease_token: &str,
        error: &str,
    ) -> Result<(), StorageError> {
        let result = sqlx::query(
            "UPDATE pipeline_runs
             SET status = 'failed', error = ?, completed_at = CURRENT_TIMESTAMP,
                 lease_until = NULL
             WHERE run_date = ? AND lease_token = ? AND status = 'running'",
        )
        .bind(error)
        .bind(date)
        .bind(lease_token)
        .execute(&self.pool)
        .await?;
        ensure_lease(result.rows_affected(), date)
    }

    pub async fn upsert_flight(&self, flight: &FlightCandidate) -> Result<i64, StorageError> {
        let payload = serde_json::to_string(flight)?;
        sqlx::query(
            "INSERT INTO flights (provider, provider_flight_id, flight_date, payload_json)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(provider, provider_flight_id, flight_date) DO UPDATE SET
               payload_json = excluded.payload_json,
               updated_at = CURRENT_TIMESTAMP",
        )
        .bind(&flight.provider)
        .bind(&flight.provider_flight_id)
        .bind(&flight.date)
        .bind(payload)
        .execute(&self.pool)
        .await?;

        Ok(sqlx::query_scalar(
            "SELECT id FROM flights
             WHERE provider = ? AND provider_flight_id = ? AND flight_date = ?",
        )
        .bind(&flight.provider)
        .bind(&flight.provider_flight_id)
        .bind(&flight.date)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn record_scoring_and_selection(
        &self,
        date: &str,
        scored_flights: &[(i64, &SelectedFlight)],
        selected_flight_id: i64,
        selected: &SelectedFlight,
        display_number: &str,
    ) -> Result<String, StorageError> {
        let mut transaction = self.pool.begin().await?;
        for (flight_id, scored) in scored_flights {
            let score = &scored.score;
            sqlx::query(
                "INSERT INTO flight_scores (
                   flight_id, algorithm_version, total, aircraft_rarity, route_rarity,
                   geographic_interest, distance, data_quality, explanation
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(flight_id, algorithm_version) DO UPDATE SET
                   total = excluded.total,
                   aircraft_rarity = excluded.aircraft_rarity,
                   route_rarity = excluded.route_rarity,
                   geographic_interest = excluded.geographic_interest,
                   distance = excluded.distance,
                   data_quality = excluded.data_quality,
                   explanation = excluded.explanation,
                   scored_at = CURRENT_TIMESTAMP",
            )
            .bind(flight_id)
            .bind(score.algorithm_version)
            .bind(i64::from(score.total))
            .bind(i64::from(score.aircraft_rarity))
            .bind(i64::from(score.route_rarity))
            .bind(i64::from(score.geographic_interest))
            .bind(i64::from(score.distance))
            .bind(i64::from(score.data_quality))
            .bind(&scored.explanation)
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query(
            "INSERT INTO daily_selections (
               selection_date, flight_id, algorithm_version, display_number, explanation
             ) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(selection_date) DO UPDATE SET
               flight_id = excluded.flight_id,
               algorithm_version = excluded.algorithm_version,
               explanation = excluded.explanation,
               selected_at = CURRENT_TIMESTAMP",
        )
        .bind(date)
        .bind(selected_flight_id)
        .bind(selected.score.algorithm_version)
        .bind(display_number)
        .bind(&selected.explanation)
        .execute(&mut *transaction)
        .await?;

        let stored_number = sqlx::query_scalar(
            "SELECT display_number FROM daily_selections WHERE selection_date = ?",
        )
        .bind(date)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(stored_number)
    }

    pub async fn artifacts_for_date(
        &self,
        date: &str,
    ) -> Result<Vec<StoredArtifact>, StorageError> {
        let rows = sqlx::query(
            "SELECT kind, path, size_bytes, sha256, renderer_version, input_hash
             FROM artifacts WHERE selection_date = ? ORDER BY kind",
        )
        .bind(date)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(StoredArtifact {
                    kind: row.try_get("kind")?,
                    path: row.try_get("path")?,
                    size_bytes: row.try_get("size_bytes")?,
                    sha256: row.try_get("sha256")?,
                    renderer_version: row.try_get("renderer_version")?,
                    input_hash: row.try_get("input_hash")?,
                })
            })
            .collect()
    }

    pub async fn record_artifact(
        &self,
        date: &str,
        artifact: &StoredArtifact,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO artifacts (
               selection_date, kind, path, size_bytes, sha256, renderer_version, input_hash
             ) VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(selection_date, kind) DO UPDATE SET
               path = excluded.path, size_bytes = excluded.size_bytes,
               sha256 = excluded.sha256, renderer_version = excluded.renderer_version,
               input_hash = excluded.input_hash, created_at = CURRENT_TIMESTAMP",
        )
        .bind(date)
        .bind(&artifact.kind)
        .bind(&artifact.path)
        .bind(artifact.size_bytes)
        .bind(&artifact.sha256)
        .bind(&artifact.renderer_version)
        .bind(&artifact.input_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn status(&self, date: Option<&str>) -> Result<Option<StatusRecord>, StorageError> {
        let row = if let Some(date) = date {
            sqlx::query(STATUS_QUERY)
                .bind(date)
                .fetch_optional(&self.pool)
                .await?
        } else {
            sqlx::query(LATEST_STATUS_QUERY)
                .fetch_optional(&self.pool)
                .await?
        };
        row.map(status_from_row).transpose()
    }

    #[cfg(test)]
    pub async fn flight_count(&self) -> Result<i64, StorageError> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM flights")
            .fetch_one(&self.pool)
            .await?)
    }

    #[cfg(test)]
    pub async fn close(self) {
        self.pool.close().await;
    }
}

const STATUS_QUERY: &str = "SELECT pr.run_date, pr.status, pr.stage, pr.attempt_count, pr.error,
            ds.display_number, f.payload_json, fs.total AS score_total
     FROM pipeline_runs pr
     LEFT JOIN daily_selections ds ON ds.selection_date = pr.run_date
     LEFT JOIN flights f ON f.id = ds.flight_id
     LEFT JOIN flight_scores fs ON fs.flight_id = ds.flight_id
       AND fs.algorithm_version = ds.algorithm_version
     WHERE pr.run_date = ?";
const LATEST_STATUS_QUERY: &str =
    "SELECT pr.run_date, pr.status, pr.stage, pr.attempt_count, pr.error,
            ds.display_number, f.payload_json, fs.total AS score_total
     FROM pipeline_runs pr
     LEFT JOIN daily_selections ds ON ds.selection_date = pr.run_date
     LEFT JOIN flights f ON f.id = ds.flight_id
     LEFT JOIN flight_scores fs ON fs.flight_id = ds.flight_id
       AND fs.algorithm_version = ds.algorithm_version
     ORDER BY pr.run_date DESC LIMIT 1";

fn status_from_row(row: sqlx::sqlite::SqliteRow) -> Result<StatusRecord, StorageError> {
    let payload: Option<String> = row.try_get("payload_json")?;
    Ok(StatusRecord {
        date: row.try_get("run_date")?,
        status: row.try_get("status")?,
        stage: row.try_get("stage")?,
        attempt_count: row.try_get("attempt_count")?,
        error: row.try_get("error")?,
        display_number: row.try_get("display_number")?,
        selected: payload
            .map(|payload| serde_json::from_str(&payload))
            .transpose()?,
        score_total: row.try_get("score_total")?,
    })
}

fn new_lease_token() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = LEASE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{timestamp}-{counter}", std::process::id())
}

fn ensure_lease(rows_affected: u64, date: &str) -> Result<(), StorageError> {
    if rows_affected == 1 {
        Ok(())
    } else {
        Err(StorageError::LeaseLost(date.to_owned()))
    }
}

#[derive(Debug)]
pub enum StorageError {
    RunAlreadyActive(String),
    LeaseLost(String),
    Sqlx(sqlx::Error),
    Migration(sqlx::migrate::MigrateError),
    Json(serde_json::Error),
    Io(std::io::Error),
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RunAlreadyActive(date) => {
                write!(
                    formatter,
                    "a pipeline run for {date} already holds the active lease"
                )
            }
            Self::LeaseLost(date) => {
                write!(
                    formatter,
                    "the pipeline run for {date} no longer owns its lease"
                )
            }
            Self::Sqlx(error) => write!(formatter, "database error: {error}"),
            Self::Migration(error) => write!(formatter, "database migration error: {error}"),
            Self::Json(error) => write!(formatter, "stored JSON error: {error}"),
            Self::Io(error) => write!(formatter, "database directory error: {error}"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<sqlx::Error> for StorageError {
    fn from(value: sqlx::Error) -> Self {
        Self::Sqlx(value)
    }
}

impl From<sqlx::migrate::MigrateError> for StorageError {
    fn from(value: sqlx::migrate::MigrateError) -> Self {
        Self::Migration(value)
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
