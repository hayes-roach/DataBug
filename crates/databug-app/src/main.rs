//! DataBug — a fast, memory-efficient database client.

// No console window behind the app on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod compare;
mod csv_import;
mod er;
mod explorer;
mod icons;
mod rename_dialog;
mod table_editor;
mod dialogs;
mod editor;
mod grid;
mod plan_view;
mod session;
mod store;
mod theme;
mod update;

fn main() -> eframe::Result<()> {
    let mut screenshot = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--screenshot" => screenshot = args.next().map(std::path::PathBuf::from),
            "--version" | "-V" => {
                println!("databug {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!("databug [--screenshot out.png]\n\nSet DATABUG_HOME to keep settings and history in a custom folder.");
                return Ok(());
            }
            _ => {}
        }
    }
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("DataBug")
            .with_app_id("dev.databug.DataBug")
            .with_icon(app_icon())
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 560.0]),
        // repaint only when something changes: idle CPU stays at zero
        run_and_return: false,
        vsync: true,
        ..Default::default()
    };
    // Low input latency: show each frame as soon as it's drawn instead of
    // queueing frames behind vsync. On Windows, mice and touchpads that report
    // hundreds of moves a second otherwise queue a frame per move, and the
    // hover highlight trails the pointer by up to a second. Mailbox is used
    // where available (no tearing); frames take ~1-2 ms, so this stays cheap.
    #[cfg(any(target_os = "macos", windows))]
    {
        options.wgpu_options = eframe::egui_wgpu::WgpuConfiguration {
            present_mode: if cfg!(windows) { eframe::wgpu::PresentMode::AutoNoVsync } else { eframe::wgpu::PresentMode::AutoVsync },
            desired_maximum_frame_latency: Some(1),
            ..Default::default()
        };
    }
    eframe::run_native("DataBug", options, Box::new(move |cc| Ok(Box::new(app::DataBugApp::new(cc, screenshot)))))
}

fn app_icon() -> std::sync::Arc<eframe::egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../assets/icon/icon.png")).map(|i| i.to_rgba8());
    std::sync::Arc::new(match img {
        Ok(rgba) => eframe::egui::IconData { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() },
        Err(_) => eframe::egui::IconData::default(),
    })
}
