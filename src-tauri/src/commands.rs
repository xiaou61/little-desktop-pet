use chrono::NaiveDate;
use tauri::{AppHandle, State};

use crate::{
    collector::CollectorHandle,
    model::{CommandError, DailyUsageSummary, TrackerStatus},
    pet_skins::{available_skins, skin_by_id, thumbnail_data_url},
    pet_window::{PetHandle, PetSizeStatus, PetSizeUpdate, PetSkinStatus, PetSkinUpdate},
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
pub fn get_pet_skins() -> Vec<PetSkinOption> {
    available_skins()
        .iter()
        .map(|skin| PetSkinOption {
            id: skin.id.into(),
            display_name: skin.display_name.into(),
            thumbnail_data_url: thumbnail_data_url(*skin),
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
) -> Result<PetSkinUpdate, CommandError> {
    let skin_id = normalize_skin_id(&skin_id)?;
    pet.set_skin(skin_id)
}

fn normalize_skin_id(value: &str) -> Result<&str, CommandError> {
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized.contains(['/', '\\', ':'])
        || normalized.starts_with("http")
        || skin_by_id(normalized).is_none()
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
pub fn quick_panel_ready(app: AppHandle, panel: State<'_, QuickPanelController>) {
    panel.correct_once(&app);
}

#[tauri::command]
pub fn quick_panel_internal_action(panel: State<'_, QuickPanelController>) {
    panel.internal_action();
}

#[tauri::command]
pub fn close_quick_panel(app: AppHandle, panel: State<'_, QuickPanelController>) {
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
    fn skin_command_normalizes_ids_but_rejects_paths_urls_and_unknown_values() {
        assert_eq!(
            normalize_skin_id(" orange-dragon ").unwrap(),
            "orange-dragon"
        );
        for invalid in [
            "",
            "C:\\pet.png",
            "/tmp/pet.png",
            "https://example.com/pet.png",
            "unknown",
        ] {
            let error = normalize_skin_id(invalid).unwrap_err();
            assert_eq!(error.code, "pet_skin_unknown_id");
        }
    }
}
