use std::{
    fmt,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use chrono::NaiveDate;

use crate::{
    accounting::AccountingState,
    model::{CommandError, DailyUsageSummary, PendingAggregates, TrackerState, TrackerStatus},
    storage::{Repository, flush_pending},
    windows_adapter::WindowsAdapter,
};

pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(30);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(7);

enum ControlMessage {
    GetDailyUsage {
        date: NaiveDate,
        reply: SyncSender<Result<DailyUsageSummary, CommandError>>,
    },
    GetStatus {
        reply: SyncSender<TrackerStatus>,
    },
    Flush {
        reply: SyncSender<Result<(), CommandError>>,
    },
    Shutdown {
        reply: SyncSender<Result<(), CommandError>>,
    },
}

struct CollectorLifetime {
    join: Mutex<Option<JoinHandle<()>>>,
    shutdown_started: AtomicBool,
}

#[derive(Clone)]
pub struct CollectorHandle {
    sender: Sender<ControlMessage>,
    lifetime: Arc<CollectorLifetime>,
}

#[derive(Debug)]
pub struct CollectorStartError(String);

impl fmt::Display for CollectorStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CollectorStartError {}

impl CollectorHandle {
    pub fn start(database_path: PathBuf) -> Result<Self, CollectorStartError> {
        let (sender, receiver) = mpsc::channel();
        let join = thread::Builder::new()
            .name("daily-usage-collector".into())
            .spawn(move || worker_loop(receiver, database_path))
            .map_err(|error| CollectorStartError(format!("failed to start collector: {error}")))?;

        Ok(Self {
            sender,
            lifetime: Arc::new(CollectorLifetime {
                join: Mutex::new(Some(join)),
                shutdown_started: AtomicBool::new(false),
            }),
        })
    }

    pub fn daily_usage(&self, date: NaiveDate) -> Result<DailyUsageSummary, CommandError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .send(ControlMessage::GetDailyUsage { date, reply })
            .map_err(|_| worker_unavailable())?;
        response
            .recv_timeout(COMMAND_TIMEOUT)
            .map_err(|_| command_timeout())?
    }

    pub fn status(&self) -> Result<TrackerStatus, CommandError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .send(ControlMessage::GetStatus { reply })
            .map_err(|_| worker_unavailable())?;
        response
            .recv_timeout(COMMAND_TIMEOUT)
            .map_err(|_| command_timeout())
    }

    pub fn flush(&self) -> Result<(), CommandError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .send(ControlMessage::Flush { reply })
            .map_err(|_| worker_unavailable())?;
        response
            .recv_timeout(SHUTDOWN_TIMEOUT)
            .map_err(|_| CommandError::new("flush_timeout", "使用统计未能及时保存。"))?
    }

    pub fn shutdown(&self) -> Result<(), CommandError> {
        if self.lifetime.shutdown_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        let (reply, response) = mpsc::sync_channel(1);
        if self
            .sender
            .send(ControlMessage::Shutdown { reply })
            .is_err()
        {
            return Err(worker_unavailable());
        }
        let flush_result = response
            .recv_timeout(SHUTDOWN_TIMEOUT)
            .map_err(|_| CommandError::new("shutdown_timeout", "采集器未能及时停止。"))?;

        if let Some(join) = self
            .lifetime
            .join
            .lock()
            .ok()
            .and_then(|mut join| join.take())
        {
            let _ = join.join();
        }
        flush_result
    }
}

fn worker_unavailable() -> CommandError {
    CommandError::new("collector_unavailable", "后台采集器暂时不可用。")
}

fn command_timeout() -> CommandError {
    CommandError::new("collector_timeout", "后台采集器响应超时，请稍后重试。")
}

struct WorkerState {
    database_path: PathBuf,
    repository: Option<Repository>,
    accounting: AccountingState,
    pending: PendingAggregates,
    persistence_error: bool,
}

impl WorkerState {
    fn open(database_path: PathBuf) -> Self {
        let repository = Repository::open(&database_path).ok();
        let persistence_error = repository.is_none();
        Self {
            database_path,
            repository,
            accounting: AccountingState::default(),
            pending: PendingAggregates::default(),
            persistence_error,
        }
    }

    #[cfg(test)]
    fn with_repository(database_path: PathBuf, repository: Repository) -> Self {
        Self {
            database_path,
            repository: Some(repository),
            accounting: AccountingState::default(),
            pending: PendingAggregates::default(),
            persistence_error: false,
        }
    }

    fn observe(&mut self, adapter: &mut WindowsAdapter) {
        self.accounting.observe(adapter.sample(), &mut self.pending);
    }

    fn effective_state(&self) -> TrackerState {
        if self.persistence_error {
            TrackerState::Error
        } else {
            self.accounting.tracker_state()
        }
    }

    fn status(&self) -> TrackerStatus {
        TrackerStatus {
            state: self.effective_state(),
            message: self
                .persistence_error
                .then(|| "本地数据暂时无法保存，采集器将自动重试。".to_string()),
        }
    }

    fn daily_usage(&self, date: NaiveDate) -> Result<DailyUsageSummary, CommandError> {
        let Some(repository) = self.repository.as_ref() else {
            return Err(CommandError::new(
                "storage_unavailable",
                "本地使用记录暂时无法读取。",
            ));
        };
        repository
            .daily_summary(date, &self.pending, self.effective_state())
            .map_err(|_| CommandError::new("storage_read_failed", "读取本地使用记录失败。"))
    }

    fn flush(&mut self) -> Result<(), CommandError> {
        if self.repository.is_none() {
            self.repository = Repository::open(&self.database_path).ok();
        }
        let Some(repository) = self.repository.as_mut() else {
            self.persistence_error = true;
            return Err(CommandError::new(
                "storage_unavailable",
                "本地数据暂时无法保存。",
            ));
        };

        match flush_pending(repository, &mut self.pending) {
            Ok(()) => {
                self.persistence_error = false;
                Ok(())
            }
            Err(_) => {
                self.persistence_error = true;
                Err(CommandError::new(
                    "storage_write_failed",
                    "本地数据保存失败，将在稍后重试。",
                ))
            }
        }
    }
}

fn worker_loop(receiver: Receiver<ControlMessage>, database_path: PathBuf) {
    let mut worker = WorkerState::open(database_path);
    let mut adapter = WindowsAdapter::new();
    let mut next_sample = Instant::now();
    let mut next_flush = Instant::now() + FLUSH_INTERVAL;

    loop {
        let now = Instant::now();
        let wake_at = next_sample.min(next_flush);
        let timeout = wake_at.saturating_duration_since(now);

        match receiver.recv_timeout(timeout) {
            Ok(ControlMessage::GetDailyUsage { date, reply }) => {
                let _ = reply.send(worker.daily_usage(date));
            }
            Ok(ControlMessage::GetStatus { reply }) => {
                let _ = reply.send(worker.status());
            }
            Ok(ControlMessage::Flush { reply }) => {
                let result = worker.flush();
                let _ = reply.send(result);
            }
            Ok(ControlMessage::Shutdown { reply }) => {
                let result = worker.flush();
                let _ = reply.send(result);
                break;
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = worker.flush();
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }

        let now = Instant::now();
        if now >= next_sample {
            worker.observe(&mut adapter);
            next_sample = now + SAMPLE_INTERVAL;
        }
        if now >= next_flush {
            let _ = worker.flush();
            next_flush = now + FLUSH_INTERVAL;
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    use super::*;
    use crate::model::ApplicationIdentity;

    fn pending_usage() -> PendingAggregates {
        let mut pending = PendingAggregates::default();
        pending.add(
            NaiveDate::from_ymd_opt(2026, 8, 14).unwrap(),
            &ApplicationIdentity {
                identity_key: "c:\\apps\\editor.exe".into(),
                executable_path: "C:\\Apps\\editor.exe".into(),
                executable_name: "editor.exe".into(),
                display_name: "编辑器".into(),
            },
            2_000,
            Utc.with_ymd_and_hms(2026, 8, 14, 12, 0, 0).unwrap(),
        );
        pending
    }

    #[test]
    fn worker_intervals_match_the_design_contract() {
        assert_eq!(SAMPLE_INTERVAL, Duration::from_secs(2));
        assert_eq!(FLUSH_INTERVAL, Duration::from_secs(30));
    }

    #[test]
    fn failed_flush_sets_error_and_keeps_pending_for_retry() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("usage.sqlite3");
        let repository = Repository::open(&path).unwrap();
        repository.set_query_only(true).unwrap();
        let mut worker = WorkerState::with_repository(path, repository);
        worker.pending = pending_usage();

        assert!(worker.flush().is_err());
        assert_eq!(worker.pending.total_ms(), 2_000);
        assert_eq!(worker.status().state, TrackerState::Error);

        worker
            .repository
            .as_ref()
            .unwrap()
            .set_query_only(false)
            .unwrap();
        worker.flush().unwrap();
        assert!(worker.pending.is_empty());
        assert_ne!(worker.status().state, TrackerState::Error);
    }

    #[test]
    fn shutdown_is_bounded_and_idempotent() {
        let directory = tempdir().unwrap();
        let handle = CollectorHandle::start(directory.path().join("usage.sqlite3")).unwrap();
        assert!(handle.status().is_ok());
        assert!(handle.flush().is_ok());
        assert!(handle.shutdown().is_ok());
        assert!(handle.shutdown().is_ok());
    }
}
