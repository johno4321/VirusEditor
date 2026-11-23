//! Desktop entry point for the Access Virus Synthesizer Editor & Patch Librarian.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod views;
mod widgets;

use app::VirusApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Access Virus Rack / B / Classic Editor & Librarian")
            .with_inner_size([1240.0, 840.0])
            .with_min_inner_size([980.0, 680.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Access Virus Editor",
        native_options,
        Box::new(|cc| Box::new(VirusApp::new(cc))),
    )
}
