use chrono::NaiveDate;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{
    collector::CollectorHandle,
    diagnostics::{
        DiagnosticEvent, DiagnosticLevel, DiagnosticPage, DiagnosticQuery, DiagnosticsConfig,
        DiagnosticsManager, ExportResult, RuntimeSnapshot,
    },
    model::{CommandError, DailyUsageSummary, TrackerStatus},
    pet_window::{PetHandle, PetSizeStatus, PetSizeUpdate, PetSkinStatus, PetSkinUpdate},
    plugin::{self, PluginDirectory, PluginError, PluginManager, PluginSummary},
    quick_panel::{QuickPanelController, QuickPanelEnvironment},
};

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetSkinOption {
    pub id: String,
    pub display_name: String,
    pub thumbnail_data_url: String,
    pub available: bool,
}

#[tauri::command]
pub fn get_daily_usage(
    date: String,
    collector: State<'_, CollectorHandle>,
) -> Result<DailyUsageSummary, CommandError> {
    collector.daily_usage(parse_date(&date)?)
}

#[tauri::command]
pub fn get_tracker_status(
    collector: State<'_, CollectorHandle>,
) -> Result<TrackerStatus, CommandError> {
    collector.status()
}

#[tauri::command]
pub fn get_pet_size(pet: State<'_, PetHandle>) -> PetSizeStatus {
    PetSizeStatus {
        size_percent: pet.size_percent(),
    }
}

#[tauri::command]
pub fn set_pet_size(
    size_percent: i64,
    pet: State<'_, PetHandle>,
) -> Result<PetSizeUpdate, CommandError> {
    pet.set_size_percent(size_percent)
}

#[tauri::command]
pub fn preview_pet_size(
    size_percent: i64,
    pet: State<'_, PetHandle>,
) -> Result<PetSizeStatus, CommandError> {
    pet.preview_size_percent(size_percent)
}

#[tauri::command]
pub fn get_pet_skins(manager: State<'_, PluginManager>) -> Vec<PetSkinOption> {
    manager
        .enabled_skins()
        .into_iter()
        .map(|skin| PetSkinOption {
            id: skin.id,
            display_name: skin.display_name,
            thumbnail_data_url: skin.thumbnail_data_url.unwrap_or_default(),
            available: true,
        })
        .collect()
}

#[tauri::command]
pub fn get_current_pet_skin(pet: State<'_, PetHandle>) -> PetSkinStatus {
    PetSkinStatus {
        skin_id: pet.skin_id(),
    }
}

#[tauri::command]
pub fn set_pet_skin(
    skin_id: String,
    pet: State<'_, PetHandle>,
    manager: State<'_, PluginManager>,
) -> Result<PetSkinUpdate, CommandError> {
    let skin_id = normalize_skin_id(&skin_id)?;
    if !manager.is_skin_enabled(skin_id) {
        return Err(CommandError::new(
            "plugin_not_enabled",
            "请先从插件目录安装并启用该皮肤。",
        ));
    }
    pet.set_skin(skin_id)
}

#[tauri::command]
pub fn get_plugin_directory(manager: State<'_, PluginManager>) -> PluginDirectory {
    manager.directory()
}

#[tauri::command]
pub fn open_plugin_manager(app: AppHandle) -> Result<(), CommandError> {
    crate::lifecycle::show_plugin_manager(&app)
        .map_err(|_| CommandError::new("plugin_manager_open_failed", "插件管理界面暂时无法打开。"))
}

#[tauri::command]
pub fn get_plugins(manager: State<'_, PluginManager>) -> PluginDirectory {
    get_plugin_directory(manager)
}

#[tauri::command]
pub fn get_plugin_capabilities(manager: State<'_, PluginManager>) -> plugin::HostCapabilities {
    manager.capabilities()
}

#[tauri::command]
pub fn preview_plugin_package(
    path: String,
    manager: State<'_, PluginManager>,
) -> Result<PluginSummary, CommandError> {
    let path = normalize_plugin_import_path(&path)?;
    let bytes = std::fs::read(path)
        .map_err(|_| CommandError::new("plugin_import_read_failed", "无法读取所选插件包。"))?;
    manager.preview_package(&bytes).map_err(plugin_error)
}

#[tauri::command]
pub fn install_plugin_package(
    path: String,
    manager: State<'_, PluginManager>,
) -> Result<PluginSummary, CommandError> {
    let path = normalize_plugin_import_path(&path)?;
    manager.import_package_file(path).map_err(plugin_error)
}

#[tauri::command]
pub fn install_official_plugin(
    plugin_id: String,
    manager: State<'_, PluginManager>,
    diagnostics: State<'_, DiagnosticsManager>,
) -> Result<PluginSummary, CommandError> {
    let plugin_id = normalize_plugin_id(&plugin_id)?;
    let result = manager
        .install_official_skin(plugin_id)
        .map_err(plugin_error);
    record_plugin_result(&diagnostics, "install", plugin_id, &result);
    result
}

#[tauri::command]
pub fn enable_plugin(
    plugin_id: String,
    manager: State<'_, PluginManager>,
    diagnostics: State<'_, DiagnosticsManager>,
) -> Result<PluginSummary, CommandError> {
    let plugin_id = normalize_plugin_id(&plugin_id)?;
    let result = manager.enable(plugin_id).map_err(plugin_error);
    record_plugin_result(&diagnostics, "enable", plugin_id, &result);
    result
}

#[tauri::command]
pub fn disable_plugin(
    plugin_id: String,
    manager: State<'_, PluginManager>,
    pet: State<'_, PetHandle>,
    diagnostics: State<'_, DiagnosticsManager>,
) -> Result<PluginSummary, CommandError> {
    let plugin_id = normalize_plugin_id(&plugin_id)?;
    let previous_skin = pet.skin_id();
    let switched_skin =
        previous_skin == plugin_id && plugin_id != crate::pet_skins::DEFAULT_SKIN_ID;
    if switched_skin {
        if !manager.is_skin_enabled(crate::pet_skins::DEFAULT_SKIN_ID) {
            return Err(CommandError::new(
                "plugin_default_skin_required",
                "当前皮肤被禁用前必须先保留默认皮肤。",
            ));
        }
        pet.set_skin(crate::pet_skins::DEFAULT_SKIN_ID)?;
    }
    let result = manager.disable(plugin_id).map_err(plugin_error);
    if result.is_err() && switched_skin {
        let _ = pet.set_skin(&previous_skin);
    }
    record_plugin_result(&diagnostics, "disable", plugin_id, &result);
    result
}

#[tauri::command]
pub fn uninstall_plugin(
    plugin_id: String,
    manager: State<'_, PluginManager>,
    diagnostics: State<'_, DiagnosticsManager>,
) -> Result<(), CommandError> {
    let plugin_id = normalize_plugin_id(&plugin_id)?;
    let result = manager.uninstall(plugin_id).map_err(plugin_error);
    record_plugin_result(&diagnostics, "uninstall", plugin_id, &result);
    result
}

fn record_plugin_result<T>(
    diagnostics: &DiagnosticsManager,
    operation: &str,
    plugin_id: &str,
    result: &Result<T, CommandError>,
) {
    let (level, event, message) = match result {
        Ok(_) => (
            DiagnosticLevel::Info,
            format!("{operation}-succeeded"),
            format!("插件操作 {operation} 已完成。"),
        ),
        Err(error) => (
            DiagnosticLevel::Error,
            format!("{operation}-failed"),
            error.message.clone(),
        ),
    };
    let mut builder =
        crate::diagnostics::EventBuilder::new(level, "plugins", event, message).plugin(plugin_id);
    if let Err(error) = result {
        builder = builder.error_code(&error.code);
    }
    diagnostics.record(builder.build());
}

#[tauri::command]
pub fn get_plugin_contributions(
    plugin_id: String,
    manager: State<'_, PluginManager>,
) -> Result<Vec<plugin::PluginContribution>, CommandError> {
    manager
        .contributions(normalize_plugin_id(&plugin_id)?)
        .map_err(plugin_error)
}

#[tauri::command]
pub fn execute_plugin_action(
    plugin_id: String,
    contribution_id: String,
    action_id: String,
    manager: State<'_, PluginManager>,
) -> Result<(), CommandError> {
    manager
        .execute_action(
            normalize_plugin_id(&plugin_id)?,
            normalize_plugin_id(&contribution_id)?,
            normalize_plugin_id(&action_id)?,
        )
        .map_err(plugin_error)
}

fn plugin_error(error: PluginError) -> CommandError {
    CommandError::new(error.code, error.message)
}

fn normalize_plugin_id(value: &str) -> Result<&str, CommandError> {
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized.len() > 96
        || normalized.starts_with('.')
        || normalized.ends_with('.')
        || normalized.contains("..")
        || normalized.contains(['/', '\\', ':'])
        || normalized.starts_with("http")
        || !normalized.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
        })
    {
        return Err(CommandError::new("plugin_id_invalid", "插件标识无效。"));
    }
    Ok(normalized)
}

fn normalize_plugin_import_path(value: &str) -> Result<&std::path::Path, CommandError> {
    let normalized = value.trim();
    let path = std::path::Path::new(normalized);
    if normalized.is_empty()
        || normalized.starts_with("http://")
        || normalized.starts_with("https://")
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        || path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("petpack"))
    {
        return Err(CommandError::new(
            "plugin_import_path_invalid",
            "请选择本地 .petpack 文件。",
        ));
    }
    Ok(path)
}

fn normalize_skin_id(value: &str) -> Result<&str, CommandError> {
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized.starts_with('.')
        || normalized.ends_with('.')
        || normalized.contains("..")
        || normalized.contains(['/', '\\', ':'])
        || normalized.starts_with("http")
        || !normalized.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
        })
    {
        return Err(CommandError::new(
            "pet_skin_unknown_id",
            "未找到可用的桌宠皮肤。",
        ));
    }
    Ok(normalized)
}

#[tauri::command]
pub fn get_quick_panel_environment(
    panel: State<'_, QuickPanelController>,
) -> QuickPanelEnvironment {
    panel.environment()
}

#[tauri::command]
pub fn quick_panel_ready(
    app: AppHandle,
    panel: State<'_, QuickPanelController>,
    diagnostics: State<'_, DiagnosticsManager>,
) {
    panel.correct_once(&app);
    let mut builder = crate::diagnostics::EventBuilder::new(
        DiagnosticLevel::Info,
        "quick-panel",
        "webview-ready",
        "快捷面板 WebView 已就绪。",
    )
    .window(crate::quick_panel::QUICK_PANEL_LABEL);
    if let Some(correlation_id) = panel.correlation_id() {
        builder = builder.correlation(correlation_id);
    }
    diagnostics.record(builder.build());
}

#[tauri::command]
pub fn quick_panel_internal_action(panel: State<'_, QuickPanelController>) {
    panel.internal_action();
}

#[tauri::command]
pub fn close_quick_panel(
    app: AppHandle,
    panel: State<'_, QuickPanelController>,
    diagnostics: State<'_, DiagnosticsManager>,
) {
    let mut builder = crate::diagnostics::EventBuilder::new(
        DiagnosticLevel::Info,
        "quick-panel",
        "close-requested",
        "快捷面板收到关闭请求。",
    )
    .window(crate::quick_panel::QUICK_PANEL_LABEL);
    if let Some(correlation_id) = panel.correlation_id() {
        builder = builder.correlation(correlation_id);
    }
    diagnostics.record(builder.build());
    panel.close(&app);
}

#[tauri::command]
pub fn open_full_statistics(
    app: AppHandle,
    panel: State<'_, QuickPanelController>,
) -> Result<(), CommandError> {
    panel.close(&app);
    crate::lifecycle::show_dashboard(&app)
        .map_err(|_| CommandError::new("dashboard_open_failed", "完整统计窗口暂时无法打开。"))
}

#[tauri::command]
pub fn get_diagnostic_events(
    query: DiagnosticQuery,
    diagnostics: State<'_, DiagnosticsManager>,
) -> DiagnosticPage {
    diagnostics.recent(query)
}

#[tauri::command]
pub fn get_recent_errors(diagnostics: State<'_, DiagnosticsManager>) -> DiagnosticPage {
    diagnostics.recent(DiagnosticQuery {
        level: Some(DiagnosticLevel::Error),
        limit: 50,
        ..DiagnosticQuery::default()
    })
}

#[tauri::command]
pub fn get_diagnostics_config(diagnostics: State<'_, DiagnosticsManager>) -> DiagnosticsConfig {
    diagnostics.config()
}

#[tauri::command]
pub fn set_diagnostics_config(
    app: AppHandle,
    config: DiagnosticsConfig,
    diagnostics: State<'_, DiagnosticsManager>,
) -> Result<DiagnosticsConfig, CommandError> {
    let config = diagnostics.set_config(config).map_err(|_| {
        CommandError::new("diagnostics_settings_write_failed", "诊断设置保存失败。")
    })?;
    if config.developer_mode {
        for window in app.webview_windows().values() {
            window.open_devtools();
        }
    }
    diagnostics.record(
        crate::diagnostics::EventBuilder::new(
            DiagnosticLevel::Info,
            "diagnostics",
            "config-changed",
            "诊断设置已更新。",
        )
        .build(),
    );
    Ok(config)
}

#[tauri::command]
pub fn get_diagnostic_snapshot(
    app: AppHandle,
    diagnostics: State<'_, DiagnosticsManager>,
) -> RuntimeSnapshot {
    let mut snapshot = diagnostics.snapshot();
    if let Some(pet) = app.try_state::<PetHandle>() {
        snapshot.pet = crate::diagnostics::ComponentSnapshot {
            available: true,
            state: Some(serde_json::json!({
                "visible": pet.is_visible(),
                "sizePercent": pet.size_percent(),
                "skinId": pet.skin_id()
            })),
            error: None,
        };
    }
    if let Some(panel) = app.try_state::<QuickPanelController>() {
        snapshot.quick_panel = crate::diagnostics::ComponentSnapshot {
            available: true,
            state: serde_json::to_value(panel.diagnostic_state()).ok(),
            error: None,
        };
    }
    if let Some(collector) = app.try_state::<CollectorHandle>() {
        snapshot.collector = match collector.status() {
            Ok(status) => crate::diagnostics::ComponentSnapshot {
                available: true,
                state: serde_json::to_value(status).ok(),
                error: None,
            },
            Err(error) => crate::diagnostics::ComponentSnapshot {
                available: false,
                state: None,
                error: Some(error.message),
            },
        };
    }
    if let Some(manager) = app.try_state::<PluginManager>() {
        snapshot.plugins = crate::diagnostics::ComponentSnapshot {
            available: true,
            state: serde_json::to_value(manager.summaries()).ok(),
            error: None,
        };
    }
    snapshot.webview_labels = app.webview_windows().keys().cloned().collect();
    diagnostics.set_snapshot(snapshot.clone());
    snapshot
}

#[tauri::command]
pub fn get_diagnostic_runtime_status(
    app: AppHandle,
    diagnostics: State<'_, DiagnosticsManager>,
) -> RuntimeSnapshot {
    get_diagnostic_snapshot(app, diagnostics)
}

#[tauri::command]
pub fn get_recent_crash(
    diagnostics: State<'_, DiagnosticsManager>,
) -> Option<crate::diagnostics::LastCrash> {
    diagnostics.last_crash()
}

#[tauri::command]
pub fn record_diagnostic_event(
    app: AppHandle,
    event: DiagnosticEvent,
    diagnostics: State<'_, DiagnosticsManager>,
) -> Result<(), CommandError> {
    if event.module.trim().is_empty() || event.event.trim().is_empty() {
        return Err(CommandError::new(
            "diagnostics_event_invalid",
            "诊断事件缺少模块或事件名。",
        ));
    }
    diagnostics.record(event.clone());
    let _ = app.emit("diagnostic-event", event);
    Ok(())
}

#[tauri::command]
pub fn copy_diagnostics_summary(diagnostics: State<'_, DiagnosticsManager>) -> String {
    diagnostics.public_summary()
}

#[tauri::command]
pub fn export_diagnostics(
    destination: String,
    diagnostics: State<'_, DiagnosticsManager>,
) -> Result<ExportResult, CommandError> {
    let destination = destination.trim();
    if destination.is_empty() || !destination.to_ascii_lowercase().ends_with(".zip") {
        return Err(CommandError::new(
            "diagnostics_export_path_invalid",
            "请选择以 .zip 结尾的本地保存位置。",
        ));
    }
    let environment = serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "buildMode": if cfg!(debug_assertions) { "debug" } else { "release" },
        "developerMode": diagnostics.config().developer_mode,
    });
    diagnostics
        .export(&PathBuf::from(destination), environment)
        .map_err(|_| CommandError::new("diagnostics_export_failed", "诊断包导出失败。"))
}

#[tauri::command]
pub fn open_diagnostics_log_directory(
    diagnostics: State<'_, DiagnosticsManager>,
) -> Result<(), CommandError> {
    let path = diagnostics.root().join("logs");
    std::fs::create_dir_all(&path)
        .map_err(|_| CommandError::new("diagnostics_log_directory_failed", "日志目录不可用。"))?;
    #[cfg(windows)]
    std::process::Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|_| CommandError::new("diagnostics_log_directory_failed", "无法打开日志目录。"))?;
    Ok(())
}

#[tauri::command]
pub fn open_diagnostics_center(app: AppHandle) -> Result<(), CommandError> {
    crate::lifecycle::show_diagnostics(&app)
        .map_err(|_| CommandError::new("diagnostics_window_open_failed", "诊断中心暂时无法打开。"))
}

fn parse_date(value: &str) -> Result<NaiveDate, CommandError> {
    if value.len() != 10 {
        return Err(invalid_date());
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| invalid_date())
}

fn invalid_date() -> CommandError {
    CommandError::new("invalid_date", "日期必须使用 YYYY-MM-DD 格式。")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_strict_calendar_dates() {
        assert_eq!(
            parse_date("2026-08-14").unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 14).unwrap()
        );
        for invalid in ["2026-8-14", "2026-02-30", "not-a-date", ""] {
            let error = parse_date(invalid).unwrap_err();
            assert_eq!(error.code, "invalid_date");
            assert!(error.message.len() < 80);
        }
    }

    #[test]
    fn skin_command_normalizes_ids_but_rejects_paths_urls_and_invalid_values() {
        assert_eq!(
            normalize_skin_id(" orange-dragon ").unwrap(),
            "orange-dragon"
        );
        assert_eq!(
            normalize_skin_id("third-party-skin").unwrap(),
            "third-party-skin"
        );
        for invalid in [
            "",
            "C:\\pet.png",
            "/tmp/pet.png",
            "https://example.com/pet.png",
            "UPPERCASE",
            "skin name",
        ] {
            let error = normalize_skin_id(invalid).unwrap_err();
            assert_eq!(error.code, "pet_skin_unknown_id");
        }
    }
}
