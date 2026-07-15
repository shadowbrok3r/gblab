//! APU: PSG channels 1-4 plus the two DMA sound FIFOs (0x04000060-0x040000AE).

use crate::dma::Dma;
use std::collections::VecDeque;

pub const SAMPLE_RATE: u32 = 48_000;
const CPU_HZ: u64 = 16_777_216;
/// Full 4-channel PSG mix peaks at 0.25 full-scale.
const PSG_SCALE: f32 = 0.25 / 4.0;
/// Each FIFO peaks at 0.25 full-scale.
const FIFO_SCALE: f32 = 0.25;

const DUTY: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 1, 1, 1],
    [0, 1, 1, 1, 1, 1, 1, 0],
];

/// Readable bits per register, indexed from offset 0x60 in halfword steps.
const READ_MASK: [u16; 24] = [
    0x007F, 0xFFC0, 0x4000, 0, // 60 62 64 66
    0xFFC0, 0, 0x4000, 0, // 68 6A 6C 6E
    0x00E0, 0xE000, 0x4000, 0, // 70 72 74 76
    0xFF00, 0, 0x40FF, 0, // 78 7A 7C 7E
    0xFF77, 0x770F, 0, 0, // 80 82 84 86
    0xC3FE, 0, 0, 0, // 88 8A 8C 8E
];

fn reg_idx(addr: u32) -> usize {
    ((addr - 0x60) >> 1) as usize
}

#[derive(Default)]
struct Square {
    sweep_enabled: bool,
    sweep_period: u8,
    sweep_negate: bool,
    sweep_shift: u8,
    sweep_timer: u8,
    sweep_shadow: u16,
    duty: u8,
    length: u16,
    length_enable: bool,
    env_start_vol: u8,
    env_add: bool,
    env_period: u8,
    env_timer: u8,
    volume: u8,
    freq: u16,
    timer: i32,
    duty_pos: u8,
    enabled: bool,
    dac: bool,
}

impl Square {
    fn output(&self) -> u8 {
        if self.enabled && self.dac {
            DUTY[self.duty as usize][self.duty_pos as usize] * self.volume
        } else {
            0
        }
    }

    fn tick(&mut self, t: u32) {
        self.timer -= t as i32;
        while self.timer <= 0 {
            self.timer += ((2048 - self.freq) as i32) * 4;
            self.duty_pos = (self.duty_pos + 1) & 7;
        }
    }

    fn clock_length(&mut self) {
        if self.length_enable && self.length > 0 {
            self.length -= 1;
            if self.length == 0 {
                self.enabled = false;
            }
        }
    }

    fn clock_envelope(&mut self) {
        if self.env_period == 0 {
            return;
        }
        if self.env_timer > 0 {
            self.env_timer -= 1;
        }
        if self.env_timer == 0 {
            self.env_timer = self.env_period;
            if self.env_add && self.volume < 15 {
                self.volume += 1;
            } else if !self.env_add && self.volume > 0 {
                self.volume -= 1;
            }
        }
    }

    fn sweep_calc(&mut self) -> u16 {
        let delta = self.sweep_shadow >> self.sweep_shift;
        let next = if self.sweep_negate {
            self.sweep_shadow.wrapping_sub(delta)
        } else {
            self.sweep_shadow + delta
        };
        if next > 2047 {
            self.enabled = false;
        }
        next
    }

    fn clock_sweep(&mut self) {
        if !self.sweep_enabled {
            return;
        }
        if self.sweep_timer > 0 {
            self.sweep_timer -= 1;
        }
        if self.sweep_timer == 0 {
            self.sweep_timer = if self.sweep_period == 0 { 8 } else { self.sweep_period };
            if self.sweep_period != 0 {
                let next = self.sweep_calc();
                if next <= 2047 && self.sweep_shift != 0 {
                    self.sweep_shadow = next;
                    self.freq = next;
                    self.sweep_calc();
                }
            }
        }
    }

    /// SOUNDxCNT_H: length/duty/envelope.
    fn write_hl(&mut self, v: u16) {
        self.length = 64 - (v & 0x3F);
        self.duty = ((v >> 6) & 3) as u8;
        self.env_period = ((v >> 8) & 7) as u8;
        self.env_add = v & 0x0800 != 0;
        self.env_start_vol = (v >> 12) as u8;
        self.dac = v & 0xF800 != 0;
        if !self.dac {
            self.enabled = false;
        }
    }

    /// SOUNDxCNT_X: frequency/length-enable/trigger.
    fn write_x(&mut self, v: u16) {
        self.freq = v & 0x7FF;
        self.length_enable = v & 0x4000 != 0;
        if v & 0x8000 != 0 {
            self.trigger();
        }
    }

    fn trigger(&mut self) {
        self.enabled = self.dac;
        if self.length == 0 {
            self.length = 64;
        }
        self.timer = ((2048 - self.freq) as i32) * 4;
        self.volume = self.env_start_vol;
        self.env_timer = self.env_period;
        self.sweep_shadow = self.freq;
        self.sweep_timer = if self.sweep_period == 0 { 8 } else { self.sweep_period };
        self.sweep_enabled = self.sweep_period != 0 || self.sweep_shift != 0;
        if self.sweep_shift != 0 {
            self.sweep_calc();
        }
    }
}

#[derive(Default)]
struct Wave {
    dac: bool,
    two_banks: bool,
    bank: u8,
    length: u16,
    length_enable: bool,
    vol_code: u8,
    force75: bool,
    freq: u16,
    timer: i32,
    pos: u8,
    enabled: bool,
    ram: [[u8; 16]; 2],
    sample: u8,
}

impl Wave {
    fn output(&self) -> u8 {
        if !(self.enabled && self.dac) {
            return 0;
        }
        if self.force75 {
            return (self.sample as u16 * 3 / 4) as u8;
        }
        match self.vol_code {
            0 => 0,
            1 => self.sample,
            2 => self.sample >> 1,
            _ => self.sample >> 2,
        }
    }

    fn tick(&mut self, t: u32) {
        if !self.enabled {
            return;
        }
        self.timer -= t as i32;
        while self.timer <= 0 {
            self.timer += ((2048 - self.freq) as i32) * 2;
            let steps = if self.two_banks { 64 } else { 32 };
            self.pos = (self.pos + 1) % steps;
            let bank = (self.bank as usize + self.pos as usize / 32) & 1;
            let idx = (self.pos & 31) as usize;
            let byte = self.ram[bank][idx / 2];
            self.sample = if idx & 1 == 0 { byte >> 4 } else { byte & 0x0F };
        }
    }

    fn clock_length(&mut self) {
        if self.length_enable && self.length > 0 {
            self.length -= 1;
            if self.length == 0 {
                self.enabled = false;
            }
        }
    }

    fn trigger(&mut self) {
        self.enabled = self.dac;
        if self.length == 0 {
            self.length = 256;
        }
        self.timer = ((2048 - self.freq) as i32) * 2;
        self.pos = 0;
    }
}

#[derive(Default)]
struct Noise {
    length: u16,
    length_enable: bool,
    env_start_vol: u8,
    env_add: bool,
    env_period: u8,
    env_timer: u8,
    volume: u8,
    clock_shift: u8,
    width7: bool,
    divisor_code: u8,
    timer: i32,
    lfsr: u16,
    enabled: bool,
    dac: bool,
}

impl Noise {
    fn period(&self) -> i32 {
        let divisor = if self.divisor_code == 0 { 8 } else { self.divisor_code as i32 * 16 };
        divisor << self.clock_shift
    }

    fn output(&self) -> u8 {
        if self.enabled && self.dac && self.lfsr & 1 == 0 { self.volume } else { 0 }
    }

    fn tick(&mut self, t: u32) {
        self.timer -= t as i32;
        while self.timer <= 0 {
            self.timer += self.period();
            let xor = (self.lfsr ^ (self.lfsr >> 1)) & 1;
            self.lfsr = (self.lfsr >> 1) | (xor << 14);
            if self.width7 {
                self.lfsr = (self.lfsr & !(1 << 6)) | (xor << 6);
            }
        }
    }

    fn clock_length(&mut self) {
        if self.length_enable && self.length > 0 {
            self.length -= 1;
            if self.length == 0 {
                self.enabled = false;
            }
        }
    }

    fn clock_envelope(&mut self) {
        if self.env_period == 0 {
            return;
        }
        if self.env_timer > 0 {
            self.env_timer -= 1;
        }
        if self.env_timer == 0 {
            self.env_timer = self.env_period;
            if self.env_add && self.volume < 15 {
                self.volume += 1;
            } else if !self.env_add && self.volume > 0 {
                self.volume -= 1;
            }
        }
    }

    fn trigger(&mut self) {
        self.enabled = self.dac;
        if self.length == 0 {
            self.length = 64;
        }
        self.timer = self.period();
        self.volume = self.env_start_vol;
        self.env_timer = self.env_period;
        self.lfsr = 0x7FFF;
    }
}

#[derive(Default)]
struct Fifo {
    buf: VecDeque<i8>,
    level: i8,
}

impl Fifo {
    /// Pushes two signed bytes, low byte first; excess bytes are dropped.
    fn push16(&mut self, v: u16) {
        for b in [v as u8, (v >> 8) as u8] {
            if self.buf.len() < 32 {
                self.buf.push_back(b as i8);
            }
        }
    }

    /// Pops the next sample into the output level; empty keeps the last level.
    fn pop(&mut self) {
        if let Some(s) = self.buf.pop_front() {
            self.level = s;
        }
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.level = 0;
    }
}

pub struct Apu {
    ch1: Square,
    ch2: Square,
    ch3: Wave,
    ch4: Noise,
    fifo_a: Fifo,
    fifo_b: Fifo,
    master: bool,
    regs: [u16; 24],
    frame_seq: u8,
    frame_timer: u32,
    psg_frac: u64,
    sample_acc: u64,
    /// Queued interleaved stereo f32 samples at `SAMPLE_RATE`.
    pub samples: Vec<f32>,
}

impl Apu {
    pub fn new() -> Self {
        let mut apu = Apu {
            ch1: Square::default(),
            ch2: Square::default(),
            ch3: Wave::default(),
            ch4: Noise::default(),
            fifo_a: Fifo::default(),
            fifo_b: Fifo::default(),
            master: false,
            regs: [0; 24],
            frame_seq: 0,
            frame_timer: 0,
            psg_frac: 0,
            sample_acc: 0,
            samples: Vec::with_capacity(4096),
        };
        // SOUNDBIAS post-BIOS default.
        apu.regs[reg_idx(0x88)] = 0x0200;
        apu
    }

    pub fn read16(&mut self, addr: u32) -> u16 {
        match addr {
            0x84 => {
                let mut v = if self.master { 0x80 } else { 0 };
                v |= self.ch1.enabled as u16;
                v |= (self.ch2.enabled as u16) << 1;
                v |= (self.ch3.enabled as u16) << 2;
                v |= (self.ch4.enabled as u16) << 3;
                v
            }
            // Wave RAM accesses hit the bank not selected for playback.
            0x90..=0x9E => {
                let bank = (self.ch3.bank ^ 1) as usize;
                let i = (addr as usize - 0x90) & 0xF;
                self.ch3.ram[bank][i] as u16 | (self.ch3.ram[bank][i + 1] as u16) << 8
            }
            0x60..=0x8E => self.regs[reg_idx(addr)] & READ_MASK[reg_idx(addr)],
            _ => 0,
        }
    }

    pub fn write16(&mut self, addr: u32, v: u16) {
        match addr {
            0x90..=0x9E => {
                let bank = (self.ch3.bank ^ 1) as usize;
                let i = (addr as usize - 0x90) & 0xF;
                self.ch3.ram[bank][i] = v as u8;
                self.ch3.ram[bank][i + 1] = (v >> 8) as u8;
                return;
            }
            0xA0 | 0xA2 => return self.fifo_a.push16(v),
            0xA4 | 0xA6 => return self.fifo_b.push16(v),
            _ => {}
        }
        if addr > 0x8E {
            return;
        }
        // PSG registers are write-protected while the master enable is off.
        if !self.master && addr <= 0x80 {
            return;
        }
        self.regs[reg_idx(addr)] = v;
        match addr {
            0x60 => {
                self.ch1.sweep_period = ((v >> 4) & 7) as u8;
                self.ch1.sweep_negate = v & 0x08 != 0;
                self.ch1.sweep_shift = (v & 7) as u8;
            }
            0x62 => self.ch1.write_hl(v),
            0x64 => self.ch1.write_x(v),
            0x68 => self.ch2.write_hl(v),
            0x6C => self.ch2.write_x(v),
            0x70 => {
                self.ch3.two_banks = v & 0x20 != 0;
                self.ch3.bank = ((v >> 6) & 1) as u8;
                self.ch3.dac = v & 0x80 != 0;
                if !self.ch3.dac {
                    self.ch3.enabled = false;
                }
            }
            0x72 => {
                self.ch3.length = 256 - (v & 0xFF);
                self.ch3.vol_code = ((v >> 13) & 3) as u8;
                self.ch3.force75 = v & 0x8000 != 0;
            }
            0x74 => {
                self.ch3.freq = v & 0x7FF;
                self.ch3.length_enable = v & 0x4000 != 0;
                if v & 0x8000 != 0 {
                    self.ch3.trigger();
                }
            }
            0x78 => {
                self.ch4.length = 64 - (v & 0x3F);
                self.ch4.env_period = ((v >> 8) & 7) as u8;
                self.ch4.env_add = v & 0x0800 != 0;
                self.ch4.env_start_vol = (v >> 12) as u8;
                self.ch4.dac = v & 0xF800 != 0;
                if !self.ch4.dac {
                    self.ch4.enabled = false;
                }
            }
            0x7C => {
                self.ch4.divisor_code = (v & 7) as u8;
                self.ch4.width7 = v & 0x08 != 0;
                self.ch4.clock_shift = ((v >> 4) & 0xF) as u8;
                self.ch4.length_enable = v & 0x4000 != 0;
                if v & 0x8000 != 0 {
                    self.ch4.trigger();
                }
            }
            0x82 => {
                self.regs[reg_idx(addr)] = v & 0x770F;
                if v & 0x0800 != 0 {
                    self.fifo_a.reset();
                }
                if v & 0x8000 != 0 {
                    self.fifo_b.reset();
                }
            }
            0x84 => self.set_master(v & 0x80 != 0),
            _ => {}
        }
    }

    /// Disabling resets all PSG state and registers 0x60-0x81; wave RAM survives.
    fn set_master(&mut self, on: bool) {
        if self.master && !on {
            let ram = self.ch3.ram;
            self.ch1 = Square::default();
            self.ch2 = Square::default();
            self.ch3 = Wave::default();
            self.ch4 = Noise::default();
            self.ch3.ram = ram;
            for r in &mut self.regs[..=reg_idx(0x80)] {
                *r = 0;
            }
            self.frame_seq = 0;
            self.frame_timer = 0;
        }
        self.master = on;
    }

    /// Advances sample generation.
    pub fn tick(&mut self, cycles: u64) {
        // PSG core runs at the GB rate: system clock / 4.
        self.psg_frac += cycles;
        let gb = (self.psg_frac / 4) as u32;
        self.psg_frac &= 3;
        if self.master && gb > 0 {
            self.ch1.tick(gb);
            self.ch2.tick(gb);
            self.ch3.tick(gb);
            self.ch4.tick(gb);
            // 512 Hz frame sequencer: length 256Hz, sweep 128Hz, envelope 64Hz.
            self.frame_timer += gb;
            while self.frame_timer >= 8192 {
                self.frame_timer -= 8192;
                match self.frame_seq {
                    0 | 4 => self.clock_lengths(),
                    2 | 6 => {
                        self.clock_lengths();
                        self.ch1.clock_sweep();
                    }
                    7 => {
                        self.ch1.clock_envelope();
                        self.ch2.clock_envelope();
                        self.ch4.clock_envelope();
                    }
                    _ => {}
                }
                self.frame_seq = (self.frame_seq + 1) & 7;
            }
        }
        self.sample_acc += cycles * SAMPLE_RATE as u64;
        while self.sample_acc >= CPU_HZ {
            self.sample_acc -= CPU_HZ;
            self.emit_sample();
        }
    }

    /// Timer overflow mask (bit n = timer n); clocks FIFOs, may request DMA.
    pub fn timer_overflow(&mut self, mask: u8, dma: &mut Dma) {
        let cnt = self.regs[reg_idx(0x82)];
        if mask & (1 << ((cnt >> 10) & 1)) != 0 {
            self.fifo_a.pop();
            if self.fifo_a.buf.len() <= 16 {
                dma.request_fifo(0x0400_00A0);
            }
        }
        if mask & (1 << ((cnt >> 14) & 1)) != 0 {
            self.fifo_b.pop();
            if self.fifo_b.buf.len() <= 16 {
                dma.request_fifo(0x0400_00A4);
            }
        }
    }

    fn clock_lengths(&mut self) {
        self.ch1.clock_length();
        self.ch2.clock_length();
        self.ch3.clock_length();
        self.ch4.clock_length();
    }

    fn emit_sample(&mut self) {
        if !self.master {
            self.samples.push(0.0);
            self.samples.push(0.0);
            return;
        }
        let cnt_l = self.regs[reg_idx(0x80)];
        let cnt_h = self.regs[reg_idx(0x82)];
        let outs = [self.ch1.output(), self.ch2.output(), self.ch3.output(), self.ch4.output()];
        let on = [
            self.ch1.enabled && self.ch1.dac,
            self.ch2.enabled && self.ch2.dac,
            self.ch3.enabled && self.ch3.dac,
            self.ch4.enabled && self.ch4.dac,
        ];
        let mut l = 0.0f32;
        let mut r = 0.0f32;
        for i in 0..4 {
            let v = if on[i] { outs[i] as f32 / 7.5 - 1.0 } else { 0.0 };
            if cnt_l & (1 << (12 + i)) != 0 {
                l += v;
            }
            if cnt_l & (1 << (8 + i)) != 0 {
                r += v;
            }
        }
        let ratio = match cnt_h & 3 {
            0 => 0.25,
            1 => 0.5,
            _ => 1.0,
        };
        l *= (((cnt_l >> 4) & 7) + 1) as f32 / 8.0 * ratio * PSG_SCALE;
        r *= ((cnt_l & 7) + 1) as f32 / 8.0 * ratio * PSG_SCALE;
        let a = self.fifo_a.level as f32 / 128.0
            * if cnt_h & 0x0004 != 0 { 1.0 } else { 0.5 }
            * FIFO_SCALE;
        if cnt_h & 0x0200 != 0 {
            l += a;
        }
        if cnt_h & 0x0100 != 0 {
            r += a;
        }
        let b = self.fifo_b.level as f32 / 128.0
            * if cnt_h & 0x0008 != 0 { 1.0 } else { 0.5 }
            * FIFO_SCALE;
        if cnt_h & 0x2000 != 0 {
            l += b;
        }
        if cnt_h & 0x1000 != 0 {
            r += b;
        }
        self.samples.push(l.clamp(-1.0, 1.0));
        self.samples.push(r.clamp(-1.0, 1.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Master on, full volume/pan, 100% PSG ratio.
    fn apu_on() -> Apu {
        let mut apu = Apu::new();
        apu.write16(0x84, 0x0080);
        apu.write16(0x80, 0xFF77);
        apu.write16(0x82, 0x0002);
        apu
    }

    fn tick_chunks(apu: &mut Apu, total: u64, chunk: u64) {
        let mut left = total;
        while left > 0 {
            let n = left.min(chunk);
            apu.tick(n);
            left -= n;
        }
    }

    fn left_channel(apu: &Apu) -> Vec<f32> {
        apu.samples.iter().step_by(2).copied().collect()
    }

    fn sign_changes(samples: &[f32]) -> usize {
        samples.windows(2).filter(|w| w[0] * w[1] < 0.0).count()
    }

    #[test]
    fn fifo_push_and_pop_order() {
        let mut apu = Apu::new();
        let mut dma = Dma::new();
        apu.write16(0xA0, 0x2211);
        apu.write16(0xA2, 0x4433);
        assert_eq!(apu.fifo_a.buf.len(), 4);
        for expect in [0x11i8, 0x22, 0x33, 0x44] {
            apu.timer_overflow(0b01, &mut dma);
            assert_eq!(apu.fifo_a.level, expect);
        }
        assert_eq!(apu.fifo_a.buf.len(), 0);
    }

    #[test]
    fn fifo_pop_signed_and_empty_keeps_level() {
        let mut apu = Apu::new();
        let mut dma = Dma::new();
        apu.write16(0xA0, 0x80FF);
        apu.timer_overflow(0b01, &mut dma);
        assert_eq!(apu.fifo_a.level, -1);
        apu.timer_overflow(0b01, &mut dma);
        assert_eq!(apu.fifo_a.level, -128);
        apu.timer_overflow(0b01, &mut dma);
        assert_eq!(apu.fifo_a.level, -128);
    }

    #[test]
    fn fifo_b_uses_selected_timer() {
        let mut apu = Apu::new();
        let mut dma = Dma::new();
        apu.write16(0x82, 0x4000); // FIFO B on timer 1, FIFO A on timer 0.
        apu.write16(0xA0, 0x0001);
        apu.write16(0xA4, 0x0002);
        apu.timer_overflow(0b01, &mut dma);
        assert_eq!(apu.fifo_a.level, 1);
        assert_eq!(apu.fifo_b.level, 0);
        apu.timer_overflow(0b10, &mut dma);
        assert_eq!(apu.fifo_b.level, 2);
    }

    #[test]
    fn fifo_reset_clears_buffer_and_level() {
        let mut apu = Apu::new();
        let mut dma = Dma::new();
        apu.write16(0xA0, 0x1234);
        apu.write16(0xA4, 0x5678);
        apu.timer_overflow(0b01, &mut dma);
        assert_ne!(apu.fifo_a.level, 0);
        apu.write16(0x82, 0x8800); // both reset bits
        assert_eq!(apu.fifo_a.buf.len(), 0);
        assert_eq!(apu.fifo_a.level, 0);
        assert_eq!(apu.fifo_b.buf.len(), 0);
        assert_eq!(apu.fifo_b.level, 0);
    }

    #[test]
    fn fifo_capacity_capped_at_32() {
        let mut apu = Apu::new();
        for _ in 0..20 {
            apu.write16(0xA0, 0xAAAA);
        }
        assert_eq!(apu.fifo_a.buf.len(), 32);
    }

    #[test]
    fn fifo_level_mixes_into_output() {
        let mut apu = apu_on();
        let mut dma = Dma::new();
        apu.write16(0x82, 0x0304); // FIFO A 100% vol, L+R enabled
        apu.write16(0xA0, 0x0040);
        apu.timer_overflow(0b01, &mut dma);
        apu.tick(350);
        assert_eq!(apu.samples.len(), 2);
        let expect = 64.0 / 128.0 * FIFO_SCALE;
        assert!((apu.samples[0] - expect).abs() < 1e-6);
        assert!((apu.samples[1] - expect).abs() < 1e-6);
    }

    #[test]
    fn square1_produces_tone_at_programmed_freq() {
        let mut apu = apu_on();
        apu.write16(0x62, 0xF080); // 50% duty, vol 15, no envelope
        apu.write16(0x64, 0x8000 | 1917); // ~1000.5 Hz, trigger
        tick_chunks(&mut apu, 3_355_440, 8); // ~0.2 s
        let left = left_channel(&apu);
        let changes = sign_changes(&left);
        assert!((320..=480).contains(&changes), "sign changes {changes}");
    }

    #[test]
    fn length_counter_silences_channel() {
        let mut apu = apu_on();
        apu.write16(0x62, 0xF080 | 62); // length = 2
        apu.write16(0x64, 0xC000 | 1917); // trigger + length enable
        tick_chunks(&mut apu, 838_860, 32); // ~0.05 s
        assert!(!apu.ch1.enabled);
        assert_eq!(apu.read16(0x84) & 1, 0);
        let left = left_channel(&apu);
        assert!(left[..400].iter().any(|&s| s != 0.0));
        assert!(left[800..].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn envelope_decreases_volume() {
        let mut apu = apu_on();
        apu.write16(0x62, 0xF100); // vol 15, decrease, period 1
        apu.write16(0x64, 0x8000 | 1024);
        tick_chunks(&mut apu, 2_516_582, 64); // ~0.15 s
        assert!(apu.ch1.volume < 15 && apu.ch1.volume > 0, "vol {}", apu.ch1.volume);
        tick_chunks(&mut apu, 2_516_582, 64);
        assert_eq!(apu.ch1.volume, 0);
    }

    #[test]
    fn sweep_raises_frequency_then_overflow_disables() {
        let mut apu = apu_on();
        apu.write16(0x60, 0x0011); // period 1, shift 1, increase
        apu.write16(0x62, 0xF080);
        apu.write16(0x64, 0x8000 | 0x100);
        tick_chunks(&mut apu, 335_544, 64); // ~0.02 s: two sweep clocks
        assert!(apu.ch1.freq >= 0x180, "freq {:#x}", apu.ch1.freq);
        tick_chunks(&mut apu, 671_089, 64); // ~0.06 s total: overflow
        assert!(!apu.ch1.enabled);
    }

    #[test]
    fn wave_plays_ram_pattern() {
        let mut apu = apu_on();
        apu.write16(0x70, 0x0040); // play bank 1 so CPU writes bank 0
        let pattern = [0x01u8, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF];
        for (k, chunk) in pattern.chunks(2).chain(pattern.chunks(2)).enumerate() {
            apu.write16(0x90 + 2 * k as u32, chunk[0] as u16 | (chunk[1] as u16) << 8);
        }
        apu.write16(0x70, 0x0080); // DAC on, play bank 0
        apu.write16(0x72, 0x2000); // 100% volume
        apu.write16(0x74, 0x8000 | 2000);
        let mut seen = [false; 16];
        for _ in 0..20_000 {
            apu.tick(16);
            seen[apu.ch3.sample as usize] = true;
        }
        assert!(seen.iter().all(|&s| s), "seen {seen:?}");
        assert!(apu.samples.iter().any(|&s| s != 0.0));
    }

    #[test]
    fn wave_two_bank_playback() {
        let mut apu = apu_on();
        apu.write16(0x70, 0x0040); // fill bank 0
        for k in 0..8 {
            apu.write16(0x90 + 2 * k, 0xAAAA);
        }
        apu.write16(0x70, 0x0000); // fill bank 1
        for k in 0..8 {
            apu.write16(0x90 + 2 * k, 0x5555);
        }
        apu.write16(0x70, 0x00A0); // DAC on, two banks, start at bank 0
        apu.write16(0x72, 0x2000);
        apu.write16(0x74, 0x8000 | 2000);
        let mut seen = [false; 16];
        for _ in 0..30_000 {
            apu.tick(16);
            seen[apu.ch3.sample as usize] = true;
        }
        assert!(seen[0xA] && seen[0x5], "seen {seen:?}");
        assert!(!seen[0x3]);
    }

    #[test]
    fn wave_ram_access_targets_non_playing_bank() {
        let mut apu = apu_on();
        apu.write16(0x70, 0x0040); // CPU sees bank 0
        apu.write16(0x90, 0x1111);
        apu.write16(0x70, 0x0000); // CPU sees bank 1
        apu.write16(0x90, 0x2222);
        assert_eq!(apu.read16(0x90), 0x2222);
        apu.write16(0x70, 0x0040);
        assert_eq!(apu.read16(0x90), 0x1111);
    }

    #[test]
    fn noise_produces_output() {
        let mut apu = apu_on();
        apu.write16(0x78, 0xF000); // vol 15, no envelope
        apu.write16(0x7C, 0x8011); // trigger, shift 1, divisor 1
        tick_chunks(&mut apu, 335_544, 32); // ~0.02 s
        let left = left_channel(&apu);
        assert!(left.iter().any(|&s| s > 0.0));
        assert!(left.iter().any(|&s| s < 0.0));
    }

    #[test]
    fn master_off_outputs_silence_and_blocks_writes() {
        let mut apu = Apu::new();
        apu.write16(0x62, 0xF080); // ignored: master off
        apu.write16(0x64, 0x8000 | 1917);
        tick_chunks(&mut apu, 838_860, 32);
        assert!(!apu.samples.is_empty());
        assert!(apu.samples.iter().all(|&s| s == 0.0));
        apu.write16(0x84, 0x0080);
        assert_eq!(apu.read16(0x62), 0);
        assert!(!apu.ch1.enabled);
    }

    #[test]
    fn master_disable_clears_psg_registers() {
        let mut apu = apu_on();
        apu.write16(0x62, 0xF080);
        apu.write16(0x64, 0x8000 | 1917);
        assert!(apu.ch1.enabled);
        apu.write16(0x84, 0x0000);
        assert_eq!(apu.read16(0x84) & 0x8F, 0);
        assert_eq!(apu.read16(0x62), 0);
        assert_eq!(apu.read16(0x80), 0);
        apu.write16(0x84, 0x0080);
        assert_eq!(apu.read16(0x62), 0);
        apu.tick(3500);
        assert!(apu.samples.iter().skip(2).all(|&s| s == 0.0));
    }

    #[test]
    fn sample_cadence_matches_ratio() {
        let mut apu = Apu::new();
        tick_chunks(&mut apu, 1_000_006, 7);
        let pairs = (apu.samples.len() / 2) as i64;
        let expected = (1_000_006u64 * 48_000 / 16_777_216) as i64;
        assert!((pairs - expected).abs() <= 1, "pairs {pairs} expected {expected}");
        let mut apu = Apu::new();
        apu.tick(16_777_216);
        assert_eq!(apu.samples.len(), 2 * 48_000);
    }

    #[test]
    fn register_read_masks() {
        let mut apu = apu_on();
        for (addr, mask) in [
            (0x60u32, 0x007Fu16),
            (0x62, 0xFFC0),
            (0x64, 0x4000),
            (0x68, 0xFFC0),
            (0x6C, 0x4000),
            (0x70, 0x00E0),
            (0x72, 0xE000),
            (0x74, 0x4000),
            (0x78, 0xFF00),
            (0x7C, 0x40FF),
            (0x80, 0xFF77),
            (0x82, 0x770F),
        ] {
            apu.write16(addr, 0xFFFF);
            assert_eq!(apu.read16(addr), mask, "reg {addr:#x}");
        }
        apu.write16(0x88, 0x0200);
        assert_eq!(apu.read16(0x88), 0x0200);
        assert_eq!(apu.read16(0xA0), 0);
    }
}
