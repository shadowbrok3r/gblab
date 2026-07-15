//! Game Boy Advance emulator core with no platform dependencies.

mod apu;
mod bus;
mod cpu;
mod dma;
mod ppu;
mod timer;

pub use apu::SAMPLE_RATE;
pub use bus::Button;
pub use ppu::{SCREEN_H, SCREEN_W};

pub struct GameBoyAdvance {
    cpu: cpu::Arm7,
}

impl GameBoyAdvance {
    /// Boots past the BIOS: PC at ROM entry, banked stacks preset, SYS mode.
    pub fn new(rom: Vec<u8>) -> Result<Self, String> {
        if rom.len() < 0xC0 {
            return Err("ROM too small for a GBA header".into());
        }
        let bus = bus::Bus::new(rom);
        Ok(GameBoyAdvance { cpu: cpu::Arm7::new(bus) })
    }

    /// Run until the PPU completes a frame.
    pub fn run_frame(&mut self) {
        self.cpu.bus.ppu.frame_done = false;
        // ~1.5 frames of cycles as a fail-safe budget.
        let mut budget: i64 = 420_000;
        while !self.cpu.bus.ppu.frame_done && budget > 0 {
            budget -= self.cpu.step() as i64;
        }
    }

    /// RGBA8 framebuffer, `SCREEN_W` x `SCREEN_H`.
    pub fn framebuffer(&self) -> &[u8] {
        &self.cpu.bus.ppu.framebuffer
    }

    pub fn set_button(&mut self, b: Button, pressed: bool) {
        self.cpu.bus.set_button(b, pressed);
    }

    /// Address of the currently executing instruction (for tests/tools).
    pub fn debug_pc(&self) -> u32 {
        self.cpu.exec_pc()
    }

    pub fn debug_reg(&self, i: usize) -> u32 {
        self.cpu.reg(i)
    }

    /// Read any bus address without side effects on timing (for tests/tools).
    pub fn debug_read8(&mut self, addr: u32) -> u8 {
        self.cpu.bus.read8(addr)
    }

    /// Game title from the cartridge header.
    pub fn title(&self) -> String {
        let raw = &self.cpu.bus.rom[0xA0..0xAC];
        raw.iter()
            .take_while(|&&b| b != 0)
            .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '?' })
            .collect()
    }

    /// Drain queued stereo f32 samples (interleaved L/R at `SAMPLE_RATE`).
    pub fn drain_audio(&mut self, out: &mut Vec<f32>) {
        out.append(&mut self.cpu.bus.apu.samples);
    }

    pub fn audio_queue_len(&self) -> usize {
        self.cpu.bus.apu.samples.len()
    }

    /// Battery-backed SRAM when the header advertises one.
    pub fn save_ram(&self) -> Option<&[u8]> {
        self.cpu.bus.has_sram.then_some(self.cpu.bus.sram.as_slice())
    }

    pub fn load_save_ram(&mut self, data: &[u8]) {
        let n = data.len().min(self.cpu.bus.sram.len());
        self.cpu.bus.sram[..n].copy_from_slice(&data[..n]);
    }
}
