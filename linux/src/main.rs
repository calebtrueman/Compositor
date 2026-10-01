//! Compositor for Linux: a free, open-source Photoshop-style image editor for compositing and
//! photo work. It opens and saves the same `.comp` projects as the macOS app.

mod adjust;
mod app;
mod doc;
mod effects;
mod filters;
mod io;
mod project;
mod render;
mod selection;
mod text;
mod tools;
mod ui;

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("compositor {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage: compositor [FILE…]\n\nOpens images (PNG, JPEG, TIFF, WebP, BMP, GIF, PSD) and .comp projects.");
        return Ok(());
    }
    let files: Vec<PathBuf> = args.into_iter().filter(|a| !a.starts_with('-')).map(PathBuf::from).collect();

    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/app-icon-256.png")).ok();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Compositor")
        .with_app_id("io.github.calebtrueman.Compositor")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([800.0, 560.0])
        .with_drag_and_drop(true);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, persist_window: true, ..Default::default() };
    eframe::run_native("Compositor", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, files)))))
}
