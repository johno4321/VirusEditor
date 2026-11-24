//! Desktop entry point for the Access Virus Rack editor & librarian.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audition;
mod compare;
mod comforts;
mod history;
mod library;
mod sections;
mod session;
mod storage;
mod views;
mod widgets;

use app::VirusApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Access Virus Rack Editor & Librarian")
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([1000.0, 680.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Access Virus Editor",
        native_options,
        Box::new(|cc| Box::new(VirusApp::new(cc))),
    )
}
