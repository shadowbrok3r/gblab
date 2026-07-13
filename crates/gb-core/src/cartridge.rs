//! Cartridge loading and MBC (memory bank controller) emulation.

enum Mbc {
    None,
    Mbc1 { bank1: u8, bank2: u8, mode: bool, ram_enable: bool },
    Mbc2 { rom_bank: u8, ram_enable: bool },
    Mbc3 { rom_bank: u8, map: u8, ram_enable: bool, rtc: Rtc },
    Mbc5 { rom_bank: u16, ram_bank: u8, ram_enable: bool },
}

/// MBC3 real-time clock. Advanced manually via `Cartridge::tick_rtc_seconds`.
struct Rtc {
    seconds: u8,
    minutes: u8,
    hours: u8,
    days: u16,
    halted: bool,
    latched: [u8; 5],
    latch_armed: bool,
}

impl Rtc {
    fn new() -> Self {
        Rtc { seconds: 0, minutes: 0, hours: 0, days: 0, halted: false, latched: [0; 5], latch_armed: false }
    }

    fn latch(&mut self) {
        self.latched = [
            self.seconds,
            self.minutes,
            self.hours,
            (self.days & 0xFF) as u8,
            ((self.days >> 8) as u8 & 0x01)
                | if self.halted { 0x40 } else { 0 }
                | if self.days > 0x1FF { 0x80 } else { 0 },
        ];
    }

    fn read(&self, reg: u8) -> u8 {
        match reg {
            0x08..=0x0C => self.latched[(reg - 0x08) as usize],
            _ => 0xFF,
        }
    }

    fn write(&mut self, reg: u8, v: u8) {
        match reg {
            0x08 => self.seconds = v & 0x3F,
            0x09 => self.minutes = v & 0x3F,
            0x0A => self.hours = v & 0x1F,
            0x0B => self.days = (self.days & 0x100) | v as u16,
            0x0C => {
                self.days = (self.days & 0xFF) | ((v as u16 & 0x01) << 8);
                self.halted = v & 0x40 != 0;
            }
            _ => {}
        }
    }
}

pub struct Cartridge {
    rom: Vec<u8>,
    ram: Vec<u8>,
    mbc: Mbc,
    pub has_battery: bool,
    pub title: String,
    pub cgb: bool,
    rom_bank_mask: u16,
}

impl Cartridge {
    pub fn new(rom: Vec<u8>) -> Result<Self, String> {
        if rom.len() < 0x150 {
            return Err("ROM too small to contain a cartridge header".into());
        }
        let title = rom[0x134..0x143]
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' })
            .collect();
        let cgb = rom[0x143] & 0x80 != 0;
        let cart_type = rom[0x147];
        let rom_banks: u16 = match rom[0x148] {
            n @ 0..=8 => 2 << n,
            _ => return Err(format!("unsupported ROM size code {:#04x}", rom[0x148])),
        };
        let ram_size = match rom[0x149] {
            0 => 0,
            2 => 0x2000,
            3 => 0x8000,
            4 => 0x20000,
            5 => 0x10000,
            _ => 0,
        };
        let (mbc, has_battery) = match cart_type {
            0x00 | 0x08 | 0x09 => (Mbc::None, cart_type == 0x09),
            0x01..=0x03 => (
                Mbc::Mbc1 { bank1: 1, bank2: 0, mode: false, ram_enable: false },
                cart_type == 0x03,
            ),
            0x05 | 0x06 => (Mbc::Mbc2 { rom_bank: 1, ram_enable: false }, cart_type == 0x06),
            0x0F..=0x13 => (
                Mbc::Mbc3 { rom_bank: 1, map: 0, ram_enable: false, rtc: Rtc::new() },
                matches!(cart_type, 0x0F | 0x10 | 0x13),
            ),
            0x19..=0x1E => (
                Mbc::Mbc5 { rom_bank: 1, ram_bank: 0, ram_enable: false },
                matches!(cart_type, 0x1B | 0x1E),
            ),
            other => return Err(format!("unsupported cartridge type {:#04x}", other)),
        };
        let ram_len = if matches!(mbc, Mbc::Mbc2 { .. }) { 512 } else { ram_size };
        Ok(Cartridge {
            rom,
            ram: vec![0xFF; ram_len],
            mbc,
            has_battery,
            title,
            cgb,
            rom_bank_mask: rom_banks - 1,
        })
    }

    fn rom_at(&self, bank: u16, offset: u16) -> u8 {
        let idx = ((bank & self.rom_bank_mask) as usize) * 0x4000 + offset as usize;
        self.rom.get(idx).copied().unwrap_or(0xFF)
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x3FFF => match &self.mbc {
                Mbc::Mbc1 { bank2, mode: true, .. } => self.rom_at((*bank2 as u16) << 5, addr),
                _ => self.rom.get(addr as usize).copied().unwrap_or(0xFF),
            },
            0x4000..=0x7FFF => {
                let bank = match &self.mbc {
                    Mbc::None => 1,
                    Mbc::Mbc1 { bank1, bank2, .. } => ((*bank2 as u16) << 5) | *bank1 as u16,
                    Mbc::Mbc2 { rom_bank, .. } => *rom_bank as u16,
                    Mbc::Mbc3 { rom_bank, .. } => *rom_bank as u16,
                    Mbc::Mbc5 { rom_bank, .. } => *rom_bank,
                };
                self.rom_at(bank, addr - 0x4000)
            }
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        match &mut self.mbc {
            Mbc::None => {}
            Mbc::Mbc1 { bank1, bank2, mode, ram_enable } => match addr {
                0x0000..=0x1FFF => *ram_enable = v & 0x0F == 0x0A,
                0x2000..=0x3FFF => {
                    *bank1 = (v & 0x1F).max(1);
                }
                0x4000..=0x5FFF => *bank2 = v & 0x03,
                0x6000..=0x7FFF => *mode = v & 0x01 != 0,
                _ => {}
            },
            Mbc::Mbc2 { rom_bank, ram_enable } => {
                if addr <= 0x3FFF {
                    if addr & 0x0100 == 0 {
                        *ram_enable = v & 0x0F == 0x0A;
                    } else {
                        *rom_bank = (v & 0x0F).max(1);
                    }
                }
            }
            Mbc::Mbc3 { rom_bank, map, ram_enable, rtc } => match addr {
                0x0000..=0x1FFF => *ram_enable = v & 0x0F == 0x0A,
                0x2000..=0x3FFF => *rom_bank = (v & 0x7F).max(1),
                0x4000..=0x5FFF => *map = v & 0x0F,
                0x6000..=0x7FFF => {
                    if rtc.latch_armed && v == 0x01 {
                        rtc.latch();
                    }
                    rtc.latch_armed = v == 0x00;
                }
                _ => {}
            },
            Mbc::Mbc5 { rom_bank, ram_bank, ram_enable } => match addr {
                0x0000..=0x1FFF => *ram_enable = v & 0x0F == 0x0A,
                0x2000..=0x2FFF => *rom_bank = (*rom_bank & 0x100) | v as u16,
                0x3000..=0x3FFF => *rom_bank = (*rom_bank & 0xFF) | ((v as u16 & 0x01) << 8),
                0x4000..=0x5FFF => *ram_bank = v & 0x0F,
                _ => {}
            },
        }
    }

    fn ram_index(&self, addr: u16) -> Option<usize> {
        let offset = (addr - 0xA000) as usize;
        let idx = match &self.mbc {
            Mbc::None => offset,
            Mbc::Mbc1 { bank2, mode, .. } => {
                if *mode { (*bank2 as usize) * 0x2000 + offset } else { offset }
            }
            Mbc::Mbc2 { .. } => offset & 0x1FF,
            Mbc::Mbc3 { map, .. } if *map <= 0x03 => (*map as usize) * 0x2000 + offset,
            Mbc::Mbc3 { .. } => return None,
            Mbc::Mbc5 { ram_bank, .. } => (*ram_bank as usize) * 0x2000 + offset,
        };
        if idx < self.ram.len() { Some(idx) } else { None }
    }

    pub fn read_ram(&self, addr: u16) -> u8 {
        match &self.mbc {
            Mbc::Mbc1 { ram_enable: false, .. }
            | Mbc::Mbc2 { ram_enable: false, .. }
            | Mbc::Mbc3 { ram_enable: false, .. }
            | Mbc::Mbc5 { ram_enable: false, .. } => return 0xFF,
            Mbc::Mbc3 { map, rtc, ram_enable: true, .. } if *map >= 0x08 => return rtc.read(*map),
            _ => {}
        }
        match self.ram_index(addr) {
            // MBC2 has 4-bit RAM; upper nibble reads open-bus.
            Some(i) => {
                if matches!(self.mbc, Mbc::Mbc2 { .. }) { self.ram[i] | 0xF0 } else { self.ram[i] }
            }
            None => 0xFF,
        }
    }

    pub fn write_ram(&mut self, addr: u16, v: u8) {
        match &mut self.mbc {
            Mbc::Mbc1 { ram_enable: false, .. }
            | Mbc::Mbc2 { ram_enable: false, .. }
            | Mbc::Mbc3 { ram_enable: false, .. }
            | Mbc::Mbc5 { ram_enable: false, .. } => return,
            Mbc::Mbc3 { map, rtc, ram_enable: true, .. } if *map >= 0x08 => {
                let reg = *map;
                rtc.write(reg, v);
                return;
            }
            _ => {}
        }
        if let Some(i) = self.ram_index(addr) {
            self.ram[i] = v;
        }
    }

    /// Advance the MBC3 RTC by whole seconds of wall-clock time.
    pub fn tick_rtc_seconds(&mut self, secs: u64) {
        if let Mbc::Mbc3 { rtc, .. } = &mut self.mbc {
            if rtc.halted {
                return;
            }
            for _ in 0..secs {
                rtc.seconds += 1;
                if rtc.seconds == 60 {
                    rtc.seconds = 0;
                    rtc.minutes += 1;
                    if rtc.minutes == 60 {
                        rtc.minutes = 0;
                        rtc.hours += 1;
                        if rtc.hours == 24 {
                            rtc.hours = 0;
                            rtc.days += 1;
                        }
                    }
                }
            }
        }
    }

    pub fn save_ram(&self) -> Option<&[u8]> {
        if self.has_battery && !self.ram.is_empty() { Some(&self.ram) } else { None }
    }

    pub fn load_save_ram(&mut self, data: &[u8]) {
        let n = data.len().min(self.ram.len());
        self.ram[..n].copy_from_slice(&data[..n]);
    }
}
