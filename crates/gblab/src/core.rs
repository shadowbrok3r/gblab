//! Emulator core dispatch: Game Boy (gb-core) or Game Boy Advance (gba-core).

use std::path::Path;

use crate::input::{self, ButtonStates};

/// GBA buttons in [`ButtonStates`] index order.
const GBA_BUTTONS: [gba_core::Button; 10] = [
    gba_core::Button::Right,
    gba_core::Button::Left,
    gba_core::Button::Up,
    gba_core::Button::Down,
    gba_core::Button::A,
    gba_core::Button::B,
    gba_core::Button::Select,
    gba_core::Button::Start,
    gba_core::Button::L,
    gba_core::Button::R,
];

pub enum Core {
    Gb(gb_core::GameBoy),
    Gba(gba_core::GameBoyAdvance),
}

impl Core {
    /// Picks the core by extension: .gba -> GBA, anything else -> GB.
    pub fn new(rom: Vec<u8>, path: &Path) -> Result<Core, String> {
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        if ext.as_deref() == Some("gba") {
            gba_core::GameBoyAdvance::new(rom).map(Core::Gba)
        } else {
            gb_core::GameBoy::new(rom).map(Core::Gb)
        }
    }

    pub fn run_frame(&mut self) {
        match self {
            Core::Gb(gb) => gb.run_frame(),
            Core::Gba(gba) => gba.run_frame(),
        }
    }

    /// RGBA8 framebuffer, `screen_size()` pixels.
    pub fn framebuffer(&self) -> &[u8] {
        match self {
            Core::Gb(gb) => gb.framebuffer(),
            Core::Gba(gba) => gba.framebuffer(),
        }
    }

    pub fn screen_size(&self) -> (usize, usize) {
        match self {
            Core::Gb(_) => (gb_core::SCREEN_W, gb_core::SCREEN_H),
            Core::Gba(_) => (gba_core::SCREEN_W, gba_core::SCREEN_H),
        }
    }

    pub fn title(&self) -> String {
        match self {
            Core::Gb(gb) => gb.title().to_string(),
            Core::Gba(gba) => gba.title(),
        }
    }

    pub fn model_label(&self) -> &'static str {
        match self {
            Core::Gb(gb) => match gb.model {
                gb_core::Model::Dmg => "DMG",
                gb_core::Model::Cgb => "CGB",
            },
            Core::Gba(_) => "GBA",
        }
    }

    pub fn set_buttons(&mut self, s: &ButtonStates) {
        match self {
            Core::Gb(gb) => {
                for (i, &b) in input::ALL_BUTTONS.iter().enumerate() {
                    gb.set_button(b, s[i]);
                }
            }
            Core::Gba(gba) => {
                for (i, &b) in GBA_BUTTONS.iter().enumerate() {
                    gba.set_button(b, s[i]);
                }
            }
        }
    }

    /// Drain queued interleaved stereo f32 samples at `sample_rate()`.
    pub fn drain_audio(&mut self, out: &mut Vec<f32>) {
        match self {
            Core::Gb(gb) => gb.drain_audio(out),
            Core::Gba(gba) => gba.drain_audio(out),
        }
    }

    pub fn audio_queue_len(&self) -> usize {
        match self {
            Core::Gb(gb) => gb.audio_queue_len(),
            Core::Gba(gba) => gba.audio_queue_len(),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        match self {
            Core::Gb(_) => gb_core::SAMPLE_RATE,
            Core::Gba(_) => gba_core::SAMPLE_RATE,
        }
    }

    pub fn save_ram(&self) -> Option<&[u8]> {
        match self {
            Core::Gb(gb) => gb.save_ram(),
            Core::Gba(gba) => gba.save_ram(),
        }
    }

    pub fn load_save_ram(&mut self, data: &[u8]) {
        match self {
            Core::Gb(gb) => gb.load_save_ram(data),
            Core::Gba(gba) => gba.load_save_ram(data),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gb_rom(cart_type: u8, ram_code: u8, cgb: bool, title: &str) -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x134..0x134 + title.len()].copy_from_slice(title.as_bytes());
        if cgb {
            rom[0x143] = 0x80;
        }
        rom[0x147] = cart_type;
        rom[0x148] = 0;
        rom[0x149] = ram_code;
        rom
    }

    fn gba_rom(title: &str) -> Vec<u8> {
        let mut rom = vec![0u8; 0x100];
        rom[0xA0..0xA0 + title.len()].copy_from_slice(title.as_bytes());
        rom
    }

    #[test]
    fn gba_extension_picks_gba() {
        let core = Core::new(gba_rom("TEST"), Path::new("/roms/game.gba")).unwrap();
        assert!(matches!(core, Core::Gba(_)));
        assert_eq!(core.screen_size(), (240, 160));
        assert_eq!(core.model_label(), "GBA");
        assert_eq!(core.title(), "TEST");
    }

    #[test]
    fn gba_extension_is_case_insensitive() {
        let core = Core::new(gba_rom(""), Path::new("GAME.GBA")).unwrap();
        assert!(matches!(core, Core::Gba(_)));
    }

    #[test]
    fn other_extensions_pick_gb() {
        for p in ["game.gb", "game.gbc", "game.rom"] {
            let core = Core::new(gb_rom(0, 0, false, "HELLO"), Path::new(p)).unwrap();
            assert!(matches!(core, Core::Gb(_)), "{p}");
        }
    }

    #[test]
    fn empty_path_defaults_to_gb() {
        let core = Core::new(gb_rom(0, 0, false, ""), Path::new("")).unwrap();
        assert!(matches!(core, Core::Gb(_)));
        assert_eq!(core.screen_size(), (160, 144));
    }

    #[test]
    fn gb_model_labels() {
        let dmg = Core::new(gb_rom(0, 0, false, ""), Path::new("a.gb")).unwrap();
        assert_eq!(dmg.model_label(), "DMG");
        let cgb = Core::new(gb_rom(0, 0, true, ""), Path::new("a.gbc")).unwrap();
        assert_eq!(cgb.model_label(), "CGB");
    }

    #[test]
    fn gb_title_passthrough() {
        let core = Core::new(gb_rom(0, 0, false, "HELLO"), Path::new("a.gb")).unwrap();
        assert_eq!(core.title(), "HELLO");
    }

    #[test]
    fn errors_propagate_from_both_cores() {
        assert!(Core::new(vec![0; 0x10], Path::new("a.gba")).is_err());
        assert!(Core::new(vec![0; 0x10], Path::new("a.gb")).is_err());
    }

    #[test]
    fn gb_frame_and_framebuffer_size() {
        let mut core = Core::new(gb_rom(0, 0, false, ""), Path::new("a.gb")).unwrap();
        core.run_frame();
        let (w, h) = core.screen_size();
        assert_eq!(core.framebuffer().len(), w * h * 4);
    }

    #[test]
    fn gba_framebuffer_size() {
        let core = Core::new(gba_rom(""), Path::new("a.gba")).unwrap();
        let (w, h) = core.screen_size();
        assert_eq!(core.framebuffer().len(), w * h * 4);
    }

    #[test]
    fn set_buttons_accepts_all_ten_on_both_cores() {
        let mut gb = Core::new(gb_rom(0, 0, false, ""), Path::new("a.gb")).unwrap();
        gb.set_buttons(&[true; 10]);
        gb.set_buttons(&[false; 10]);
        let mut gba = Core::new(gba_rom(""), Path::new("a.gba")).unwrap();
        gba.set_buttons(&[true; 10]);
        gba.set_buttons(&[false; 10]);
    }

    #[test]
    fn sample_rate_is_48khz_on_both() {
        let gb = Core::new(gb_rom(0, 0, false, ""), Path::new("a.gb")).unwrap();
        assert_eq!(gb.sample_rate(), 48_000);
        let gba = Core::new(gba_rom(""), Path::new("a.gba")).unwrap();
        assert_eq!(gba.sample_rate(), 48_000);
    }

    #[test]
    fn gb_save_ram_roundtrip() {
        // MBC1+RAM+BATTERY with 8 KiB RAM.
        let mut core = Core::new(gb_rom(0x03, 2, false, ""), Path::new("a.gb")).unwrap();
        let data = vec![0xAB; 0x2000];
        core.load_save_ram(&data);
        let ram = core.save_ram().expect("battery RAM present");
        assert_eq!(ram.len(), 0x2000);
        assert!(ram.iter().all(|&b| b == 0xAB));
    }

    #[test]
    fn gb_without_battery_has_no_save_ram() {
        let core = Core::new(gb_rom(0, 0, false, ""), Path::new("a.gb")).unwrap();
        assert!(core.save_ram().is_none());
    }

    #[test]
    fn drain_audio_appends() {
        let mut core = Core::new(gb_rom(0, 0, false, ""), Path::new("a.gb")).unwrap();
        core.run_frame();
        let mut out = vec![1.0f32];
        core.drain_audio(&mut out);
        assert!(!out.is_empty());
        assert_eq!(out[0], 1.0);
        assert_eq!(core.audio_queue_len(), 0);
    }
}
