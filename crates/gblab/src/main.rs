#[cfg(not(target_os = "android"))]
fn main() -> eframe::Result<()> {
    env_logger::init();
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([540.0, 900.0])
            .with_title("GBLab"),
        ..Default::default()
    };
    eframe::run_native("GBLab", options, Box::new(|cc| Ok(Box::new(gblab::GbLabApp::new(cc)))))
}

#[cfg(target_os = "android")]
fn main() {}
