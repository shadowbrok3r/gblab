//! GBLab frontend: shared egui app for desktop and Android.

mod app;
mod audio;
mod input;

pub use app::GbLabApp;

/// Android entry point. NativeActivity loads libgblab.so and calls this.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(android_app: android_activity::AndroidApp) {
    use eframe::Renderer;

    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("gblab"),
    );
    std::panic::set_hook(Box::new(|info| {
        log::error!("panic: {info}");
    }));

    let mut options = eframe::NativeOptions::default();
    options.android_app = Some(android_app);
    options.renderer = Renderer::Wgpu;
    if let Err(e) = eframe::run_native(
        "GBLab",
        options,
        Box::new(|cc| Ok(Box::new(GbLabApp::new(cc)))),
    ) {
        log::error!("eframe exited with error: {e}");
    }
}
