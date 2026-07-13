//! GBLab frontend: shared egui app for desktop and Android.

mod app;
mod audio;
#[cfg(target_os = "android")]
mod ble;
mod input;
#[cfg(target_os = "android")]
mod insets;
#[cfg(target_os = "android")]
mod jvm;

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

    jvm::set_activity(android_app.activity_as_ptr());

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
