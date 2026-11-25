#![windows_subsystem = "windows"]
//! inkTeX - Pure Rust Native Desktop LaTeX Editor.
//! Run: inkTeX [projects_folder | paper_folder]

mod backend;
mod editor;
mod gdrive;
mod nn;
mod pdf;
mod theme;
mod ui;

use backend::{newest_tex, ICON, WORKSPACE};
use eframe::egui;
use std::path::PathBuf;
use std::{env, fs};
use ui::TexCraftApp;

fn load_window_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(ICON).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    })
}

fn main() {
    let dir = match env::args().nth(1) {
        Some(d) => std::path::absolute(d).expect("bad folder path"),
        None => {
            let d = PathBuf::from(env::var("USERPROFILE").unwrap_or_else(|_| ".".into()))
                .join(r"Documents\TeX Projects");
            fs::create_dir_all(&d).expect("cannot create the projects folder");
            d
        }
    };
    assert!(dir.is_dir(), "folder not found: {}", dir.display());

    // Given a paper folder itself: its parent holds the projects, and the paper opens straight away.
    let (ws, initial_project) = match (newest_tex(&dir), dir.parent(), dir.file_name()) {
        (Some(_), Some(parent), Some(n)) => {
            (parent.to_path_buf(), Some(n.to_string_lossy().to_string()))
        }
        _ => (dir, None),
    };
    WORKSPACE.set(ws).unwrap();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("inkTeX")
        .with_inner_size([1360.0, 860.0])
        .with_min_inner_size([860.0, 560.0])
        // Frameless: the app draws a VS Code-style merged title + menu bar (ui::ui_top_bar).
        .with_decorations(false);
    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    let _ = eframe::run_native(
        "inkTeX",
        options,
        Box::new(move |cc| Ok(Box::new(TexCraftApp::new(cc, initial_project)))),
    );
}
