//! Desktop entry point for the Access Virus Rack editor & librarian.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audition;
mod comforts;
mod compare;
mod dialogs;
mod history;
mod library;
mod multi_session;
mod renderer;
mod sections;
mod session;
mod settings;
mod storage;
mod synth_select;
mod theme;
mod views;
mod widgets;

use app::VirusApp;
use eframe::{egui, Renderer};
use settings::{Loaded, SettingsStore};

fn main() -> eframe::Result<()> {
    let requested = renderer::requested(
        std::env::args().skip(1),
        std::env::var(renderer::RENDERER_ENV).ok(),
    )
    .unwrap_or_else(|err| {
        eprintln!("{err}");
        std::process::exit(2);
    });

    // Load once: a corrupt file is set aside on the first load, so a second load would lose the
    // warning. Each start attempt gets its own copy.
    let (store, loaded) = SettingsStore::load();

    match requested {
        Some(renderer) => run(renderer, store, loaded),
        // No GPU / OpenGL < 2.0 (e.g. a VM over RDP): fall back to wgpu, which can use WARP.
        None => run(Renderer::Glow, store.clone(), loaded.clone()).or_else(|err| {
            if !renderer::is_glow_init_error(&err) {
                return Err(err);
            }
            eprintln!("OpenGL renderer unavailable ({err}); falling back to wgpu");
            run(Renderer::Wgpu, store, loaded)
        }),
    }
}

fn run(renderer: Renderer, store: SettingsStore, loaded: Loaded) -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Access Virus Rack Editor & Librarian")
        .with_inner_size([1280.0, 860.0])
        .with_min_inner_size(settings::MIN_WINDOW);
    if let Some(window) = &loaded.settings.window {
        viewport = window.apply(viewport);
    }
    let mut native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    renderer::configure(&mut native_options, renderer);

    eframe::run_native(
        "Access Virus Editor",
        native_options,
        Box::new(move |cc| Box::new(VirusApp::new(cc, store, loaded))),
    )
}
