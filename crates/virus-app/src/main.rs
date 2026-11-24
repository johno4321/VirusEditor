//! Desktop entry point for the Access Virus Rack editor & librarian.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audition;
mod dialogs;
mod library;
mod session;
mod settings;
mod storage;
mod synth_select;
mod theme;
mod views;
mod widgets;

use app::VirusApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let (store, loaded) = settings::SettingsStore::load();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Access Virus Rack Editor & Librarian")
        .with_inner_size([1280.0, 860.0])
        .with_min_inner_size(settings::MIN_WINDOW);
    if let Some(window) = &loaded.settings.window {
        viewport = window.apply(viewport);
    }
    let native_options = eframe::NativeOptions { viewport, ..Default::default() };

    eframe::run_native(
        "Access Virus Editor",
        native_options,
        Box::new(move |cc| Box::new(VirusApp::new(cc, store, loaded))),
    )
}
