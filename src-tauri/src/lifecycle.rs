use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{
    App, AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::{
    collector::CollectorHandle,
    diagnostics::{DiagnosticLevel, DiagnosticsManager, EventBuilder},
    pet_window::{PetHandle, PetStartError},
    plugin::PluginManager,
    quick_panel::QuickPanelController,
};

const DASHBOARD_LABEL: &str = "dashboard";
const TRAY_ID: &str = "desktop-pet-tray";
const TOGGLE_PET_MENU_ID: &str = "toggle-pet";
const OPEN_MENU_ID: &str = "open-dashboard";
const OPEN_PLUGINS_MENU_ID: &str = "open-plugins";
const OPEN_DIAGNOSTICS_MENU_ID: &str = "open-diagnostics";
const EXIT_MENU_ID: &str = "exit-pet";
pub(crate) const DIAGNOSTICS_LABEL: &str = "diagnostics";
static EXPLICIT_EXIT: AtomicBool = AtomicBool::new(false);
static SHUTDOWN_STARTED: AtomicBool = AtomicBool::new(false);

fn developer_mode_enabled(app: Option<&AppHandle>) -> bool {
    let environment_enabled = cfg!(debug_assertions)
        && std::env::var("TAURI_DEV_MODE").ok().is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        });
    environment_enabled
        || app
            .and_then(|handle| handle.try_state::<DiagnosticsManager>())
            .is_some_and(|manager| manager.config().developer_mode)
}

fn developer_window_on_startup() -> Option<&'static str> {
    if !developer_mode_enabled(None) {
        return None;
    }
    match std::env::var("TAURI_DEV_WINDOW")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("diagnostics") => Some(DIAGNOSTICS_LABEL),
        Some("plugin-manager") | Some("plugins") => Some("plugin-manager"),
        Some("dashboard") => Some(DASHBOARD_LABEL),
        _ => None,
    }
}

fn open_developer_tools(window: &WebviewWindow, enabled: bool) {
    if enabled {
        window.open_devtools();
    }
}

struct TrayControls {
    pet_toggle: MenuItem<tauri::Wry>,
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            |app, _arguments, _cwd| {
                show_pet(app);
            },
        ))
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(tauri_plugin_log::log::LevelFilter::Info)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(5))
                .max_file_size(2 * 1024 * 1024)
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            crate::commands::get_daily_usage,
            crate::commands::get_tracker_status,
            crate::commands::get_pet_size,
            crate::commands::set_pet_size,
            crate::commands::preview_pet_size,
            crate::commands::get_pet_skins,
            crate::commands::get_current_pet_skin,
            crate::commands::set_pet_skin,
            crate::commands::get_plugin_directory,
            crate::commands::get_plugins,
            crate::commands::get_plugin_capabilities,
            crate::commands::open_plugin_manager,
            crate::commands::preview_plugin_package,
            crate::commands::install_plugin_package,
            crate::commands::install_official_plugin,
            crate::commands::enable_plugin,
            crate::commands::disable_plugin,
            crate::commands::uninstall_plugin,
            crate::commands::get_plugin_contributions,
            crate::commands::execute_plugin_action,
            crate::commands::get_quick_panel_environment,
            crate::commands::quick_panel_ready,
            crate::commands::quick_panel_internal_action,
            crate::commands::close_quick_panel,
            crate::commands::open_full_statistics,
            crate::commands::get_diagnostic_events,
            crate::commands::get_recent_errors,
            crate::commands::get_diagnostics_config,
            crate::commands::set_diagnostics_config,
            crate::commands::get_diagnostic_snapshot,
            crate::commands::get_diagnostic_runtime_status,
            crate::commands::get_recent_crash,
            crate::commands::record_diagnostic_event,
            crate::commands::copy_diagnostics_summary,
            crate::commands::export_diagnostics,
            crate::commands::open_diagnostics_log_directory,
            crate::commands::open_diagnostics_center
        ])
        .setup(setup)
        .build(tauri::generate_context!())
        .expect("failed to build 小桌宠");

    app.run(|app, event| match event {
        RunEvent::ExitRequested {
            code: None, api, ..
        } if !EXPLICIT_EXIT.load(Ordering::Acquire) => {
            api.prevent_exit();
        }
        RunEvent::ExitRequested { .. } | RunEvent::Exit => shutdown_all(app),
        _ => {}
    });
}

fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let data_directory = app.path().app_local_data_dir()?;
    std::fs::create_dir_all(&data_directory)?;

    let diagnostics = DiagnosticsManager::new(data_directory.join("diagnostics"))?;
    if developer_mode_enabled(None) {
        let mut config = diagnostics.config();
        config.developer_mode = true;
        if config.level == DiagnosticLevel::Info {
            config.level = DiagnosticLevel::Debug;
        }
        let _ = diagnostics.set_config(config);
    }
    diagnostics.install_global();
    diagnostics.install_panic_hook();
    diagnostics.session_start();
    diagnostics.record(
        EventBuilder::new(
            DiagnosticLevel::Info,
            "lifecycle",
            "app-started",
            "应用启动。",
        )
        .build(),
    );
    app.manage(diagnostics.clone());

    let plugin_manager = match PluginManager::bootstrap(data_directory.clone()) {
        Ok(manager) => manager,
        Err(error) => {
            diagnostics.record(
                EventBuilder::new(
                    DiagnosticLevel::Error,
                    "plugins",
                    "bootstrap-failed",
                    error.to_string(),
                )
                .error_code("plugin_bootstrap_failed")
                .build(),
            );
            eprintln!("插件注册表初始化失败，使用内存中的核心插件状态：{error}");
            PluginManager::in_memory(data_directory.clone())
        }
    };
    app.manage(plugin_manager.clone());

    let collector = CollectorHandle::start_with_diagnostics(
        data_directory.join("daily-usage.sqlite3"),
        diagnostics.clone(),
    )?;
    app.manage(collector);
    app.manage(QuickPanelController::default());

    let skin_manager = plugin_manager.clone();
    let resource_manager = plugin_manager.clone();
    let click_app = app.handle().clone();
    let anchor_app = app.handle().clone();
    match PetHandle::start_with_skin_provider(
        data_directory.join("pet-state.json"),
        data_directory.join("pet-preferences.json"),
        data_directory.join("pet-skin-preferences.json"),
        move |skin_id| skin_manager.is_skin_enabled(skin_id),
        move |skin_id| {
            let bytes = resource_manager
                .skin_png(skin_id)
                .map_err(|error| PetStartError::new(error.message))?;
            image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                .map(|image| image.into_rgba8())
                .map_err(|error| PetStartError::new(format!("PNG resource failed: {error}")))
        },
        move || {
            let scheduling_handle = click_app.clone();
            let main_thread_handle = scheduling_handle.clone();
            let _ = scheduling_handle.run_on_main_thread(move || {
                let Some(panel) = main_thread_handle.try_state::<QuickPanelController>() else {
                    return;
                };
                let Some(pet) = main_thread_handle.try_state::<PetHandle>() else {
                    return;
                };
                match pet.anchor() {
                    Ok(anchor) => {
                        let correlation_id = crate::diagnostics::new_correlation_id();
                        if let Some(diagnostics) =
                            main_thread_handle.try_state::<DiagnosticsManager>()
                        {
                            diagnostics.record(
                                EventBuilder::new(
                                    DiagnosticLevel::Info,
                                    "pet",
                                    "clicked",
                                    "用户点击桌宠。",
                                )
                                .correlation(&correlation_id)
                                .build(),
                            );
                        }
                        panel.toggle_with_correlation(&main_thread_handle, anchor, correlation_id);
                    }
                    Err(error) => eprintln!("快捷面板无法读取桌宠锚点：{}", error.message),
                }
            });
        },
        move |anchor| {
            let scheduling_handle = anchor_app.clone();
            let main_thread_handle = scheduling_handle.clone();
            let _ = scheduling_handle.run_on_main_thread(move || {
                if let Some(panel) = main_thread_handle.try_state::<QuickPanelController>() {
                    panel.update_anchor(&main_thread_handle, anchor);
                }
            });
        },
    ) {
        Ok(pet) => {
            app.manage(pet);
        }
        Err(error) => {
            diagnostics.record(
                EventBuilder::new(
                    DiagnosticLevel::Error,
                    "pet",
                    "start-failed",
                    error.to_string(),
                )
                .error_code("pet_start_failed")
                .build(),
            );
            eprintln!("桌宠初始化失败，使用统计和托盘仍会继续运行：{error}");
        }
    }

    install_tray(app)?;
    if let Some(label) = developer_window_on_startup() {
        let result = match label {
            DIAGNOSTICS_LABEL => show_diagnostics(app.handle()),
            "plugin-manager" => show_plugin_manager(app.handle()),
            DASHBOARD_LABEL => show_dashboard(app.handle()),
            _ => Ok(()),
        };
        if let Err(error) = result {
            diagnostics.record(
                EventBuilder::new(
                    DiagnosticLevel::Error,
                    "lifecycle",
                    "developer-window-open-failed",
                    error.to_string(),
                )
                .error_code("developer_window_open_failed")
                .build(),
            );
        }
    }
    Ok(())
}

fn install_tray(app: &App) -> tauri::Result<()> {
    let pet_is_visible = app
        .try_state::<PetHandle>()
        .is_some_and(|pet| pet.is_visible());
    let pet_toggle = MenuItem::with_id(
        app,
        TOGGLE_PET_MENU_ID,
        pet_menu_text(pet_is_visible),
        true,
        None::<&str>,
    )?;
    let open = MenuItem::with_id(app, OPEN_MENU_ID, "打开使用统计", true, None::<&str>)?;
    let plugins = MenuItem::with_id(app, OPEN_PLUGINS_MENU_ID, "管理插件", true, None::<&str>)?;
    let diagnostics = MenuItem::with_id(
        app,
        OPEN_DIAGNOSTICS_MENU_ID,
        "打开诊断中心",
        true,
        None::<&str>,
    )?;
    let exit = MenuItem::with_id(app, EXIT_MENU_ID, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&pet_toggle, &open, &plugins, &diagnostics, &exit])?;

    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("小桌宠")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            TOGGLE_PET_MENU_ID => toggle_pet(app),
            OPEN_MENU_ID => {
                let _ = show_dashboard(app);
            }
            OPEN_PLUGINS_MENU_ID => {
                let _ = show_plugin_manager(app);
            }
            OPEN_DIAGNOSTICS_MENU_ID => {
                let _ = show_diagnostics(app);
            }
            EXIT_MENU_ID => exit_from_tray(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_pet(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }
    tray.build(app)?;
    app.manage(TrayControls { pet_toggle });
    Ok(())
}

fn toggle_pet(app: &AppHandle) {
    let Some(pet) = app.try_state::<PetHandle>() else {
        return;
    };
    if pet.is_visible() {
        if pet.hide() {
            if let Some(panel) = app.try_state::<QuickPanelController>() {
                panel.close(app);
            }
            set_pet_menu_text(app, false);
            record_runtime_event(app, "pet", "hidden", "桌宠已隐藏。", None);
        }
    } else if pet.show() {
        set_pet_menu_text(app, true);
        record_runtime_event(app, "pet", "shown", "桌宠已显示。", None);
    }
}

fn show_pet(app: &AppHandle) {
    if let Some(pet) = app.try_state::<PetHandle>()
        && pet.ensure_visible()
    {
        set_pet_menu_text(app, true);
    }
}

fn set_pet_menu_text(app: &AppHandle, visible: bool) {
    if let Some(controls) = app.try_state::<TrayControls>() {
        let _ = controls.pet_toggle.set_text(pet_menu_text(visible));
    }
}

fn pet_menu_text(visible: bool) -> &'static str {
    if visible {
        "隐藏桌宠"
    } else {
        "显示桌宠"
    }
}

pub(crate) fn show_dashboard(app: &AppHandle) -> tauri::Result<()> {
    if let Some(panel) = app.try_state::<QuickPanelController>() {
        panel.close(app);
    }
    if let Some(window) = app.get_webview_window(DASHBOARD_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        return window.set_focus();
    }

    let window =
        WebviewWindowBuilder::new(app, DASHBOARD_LABEL, WebviewUrl::App("index.html".into()))
            .title("使用统计")
            .inner_size(460.0, 620.0)
            .min_inner_size(380.0, 500.0)
            .resizable(true)
            .devtools(developer_mode_enabled(Some(app)))
            .center()
            .build()?;
    open_developer_tools(&window, developer_mode_enabled(Some(app)));
    let window_to_destroy = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = window_to_destroy.destroy();
        }
    });
    window.set_focus()?;
    record_runtime_event(
        app,
        "dashboard",
        "window-opened",
        "完整统计窗口已打开。",
        Some(DASHBOARD_LABEL),
    );
    Ok(())
}

pub(crate) fn show_plugin_manager(app: &AppHandle) -> tauri::Result<()> {
    const LABEL: &str = "plugin-manager";
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        return window.set_focus();
    }
    let window =
        WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("plugin-manager.html".into()))
            .title("插件管理")
            .inner_size(620.0, 700.0)
            .min_inner_size(480.0, 520.0)
            .resizable(true)
            .devtools(developer_mode_enabled(Some(app)))
            .center()
            .build()?;
    open_developer_tools(&window, developer_mode_enabled(Some(app)));
    let window_to_destroy = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = window_to_destroy.destroy();
        }
    });
    window.set_focus()?;
    record_runtime_event(
        app,
        "plugins",
        "window-opened",
        "插件管理窗口已打开。",
        Some(LABEL),
    );
    Ok(())
}

pub(crate) fn show_diagnostics(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(DIAGNOSTICS_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        return window.set_focus();
    }

    let developer_mode = developer_mode_enabled(Some(app));
    let window = WebviewWindowBuilder::new(
        app,
        DIAGNOSTICS_LABEL,
        WebviewUrl::App("diagnostics.html".into()),
    )
    .title("诊断中心")
    .inner_size(980.0, 720.0)
    .min_inner_size(760.0, 560.0)
    .resizable(true)
    .devtools(developer_mode)
    .center()
    .build()?;
    open_developer_tools(&window, developer_mode);
    let window_to_destroy = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = window_to_destroy.destroy();
        }
    });
    if let Some(diagnostics) = app.try_state::<DiagnosticsManager>() {
        diagnostics.record(
            EventBuilder::new(
                DiagnosticLevel::Info,
                "diagnostics",
                "window-opened",
                "诊断中心已打开。",
            )
            .window(DIAGNOSTICS_LABEL)
            .build(),
        );
    }
    window.set_focus()?;
    Ok(())
}

fn exit_from_tray(app: &AppHandle) {
    if EXPLICIT_EXIT.swap(true, Ordering::AcqRel) {
        return;
    }
    shutdown_all(app);
    app.exit(0);
}

fn shutdown_all(app: &AppHandle) {
    if SHUTDOWN_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }

    if let Some(pet) = app.try_state::<PetHandle>() {
        let _ = pet.save_position();
    }
    if let Some(collector) = app.try_state::<CollectorHandle>() {
        let _ = collector.flush();
    }

    if let Some(panel) = app.try_state::<QuickPanelController>() {
        panel.shutdown(app);
    }
    if let Some(pet) = app.try_state::<PetHandle>() {
        pet.shutdown();
    }
    if let Some(window) = app.get_webview_window(DASHBOARD_LABEL) {
        let _ = window.destroy();
    }
    if let Some(window) = app.get_webview_window("plugin-manager") {
        let _ = window.destroy();
    }
    if let Some(window) = app.get_webview_window(DIAGNOSTICS_LABEL) {
        let _ = window.destroy();
    }
    let _ = app.remove_tray_by_id(TRAY_ID);
    if let Some(collector) = app.try_state::<CollectorHandle>() {
        let _ = collector.shutdown();
    }
    if let Some(diagnostics) = app.try_state::<DiagnosticsManager>() {
        diagnostics.record(
            EventBuilder::new(
                DiagnosticLevel::Info,
                "lifecycle",
                "app-stopped",
                "应用正常退出。",
            )
            .build(),
        );
        diagnostics.session_end();
    }
}

fn record_runtime_event(
    app: &AppHandle,
    module: &str,
    event: &str,
    message: &str,
    window: Option<&str>,
) {
    let Some(diagnostics) = app.try_state::<DiagnosticsManager>() else {
        return;
    };
    let mut builder = EventBuilder::new(DiagnosticLevel::Info, module, event, message);
    if let Some(window) = window {
        builder = builder.window(window);
    }
    diagnostics.record(builder.build());
}
