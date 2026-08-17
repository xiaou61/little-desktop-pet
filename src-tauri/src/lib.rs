mod accounting;
mod collector;
mod commands;
pub mod diagnostics;
mod lifecycle;
mod model;
mod panel_model;
mod pet_preferences;
mod pet_skin_preferences;
mod pet_skins;
mod pet_window;
pub mod plugin;
mod quick_panel;
mod storage;
mod windows_adapter;

pub fn run() {
    lifecycle::run();
}
