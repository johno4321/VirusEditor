//! Desktop entry point for the Access Virus Rack editor & librarian.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod library;
mod renderer;
mod session;
mod storage;
mod views;
mod widgets;

use app::VirusApp;
use eframe::{egui, Renderer};

fn main() -> eframe::Result<()> {
    let requested = renderer::requested(
        std::env::args().skip(1),
        std::env::var(renderer::RENDERER_ENV).ok(),
    )
    .unwrap_or_else(|err| {
        eprintln!("{err}");
        std::process::exit(2);
    });

    match requested {
        Some(renderer) => run(renderer),
        // No GPU / OpenGL < 2.0 (e.g. a VM over RDP): fall back to wgpu, which can use WARP.
        None => run(Renderer::Glow).or_else(|err| {
            if !renderer::is_glow_init_error(&err) {
                return Err(err);
            }
            eprintln!("OpenGL renderer unavailable ({err}); falling back to wgpu");
            run(Renderer::Wgpu)
        }),
    }
}

fn run(renderer: Renderer) -> eframe::Result<()> {
    let mut native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Access Virus Rack Editor & Librarian")
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([1000.0, 680.0]),
        ..Default::default()
    };
    renderer::configure(&mut native_options, renderer);

    eframe::run_native(
        "Access Virus Editor",
        native_options,
        Box::new(|cc| Box::new(VirusApp::new(cc))),
    )
}
