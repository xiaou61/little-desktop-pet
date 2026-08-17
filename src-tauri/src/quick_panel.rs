use std::{
    ffi::c_void,
    mem::{size_of, size_of_val},
    sync::Mutex,
    thread,
    time::Duration,
};

use serde::Serialize;
use tauri::{
    AppHandle, Manager, PhysicalPosition as TauriPhysicalPosition,
    PhysicalSize as TauriPhysicalSize, Position, Size, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};
use windows::{
    Win32::{
        Foundation::{ERROR_SUCCESS, HWND as WindowsHwnd},
        Graphics::Dwm::{
            DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DwmIsCompositionEnabled,
            DwmSetWindowAttribute,
        },
        System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
        UI::{
            Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
            WindowsAndMessaging::{
                SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST,
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
            },
        },
    },
    core::{BOOL, w},
};

use crate::diagnostics::{DiagnosticLevel, DiagnosticsManager, EventBuilder, new_correlation_id};
use crate::panel_model::{
    PanelEffect, PanelState, PendingFocusLoss, PetAnchor, PhysicalSize, place_panel,
};

pub const QUICK_PANEL_LABEL: &str = "quick-panel";
const PANEL_LOGICAL_WIDTH: i32 = 360;
const PANEL_LOGICAL_HEIGHT: i32 = 470;
const PANEL_GAP_LOGICAL: i32 = 12;
const READY_TIMEOUT: Duration = Duration::from_secs(3);
const FOCUS_LOSS_DELAY: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickPanelEnvironment {
    pub glass_available: bool,
    pub high_contrast: bool,
    pub reduce_motion: bool,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickPanelDiagnosticState {
    pub open: bool,
    pub generation: u64,
    pub correlation_id: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Default)]
struct RuntimeState {
    panel: PanelState,
    anchor: Option<PetAnchor>,
    corrected_generation: Option<u64>,
    correlation_id: Option<String>,
    environment: QuickPanelEnvironment,
}

#[derive(Default)]
pub struct QuickPanelController {
    state: Mutex<RuntimeState>,
}

impl QuickPanelController {
    pub fn toggle(&self, app: &AppHandle, anchor: PetAnchor) {
        self.toggle_with_correlation(app, anchor, new_correlation_id());
    }

    pub fn toggle_with_correlation(
        &self,
        app: &AppHandle,
        anchor: PetAnchor,
        correlation_id: String,
    ) {
        let effect = {
            let mut state = self.lock();
            state.anchor = Some(anchor);
            state.correlation_id = Some(correlation_id.clone());
            state.panel.toggle()
        };
        match effect {
            PanelEffect::Open { generation } => {
                record_panel_event(
                    app,
                    DiagnosticLevel::Info,
                    "open-started",
                    "快捷面板开始创建。",
                    Some(&correlation_id),
                    None,
                );
                if let Err(error) = self.create(app, anchor, generation) {
                    self.fail_generation(app, generation, &error);
                }
            }
            PanelEffect::Close { .. } => {
                record_panel_event(
                    app,
                    DiagnosticLevel::Info,
                    "close-started",
                    "快捷面板开始关闭。",
                    Some(&correlation_id),
                    None,
                );
                self.destroy_window(app);
            }
            PanelEffect::None => {}
        }
    }

    pub fn close(&self, app: &AppHandle) {
        let should_destroy = {
            let mut state = self.lock();
            !matches!(state.panel.close(), PanelEffect::None)
        };
        if should_destroy {
            self.destroy_window(app);
        }
    }

    pub fn update_anchor(&self, app: &AppHandle, anchor: PetAnchor) {
        let open = {
            let mut state = self.lock();
            state.anchor = Some(anchor);
            state.panel.is_open()
        };
        if !open {
            return;
        }
        if !anchor.visible || !self.reposition(app, anchor, None) {
            self.close(app);
        }
    }

    pub fn correct_once(&self, app: &AppHandle) {
        let pending = {
            let state = self.lock();
            if !state.panel.is_open() {
                return;
            }
            let generation = state.panel.generation();
            if state.corrected_generation == Some(generation) {
                return;
            }
            (generation, state.anchor)
        };
        if let (generation, Some(anchor)) = pending {
            if self.reposition(app, anchor, None) {
                self.lock().corrected_generation = Some(generation);
            } else {
                self.close_generation(app, generation);
            }
        }
    }

    pub fn environment(&self) -> QuickPanelEnvironment {
        self.lock().environment.clone()
    }

    pub fn diagnostic_state(&self) -> QuickPanelDiagnosticState {
        let state = self.lock();
        QuickPanelDiagnosticState {
            open: state.panel.is_open(),
            generation: state.panel.generation(),
            correlation_id: state.correlation_id.clone(),
            last_error: state.environment.last_error.clone(),
        }
    }

    pub fn correlation_id(&self) -> Option<String> {
        self.lock().correlation_id.clone()
    }

    pub fn internal_action(&self) {
        let _ = self.lock().panel.internal_action();
    }

    pub fn shutdown(&self, app: &AppHandle) {
        self.close(app);
        if app.get_webview_window(QUICK_PANEL_LABEL).is_some() {
            self.destroy_window(app);
        }
    }

    fn create(&self, app: &AppHandle, anchor: PetAnchor, generation: u64) -> tauri::Result<()> {
        if let Some(orphan) = app.get_webview_window(QUICK_PANEL_LABEL) {
            let _ = orphan.destroy();
        }

        let window = WebviewWindowBuilder::new(
            app,
            QUICK_PANEL_LABEL,
            WebviewUrl::App("quick-panel.html".into()),
        )
        .title("小桌宠快捷面板")
        .inner_size(PANEL_LOGICAL_WIDTH as f64, PANEL_LOGICAL_HEIGHT as f64)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(true)
        .transparent(true)
        .devtools(
            app.try_state::<DiagnosticsManager>()
                .is_some_and(|manager| manager.config().developer_mode),
        )
        .visible(false)
        .focused(true)
        .build()?;

        self.reposition_window(&window, anchor, Some(panel_physical_size(anchor.dpi)))?;
        let environment = detect_environment(&window);
        {
            let mut state = self.lock();
            if !state.panel.accepts_response(generation) {
                let _ = window.destroy();
                return Ok(());
            }
            state.environment = environment;
            state.corrected_generation = None;
        }

        let event_app = app.clone();
        window.on_window_event(move |event| match event {
            WindowEvent::Focused(focused) => {
                if let Some(controller) = event_app.try_state::<QuickPanelController>() {
                    controller.focus_changed(&event_app, generation, *focused);
                }
            }
            WindowEvent::CloseRequested { .. } => {
                if let Some(controller) = event_app.try_state::<QuickPanelController>() {
                    controller.close_generation(&event_app, generation);
                }
            }
            WindowEvent::Destroyed => {
                if let Some(controller) = event_app.try_state::<QuickPanelController>() {
                    controller.mark_destroyed(generation);
                }
            }
            _ => {}
        });

        window.show()?;
        window.set_focus()?;
        record_panel_event(
            app,
            DiagnosticLevel::Info,
            "open-succeeded",
            "快捷面板已打开。",
            self.correlation_id().as_deref(),
            None,
        );
        let ready_app = app.clone();
        thread::Builder::new()
            .name("quick-panel-ready-watchdog".into())
            .spawn(move || {
                thread::sleep(READY_TIMEOUT);
                let main_thread_handle = ready_app.clone();
                let _ = ready_app.run_on_main_thread(move || {
                    if let Some(controller) = main_thread_handle.try_state::<QuickPanelController>()
                    {
                        controller.check_ready(&main_thread_handle, generation);
                    }
                });
            })
            .map_err(tauri::Error::Io)?;
        Ok(())
    }

    fn check_ready(&self, app: &AppHandle, generation: u64) {
        let timed_out = {
            let state = self.lock();
            state.panel.accepts_response(generation)
                && state.corrected_generation != Some(generation)
        };
        if !timed_out {
            return;
        }
        self.record_error("快捷面板渲染超时，已关闭面板。".into());
        self.close_generation(app, generation);
    }

    fn close_generation(&self, app: &AppHandle, generation: u64) {
        let should_destroy = {
            let mut state = self.lock();
            !matches!(state.panel.close_generation(generation), PanelEffect::None)
        };
        if should_destroy {
            self.destroy_window(app);
        }
    }

    fn focus_changed(&self, app: &AppHandle, generation: u64, focused: bool) {
        let pending = self.lock().panel.record_focus_change(generation, focused);
        let Some(pending) = pending else {
            return;
        };
        let focus_app = app.clone();
        let spawn_result = thread::Builder::new()
            .name("quick-panel-focus-loss".into())
            .spawn(move || {
                thread::sleep(FOCUS_LOSS_DELAY);
                let main_thread_handle = focus_app.clone();
                let _ = focus_app.run_on_main_thread(move || {
                    if let Some(controller) = main_thread_handle.try_state::<QuickPanelController>()
                    {
                        controller.confirm_focus_loss(&main_thread_handle, pending);
                    }
                });
            });
        if let Err(error) = spawn_result {
            self.record_error(format!("快捷面板失焦监听启动失败：{error}"));
        }
    }

    fn confirm_focus_loss(&self, app: &AppHandle, pending: PendingFocusLoss) {
        let should_destroy = {
            let mut state = self.lock();
            !matches!(state.panel.confirm_focus_loss(pending), PanelEffect::None)
        };
        if should_destroy {
            self.destroy_window(app);
        }
    }

    fn mark_destroyed(&self, generation: u64) {
        let mut state = self.lock();
        let _ = state.panel.close_generation(generation);
    }

    fn reposition(&self, app: &AppHandle, anchor: PetAnchor, size: Option<PhysicalSize>) -> bool {
        let Some(window) = app.get_webview_window(QUICK_PANEL_LABEL) else {
            self.record_error("快捷面板窗口已不可用。".into());
            return false;
        };
        if let Err(error) = self.reposition_window(&window, anchor, size) {
            self.record_error(format!("快捷面板定位失败：{error}"));
            return false;
        }
        true
    }

    fn reposition_window(
        &self,
        window: &WebviewWindow,
        anchor: PetAnchor,
        requested_size: Option<PhysicalSize>,
    ) -> tauri::Result<()> {
        let size = requested_size.unwrap_or_else(|| {
            window
                .inner_size()
                .map(|value| PhysicalSize {
                    width: i32::try_from(value.width).unwrap_or(i32::MAX),
                    height: i32::try_from(value.height).unwrap_or(i32::MAX),
                })
                .unwrap_or_else(|_| panel_physical_size(anchor.dpi))
        });
        if requested_size.is_some() {
            window.set_size(Size::Physical(TauriPhysicalSize::new(
                size.width.max(1) as u32,
                size.height.max(1) as u32,
            )))?;
        }
        let gap = scale_logical(PANEL_GAP_LOGICAL, anchor.dpi);
        let placement = place_panel(anchor, size, gap);
        window.set_position(Position::Physical(TauriPhysicalPosition::new(
            placement.position.x,
            placement.position.y,
        )))
    }

    fn destroy_window(&self, app: &AppHandle) {
        if let Some(window) = app.get_webview_window(QUICK_PANEL_LABEL)
            && let Err(error) = window.destroy()
        {
            self.record_error(format!("快捷面板关闭失败：{error}"));
        }
    }

    fn fail_generation(&self, app: &AppHandle, generation: u64, error: &tauri::Error) {
        let message = format!("快捷面板创建失败：{error}");
        {
            let mut state = self.lock();
            let _ = state.panel.close_generation(generation);
            state.environment.last_error = Some(message.clone());
        }
        self.destroy_window(app);
        record_panel_event(
            &app,
            DiagnosticLevel::Error,
            "open-failed",
            &message,
            self.correlation_id().as_deref(),
            Some("quick_panel_open_failed"),
        );
        eprintln!("{message}");
    }

    fn record_error(&self, message: String) {
        let bounded = message.chars().take(240).collect::<String>();
        self.lock().environment.last_error = Some(bounded.clone());
        eprintln!("{bounded}");
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RuntimeState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn record_panel_event(
    app: &AppHandle,
    level: DiagnosticLevel,
    event: &str,
    message: &str,
    correlation_id: Option<&str>,
    error_code: Option<&str>,
) {
    let Some(diagnostics) = app.try_state::<DiagnosticsManager>() else {
        return;
    };
    let mut builder =
        EventBuilder::new(level, "quick-panel", event, message).window(QUICK_PANEL_LABEL);
    if let Some(correlation_id) = correlation_id {
        builder = builder.correlation(correlation_id);
    }
    if let Some(error_code) = error_code {
        builder = builder.error_code(error_code);
    }
    diagnostics.record(builder.build());
}

fn panel_physical_size(dpi: u32) -> PhysicalSize {
    PhysicalSize {
        width: scale_logical(PANEL_LOGICAL_WIDTH, dpi),
        height: scale_logical(PANEL_LOGICAL_HEIGHT, dpi),
    }
}

fn scale_logical(value: i32, dpi: u32) -> i32 {
    ((i64::from(value) * i64::from(dpi.max(96)) + 48) / 96).max(1) as i32
}

fn detect_environment(window: &WebviewWindow) -> QuickPanelEnvironment {
    let high_contrast = high_contrast_enabled();
    let reduce_motion = !client_area_animation_enabled();
    let transparency = transparency_enabled();
    let composition = unsafe { DwmIsCompositionEnabled() }
        .map(|enabled| enabled.as_bool())
        .unwrap_or(false);
    let glass_available = if !high_contrast && transparency && composition {
        window.hwnd().ok().is_some_and(|hwnd| unsafe {
            let hwnd = WindowsHwnd(hwnd.0);
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                (&DWMSBT_TRANSIENTWINDOW
                    as *const windows::Win32::Graphics::Dwm::DWM_SYSTEMBACKDROP_TYPE)
                    .cast::<c_void>(),
                size_of_val(&DWMSBT_TRANSIENTWINDOW) as u32,
            )
            .is_ok()
        })
    } else {
        false
    };
    QuickPanelEnvironment {
        glass_available,
        high_contrast,
        reduce_motion,
        last_error: None,
    }
}

fn high_contrast_enabled() -> bool {
    let mut high_contrast = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            high_contrast.cbSize,
            Some((&mut high_contrast as *mut HIGHCONTRASTW).cast::<c_void>()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok()
        && (high_contrast.dwFlags & HCF_HIGHCONTRASTON).0 != 0
}

fn client_area_animation_enabled() -> bool {
    let mut enabled = BOOL(1);
    unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut enabled as *mut BOOL).cast::<c_void>()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok()
        && enabled.as_bool()
}

fn transparency_enabled() -> bool {
    let mut value = 1_u32;
    let mut size = size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("EnableTransparency"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast::<c_void>()),
            Some(&mut size),
        )
    };
    result == ERROR_SUCCESS && value != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_physical_size_scales_with_anchor_dpi() {
        assert_eq!(
            panel_physical_size(96),
            PhysicalSize {
                width: 360,
                height: 470
            }
        );
        assert_eq!(
            panel_physical_size(120),
            PhysicalSize {
                width: 450,
                height: 588
            }
        );
        assert_eq!(
            panel_physical_size(144),
            PhysicalSize {
                width: 540,
                height: 705
            }
        );
    }
}
