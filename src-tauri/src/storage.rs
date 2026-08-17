use std::{collections::HashMap, fmt, path::Path, time::Duration};

use chrono::{NaiveDate, SecondsFormat};
#[cfg(test)]
use rusqlite::OptionalExtension;
use rusqlite::{Connection, params};

use crate::model::{DailyApplicationUsage, DailyUsageSummary, PendingAggregates, TrackerState};

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug)]
pub enum StorageError {
    Sqlite(rusqlite::Error),
    UnsupportedSchema(i64),
    DurationOverflow(u64),
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported database schema version {version}")
            }
            Self::DurationOverflow(value) => {
                write!(formatter, "duration {value} does not fit in SQLite INTEGER")
            }
        }
    }
}

impl std::error::Error for StorageError {}

impl From<rusqlite::Error> for StorageError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

pub struct Repository {
    connection: Connection,
}

impl Repository {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    #[cfg(test)]
    fn open_in_memory() -> Result<Self, StorageError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, StorageError> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )?;

        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        match version {
            0 => migrate_version_1(&connection)?,
            SCHEMA_VERSION => {}
            other => return Err(StorageError::UnsupportedSchema(other)),
        }

        Ok(Self { connection })
    }

    pub fn flush(&mut self, pending: &PendingAggregates) -> Result<(), StorageError> {
        if pending.is_empty() {
            return Ok(());
        }

        let transaction = self.connection.transaction()?;
        for ((date, _), usage) in pending.iter() {
            let observed_utc = usage
                .observed_utc
                .to_rfc3339_opts(SecondsFormat::Millis, true);
            let application_id: i64 = transaction.query_row(
                "INSERT INTO applications (
                    identity_key,
                    executable_path,
                    executable_name,
                    display_name,
                    first_seen_utc,
                    last_seen_utc
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(identity_key) DO UPDATE SET
                    executable_path = excluded.executable_path,
                    executable_name = excluded.executable_name,
                    display_name = excluded.display_name,
                    last_seen_utc = excluded.last_seen_utc
                 RETURNING id",
                params![
                    usage.application.identity_key,
                    usage.application.executable_path,
                    usage.application.executable_name,
                    usage.application.display_name,
                    observed_utc,
                ],
                |row| row.get(0),
            )?;
            let active_ms = i64::try_from(usage.active_ms)
                .map_err(|_| StorageError::DurationOverflow(usage.active_ms))?;

            transaction.execute(
                "INSERT INTO daily_usage (
                    local_date,
                    application_id,
                    active_ms,
                    updated_at_utc
                 ) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(local_date, application_id) DO UPDATE SET
                    active_ms = daily_usage.active_ms + excluded.active_ms,
                    updated_at_utc = excluded.updated_at_utc",
                params![
                    date.format("%Y-%m-%d").to_string(),
                    application_id,
                    active_ms,
                    observed_utc
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn daily_summary(
        &self,
        date: NaiveDate,
        pending: &PendingAggregates,
        tracker_state: TrackerState,
    ) -> Result<DailyUsageSummary, StorageError> {
        #[derive(Debug)]
        struct CombinedUsage {
            display_name: String,
            executable_name: String,
            active_ms: u64,
        }

        let mut combined: HashMap<String, CombinedUsage> = HashMap::new();
        let mut statement = self.connection.prepare(
            "SELECT
                applications.identity_key,
                applications.executable_name,
                applications.display_name,
                daily_usage.active_ms
             FROM daily_usage
             JOIN applications ON applications.id = daily_usage.application_id
             WHERE daily_usage.local_date = ?1",
        )?;
        let rows = statement.query_map([date.format("%Y-%m-%d").to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;

        for row in rows {
            let (identity_key, executable_name, display_name, active_ms) = row?;
            combined.insert(
                identity_key,
                CombinedUsage {
                    display_name,
                    executable_name,
                    active_ms: active_ms.max(0) as u64,
                },
            );
        }

        for usage in pending.for_date(date) {
            let entry = combined
                .entry(usage.application.identity_key.clone())
                .or_insert_with(|| CombinedUsage {
                    display_name: usage.application.display_name.clone(),
                    executable_name: usage.application.executable_name.clone(),
                    active_ms: 0,
                });
            entry.active_ms = entry.active_ms.saturating_add(usage.active_ms);
            entry.display_name = usage.application.display_name.clone();
            entry.executable_name = usage.application.executable_name.clone();
        }

        let total_active_ms = combined
            .values()
            .map(|usage| usage.active_ms)
            .fold(0_u64, u64::saturating_add);
        let mut applications: Vec<_> = combined
            .into_values()
            .map(|usage| DailyApplicationUsage {
                display_name: usage.display_name,
                executable_name: usage.executable_name,
                active_ms: usage.active_ms,
                share: if total_active_ms == 0 {
                    0.0
                } else {
                    usage.active_ms as f64 / total_active_ms as f64
                },
            })
            .collect();
        applications.sort_by(|left, right| {
            right
                .active_ms
                .cmp(&left.active_ms)
                .then_with(|| left.display_name.cmp(&right.display_name))
                .then_with(|| left.executable_name.cmp(&right.executable_name))
        });

        Ok(DailyUsageSummary {
            date: date.format("%Y-%m-%d").to_string(),
            tracker_state,
            total_active_ms,
            applications,
        })
    }

    #[cfg(test)]
    fn application_count(&self) -> Result<i64, StorageError> {
        Ok(self
            .connection
            .query_row("SELECT COUNT(*) FROM applications", [], |row| row.get(0))?)
    }

    #[cfg(test)]
    fn active_ms(&self, date: NaiveDate, identity_key: &str) -> Result<Option<u64>, StorageError> {
        let value = self
            .connection
            .query_row(
                "SELECT daily_usage.active_ms
                 FROM daily_usage
                 JOIN applications ON applications.id = daily_usage.application_id
                 WHERE daily_usage.local_date = ?1 AND applications.identity_key = ?2",
                params![date.format("%Y-%m-%d").to_string(), identity_key],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        Ok(value.map(|value| value.max(0) as u64))
    }

    #[cfg(test)]
    pub(crate) fn set_query_only(&self, enabled: bool) -> Result<(), StorageError> {
        self.connection.execute_batch(if enabled {
            "PRAGMA query_only = ON"
        } else {
            "PRAGMA query_only = OFF"
        })?;
        Ok(())
    }
}

pub fn flush_pending(
    repository: &mut Repository,
    pending: &mut PendingAggregates,
) -> Result<(), StorageError> {
    repository.flush(pending)?;
    pending.clear();
    Ok(())
}

fn migrate_version_1(connection: &Connection) -> Result<(), StorageError> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE applications (
           id                INTEGER PRIMARY KEY,
           identity_key      TEXT NOT NULL UNIQUE,
           executable_path   TEXT NOT NULL,
           executable_name   TEXT NOT NULL,
           display_name      TEXT NOT NULL,
           first_seen_utc    TEXT NOT NULL,
           last_seen_utc     TEXT NOT NULL
         );

         CREATE TABLE daily_usage (
           local_date        TEXT NOT NULL,
           application_id    INTEGER NOT NULL REFERENCES applications(id),
           active_ms         INTEGER NOT NULL CHECK (active_ms >= 0),
           updated_at_utc    TEXT NOT NULL,
           PRIMARY KEY (local_date, application_id)
         );

         PRAGMA user_version = 1;
         COMMIT;",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    use super::*;
    use crate::model::ApplicationIdentity;

    fn app(identity: &str, display_name: &str) -> ApplicationIdentity {
        ApplicationIdentity {
            identity_key: format!("c:\\\\apps\\\\{identity}.exe"),
            executable_path: format!("C:\\\\Apps\\\\{identity}.exe"),
            executable_name: format!("{identity}.exe"),
            display_name: display_name.into(),
        }
    }

    fn add(
        pending: &mut PendingAggregates,
        date: &str,
        application: &ApplicationIdentity,
        active_ms: u64,
    ) {
        pending.add(
            NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            application,
            active_ms,
            Utc.with_ymd_and_hms(2026, 8, 14, 12, 0, 0).unwrap(),
        );
    }

    #[test]
    fn migration_enables_required_pragmas_and_schema() {
        let repository = Repository::open_in_memory().unwrap();
        let version: i64 = repository
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        let foreign_keys: i64 = repository
            .connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        let synchronous: i64 = repository
            .connection
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .unwrap();

        assert_eq!(version, 1);
        assert_eq!(foreign_keys, 1);
        assert_eq!(synchronous, 1);
    }

    #[test]
    fn transactional_flush_upserts_applications_and_increments_days() {
        let mut repository = Repository::open_in_memory().unwrap();
        let editor = app("editor", "编辑器");

        let mut first = PendingAggregates::default();
        add(&mut first, "2026-08-14", &editor, 2_000);
        repository.flush(&first).unwrap();

        let mut repeated = PendingAggregates::default();
        add(&mut repeated, "2026-08-14", &editor, 3_000);
        add(&mut repeated, "2026-08-15", &editor, 4_000);
        repository.flush(&repeated).unwrap();

        assert_eq!(repository.application_count().unwrap(), 1);
        assert_eq!(
            repository
                .active_ms(
                    NaiveDate::from_ymd_opt(2026, 8, 14).unwrap(),
                    &editor.identity_key
                )
                .unwrap(),
            Some(5_000)
        );
        assert_eq!(
            repository
                .active_ms(
                    NaiveDate::from_ymd_opt(2026, 8, 15).unwrap(),
                    &editor.identity_key
                )
                .unwrap(),
            Some(4_000)
        );
    }

    #[test]
    fn failed_flush_retains_pending_data_until_a_commit_succeeds() {
        let mut repository = Repository::open_in_memory().unwrap();
        let mut pending = PendingAggregates::default();
        add(&mut pending, "2026-08-14", &app("editor", "编辑器"), 2_000);
        repository
            .connection
            .execute_batch("PRAGMA query_only = ON")
            .unwrap();

        assert!(flush_pending(&mut repository, &mut pending).is_err());
        assert_eq!(pending.total_ms(), 2_000);

        repository
            .connection
            .execute_batch("PRAGMA query_only = OFF")
            .unwrap();
        flush_pending(&mut repository, &mut pending).unwrap();
        assert!(pending.is_empty());
    }

    #[test]
    fn daily_summary_merges_committed_and_pending_exactly_once_and_sorts() {
        let mut repository = Repository::open_in_memory().unwrap();
        let editor = app("editor", "编辑器");
        let browser = app("browser", "浏览器");
        let terminal = app("terminal", "终端");
        let date = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();

        let mut committed = PendingAggregates::default();
        add(&mut committed, "2026-08-14", &editor, 3_000);
        add(&mut committed, "2026-08-14", &browser, 2_000);
        repository.flush(&committed).unwrap();

        let mut pending = PendingAggregates::default();
        add(&mut pending, "2026-08-14", &editor, 2_000);
        add(&mut pending, "2026-08-14", &terminal, 5_000);

        let summary = repository
            .daily_summary(date, &pending, TrackerState::Recording)
            .unwrap();

        assert_eq!(summary.total_active_ms, 12_000);
        assert_eq!(summary.applications.len(), 3);
        assert_eq!(summary.applications[0].display_name, "终端");
        assert_eq!(summary.applications[1].display_name, "编辑器");
        assert_eq!(summary.applications[2].display_name, "浏览器");
        assert_eq!(summary.applications[1].active_ms, 5_000);
        assert!((summary.applications[1].share - 5.0 / 12.0).abs() < f64::EPSILON);
        let serialized = serde_json::to_string(&summary).unwrap();
        assert!(!serialized.contains("executablePath"));
        assert!(!serialized.contains("C:\\\\Apps"));
    }

    #[test]
    fn committed_totals_survive_reopen_and_uncommitted_data_can_be_absent() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("usage.sqlite3");
        let editor = app("editor", "编辑器");
        let date = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();

        {
            let mut repository = Repository::open(&path).unwrap();
            let mut committed = PendingAggregates::default();
            add(&mut committed, "2026-08-14", &editor, 8_000);
            repository.flush(&committed).unwrap();

            let mut uncommitted = PendingAggregates::default();
            add(&mut uncommitted, "2026-08-14", &editor, 2_000);
            assert_eq!(uncommitted.total_ms(), 2_000);
        }

        let repository = Repository::open(&path).unwrap();
        let summary = repository
            .daily_summary(date, &PendingAggregates::default(), TrackerState::Recording)
            .unwrap();
        assert_eq!(summary.total_active_ms, 8_000);
    }
}
