use std::time::Duration;

use chrono::NaiveTime;

use crate::model::{ActivitySnapshot, Availability, PendingAggregates, TrackerState};

pub const IDLE_THRESHOLD: Duration = Duration::from_secs(5 * 60);
pub const MAX_ATTRIBUTABLE_GAP: Duration = Duration::from_secs(10);
const WALL_CLOCK_TOLERANCE_MS: i64 = 1_000;

#[derive(Debug)]
pub struct AccountingState {
    previous: Option<ActivitySnapshot>,
    tracker_state: TrackerState,
}

impl Default for AccountingState {
    fn default() -> Self {
        Self {
            previous: None,
            tracker_state: TrackerState::Unavailable,
        }
    }
}

impl AccountingState {
    pub fn tracker_state(&self) -> TrackerState {
        self.tracker_state
    }

    pub fn observe(&mut self, sample: ActivitySnapshot, pending: &mut PendingAggregates) {
        self.tracker_state = state_for(&sample);

        if let Some(previous) = self.previous.as_ref() {
            self.attribute_interval(previous, &sample, pending);
        }

        self.previous = Some(sample);
    }

    fn attribute_interval(
        &self,
        previous: &ActivitySnapshot,
        current: &ActivitySnapshot,
        pending: &mut PendingAggregates,
    ) {
        let Some(elapsed) = current.monotonic.checked_sub(previous.monotonic) else {
            return;
        };
        if elapsed.is_zero() || elapsed > MAX_ATTRIBUTABLE_GAP {
            return;
        }
        if state_for(previous) != TrackerState::Recording
            || state_for(current) != TrackerState::Recording
        {
            return;
        }

        let (Some(previous_app), Some(current_app)) = (&previous.application, &current.application)
        else {
            return;
        };
        if previous_app.identity_key != current_app.identity_key {
            return;
        }

        let elapsed_ms = duration_ms(elapsed);
        for (date, active_ms) in allocate_local_days(previous, current, elapsed_ms) {
            pending.add(date, current_app, active_ms, current.observed_utc);
        }
    }
}

fn state_for(sample: &ActivitySnapshot) -> TrackerState {
    if sample.availability != Availability::Available {
        return TrackerState::Unavailable;
    }
    if sample.idle_for >= IDLE_THRESHOLD {
        return TrackerState::Idle;
    }
    if sample.is_self_process || sample.application.is_none() {
        return TrackerState::Unavailable;
    }
    TrackerState::Recording
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn allocate_local_days(
    previous: &ActivitySnapshot,
    current: &ActivitySnapshot,
    elapsed_ms: u64,
) -> Vec<(chrono::NaiveDate, u64)> {
    let previous_date = previous.local_time.date_naive();
    let current_date = current.local_time.date_naive();
    let wall_ms = current
        .local_time
        .signed_duration_since(previous.local_time)
        .num_milliseconds();
    let elapsed_i64 = i64::try_from(elapsed_ms).unwrap_or(i64::MAX);
    let clock_is_continuous = previous.local_time.offset() == current.local_time.offset()
        && wall_ms > 0
        && wall_ms.abs_diff(elapsed_i64) <= WALL_CLOCK_TOLERANCE_MS as u64;

    if !clock_is_continuous {
        return vec![(current_date, elapsed_ms)];
    }
    if previous_date == current_date {
        return vec![(current_date, elapsed_ms)];
    }

    let Some(next_date) = previous_date.succ_opt() else {
        return vec![(current_date, elapsed_ms)];
    };
    if current_date != next_date {
        return vec![(current_date, elapsed_ms)];
    }

    let midnight = next_date.and_time(NaiveTime::MIN);
    let previous_naive = previous.local_time.naive_local();
    let before_midnight_ms = midnight
        .signed_duration_since(previous_naive)
        .num_milliseconds()
        .clamp(0, elapsed_i64) as u64;
    let after_midnight_ms = elapsed_ms.saturating_sub(before_midnight_ms);

    let mut allocations = Vec::with_capacity(2);
    if before_midnight_ms > 0 {
        allocations.push((previous_date, before_midnight_ms));
    }
    if after_midnight_ms > 0 {
        allocations.push((current_date, after_midnight_ms));
    }
    allocations
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;
    use crate::model::{ApplicationIdentity, PendingAggregates};

    fn app(name: &str) -> ApplicationIdentity {
        ApplicationIdentity {
            identity_key: format!("c:\\\\apps\\\\{name}.exe"),
            executable_path: format!("C:\\\\Apps\\\\{name}.exe"),
            executable_name: format!("{name}.exe"),
            display_name: name.into(),
        }
    }

    fn snapshot(
        monotonic_ms: u64,
        local_time: &str,
        application: Option<ApplicationIdentity>,
    ) -> ActivitySnapshot {
        let local_time = DateTime::parse_from_rfc3339(local_time).unwrap();
        ActivitySnapshot {
            monotonic: Duration::from_millis(monotonic_ms),
            observed_utc: local_time.with_timezone(&Utc),
            local_time,
            idle_for: Duration::ZERO,
            availability: Availability::Available,
            application,
            is_self_process: false,
        }
    }

    fn date(value: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").unwrap()
    }

    fn total_for(pending: &PendingAggregates, value: &str) -> u64 {
        pending
            .for_date(date(value))
            .map(|usage| usage.active_ms)
            .sum()
    }

    #[test]
    fn table_driven_attribution_assigns_each_interval_at_most_once() {
        struct Case {
            name: &'static str,
            first_app: Option<ApplicationIdentity>,
            second_app: Option<ApplicationIdentity>,
            self_process: bool,
            expected_ms: u64,
        }

        let cases = [
            Case {
                name: "stable active application",
                first_app: Some(app("editor")),
                second_app: Some(app("editor")),
                self_process: false,
                expected_ms: 2_000,
            },
            Case {
                name: "application switch is uncertain",
                first_app: Some(app("editor")),
                second_app: Some(app("browser")),
                self_process: false,
                expected_ms: 0,
            },
            Case {
                name: "unknown process",
                first_app: None,
                second_app: None,
                self_process: false,
                expected_ms: 0,
            },
            Case {
                name: "self process",
                first_app: Some(app("tracker")),
                second_app: Some(app("tracker")),
                self_process: true,
                expected_ms: 0,
            },
        ];

        for case in cases {
            let mut accounting = AccountingState::default();
            let mut pending = PendingAggregates::default();
            let mut first = snapshot(0, "2026-08-14T10:00:00+08:00", case.first_app);
            let mut second = snapshot(2_000, "2026-08-14T10:00:02+08:00", case.second_app);
            first.is_self_process = case.self_process;
            second.is_self_process = case.self_process;

            accounting.observe(first, &mut pending);
            accounting.observe(second.clone(), &mut pending);
            accounting.observe(second, &mut pending);

            assert_eq!(pending.total_ms(), case.expected_ms, "{}", case.name);
        }
    }

    #[test]
    fn idle_lock_and_resume_never_backfill_excluded_time() {
        let mut accounting = AccountingState::default();
        let mut pending = PendingAggregates::default();
        let editor = app("editor");

        accounting.observe(
            snapshot(0, "2026-08-14T10:00:00+08:00", Some(editor.clone())),
            &mut pending,
        );
        accounting.observe(
            snapshot(2_000, "2026-08-14T10:00:02+08:00", Some(editor.clone())),
            &mut pending,
        );

        let mut idle = snapshot(4_000, "2026-08-14T10:00:04+08:00", Some(editor.clone()));
        idle.idle_for = IDLE_THRESHOLD;
        accounting.observe(idle, &mut pending);
        assert_eq!(accounting.tracker_state(), TrackerState::Idle);

        let resumed = snapshot(6_000, "2026-08-14T10:00:06+08:00", Some(editor.clone()));
        accounting.observe(resumed, &mut pending);
        accounting.observe(
            snapshot(8_000, "2026-08-14T10:00:08+08:00", Some(editor.clone())),
            &mut pending,
        );

        let mut locked = snapshot(10_000, "2026-08-14T10:00:10+08:00", Some(editor.clone()));
        locked.availability = Availability::LockedOrSecureDesktop;
        accounting.observe(locked, &mut pending);

        assert_eq!(pending.total_ms(), 4_000);
        assert_eq!(accounting.tracker_state(), TrackerState::Unavailable);
    }

    #[test]
    fn normal_midnight_is_split_between_local_dates() {
        let mut accounting = AccountingState::default();
        let mut pending = PendingAggregates::default();
        let editor = app("editor");

        accounting.observe(
            snapshot(0, "2026-08-14T23:59:59+08:00", Some(editor.clone())),
            &mut pending,
        );
        accounting.observe(
            snapshot(2_000, "2026-08-15T00:00:01+08:00", Some(editor)),
            &mut pending,
        );

        assert_eq!(total_for(&pending, "2026-08-14"), 1_000);
        assert_eq!(total_for(&pending, "2026-08-15"), 1_000);
    }

    #[test]
    fn wall_clock_or_timezone_jump_keeps_monotonic_duration_on_new_date() {
        let cases = [
            ("2026-08-14T23:00:00+08:00", "2026-08-15T08:00:02+08:00"),
            ("2026-08-14T23:00:00+08:00", "2026-08-15T00:00:02+09:00"),
        ];

        for (before, after) in cases {
            let mut accounting = AccountingState::default();
            let mut pending = PendingAggregates::default();
            let editor = app("editor");
            accounting.observe(snapshot(0, before, Some(editor.clone())), &mut pending);
            accounting.observe(snapshot(2_000, after, Some(editor)), &mut pending);

            assert_eq!(pending.total_ms(), 2_000);
            assert_eq!(total_for(&pending, "2026-08-14"), 0);
            assert_eq!(total_for(&pending, "2026-08-15"), 2_000);
        }
    }

    #[test]
    fn gaps_over_ten_seconds_and_backwards_monotonic_time_are_rejected() {
        let editor = app("editor");
        for second_ms in [10_002, 0] {
            let mut accounting = AccountingState::default();
            let mut pending = PendingAggregates::default();
            accounting.observe(
                snapshot(1, "2026-08-14T10:00:00+08:00", Some(editor.clone())),
                &mut pending,
            );
            accounting.observe(
                snapshot(second_ms, "2026-08-14T10:00:20+08:00", Some(editor.clone())),
                &mut pending,
            );
            assert_eq!(pending.total_ms(), 0);
        }
    }
}
