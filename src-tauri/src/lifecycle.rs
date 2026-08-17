use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{
    App, AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::{collector::CollectorHandle, pet_window::PetHandle, quick_panel::QuickPanelController};

const DASHBOARD_LABEL: &str = "dashboard";
const TRAY_ID: &str = "desktop-pet-tray";
const TOGGLE_PET_MENU_ID: &str = "toggle-pet";
const OPEN_MENU_ID: &str = "open-dashboard";
const EXIT_MENU_ID: &str = "exit-pet";
static EXPLICIT_EXIT: AtomicBool = AtomicBool::new(false);
static SHUTDOWN_STARTED: AtomicBool = AtomicBool::new(false);

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
        .invoke_handler(tauri::generate_handler![
            crate::commands::get_daily_usage,
            crate::commands::get_tracker_status,
            crate::commands::get_pet_size,
            crate::commands::set_pet_size,
            crate::commands::preview_pet_size,
            crate::commands::get_pet_skins,
            crate::commands::get_current_pet_skin,
            crate::commands::set_pet_skin,
            crate::commands::get_quick_panel_environment,
            crate::commands::quick_panel_ready,
            crate::commands::quick_panel_internal_action,
            crate::commands::close_quick_panel,
            crate::commands::open_full_statistics
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

    let collector = CollectorHandle::start(data_directory.join("daily-usage.sqlite3"))?;
    app.manage(collector);
    app.manage(QuickPanelController::default());

    let click_app = app.handle().clone();
    let anchor_app = app.handle().clone();
    match PetHandle::start(
        data_directory.join("pet-state.json"),
        data_directory.join("pet-preferences.json"),
        data_directory.join("pet-skin-preferences.json"),
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
                        panel.toggle(&main_thread_handle, anchor);
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
            eprintln!("桌宠初始化失败，使用统计和托盘仍会继续运行：{error}");
        }
    }

    install_tray(app)?;
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
    let exit = MenuItem::with_id(app, EXIT_MENU_ID, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&pet_toggle, &open, &exit])?;

    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("小桌宠")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            TOGGLE_PET_MENU_ID => toggle_pet(app),
            OPEN_MENU_ID => {
                let _ = show_dashboard(app);
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
        }
    } else if pet.show() {
        set_pet_menu_text(app, true);
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
            .center()
            .build()?;
    let window_to_destroy = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = window_to_destroy.destroy();
        }
    });
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
    let _ = app.remove_tray_by_id(TRAY_ID);
    if let Some(collector) = app.try_state::<CollectorHandle>() {
        let _ = collector.shutdown();
    }
}
