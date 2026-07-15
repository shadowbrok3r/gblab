//! The GBLab egui application, shared by desktop and Android.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{CentralPanel, Color32, ColorImage, Panel, TextureHandle, TextureOptions};

use crate::audio::AudioOut;
use crate::core::Core;
use crate::input::{self, ControllerLink, TouchTracker};
#[cfg(not(target_os = "android"))]
use crate::input::NullController;

/// Native frame duration, shared by GB and GBA (59.7275 Hz).
const FRAME_TIME: Duration = Duration::from_nanos(16_742_706);
/// Audio backlog (stereo samples) above which we skip emulating extra frames.
const AUDIO_BACKLOG_SKIP: usize = 9_600;

pub struct GbLabApp {
    core: Option<Core>,
    rom: Vec<u8>,
    rom_path: Option<PathBuf>,
    error: Option<String>,
    paused: bool,

    texture: Option<TextureHandle>,
    audio: Option<AudioOut>,
    controller: Box<dyn ControllerLink>,
    touch: TouchTracker,
    show_touch_pad: bool,
    show_browser: bool,

    last_instant: Option<Instant>,
    accumulator: Duration,
    frames_run: u64,
    save_countdown: u32,
    pad_states: input::ButtonStates,
}

impl GbLabApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let audio = match AudioOut::new() {
            Ok(a) => Some(a),
            Err(e) => {
                log::warn!("audio unavailable: {e}");
                None
            }
        };
        #[cfg(target_os = "android")]
        let controller: Box<dyn ControllerLink> = Box::new(crate::ble::BleLink::new());
        #[cfg(not(target_os = "android"))]
        let controller: Box<dyn ControllerLink> = Box::new(NullController);
        let mut app = GbLabApp {
            core: None,
            rom: Vec::new(),
            rom_path: None,
            error: None,
            paused: false,
            texture: None,
            audio,
            controller,
            touch: TouchTracker::default(),
            show_touch_pad: cfg!(target_os = "android"),
            show_browser: false,
            last_instant: None,
            accumulator: Duration::ZERO,
            frames_run: 0,
            save_countdown: 0,
            pad_states: [false; 10],
        };
        if let Some(path) = std::env::args().nth(1) {
            app.load_rom_from(PathBuf::from(path));
        }
        app
    }

    fn load_rom_from(&mut self, path: PathBuf) {
        match std::fs::read(&path) {
            Ok(bytes) => {
                self.rom = bytes;
                self.rom_path = Some(path);
                self.start();
            }
            Err(e) => self.error = Some(format!("could not read ROM: {e}")),
        }
    }

    fn start(&mut self) {
        // A ROM with no path (extension unknown) defaults to GB.
        let path = self.rom_path.clone().unwrap_or_default();
        match Core::new(self.rom.clone(), &path) {
            Ok(mut core) => {
                if let Some(sav) = self.sav_path()
                    && let Ok(data) = std::fs::read(&sav)
                {
                    core.load_save_ram(&data);
                }
                self.core = Some(core);
                self.error = None;
                self.paused = false;
                self.last_instant = None;
                self.accumulator = Duration::ZERO;
            }
            Err(e) => {
                self.core = None;
                self.error = Some(e);
            }
        }
    }

    fn sav_path(&self) -> Option<PathBuf> {
        self.rom_path.as_ref().map(|p| p.with_extension("sav"))
    }

    fn write_save(&self) {
        if let (Some(core), Some(path)) = (&self.core, self.sav_path())
            && let Some(ram) = core.save_ram()
            && let Err(e) = std::fs::write(&path, ram)
        {
            log::warn!("failed to write save {}: {e}", path.display());
        }
    }

    fn run_emulation(&mut self, ctx: &egui::Context) {
        let pad = self.pad_states;
        let ext = self.controller.poll();
        let Some(core) = &mut self.core else { return };
        if self.paused {
            self.last_instant = None;
            return;
        }

        let now = Instant::now();
        if let Some(last) = self.last_instant {
            self.accumulator += now - last;
        }
        self.last_instant = Some(now);

        // Merge all input sources into the joypad.
        let mut states = input::keyboard(ctx);
        if let Some(ext) = ext {
            states = input::merge(states, ext);
        }
        states = input::merge(states, pad);
        core.set_buttons(&states);

        let backlog = core.audio_queue_len();
        let mut frames = 0;
        while self.accumulator >= FRAME_TIME && frames < 4 {
            self.accumulator -= FRAME_TIME;
            if frames > 0 && backlog > AUDIO_BACKLOG_SKIP {
                continue;
            }
            core.run_frame();
            frames += 1;
            self.frames_run += 1;
        }
        if self.accumulator > FRAME_TIME * 4 {
            self.accumulator = Duration::ZERO;
        }

        let mut samples = Vec::new();
        core.drain_audio(&mut samples);
        if let Some(audio) = &mut self.audio {
            audio.push(&samples, core.sample_rate());
        }

        if frames > 0 {
            let (w, h) = core.screen_size();
            let img = ColorImage::from_rgba_unmultiplied([w, h], core.framebuffer());
            match &mut self.texture {
                Some(t) => t.set(img, TextureOptions::NEAREST),
                None => {
                    self.texture = Some(ctx.load_texture("gb-screen", img, TextureOptions::NEAREST));
                }
            }
        }

        self.save_countdown += 1;
        if self.save_countdown >= 300 {
            self.save_countdown = 0;
            self.write_save();
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            #[cfg(not(target_os = "android"))]
            if ui.button("Open ROM").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("ROM", &["gb", "gbc", "gba"])
                    .pick_file()
            {
                self.load_rom_from(path);
            }
            #[cfg(target_os = "android")]
            if ui.button("ROMs").clicked() {
                self.show_browser = !self.show_browser;
            }

            let has_rom = self.core.is_some();
            if ui.add_enabled(has_rom, egui::Button::new(if self.paused { "Resume" } else { "Pause" })).clicked() {
                self.paused = !self.paused;
                if self.paused {
                    self.write_save();
                }
            }
            if ui.add_enabled(has_rom, egui::Button::new("Reset")).clicked() {
                self.write_save();
                self.start();
            }
            #[cfg(not(target_os = "android"))]
            ui.checkbox(&mut self.show_touch_pad, "Pad");

            if let Some(core) = &self.core {
                ui.separator();
                ui.label(format!("{} [{}]", core.title(), core.model_label()));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.controller.enabled() {
                    if ui.small_button("off").clicked() {
                        self.controller.set_enabled(false);
                    }
                    ui.label(self.controller.status());
                } else if cfg!(target_os = "android") {
                    if ui.button("Connect pad").clicked() {
                        self.controller.set_enabled(true);
                    }
                } else {
                    ui.label(self.controller.status());
                }
            });
        });
    }

    fn rom_browser(&mut self, ui: &mut egui::Ui) {
        ui.heading("Load a ROM");
        let dirs = rom_dirs();
        let mut picked = None;
        for dir in &dirs {
            ui.label(dir.display().to_string());
            match std::fs::read_dir(dir) {
                Ok(entries) => {
                    let mut roms: Vec<PathBuf> = entries
                        .filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| {
                            matches!(
                                p.extension().and_then(|e| e.to_str()),
                                Some("gb") | Some("gbc") | Some("gba")
                            )
                        })
                        .collect();
                    roms.sort();
                    if roms.is_empty() {
                        ui.weak("  (no .gb/.gbc/.gba files)");
                    }
                    for rom in roms {
                        let name = rom.file_name().unwrap_or_default().to_string_lossy().to_string();
                        if ui.button(name).clicked() {
                            picked = Some(rom);
                        }
                    }
                }
                Err(e) => {
                    ui.weak(format!("  (not readable: {e})"));
                }
            }
            ui.add_space(8.0);
        }
        if let Some(p) = picked {
            self.load_rom_from(p);
            self.show_browser = false;
        }
    }
}

fn rom_dirs() -> Vec<PathBuf> {
    if cfg!(target_os = "android") {
        vec![
            PathBuf::from("/storage/emulated/0/Android/data/com.kingsofalchemy.gblab/files"),
            PathBuf::from("/storage/emulated/0/Download"),
        ]
    } else {
        let mut v = Vec::new();
        if let Some(home) = std::env::var_os("HOME") {
            v.push(PathBuf::from(home).join("ROMs"));
        }
        v
    }
}

impl eframe::App for GbLabApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.touch.update(&ctx);
        self.run_emulation(&ctx);

        #[cfg(target_os = "android")]
        let (inset_top, inset_bottom) = crate::insets::safe_area(ctx.pixels_per_point());
        #[cfg(not(target_os = "android"))]
        let (inset_top, inset_bottom) = (0.0f32, 0.0f32);

        Panel::top("gblab-top").show(ui, |ui| {
            ui.add_space(inset_top);
            self.top_bar(ui);
        });

        if self.show_touch_pad {
            let shoulders = matches!(&self.core, Some(Core::Gba(_)));
            Panel::bottom("gblab-pad").show(ui, |ui| {
                self.pad_states = input::virtual_gamepad(ui, &self.touch, shoulders);
                ui.add_space(inset_bottom);
            });
        } else {
            self.pad_states = [false; 10];
        }

        CentralPanel::default().show(ui, |ui| {
            if self.show_browser || self.core.is_none() {
                if let Some(err) = &self.error {
                    ui.colored_label(Color32::from_rgb(230, 90, 90), err);
                }
                self.rom_browser(ui);
                return;
            }
            if let (Some(core), Some(tex)) = (&self.core, &self.texture) {
                let (w, h) = core.screen_size();
                let avail = ui.available_size();
                let scale = (avail.x / w as f32).min(avail.y / h as f32).max(1.0);
                let scale = if scale > 1.5 { scale.floor() } else { scale };
                let size = egui::vec2(w as f32 * scale, h as f32 * scale);
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new(tex).fit_to_exact_size(size));
                });
            } else {
                ui.centered_and_justified(|ui| ui.label("Starting..."));
            }
        });

        // The emulator renders every display frame.
        ctx.request_repaint();
    }
}
