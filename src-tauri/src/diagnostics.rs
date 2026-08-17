//! Local, structured diagnostics.
//!
//! This module deliberately owns the event format, filtering, persistence and
//! export rules. Callers should record an event here instead of writing to a
//! log file directly. The public data types are also the Rust side of the
//! diagnostics-center TypeScript contract.

use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

static GLOBAL_MANAGER: std::sync::OnceLock<DiagnosticsManager> = std::sync::OnceLock::new();
static CORRELATION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub fn new_correlation_id() -> String {
    let sequence = CORRELATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("diag-{}-{sequence}", Utc::now().timestamp_millis())
}

pub const EVENT_RING_CAPACITY: usize = 500;
pub const LOG_FILE_LIMIT_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_ARCHIVED_LOG_FILES: usize = 5;
pub const MAX_TOTAL_LOG_BYTES: u64 = 10 * 1024 * 1024;
const WRITER_QUEUE_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl DiagnosticLevel {
    pub fn allows(self, event_level: Self) -> bool {
        event_level >= self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsConfig {
    pub developer_mode: bool,
    pub level: DiagnosticLevel,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEvent {
    pub timestamp: String,
    pub level: DiagnosticLevel,
    pub module: String,
    pub event: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub context: BTreeMap<String, Value>,
}

impl DiagnosticEvent {
    pub fn new(
        level: DiagnosticLevel,
        module: impl Into<String>,
        event: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            level,
            module: module.into(),
            event: event.into(),
            message: message.into(),
            error_code: None,
            window_label: None,
            plugin_id: None,
            correlation_id: None,
            duration_ms: None,
            context: BTreeMap::new(),
        }
    }

    pub fn sanitized(mut self) -> Self {
        if chrono::DateTime::parse_from_rfc3339(&self.timestamp).is_err() {
            self.timestamp = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        }
        self.module = bounded(&self.module, 80);
        self.event = bounded(&self.event, 120);
        self.message = redact_text(&bounded(&self.message, 2_000));
        self.error_code = self
            .error_code
            .map(|value| bounded(&redact_text(&value), 80));
        self.window_label = self
            .window_label
            .map(|value| bounded(&redact_text(&value), 80));
        self.plugin_id = self
            .plugin_id
            .map(|value| bounded(&redact_text(&value), 96));
        self.correlation_id = self
            .correlation_id
            .map(|value| bounded(&redact_text(&value), 96));
        self.context = sanitize_context(self.context);
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiagnosticQuery {
    pub level: Option<DiagnosticLevel>,
    pub module: Option<String>,
    pub window_label: Option<String>,
    pub plugin_id: Option<String>,
    pub correlation_id: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticPage {
    pub events: Vec<DiagnosticEvent>,
    pub total: usize,
    pub dropped_events: u64,
    pub persistence_degraded: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentSnapshot {
    pub available: bool,
    pub state: Option<Value>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshot {
    pub app_version: String,
    pub build_mode: String,
    pub developer_mode: bool,
    pub pet: ComponentSnapshot,
    pub quick_panel: ComponentSnapshot,
    pub collector: ComponentSnapshot,
    pub plugins: ComponentSnapshot,
    pub webview_labels: Vec<String>,
    pub persistence_degraded: bool,
    pub dropped_events: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastCrash {
    pub timestamp: String,
    pub source: String,
    pub message: String,
    pub backtrace: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub path: String,
    pub files: Vec<String>,
    pub last_crash_included: bool,
}

#[derive(Clone, Debug)]
pub struct EventBuilder {
    event: DiagnosticEvent,
}

impl EventBuilder {
    pub fn new(
        level: DiagnosticLevel,
        module: impl Into<String>,
        event: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            event: DiagnosticEvent::new(level, module, event, message),
        }
    }

    pub fn error_code(mut self, value: impl Into<String>) -> Self {
        self.event.error_code = Some(value.into());
        self
    }

    pub fn window(mut self, value: impl Into<String>) -> Self {
        self.event.window_label = Some(value.into());
        self
    }

    pub fn plugin(mut self, value: impl Into<String>) -> Self {
        self.event.plugin_id = Some(value.into());
        self
    }

    pub fn correlation(mut self, value: impl Into<String>) -> Self {
        self.event.correlation_id = Some(value.into());
        self
    }

    pub fn duration_ms(mut self, value: u64) -> Self {
        self.event.duration_ms = Some(value);
        self
    }

    pub fn context(mut self, value: impl IntoIterator<Item = (String, Value)>) -> Self {
        self.event.context = value.into_iter().collect();
        self
    }

    pub fn build(self) -> DiagnosticEvent {
        self.event
    }
}

pub struct EventRing {
    capacity: usize,
    events: VecDeque<DiagnosticEvent>,
}

impl EventRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            events: VecDeque::with_capacity(capacity.max(1)),
        }
    }

    pub fn push(&mut self, event: DiagnosticEvent) {
        if self.events.len() == self.capacity {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    pub fn query(&self, query: &DiagnosticQuery) -> Vec<DiagnosticEvent> {
        self.events
            .iter()
            .rev()
            .filter(|event| matches_query(event, query))
            .cloned()
            .collect()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }
}

type Provider = Arc<dyn Fn() -> Result<Value, String> + Send + Sync>;

struct ManagerState {
    ring: EventRing,
    config: DiagnosticsConfig,
    snapshot: RuntimeSnapshot,
    providers: Vec<(String, Provider)>,
}

enum WriterCommand {
    Event(DiagnosticEvent),
    Flush(SyncSender<()>),
    Shutdown(SyncSender<()>),
}

struct ManagerInner {
    root: PathBuf,
    state: Mutex<ManagerState>,
    sender: SyncSender<WriterCommand>,
    writer: Mutex<Option<thread::JoinHandle<()>>>,
    dropped_events: AtomicU64,
    persistence_degraded: Arc<AtomicBool>,
    shutdown_started: AtomicBool,
    previous_session_running: bool,
}

#[derive(Clone)]
pub struct DiagnosticsManager {
    inner: Arc<ManagerInner>,
}

impl DiagnosticsManager {
    pub fn new(root: PathBuf) -> io::Result<Self> {
        let storage_available = fs::create_dir_all(root.join("logs")).is_ok();
        let config = load_json(&root.join("diagnostics-settings.json")).unwrap_or_default();
        let previous_session_running = load_json::<SessionState>(&root.join("session-state.json"))
            .is_some_and(|state| state.running);
        let (sender, receiver) = mpsc::sync_channel(WRITER_QUEUE_CAPACITY);
        let writer_root = root.clone();
        let degraded = Arc::new(AtomicBool::new(false));
        degraded.store(!storage_available, Ordering::Release);
        let writer_degraded = degraded.clone();
        let writer = thread::Builder::new()
            .name("diagnostics-log-writer".into())
            .spawn(move || writer_loop(receiver, writer_root, writer_degraded))?;
        Ok(Self {
            inner: Arc::new(ManagerInner {
                root,
                state: Mutex::new(ManagerState {
                    ring: EventRing::new(EVENT_RING_CAPACITY),
                    config,
                    snapshot: RuntimeSnapshot {
                        app_version: env!("CARGO_PKG_VERSION").into(),
                        build_mode: if cfg!(debug_assertions) {
                            "debug".into()
                        } else {
                            "release".into()
                        },
                        ..RuntimeSnapshot::default()
                    },
                    providers: Vec::new(),
                }),
                sender,
                writer: Mutex::new(Some(writer)),
                dropped_events: AtomicU64::new(0),
                persistence_degraded: degraded,
                shutdown_started: AtomicBool::new(false),
                previous_session_running,
            }),
        })
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    pub fn install_global(&self) {
        let _ = GLOBAL_MANAGER.set(self.clone());
    }

    pub fn event(&self, builder: EventBuilder) {
        self.record(builder.build());
    }

    pub fn record(&self, event: DiagnosticEvent) {
        let event = event.sanitized();
        let enabled = {
            let mut state = self.lock_state();
            let enabled = state.config.level.allows(event.level);
            if enabled {
                state.ring.push(event.clone());
            }
            enabled
        };
        if !enabled {
            return;
        }
        match self.inner.sender.try_send(WriterCommand::Event(event)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.inner.dropped_events.fetch_add(1, Ordering::Relaxed);
                self.inner
                    .persistence_degraded
                    .store(true, Ordering::Release);
            }
            Err(TrySendError::Disconnected(_)) => {
                self.inner
                    .persistence_degraded
                    .store(true, Ordering::Release);
            }
        }
    }

    pub fn recent(&self, query: DiagnosticQuery) -> DiagnosticPage {
        let state = self.lock_state();
        let mut events = state.ring.query(&query);
        let total = events.len();
        let offset = query.offset.min(events.len());
        events.drain(..offset);
        if query.limit > 0 {
            events.truncate(query.limit);
        }
        DiagnosticPage {
            events,
            total,
            dropped_events: self.inner.dropped_events.load(Ordering::Relaxed),
            persistence_degraded: self.persistence_degraded(),
        }
    }

    pub fn config(&self) -> DiagnosticsConfig {
        self.lock_state().config.clone()
    }

    pub fn set_config(&self, mut config: DiagnosticsConfig) -> io::Result<DiagnosticsConfig> {
        if !config.developer_mode {
            config.level = config.level.max(DiagnosticLevel::Info);
        }
        let bytes = serde_json::to_vec_pretty(&config).map_err(io::Error::other)?;
        atomic_write(&self.inner.root.join("diagnostics-settings.json"), &bytes)?;
        self.lock_state().config = config.clone();
        self.lock_state().snapshot.developer_mode = config.developer_mode;
        Ok(config)
    }

    pub fn persistence_degraded(&self) -> bool {
        self.inner.persistence_degraded.load(Ordering::Acquire)
    }

    pub fn flush(&self) {
        let (reply, receiver) = mpsc::sync_channel(1);
        if self.inner.sender.send(WriterCommand::Flush(reply)).is_ok() {
            let _ = receiver.recv();
        }
    }

    pub fn session_start(&self) {
        if self.inner.previous_session_running {
            self.record(
                EventBuilder::new(
                    DiagnosticLevel::Error,
                    "lifecycle",
                    "previous-session-aborted",
                    "上一次会话未正常结束。",
                )
                .error_code("previous_session_aborted")
                .build(),
            );
        }
        let marker = SessionState {
            running: true,
            started_at: Some(Utc::now().to_rfc3339()),
            ended_at: None,
        };
        let _ = write_json_atomic(&self.inner.root.join("session-state.json"), &marker);
    }

    pub fn session_end(&self) {
        let marker = SessionState {
            running: false,
            started_at: None,
            ended_at: Some(Utc::now().to_rfc3339()),
        };
        let _ = write_json_atomic(&self.inner.root.join("session-state.json"), &marker);
        self.flush();
    }

    pub fn last_crash(&self) -> Option<LastCrash> {
        load_json(&self.inner.root.join("last-crash.json")).map(LastCrash::sanitized)
    }

    pub fn public_summary(&self) -> String {
        self.summary_text(self.last_crash().as_ref())
    }

    pub fn record_panic(&self, source: &str, payload: &str, backtrace: Option<&str>) {
        let crash = LastCrash {
            timestamp: Utc::now().to_rfc3339(),
            source: source.into(),
            message: payload.into(),
            backtrace: backtrace.map(str::to_owned),
        }
        .sanitized();
        let _ = write_json_atomic(&self.inner.root.join("last-crash.json"), &crash);
        self.record(
            EventBuilder::new(
                DiagnosticLevel::Error,
                "panic",
                "panic-captured",
                &crash.message,
            )
            .error_code("panic")
            .context([
                ("source".into(), Value::String(crash.source.clone())),
                (
                    "hasBacktrace".into(),
                    Value::Bool(crash.backtrace.is_some()),
                ),
            ])
            .build(),
        );
    }

    pub fn record_native_panic(source: &str, payload: &str) {
        if let Some(manager) = GLOBAL_MANAGER.get() {
            manager.record_panic(source, payload, None);
        }
    }

    pub fn install_panic_hook(&self) {
        let manager = self.clone();
        std::panic::set_hook(Box::new(move |info| {
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                .unwrap_or("panic payload unavailable");
            let backtrace = std::backtrace::Backtrace::capture();
            let backtrace = (backtrace.status() == std::backtrace::BacktraceStatus::Captured)
                .then(|| backtrace.to_string());
            manager.record_panic("rust", payload, backtrace.as_deref());
        }));
    }

    pub fn set_snapshot(&self, mut snapshot: RuntimeSnapshot) {
        snapshot.developer_mode = self.config().developer_mode;
        snapshot.persistence_degraded = self.persistence_degraded();
        snapshot.dropped_events = self.inner.dropped_events.load(Ordering::Relaxed);
        self.lock_state().snapshot = snapshot;
    }

    pub fn register_provider<F>(&self, name: impl Into<String>, provider: F)
    where
        F: Fn() -> Result<Value, String> + Send + Sync + 'static,
    {
        self.lock_state()
            .providers
            .push((name.into(), Arc::new(provider)));
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        let (mut snapshot, providers) = {
            let state = self.lock_state();
            (state.snapshot.clone(), state.providers.clone())
        };
        for (name, provider) in providers {
            let target = match name.as_str() {
                "pet" => &mut snapshot.pet,
                "quickPanel" | "quick_panel" => &mut snapshot.quick_panel,
                "collector" => &mut snapshot.collector,
                "plugins" => &mut snapshot.plugins,
                _ => continue,
            };
            match provider() {
                Ok(value) => {
                    target.available = true;
                    target.state = Some(value);
                    target.error = None;
                }
                Err(error) => {
                    target.available = false;
                    target.error = Some(redact_text(&error));
                }
            }
        }
        snapshot.developer_mode = self.config().developer_mode;
        snapshot.persistence_degraded = self.persistence_degraded();
        snapshot.dropped_events = self.inner.dropped_events.load(Ordering::Relaxed);
        snapshot
    }

    pub fn export(&self, destination: &Path, environment: Value) -> io::Result<ExportResult> {
        self.flush();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp = self.inner.root.join(format!("diagnostics-export-{stamp}"));
        fs::create_dir_all(&temp)?;
        let result = (|| {
            let last_crash = self.last_crash();
            let files = ["summary.md", "logs.jsonl", "state.json", "environment.json"];
            let summary = self.summary_text(last_crash.as_ref());
            fs::write(temp.join("summary.md"), summary.as_bytes())?;
            fs::write(temp.join("logs.jsonl"), self.export_logs().as_bytes())?;
            let state =
                sanitize_json(serde_json::to_value(self.snapshot()).map_err(io::Error::other)?);
            fs::write(
                temp.join("state.json"),
                serde_json::to_vec_pretty(&state).map_err(io::Error::other)?,
            )?;
            let environment = sanitize_json(environment);
            fs::write(
                temp.join("environment.json"),
                serde_json::to_vec_pretty(&environment).map_err(io::Error::other)?,
            )?;
            let mut included = files
                .iter()
                .map(|value| (*value).to_string())
                .collect::<Vec<_>>();
            if let Some(ref crash) = last_crash {
                fs::write(temp.join("last-crash.txt"), crash.as_text().as_bytes())?;
                included.push("last-crash.txt".into());
            }

            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            let output = File::create(destination)?;
            let mut zip = zip::ZipWriter::new(output);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for name in &included {
                zip.start_file(name, options).map_err(io::Error::other)?;
                let mut file = File::open(temp.join(name))?;
                io::copy(&mut file, &mut zip).map_err(io::Error::other)?;
            }
            zip.finish().map_err(io::Error::other)?;
            Ok(ExportResult {
                path: destination.to_string_lossy().into_owned(),
                files: included,
                last_crash_included: last_crash.is_some(),
            })
        })();
        let _ = fs::remove_dir_all(&temp);
        result
    }

    fn export_logs(&self) -> String {
        let mut output = String::new();
        for path in log_files(&self.inner.root.join("logs")) {
            if let Ok(mut file) = File::open(path) {
                let mut text = String::new();
                if file.read_to_string(&mut text).is_ok() {
                    for line in text.lines() {
                        output.push_str(&redact_text(line));
                        output.push('\n');
                    }
                }
            }
        }
        output
    }

    fn summary_text(&self, last_crash: Option<&LastCrash>) -> String {
        let page = self.recent(DiagnosticQuery {
            limit: 30,
            ..DiagnosticQuery::default()
        });
        let mut summary = String::from("# 小桌宠诊断摘要\n\n");
        summary.push_str(&format!("- 事件数：{}\n", page.total));
        summary.push_str(&format!("- 持久化降级：{}\n", page.persistence_degraded));
        summary.push_str(&format!("- 丢弃事件：{}\n", page.dropped_events));
        summary.push_str(&format!(
            "- 最近异常：{}\n\n",
            if last_crash.is_some() {
                "有"
            } else {
                "不存在"
            }
        ));
        summary.push_str("## 最近事件\n\n");
        for event in page.events {
            summary.push_str(&format!(
                "- `{}` `{}` `{}`：{}\n",
                event.timestamp, event.level as u8, event.module, event.message
            ));
        }
        summary
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, ManagerState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Drop for DiagnosticsManager {
    fn drop(&mut self) {
        if !Arc::strong_count(&self.inner).eq(&1) {
            return;
        }
        if self.inner.shutdown_started.swap(true, Ordering::AcqRel) {
            return;
        }
        self.session_end();
        let (reply, receiver) = mpsc::sync_channel(1);
        if self
            .inner
            .sender
            .send(WriterCommand::Shutdown(reply))
            .is_ok()
        {
            let _ = receiver.recv();
        }
        if let Some(join) = self
            .inner
            .writer
            .lock()
            .ok()
            .and_then(|mut writer| writer.take())
        {
            let _ = join.join();
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionState {
    running: bool,
    started_at: Option<String>,
    ended_at: Option<String>,
}

impl LastCrash {
    fn sanitized(mut self) -> Self {
        self.source = bounded(&redact_text(&self.source), 80);
        self.message = bounded(&redact_text(&self.message), 2_000);
        self.backtrace = self
            .backtrace
            .map(|value| bounded(&redact_text(&value), 4_000));
        self
    }

    fn as_text(&self) -> String {
        format!(
            "source: {}\ntime: {}\nmessage: {}\nbacktrace: {}\n",
            self.source,
            self.timestamp,
            self.message,
            self.backtrace.as_deref().unwrap_or("unavailable")
        )
    }
}

fn writer_loop(receiver: Receiver<WriterCommand>, root: PathBuf, degraded: Arc<AtomicBool>) {
    while let Ok(command) = receiver.recv() {
        match command {
            WriterCommand::Event(event) => {
                if append_event(&root, &event).is_err() {
                    degraded.store(true, Ordering::Release);
                }
            }
            WriterCommand::Flush(reply) => {
                let _ = reply.send(());
            }
            WriterCommand::Shutdown(reply) => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

fn append_event(root: &Path, event: &DiagnosticEvent) -> io::Result<()> {
    let path = root.join("logs").join("diagnostics.log");
    fs::create_dir_all(path.parent().unwrap_or(root))?;
    let line = serde_json::to_string(event).map_err(io::Error::other)? + "\n";
    rotate_if_needed(&path, line.len() as u64)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    file.flush()?;
    trim_log_total(root.join("logs").as_path())
}

fn rotate_if_needed(path: &Path, incoming: u64) -> io::Result<()> {
    let current = fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if current.saturating_add(incoming) <= LOG_FILE_LIMIT_BYTES {
        return Ok(());
    }
    for index in (1..=MAX_ARCHIVED_LOG_FILES).rev() {
        let source = path.with_file_name(format!("diagnostics.log.{index}"));
        let target = path.with_file_name(format!("diagnostics.log.{}", index + 1));
        if index == MAX_ARCHIVED_LOG_FILES {
            let _ = fs::remove_file(&source);
        } else if source.exists() {
            let _ = fs::rename(&source, &target);
        }
    }
    if path.exists() {
        fs::rename(path, path.with_file_name("diagnostics.log.1"))?;
    }
    trim_log_total(path.parent().unwrap_or(Path::new(".")))
}

fn trim_log_total(directory: &Path) -> io::Result<()> {
    let mut files = log_files(directory);
    files.sort_by_key(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("diagnostics.log."))
            .and_then(|index| index.parse::<usize>().ok())
            .unwrap_or(0)
    });
    let mut total = files
        .iter()
        .filter_map(|path| fs::metadata(path).ok().map(|metadata| metadata.len()))
        .sum::<u64>();
    while total > MAX_TOTAL_LOG_BYTES {
        let Some(path) = files.pop() else { break };
        let size = fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let _ = fs::remove_file(&path);
        total = total.saturating_sub(size);
    }
    Ok(())
}

fn log_files(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name == "diagnostics.log" || name.starts_with("diagnostics.log.")
                })
        })
        .collect()
}

fn matches_query(event: &DiagnosticEvent, query: &DiagnosticQuery) -> bool {
    query.level.is_none_or(|level| level.allows(event.level))
        && query
            .module
            .as_deref()
            .is_none_or(|value| event.module == value)
        && query
            .window_label
            .as_deref()
            .is_none_or(|value| event.window_label.as_deref() == Some(value))
        && query
            .plugin_id
            .as_deref()
            .is_none_or(|value| event.plugin_id.as_deref() == Some(value))
        && query
            .correlation_id
            .as_deref()
            .is_none_or(|value| event.correlation_id.as_deref() == Some(value))
        && query
            .from
            .as_deref()
            .is_none_or(|value| event.timestamp.as_str() >= value)
        && query
            .to
            .as_deref()
            .is_none_or(|value| event.timestamp.as_str() <= value)
}

fn sanitize_context(context: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    const ALLOWED: &[&str] = &[
        "errorCode",
        "pluginId",
        "pluginVersion",
        "windowLabel",
        "status",
        "durationMs",
        "correlationId",
        "source",
        "hasBacktrace",
        "attempt",
        "count",
    ];
    context
        .into_iter()
        .filter_map(|(key, value)| {
            ALLOWED.contains(&key.as_str()).then_some((
                key,
                match value {
                    Value::String(value) => Value::String(redact_text(&bounded(&value, 240))),
                    Value::Number(_) | Value::Bool(_) | Value::Null => value,
                    _ => Value::String("<summary>".into()),
                },
            ))
        })
        .collect()
}

pub fn sanitize_json(value: Value) -> Value {
    match value {
        Value::String(value) => Value::String(redact_text(&value)),
        Value::Array(values) => Value::Array(values.into_iter().map(sanitize_json).collect()),
        Value::Object(values) => {
            let map = values
                .into_iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "windowTitle"
                            | "documentName"
                            | "keyboard"
                            | "keyboardContent"
                            | "screenshot"
                            | "database"
                            | "usageDatabase"
                            | "absolutePath"
                            | "userName"
                            | "machineName"
                    )
                })
                .map(|(key, value)| (key, sanitize_json(value)))
                .collect::<Map<String, Value>>();
            Value::Object(map)
        }
        value => value,
    }
}

pub fn redact_text(input: &str) -> String {
    let mut value = input.to_string();
    for key in [
        "USERNAME",
        "USER",
        "USERPROFILE",
        "HOME",
        "COMPUTERNAME",
        "HOSTNAME",
    ] {
        if let Ok(secret) = std::env::var(key)
            && !secret.is_empty()
        {
            value = replace_case_insensitive(
                &value,
                &secret,
                if key.contains("COMPUTER") || key == "HOSTNAME" {
                    "<machine>"
                } else {
                    "<user>"
                },
            );
        }
    }
    value = redact_user_paths(&value);
    value = redact_absolute_paths(&value);
    redact_credentials(&value)
}

fn redact_user_paths(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    while index < bytes.len() {
        if index + 9 < bytes.len()
            && bytes[index].is_ascii_alphabetic()
            && bytes[index + 1] == b':'
            && (bytes[index + 2] == b'\\' || bytes[index + 2] == b'/')
            && bytes[index + 3..]
                .to_ascii_lowercase()
                .starts_with(b"users")
        {
            let mut end = index + 3;
            while end < bytes.len()
                && !bytes[end].is_ascii_whitespace()
                && !b",;)]}".contains(&bytes[end])
            {
                end += 1;
            }
            output.push_str("<user>");
            index = end;
        } else if index + 7 < bytes.len()
            && bytes[index] == b'/'
            && bytes[index + 1..]
                .to_ascii_lowercase()
                .starts_with(b"users/")
        {
            let mut end = index + 2;
            while end < bytes.len()
                && !bytes[end].is_ascii_whitespace()
                && !b",;)]}".contains(&bytes[end])
            {
                end += 1;
            }
            output.push_str("<user>");
            index = end;
        } else {
            let character = input[index..].chars().next().unwrap();
            output.push(character);
            index += character.len_utf8();
        }
    }
    output
}

fn redact_absolute_paths(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    while index < bytes.len() {
        let drive_path = index + 2 < bytes.len()
            && bytes[index].is_ascii_alphabetic()
            && bytes[index + 1] == b':'
            && (bytes[index + 2] == b'\\' || bytes[index + 2] == b'/');
        if drive_path {
            let mut end = index + 3;
            while end < bytes.len()
                && !bytes[end].is_ascii_whitespace()
                && !b",;)]}".contains(&bytes[end])
            {
                end += 1;
            }
            output.push_str("<path>");
            index = end;
        } else {
            let character = input[index..].chars().next().unwrap();
            output.push(character);
            index += character.len_utf8();
        }
    }
    output
}

fn redact_credentials(input: &str) -> String {
    let mut output = input.to_string();
    for key in ["password", "passwd", "token", "secret", "api_key", "apikey"] {
        let mut offset = 0;
        loop {
            let lower = output.to_ascii_lowercase();
            let Some(found) = lower[offset..].find(key) else {
                break;
            };
            let start = offset + found;
            let after = start + key.len();
            let separator = output[after..]
                .char_indices()
                .take_while(|(_, character)| {
                    character.is_ascii_whitespace() || matches!(character, '=' | ':')
                })
                .find(|(_, character)| matches!(character, '=' | ':'))
                .map(|(value, _)| after + value + 1);
            if let Some(value_start) = separator {
                let value_start = value_start
                    + output[value_start..]
                        .chars()
                        .take_while(|character| character.is_ascii_whitespace())
                        .map(char::len_utf8)
                        .sum::<usize>();
                let value_end = output[value_start..]
                    .find(|character: char| {
                        character.is_whitespace()
                            || matches!(character, ',' | ';' | '&' | ')' | ']')
                    })
                    .map(|value| value_start + value)
                    .unwrap_or(output.len());
                output.replace_range(value_start..value_end, "<redacted>");
                offset = value_start + "<redacted>".len();
            } else {
                offset = after;
            }
        }
    }
    output
}

fn replace_case_insensitive(input: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return input.to_string();
    }
    let lower_input = input.to_ascii_lowercase();
    let lower_needle = needle.to_ascii_lowercase();
    let mut output = String::new();
    let mut offset = 0;
    while let Some(found) = lower_input[offset..].find(&lower_needle) {
        let start = offset + found;
        output.push_str(&input[offset..start]);
        output.push_str(replacement);
        offset = start + needle.len();
    }
    output.push_str(&input[offset..]);
    output
}

fn bounded(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    atomic_write(path, &bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    {
        let mut file = File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn event(index: usize) -> DiagnosticEvent {
        EventBuilder::new(
            DiagnosticLevel::Info,
            "test",
            format!("event-{index}"),
            "message",
        )
        .correlation("corr-1")
        .build()
    }

    #[test]
    fn redaction_uses_stable_placeholders_and_removes_credentials() {
        let input = r"C:\Users\Alice\Documents\note.txt host=secret password=topsecret";
        let output = redact_text(input);
        assert!(!output.contains("Alice"));
        assert!(!output.contains("topsecret"));
        assert!(output.contains("<user>") || output.contains("<path>"));
        assert!(output.contains("<redacted>"));
    }

    #[test]
    fn redaction_preserves_non_path_unicode_text() {
        let output = redact_text("快捷面板打开失败：请稍后重试 C:\\Temp\\details.log");
        assert_eq!(output, "快捷面板打开失败：请稍后重试 <path>");
    }

    #[test]
    fn context_allowlist_drops_arbitrary_objects() {
        let context = BTreeMap::from([
            ("windowTitle".into(), Value::String("secret".into())),
            ("status".into(), Value::String("idle".into())),
            ("nested".into(), serde_json::json!({"path": "C:\\secret"})),
        ]);
        let sanitized = sanitize_context(context);
        assert!(!sanitized.contains_key("windowTitle"));
        assert!(!sanitized.contains_key("nested"));
        assert_eq!(sanitized["status"], "idle");
    }

    #[test]
    fn ring_evicts_oldest_and_filters_empty_and_correlation_queries() {
        let mut ring = EventRing::new(2);
        ring.push(event(1));
        ring.push(event(2));
        ring.push(EventBuilder::new(DiagnosticLevel::Error, "other", "third", "error").build());
        assert_eq!(ring.len(), 2);
        assert_eq!(ring.query(&DiagnosticQuery::default()).len(), 2);
        assert_eq!(
            ring.query(&DiagnosticQuery {
                correlation_id: Some("corr-1".into()),
                ..DiagnosticQuery::default()
            })
            .len(),
            1
        );
        assert!(
            ring.query(&DiagnosticQuery {
                module: Some("other".into()),
                ..DiagnosticQuery::default()
            })
            .len()
                == 1
        );
    }

    #[test]
    fn diagnostic_query_accepts_partial_frontend_payloads() {
        let query: DiagnosticQuery = serde_json::from_value(serde_json::json!({
            "limit": 500
        }))
        .unwrap();

        assert_eq!(query.offset, 0);
        assert_eq!(query.limit, 500);
        assert!(query.level.is_none());
        assert!(query.module.is_none());
    }

    #[test]
    fn manager_persists_events_and_recovers_corrupt_settings() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("diagnostics-settings.json"),
            b"not json",
        )
        .unwrap();
        let manager = DiagnosticsManager::new(directory.path().to_path_buf()).unwrap();
        manager.record(event(1));
        manager.flush();
        assert!(directory.path().join("logs/diagnostics.log").exists());
        assert_eq!(manager.recent(DiagnosticQuery::default()).total, 1);
    }

    #[test]
    fn previous_running_session_is_reported_on_start() {
        let directory = tempdir().unwrap();
        write_json_atomic(
            &directory.path().join("session-state.json"),
            &SessionState {
                running: true,
                ..SessionState::default()
            },
        )
        .unwrap();
        let manager = DiagnosticsManager::new(directory.path().to_path_buf()).unwrap();
        manager.session_start();
        assert_eq!(manager.recent(DiagnosticQuery::default()).total, 1);
        manager.record(EventBuilder::new(DiagnosticLevel::Info, "test", "ready", "ok").build());
        manager.flush();
    }

    #[test]
    fn normal_session_end_does_not_report_an_abnormal_restart() {
        let directory = tempdir().unwrap();
        let manager = DiagnosticsManager::new(directory.path().to_path_buf()).unwrap();
        manager.session_start();
        manager.session_end();

        let restarted = DiagnosticsManager::new(directory.path().to_path_buf()).unwrap();
        restarted.session_start();
        assert_eq!(restarted.recent(DiagnosticQuery::default()).total, 0);
    }

    #[test]
    fn unwritable_layout_degrades_without_losing_the_memory_ring() {
        let directory = tempdir().unwrap();
        let blocker = directory.path().join("not-a-directory");
        fs::write(&blocker, b"file").unwrap();
        let manager = DiagnosticsManager::new(blocker).unwrap();
        manager.record(event(1));
        manager.flush();
        assert!(manager.persistence_degraded());
        assert_eq!(manager.recent(DiagnosticQuery::default()).total, 1);
    }

    #[test]
    fn panic_fallback_is_redacted_and_accepts_missing_backtrace() {
        let directory = tempdir().unwrap();
        let manager = DiagnosticsManager::new(directory.path().to_path_buf()).unwrap();
        manager.record_panic(
            "test-thread",
            r"failed at C:\Users\Alice\secret.txt token=super-secret",
            None,
        );
        let crash = manager.last_crash().unwrap();
        assert!(!crash.message.contains("Alice"));
        assert!(!crash.message.contains("super-secret"));
        assert!(crash.backtrace.is_none());
    }

    #[test]
    fn log_rotation_keeps_archives_within_the_total_budget() {
        let directory = tempdir().unwrap();
        let logs = directory.path().join("logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(
            logs.join("diagnostics.log"),
            vec![b'x'; LOG_FILE_LIMIT_BYTES as usize],
        )
        .unwrap();
        for index in 1..=MAX_ARCHIVED_LOG_FILES {
            fs::write(
                logs.join(format!("diagnostics.log.{index}")),
                vec![b'x'; LOG_FILE_LIMIT_BYTES as usize],
            )
            .unwrap();
        }
        append_event(directory.path(), &event(1)).unwrap();
        let total = log_files(&logs)
            .iter()
            .map(|path| fs::metadata(path).unwrap().len())
            .sum::<u64>();
        assert!(total <= MAX_TOTAL_LOG_BYTES);
        assert!(logs.join("diagnostics.log.1").exists());
    }

    #[test]
    fn snapshot_keeps_successful_providers_when_one_provider_fails() {
        let directory = tempdir().unwrap();
        let manager = DiagnosticsManager::new(directory.path().to_path_buf()).unwrap();
        manager.register_provider("pet", || Ok(serde_json::json!({ "visible": true })));
        manager.register_provider("collector", || Err(r"C:\Users\Alice\failure".into()));
        let snapshot = manager.snapshot();
        assert!(snapshot.pet.available);
        assert!(!snapshot.collector.available);
        assert!(!snapshot.collector.error.unwrap().contains("Alice"));
    }

    #[test]
    fn export_has_a_fixed_redacted_manifest_and_cleans_temporary_files() {
        let directory = tempdir().unwrap();
        let manager = DiagnosticsManager::new(directory.path().join("data")).unwrap();
        manager.record(
            EventBuilder::new(
                DiagnosticLevel::Error,
                "plugins",
                "failed",
                r"C:\Users\Alice\Documents\private.txt password=hunter2",
            )
            .build(),
        );
        manager.flush();
        let destination = directory.path().join("diagnostics.zip");
        let result = manager
            .export(
                &destination,
                serde_json::json!({
                    "os": "windows",
                    "absolutePath": r"C:\Users\Alice",
                    "machineName": "WORKSTATION"
                }),
            )
            .unwrap();
        assert_eq!(
            result.files,
            vec!["summary.md", "logs.jsonl", "state.json", "environment.json"]
        );
        assert!(!result.last_crash_included);

        let mut archive = zip::ZipArchive::new(File::open(destination).unwrap()).unwrap();
        let mut contents = String::new();
        for name in &result.files {
            let mut file = archive.by_name(name).unwrap();
            file.read_to_string(&mut contents).unwrap();
        }
        assert!(!contents.contains("Alice"));
        assert!(!contents.contains("hunter2"));
        assert!(!contents.contains("WORKSTATION"));
        assert!(!contents.contains("absolutePath"));
        let leftovers = fs::read_dir(manager.root())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("diagnostics-export-")
            })
            .count();
        assert_eq!(leftovers, 0);
    }
}
