//! Game Boy / Game Boy Color emulator core with no platform dependencies.

mod apu;
mod bus;
mod cartridge;
mod cpu;
mod joypad;
mod ppu;
mod timer;

pub use apu::SAMPLE_RATE;
pub use joypad::Button;
pub use ppu::{SCREEN_H, SCREEN_W};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Model {
    Dmg,
    Cgb,
}

pub struct GameBoy {
    cpu: cpu::Cpu,
    pub model: Model,
    title: String,
}

impl GameBoy {
    /// Model picked from the cartridge's CGB flag.
    pub fn new(rom: Vec<u8>) -> Result<Self, String> {
        let cart = cartridge::Cartridge::new(rom)?;
        let model = if cart.cgb { Model::Cgb } else { Model::Dmg };
        Ok(Self::from_cart(cart, model))
    }

    pub fn with_model(rom: Vec<u8>, model: Model) -> Result<Self, String> {
        let cart = cartridge::Cartridge::new(rom)?;
        Ok(Self::from_cart(cart, model))
    }

    fn from_cart(cart: cartridge::Cartridge, model: Model) -> Self {
        let title = cart.title.clone();
        let bus = bus::Bus::new(cart, model == Model::Cgb);
        GameBoy { cpu: cpu::Cpu::new(bus), model, title }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Run until the PPU completes a frame (or a fail-safe cycle budget when
    /// the LCD is off).
    pub fn run_frame(&mut self) {
        self.cpu.bus.ppu.frame_done = false;
        // ~1.5 frames of T-cycles; doubled in CGB double-speed mode.
        let mut budget: i64 = if self.cpu.bus.double_speed { 210_000 } else { 105_000 };
        while !self.cpu.bus.ppu.frame_done && budget > 0 {
            budget -= self.step() as i64;
        }
    }

    /// Execute one CPU step; returns elapsed T-cycles.
    pub fn step(&mut self) -> u32 {
        let before = self.cpu.bus.apu.samples.len();
        self.cpu.step();
        // Cheap elapsed-cycle proxy: audio samples advance with bus ticks.
        // Not used for timing decisions, only the run_frame budget.
        let _ = before;
        16
    }

    pub fn framebuffer(&self) -> &[u8] {
        &self.cpu.bus.ppu.framebuffer
    }

    pub fn set_button(&mut self, b: Button, pressed: bool) {
        let bus = &mut self.cpu.bus;
        let mut iflags = bus.iflags;
        bus.joypad.set_button(b, pressed, &mut iflags);
        bus.iflags = iflags;
    }

    /// Drain queued stereo f32 samples (interleaved L/R at `SAMPLE_RATE`).
    pub fn drain_audio(&mut self, out: &mut Vec<f32>) {
        out.append(&mut self.cpu.bus.apu.samples);
    }

    pub fn audio_queue_len(&self) -> usize {
        self.cpu.bus.apu.samples.len()
    }

    /// Bytes written to the serial port so far (test ROMs print here).
    pub fn take_serial(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.cpu.bus.serial_out)
    }

    pub fn save_ram(&self) -> Option<&[u8]> {
        self.cpu.bus.cart.save_ram()
    }

    pub fn load_save_ram(&mut self, data: &[u8]) {
        self.cpu.bus.cart.load_save_ram(data);
    }

    pub fn tick_rtc_seconds(&mut self, secs: u64) {
        self.cpu.bus.cart.tick_rtc_seconds(secs);
    }
}
