//! System bus: memory map, open bus, keypad, and IO dispatch.

use crate::apu::Apu;
use crate::dma::Dma;
use crate::ppu::Ppu;
use crate::timer::Timers;

/// KEYINPUT bit order (0 = pressed on hardware; inverted internally).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    A = 0,
    B = 1,
    Select = 2,
    Start = 3,
    Right = 4,
    Left = 5,
    Up = 6,
    Down = 7,
    R = 8,
    L = 9,
}

pub struct Bus {
    bios: Vec<u8>,
    ewram: Vec<u8>,
    iwram: Vec<u8>,
    pub(crate) rom: Vec<u8>,
    pub(crate) sram: Vec<u8>,
    pub(crate) has_sram: bool,
    pub ppu: Ppu,
    pub apu: Apu,
    pub dma: Dma,
    pub timers: Timers,
    keys: u16,
    keycnt: u16,
    pub ime: bool,
    pub ie_reg: u16,
    pub if_reg: u16,
    pub halted: bool,
    waitcnt: u16,
    /// Last value driven onto the bus, returned for unmapped reads.
    pub open_bus: u32,
    pub cycles: u64,
}

impl Bus {
    pub fn new(rom: Vec<u8>) -> Self {
        let has_sram = rom
            .windows(6)
            .any(|w| w == b"SRAM_V" || w == b"FLASH_" || w == b"FLASH5" || w == b"FLASH1");
        Bus {
            bios: stub_bios(),
            ewram: vec![0; 256 * 1024],
            iwram: vec![0; 32 * 1024],
            rom,
            sram: vec![0xFF; 64 * 1024],
            has_sram,
            ppu: Ppu::new(),
            apu: Apu::new(),
            dma: Dma::new(),
            timers: Timers::new(),
            keys: 0,
            keycnt: 0,
            ime: false,
            ie_reg: 0,
            if_reg: 0,
            halted: false,
            waitcnt: 0,
            open_bus: 0,
            cycles: 0,
        }
    }

    #[cfg(test)]
    pub fn new_test(rom: Vec<u8>) -> Self {
        Bus::new(rom)
    }

    pub fn set_button(&mut self, b: Button, pressed: bool) {
        let bit = 1u16 << (b as u16);
        if pressed {
            self.keys |= bit;
        } else {
            self.keys &= !bit;
        }
        // KEYCNT interrupt: logical AND (bit15) or OR of the selected keys.
        if self.keycnt & 0x4000 != 0 {
            let mask = self.keycnt & 0x03FF;
            let hit = if self.keycnt & 0x8000 != 0 {
                mask != 0 && self.keys & mask == mask
            } else {
                self.keys & mask != 0
            };
            if hit {
                self.if_reg |= 1 << 12;
            }
        }
    }

    /// Internal (non-memory) CPU cycle.
    pub fn idle(&mut self) {
        self.tick(1);
    }

    fn tick(&mut self, n: u64) {
        self.cycles += n;
        self.ppu.tick(n, &mut self.if_reg, &mut self.dma);
        let overflow = self.timers.tick(n, &mut self.if_reg);
        if overflow != 0 {
            self.apu.timer_overflow(overflow, &mut self.dma);
        }
        self.apu.tick(n);
    }

    pub fn read8(&mut self, addr: u32) -> u8 {
        let v = self.read32_raw(addr & !3);
        (v >> (8 * (addr & 3))) as u8
    }

    /// Halfword-aligned read; CPU applies unaligned rotation itself.
    pub fn read16(&mut self, addr: u32) -> u16 {
        let v = self.read32_raw(addr & !3);
        (v >> (8 * (addr & 2))) as u16
    }

    /// Word-aligned read; CPU applies unaligned rotation itself.
    pub fn read32(&mut self, addr: u32) -> u32 {
        self.read32_raw(addr & !3)
    }

    fn read32_raw(&mut self, addr: u32) -> u32 {
        self.tick(1);
        let v = self.load32(addr);
        self.open_bus = v;
        v
    }

    /// Untimed aligned read with no open-bus update (RMW composition, debug).
    fn load32(&mut self, addr: u32) -> u32 {
        match addr >> 24 {
            0x00 => {
                if (addr as usize) < self.bios.len() {
                    word(&self.bios, addr as usize)
                } else {
                    self.open_bus
                }
            }
            0x02 => word(&self.ewram, (addr as usize) & 0x3_FFFF),
            0x03 => word(&self.iwram, (addr as usize) & 0x7FFF),
            0x04 => {
                let a = addr & 0x00FF_FFFC;
                self.io_read16(a) as u32 | (self.io_read16(a + 2) as u32) << 16
            }
            0x05 => word(&self.ppu.palette, (addr as usize) & 0x3FF),
            0x06 => word(&self.ppu.vram, vram_index(addr)),
            0x07 => word(&self.ppu.oam, (addr as usize) & 0x3FF),
            0x08..=0x0D => {
                let i = (addr as usize) & 0x01FF_FFFF;
                if i + 3 < self.rom.len() {
                    word(&self.rom, i)
                } else {
                    // Out-of-bounds cartridge reads return the address bus value.
                    let lo = ((addr >> 1) & 0xFFFF) as u32;
                    lo | (lo.wrapping_add(1) << 16)
                }
            }
            0x0E | 0x0F => {
                let b = self.sram[(addr as usize) & 0xFFFF] as u32;
                b * 0x0101_0101
            }
            _ => self.open_bus,
        }
    }

    pub fn write8(&mut self, addr: u32, v: u8) {
        match addr >> 24 {
            // Byte writes to palette/VRAM duplicate into the halfword.
            0x05 | 0x06 => self.write16(addr & !1, v as u16 * 0x0101),
            0x07 => {} // OAM ignores byte writes.
            0x0E | 0x0F => {
                self.tick(1);
                self.sram[(addr as usize) & 0xFFFF] = v;
            }
            _ => {
                let sh = 8 * (addr & 3);
                let cur = self.load32(addr & !3);
                let nv = (cur & !(0xFF << sh)) | ((v as u32) << sh);
                self.write32(addr & !3, nv);
            }
        }
    }

    pub fn write16(&mut self, addr: u32, v: u16) {
        let addr = addr & !1;
        let sh = 8 * (addr & 2);
        match addr >> 24 {
            0x04 => {
                self.tick(1);
                self.io_write16(addr & 0x00FF_FFFE, v);
            }
            _ => {
                let cur = self.load32(addr & !3);
                let nv = (cur & !(0xFFFFu32 << sh)) | ((v as u32) << sh);
                self.write32(addr & !3, nv);
            }
        }
    }

    pub fn write32(&mut self, addr: u32, v: u32) {
        let addr = addr & !3;
        self.tick(1);
        self.open_bus = v;
        match addr >> 24 {
            0x02 => set_word(&mut self.ewram, (addr as usize) & 0x3_FFFF, v),
            0x03 => set_word(&mut self.iwram, (addr as usize) & 0x7FFF, v),
            0x04 => {
                self.io_write16(addr & 0x00FF_FFFC, v as u16);
                self.io_write16((addr & 0x00FF_FFFC) + 2, (v >> 16) as u16);
            }
            0x05 => set_word(&mut self.ppu.palette, (addr as usize) & 0x3FF, v),
            0x06 => set_word(&mut self.ppu.vram, vram_index(addr), v),
            0x07 => set_word(&mut self.ppu.oam, (addr as usize) & 0x3FF, v),
            0x0E | 0x0F => self.sram[(addr as usize) & 0xFFFF] = v as u8,
            _ => {}
        }
    }

    fn io_read16(&mut self, addr: u32) -> u16 {
        match addr {
            0x000..=0x05E => self.ppu.io_read16(addr),
            0x060..=0x0AE => self.apu.read16(addr),
            0x0B0..=0x0DE => self.dma.read16(addr),
            0x100..=0x10E => self.timers.read16(addr),
            0x130 => 0x03FF & !self.keys,
            0x132 => self.keycnt,
            0x200 => self.ie_reg,
            0x202 => self.if_reg,
            0x204 => self.waitcnt,
            0x208 => self.ime as u16,
            _ => (self.open_bus >> (8 * (addr & 2))) as u16,
        }
    }

    fn io_write16(&mut self, addr: u32, v: u16) {
        match addr {
            0x000..=0x05E => self.ppu.io_write16(addr, v),
            0x060..=0x0AE => self.apu.write16(addr, v),
            0x0B0..=0x0DE => self.dma.write16(addr, v),
            0x100..=0x10E => self.timers.write16(addr, v),
            0x132 => self.keycnt = v,
            0x200 => self.ie_reg = v,
            0x202 => self.if_reg &= !v, // acknowledge by writing 1s
            0x204 => self.waitcnt = v,
            0x208 => self.ime = v & 1 != 0,
            0x300 => self.halted = true, // HALTCNT lives in the upper byte
            _ => {}
        }
    }
}

/// VRAM is 96K; the upper 32K maps twice in the 128K window.
fn vram_index(addr: u32) -> usize {
    let i = (addr as usize) & 0x1_FFFF;
    if i >= 0x1_8000 { i - 0x8000 } else { i }
}

fn word(mem: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([mem[i], mem[i + 1], mem[i + 2], mem[i + 3]])
}

fn set_word(mem: &mut [u8], i: usize, v: u32) {
    mem[i..i + 4].copy_from_slice(&v.to_le_bytes());
}

/// Exception vectors plus the real BIOS's IRQ dispatch sequence; SWIs that
/// reach the vector (not HLE'd) return immediately.
fn stub_bios() -> Vec<u8> {
    let mut b = vec![0u8; 16 * 1024];
    let put = |b: &mut Vec<u8>, at: usize, op: u32| b[at..at + 4].copy_from_slice(&op.to_le_bytes());
    put(&mut b, 0x00, 0xEAFF_FFFE); // reset: b .
    put(&mut b, 0x04, 0xE1B0_F00E); // undefined: movs pc, lr
    put(&mut b, 0x08, 0xE1B0_F00E); // swi: movs pc, lr
    put(&mut b, 0x0C, 0xE25E_F004); // prefetch abort: subs pc, lr, #4
    put(&mut b, 0x10, 0xE25E_F004); // data abort
    put(&mut b, 0x14, 0xE1B0_F00E); // reserved
    put(&mut b, 0x18, 0xEA00_0008); // irq: b 0x40
    put(&mut b, 0x1C, 0xE25E_F004); // fiq
    // IRQ dispatcher: calls the user handler at [0x03007FFC].
    put(&mut b, 0x40, 0xE92D_500F); // stmfd sp!, {r0-r3, r12, lr}
    put(&mut b, 0x44, 0xE3A0_0301); // mov r0, #0x04000000
    put(&mut b, 0x48, 0xE28F_E000); // add lr, pc, #0
    put(&mut b, 0x4C, 0xE510_F004); // ldr pc, [r0, #-4]
    put(&mut b, 0x50, 0xE8BD_500F); // ldmfd sp!, {r0-r3, r12, lr}
    put(&mut b, 0x54, 0xE25E_F004); // subs pc, lr, #4
    b
}
