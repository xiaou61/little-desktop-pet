use std::{collections::HashMap, time::Duration};

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationIdentity {
    pub identity_key: String,
    pub executable_path: String,
    pub executable_name: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Available,
    LockedOrSecureDesktop,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivitySnapshot {
    pub monotonic: Duration,
    pub observed_utc: DateTime<Utc>,
    pub local_time: DateTime<FixedOffset>,
    pub idle_for: Duration,
    pub availability: Availability,
    pub application: Option<ApplicationIdentity>,
    pub is_self_process: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackerState {
    Recording,
    Idle,
    Unavailable,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingUsage {
    pub application: ApplicationIdentity,
    pub active_ms: u64,
    pub observed_utc: DateTime<Utc>,
}

#[derive(Debug, Default)]
pub struct PendingAggregates {
    entries: HashMap<(NaiveDate, String), PendingUsage>,
}

impl PendingAggregates {
    pub fn add(
        &mut self,
        date: NaiveDate,
        application: &ApplicationIdentity,
        active_ms: u64,
        observed_utc: DateTime<Utc>,
    ) {
        if active_ms == 0 {
            return;
        }

        let entry = self
            .entries
            .entry((date, application.identity_key.clone()))
            .or_insert_with(|| PendingUsage {
                application: application.clone(),
                active_ms: 0,
                observed_utc,
            });
        entry.active_ms = entry.active_ms.saturating_add(active_ms);
        entry.application = application.clone();
        entry.observed_utc = observed_utc;
    }

    pub fn iter(&self) -> impl Iterator<Item = (&(NaiveDate, String), &PendingUsage)> {
        self.entries.iter()
    }

    pub fn for_date(&self, date: NaiveDate) -> impl Iterator<Item = &PendingUsage> {
        self.entries
            .iter()
            .filter_map(move |((entry_date, _), usage)| (*entry_date == date).then_some(usage))
    }

    #[cfg(test)]
    pub fn total_ms(&self) -> u64 {
        self.entries.values().map(|usage| usage.active_ms).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyApplicationUsage {
    pub display_name: String,
    pub executable_name: String,
    pub active_ms: u64,
    pub share: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsageSummary {
    pub date: String,
    pub tracker_state: TrackerState,
    pub total_active_ms: u64,
    pub applications: Vec<DailyApplicationUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerStatus {
    pub state: TrackerState,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

impl CommandError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_summary_serializes_to_the_frontend_contract_without_paths() {
        let summary = DailyUsageSummary {
            date: "2026-08-14".into(),
            tracker_state: TrackerState::Recording,
            total_active_ms: 90_000,
            applications: vec![DailyApplicationUsage {
                display_name: "编辑器".into(),
                executable_name: "editor.exe".into(),
                active_ms: 90_000,
                share: 1.0,
            }],
        };

        let value = serde_json::to_value(summary).unwrap();
        assert_eq!(value["trackerState"], "recording");
        assert_eq!(value["totalActiveMs"], 90_000);
        assert_eq!(value["applications"][0]["executableName"], "editor.exe");
        assert!(value.get("executablePath").is_none());
        assert!(!value.to_string().contains("C:\\\\Users"));
    }

    #[test]
    fn tracker_status_uses_bounded_public_fields() {
        let status = TrackerStatus {
            state: TrackerState::Error,
            message: Some("本地存储暂时不可用".into()),
        };

        let value = serde_json::to_value(status).unwrap();
        assert_eq!(value["state"], "error");
        assert_eq!(value.as_object().unwrap().len(), 2);
    }
}
