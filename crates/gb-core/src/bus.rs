//! System bus: memory map, DMA, serial, and CGB speed/banking registers.

use crate::apu::Apu;
use crate::cartridge::Cartridge;
use crate::joypad::Joypad;
use crate::ppu::Ppu;
use crate::timer::Timer;

const INT_SERIAL: u8 = 1 << 3;

pub struct Bus {
    pub cart: Cartridge,
    pub ppu: Ppu,
    pub apu: Apu,
    pub timer: Timer,
    pub joypad: Joypad,
    wram: [u8; 0x8000],
    wram_bank: u8,
    hram: [u8; 0x7F],
    pub ie: u8,
    pub iflags: u8,
    pub cgb: bool,
    pub double_speed: bool,
    speed_switch_armed: bool,

    // Serial port.
    sb: u8,
    sc: u8,
    serial_countdown: u32,
    pub serial_out: Vec<u8>,

    // OAM DMA.
    dma_src: u16,
    dma_index: u16,
    dma_active: bool,

    // CGB HDMA.
    hdma_src: u16,
    hdma_dst: u16,
    hdma_len: u16,
    hdma_hblank: bool,
    hdma_active: bool,
    hdma_prev_hblank: bool,
}

impl Bus {
    pub fn new(cart: Cartridge, cgb: bool) -> Self {
        Bus {
            ppu: Ppu::new(cgb),
            apu: Apu::new(),
            timer: Timer::new(),
            joypad: Joypad::new(),
            cart,
            wram: [0; 0x8000],
            wram_bank: 1,
            hram: [0; 0x7F],
            ie: 0,
            iflags: 0xE1 & 0x1F,
            cgb,
            double_speed: false,
            speed_switch_armed: false,
            sb: 0,
            sc: 0x7E,
            serial_countdown: 0,
            serial_out: Vec::new(),
            dma_src: 0,
            dma_index: 0xA0,
            dma_active: false,
            hdma_src: 0,
            hdma_dst: 0,
            hdma_len: 0,
            hdma_hblank: false,
            hdma_active: false,
            hdma_prev_hblank: false,
        }
    }

    /// Advance all components by CPU T-cycles (multiple of 4).
    pub fn tick(&mut self, t: u32) {
        self.timer.tick(t, &mut self.iflags);

        if self.serial_countdown > 0 {
            self.serial_countdown = self.serial_countdown.saturating_sub(t);
            if self.serial_countdown == 0 {
                self.sb = 0xFF;
                self.sc &= !0x80;
                self.iflags |= INT_SERIAL;
            }
        }

        if self.dma_active {
            for _ in 0..t / 4 {
                if self.dma_index < 0xA0 {
                    let v = self.dma_read(self.dma_src + self.dma_index);
                    self.ppu.oam[self.dma_index as usize] = v;
                    self.dma_index += 1;
                } else {
                    self.dma_active = false;
                    break;
                }
            }
        }

        let vid_t = if self.double_speed { t / 2 } else { t };
        self.ppu.tick(vid_t, &mut self.iflags);
        self.apu.tick(vid_t);

        if self.hdma_active && self.hdma_hblank {
            let now = self.ppu.in_hblank();
            if now && !self.hdma_prev_hblank {
                self.hdma_copy_block();
            }
            self.hdma_prev_hblank = now;
        }
    }

    fn dma_read(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.cart.read(addr),
            0x8000..=0x9FFF => self.ppu.read_vram(addr),
            0xA000..=0xBFFF => self.cart.read_ram(addr),
            0xC000..=0xCFFF => self.wram[(addr & 0x0FFF) as usize],
            0xD000..=0xDFFF => {
                self.wram[self.wram_bank as usize * 0x1000 + (addr & 0x0FFF) as usize]
            }
            _ => 0xFF,
        }
    }

    fn hdma_copy_block(&mut self) {
        for _ in 0..16 {
            let v = self.dma_read(self.hdma_src);
            self.ppu.hdma_write(self.hdma_dst, v);
            self.hdma_src = self.hdma_src.wrapping_add(1);
            self.hdma_dst = 0x8000 | ((self.hdma_dst + 1) & 0x1FFF);
        }
        self.hdma_len -= 1;
        if self.hdma_len == 0 {
            self.hdma_active = false;
        }
    }

    pub fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.cart.read(addr),
            0x8000..=0x9FFF => self.ppu.read_vram(addr),
            0xA000..=0xBFFF => self.cart.read_ram(addr),
            0xC000..=0xCFFF => self.wram[(addr & 0x0FFF) as usize],
            0xD000..=0xDFFF => {
                self.wram[self.wram_bank as usize * 0x1000 + (addr & 0x0FFF) as usize]
            }
            0xE000..=0xFDFF => self.read(addr - 0x2000),
            0xFE00..=0xFE9F => self.ppu.oam[(addr - 0xFE00) as usize],
            0xFEA0..=0xFEFF => 0x00,
            0xFF00 => self.joypad.read(),
            0xFF01 => self.sb,
            0xFF02 => self.sc | if self.cgb { 0x7C } else { 0x7E },
            0xFF04..=0xFF07 => self.timer.read(addr),
            0xFF0F => 0xE0 | self.iflags,
            0xFF10..=0xFF3F => self.apu.read(addr),
            0xFF46 => (self.dma_src >> 8) as u8,
            0xFF4D if self.cgb => {
                (if self.double_speed { 0x80 } else { 0 })
                    | (if self.speed_switch_armed { 0x01 } else { 0 })
                    | 0x7E
            }
            0xFF51..=0xFF54 if self.cgb => 0xFF,
            0xFF55 if self.cgb => {
                if self.hdma_active {
                    (self.hdma_len - 1) as u8 & 0x7F
                } else {
                    0xFF
                }
            }
            0xFF70 if self.cgb => self.wram_bank | 0xF8,
            0xFF40..=0xFF6C => self.ppu.read_reg(addr),
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],
            0xFFFF => self.ie,
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        match addr {
            0x0000..=0x7FFF => self.cart.write(addr, v),
            0x8000..=0x9FFF => self.ppu.write_vram(addr, v),
            0xA000..=0xBFFF => self.cart.write_ram(addr, v),
            0xC000..=0xCFFF => self.wram[(addr & 0x0FFF) as usize] = v,
            0xD000..=0xDFFF => {
                self.wram[self.wram_bank as usize * 0x1000 + (addr & 0x0FFF) as usize] = v
            }
            0xE000..=0xFDFF => self.write(addr - 0x2000, v),
            0xFE00..=0xFE9F => self.ppu.oam[(addr - 0xFE00) as usize] = v,
            0xFEA0..=0xFEFF => {}
            0xFF00 => self.joypad.write(v),
            0xFF01 => self.sb = v,
            0xFF02 => {
                self.sc = v & 0x83;
                if v & 0x80 != 0 && v & 0x01 != 0 {
                    // Internal clock: byte leaves the port; no link partner.
                    self.serial_out.push(self.sb);
                    self.serial_countdown = 8 * 512;
                }
            }
            0xFF04..=0xFF07 => self.timer.write(addr, v),
            0xFF0F => self.iflags = v & 0x1F,
            0xFF10..=0xFF3F => self.apu.write(addr, v),
            0xFF46 => {
                self.dma_src = (v as u16) << 8;
                self.dma_index = 0;
                self.dma_active = true;
            }
            0xFF4D if self.cgb => self.speed_switch_armed = v & 0x01 != 0,
            0xFF51 if self.cgb => self.hdma_src = (self.hdma_src & 0x00FF) | ((v as u16) << 8),
            0xFF52 if self.cgb => self.hdma_src = (self.hdma_src & 0xFF00) | (v as u16 & 0xF0),
            0xFF53 if self.cgb => {
                self.hdma_dst = 0x8000 | ((v as u16 & 0x1F) << 8) | (self.hdma_dst & 0x00F0)
            }
            0xFF54 if self.cgb => {
                self.hdma_dst = 0x8000 | (self.hdma_dst & 0x1F00) | (v as u16 & 0xF0)
            }
            0xFF55 if self.cgb => {
                if self.hdma_active && v & 0x80 == 0 {
                    self.hdma_active = false;
                    return;
                }
                self.hdma_len = (v as u16 & 0x7F) + 1;
                self.hdma_hblank = v & 0x80 != 0;
                if self.hdma_hblank {
                    self.hdma_active = true;
                    self.hdma_prev_hblank = self.ppu.in_hblank();
                    if self.hdma_prev_hblank {
                        self.hdma_copy_block();
                    }
                } else {
                    while self.hdma_len > 0 {
                        self.hdma_len -= 1;
                        for _ in 0..16 {
                            let b = self.dma_read(self.hdma_src);
                            self.ppu.hdma_write(self.hdma_dst, b);
                            self.hdma_src = self.hdma_src.wrapping_add(1);
                            self.hdma_dst = 0x8000 | ((self.hdma_dst + 1) & 0x1FFF);
                        }
                    }
                }
            }
            0xFF70 if self.cgb => self.wram_bank = (v & 0x07).max(1),
            0xFF40..=0xFF6C => {
                let mut iflags = self.iflags;
                self.ppu.write_reg(addr, v, &mut iflags);
                self.iflags = iflags;
            }
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = v,
            0xFFFF => self.ie = v,
            _ => {}
        }
    }

    /// STOP with a speed switch armed (CGB): toggle speed, reset DIV.
    pub fn perform_speed_switch(&mut self) -> bool {
        if self.cgb && self.speed_switch_armed {
            self.double_speed = !self.double_speed;
            self.speed_switch_armed = false;
            self.timer.write(0xFF04, 0);
            true
        } else {
            false
        }
    }
}
