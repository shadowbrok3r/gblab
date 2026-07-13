//! Audio processing unit: 2 square channels, wave, noise; 48 kHz stereo out.

pub const SAMPLE_RATE: u32 = 48_000;
const CPU_HZ: u32 = 4_194_304;

const DUTY: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 1, 1, 1],
    [0, 1, 1, 1, 1, 1, 1, 0],
];

#[derive(Default)]
struct Square {
    sweep_enabled: bool,
    // NRx0 sweep (channel 1 only).
    sweep_period: u8,
    sweep_negate: bool,
    sweep_shift: u8,
    sweep_timer: u8,
    sweep_shadow: u16,
    // NRx1 duty/length.
    duty: u8,
    length: u16,
    length_enable: bool,
    // NRx2 envelope.
    env_start_vol: u8,
    env_add: bool,
    env_period: u8,
    env_timer: u8,
    volume: u8,
    // NRx3/4 frequency.
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
    length: u16,
    length_enable: bool,
    volume_code: u8,
    freq: u16,
    timer: i32,
    pos: u8,
    enabled: bool,
    ram: [u8; 16],
    sample: u8,
}

impl Wave {
    fn output(&self) -> u8 {
        if self.enabled && self.dac {
            match self.volume_code {
                0 => 0,
                1 => self.sample,
                2 => self.sample >> 1,
                _ => self.sample >> 2,
            }
        } else {
            0
        }
    }

    fn tick(&mut self, t: u32) {
        if !self.enabled {
            return;
        }
        self.timer -= t as i32;
        while self.timer <= 0 {
            self.timer += ((2048 - self.freq) as i32) * 2;
            self.pos = (self.pos + 1) & 31;
            let byte = self.ram[(self.pos / 2) as usize];
            self.sample = if self.pos & 1 == 0 { byte >> 4 } else { byte & 0x0F };
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

pub struct Apu {
    ch1: Square,
    ch2: Square,
    ch3: Wave,
    ch4: Noise,
    power: bool,
    nr50: u8,
    nr51: u8,
    frame_seq: u8,
    frame_timer: u32,
    sample_acc: u32,
    /// Interleaved stereo f32 samples, drained by the frontend.
    pub samples: Vec<f32>,
    hp_cap: [f32; 2],
}

impl Apu {
    pub fn new() -> Self {
        let mut apu = Apu {
            ch1: Square::default(),
            ch2: Square::default(),
            ch3: Wave::default(),
            ch4: Noise::default(),
            power: true,
            nr50: 0x77,
            nr51: 0xF3,
            frame_seq: 0,
            frame_timer: 0,
            sample_acc: 0,
            samples: Vec::with_capacity(4096),
            hp_cap: [0.0; 2],
        };
        // Post-boot channel 1 state (boot ROM beep).
        apu.ch1.dac = true;
        apu.ch1.duty = 2;
        apu.ch1.env_start_vol = 0x0F;
        apu.ch1.volume = 0x0F;
        apu.ch1.enabled = true;
        apu
    }

    pub fn tick(&mut self, t: u32) {
        if self.power {
            self.ch1.tick(t);
            self.ch2.tick(t);
            self.ch3.tick(t);
            self.ch4.tick(t);

            self.frame_timer += t;
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

        self.sample_acc += t * SAMPLE_RATE;
        while self.sample_acc >= CPU_HZ {
            self.sample_acc -= CPU_HZ;
            self.emit_sample();
        }
    }

    fn clock_lengths(&mut self) {
        self.ch1.clock_length();
        self.ch2.clock_length();
        self.ch3.clock_length();
        self.ch4.clock_length();
    }

    fn emit_sample(&mut self) {
        let outs = [self.ch1.output(), self.ch2.output(), self.ch3.output(), self.ch4.output()];
        let mut mixed = [0.0f32; 2];
        for (i, &o) in outs.iter().enumerate() {
            let dac = o as f32 / 7.5 - 1.0;
            let dac_on = match i {
                0 => self.ch1.dac && self.ch1.enabled,
                1 => self.ch2.dac && self.ch2.enabled,
                2 => self.ch3.dac && self.ch3.enabled,
                _ => self.ch4.dac && self.ch4.enabled,
            };
            let v = if dac_on { dac } else { 0.0 };
            if self.nr51 & (1 << (i + 4)) != 0 {
                mixed[0] += v;
            }
            if self.nr51 & (1 << i) != 0 {
                mixed[1] += v;
            }
        }
        let vol_l = ((self.nr50 >> 4) & 0x07) as f32 + 1.0;
        let vol_r = (self.nr50 & 0x07) as f32 + 1.0;
        mixed[0] *= vol_l / 8.0 / 4.0;
        mixed[1] *= vol_r / 8.0 / 4.0;
        // High-pass to remove DC offset.
        for ch in 0..2 {
            let out = mixed[ch] - self.hp_cap[ch];
            self.hp_cap[ch] = mixed[ch] - out * 0.9995;
            mixed[ch] = out;
        }
        self.samples.push(mixed[0]);
        self.samples.push(mixed[1]);
    }

    pub fn read(&self, addr: u16) -> u8 {
        const OR_MASK: [u8; 22] = [
            0x80, 0x3F, 0x00, 0xFF, 0xBF, 0xFF, 0x3F, 0x00, 0xFF, 0xBF, 0x7F, 0xFF, 0x9F, 0xFF,
            0xBF, 0xFF, 0xFF, 0x00, 0x00, 0xBF, 0x00, 0x00,
        ];
        match addr {
            0xFF10 => self.reg_raw(addr) | OR_MASK[0],
            0xFF11..=0xFF25 => self.reg_raw(addr) | OR_MASK[(addr - 0xFF10) as usize],
            0xFF26 => {
                let mut v = if self.power { 0x80 } else { 0 } | 0x70;
                if self.ch1.enabled {
                    v |= 0x01;
                }
                if self.ch2.enabled {
                    v |= 0x02;
                }
                if self.ch3.enabled {
                    v |= 0x04;
                }
                if self.ch4.enabled {
                    v |= 0x08;
                }
                v
            }
            0xFF30..=0xFF3F => self.ch3.ram[(addr - 0xFF30) as usize],
            _ => 0xFF,
        }
    }

    fn reg_raw(&self, addr: u16) -> u8 {
        match addr {
            0xFF10 => {
                (self.ch1.sweep_period << 4)
                    | if self.ch1.sweep_negate { 0x08 } else { 0 }
                    | self.ch1.sweep_shift
            }
            0xFF11 => self.ch1.duty << 6,
            0xFF12 => {
                (self.ch1.env_start_vol << 4)
                    | if self.ch1.env_add { 0x08 } else { 0 }
                    | self.ch1.env_period
            }
            0xFF14 => if self.ch1.length_enable { 0x40 } else { 0 },
            0xFF16 => self.ch2.duty << 6,
            0xFF17 => {
                (self.ch2.env_start_vol << 4)
                    | if self.ch2.env_add { 0x08 } else { 0 }
                    | self.ch2.env_period
            }
            0xFF19 => if self.ch2.length_enable { 0x40 } else { 0 },
            0xFF1A => if self.ch3.dac { 0x80 } else { 0 },
            0xFF1C => self.ch3.volume_code << 5,
            0xFF1E => if self.ch3.length_enable { 0x40 } else { 0 },
            0xFF21 => {
                (self.ch4.env_start_vol << 4)
                    | if self.ch4.env_add { 0x08 } else { 0 }
                    | self.ch4.env_period
            }
            0xFF22 => {
                (self.ch4.clock_shift << 4)
                    | if self.ch4.width7 { 0x08 } else { 0 }
                    | self.ch4.divisor_code
            }
            0xFF23 => if self.ch4.length_enable { 0x40 } else { 0 },
            0xFF24 => self.nr50,
            0xFF25 => self.nr51,
            _ => 0,
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        if !self.power && addr != 0xFF26 && !(0xFF30..=0xFF3F).contains(&addr) {
            return;
        }
        match addr {
            0xFF10 => {
                self.ch1.sweep_period = (v >> 4) & 0x07;
                self.ch1.sweep_negate = v & 0x08 != 0;
                self.ch1.sweep_shift = v & 0x07;
            }
            0xFF11 => {
                self.ch1.duty = v >> 6;
                self.ch1.length = 64 - (v & 0x3F) as u16;
            }
            0xFF12 => {
                self.ch1.env_start_vol = v >> 4;
                self.ch1.env_add = v & 0x08 != 0;
                self.ch1.env_period = v & 0x07;
                self.ch1.dac = v & 0xF8 != 0;
                if !self.ch1.dac {
                    self.ch1.enabled = false;
                }
            }
            0xFF13 => self.ch1.freq = (self.ch1.freq & 0x700) | v as u16,
            0xFF14 => {
                self.ch1.freq = (self.ch1.freq & 0xFF) | ((v as u16 & 0x07) << 8);
                self.ch1.length_enable = v & 0x40 != 0;
                if v & 0x80 != 0 {
                    self.ch1.trigger();
                }
            }
            0xFF16 => {
                self.ch2.duty = v >> 6;
                self.ch2.length = 64 - (v & 0x3F) as u16;
            }
            0xFF17 => {
                self.ch2.env_start_vol = v >> 4;
                self.ch2.env_add = v & 0x08 != 0;
                self.ch2.env_period = v & 0x07;
                self.ch2.dac = v & 0xF8 != 0;
                if !self.ch2.dac {
                    self.ch2.enabled = false;
                }
            }
            0xFF18 => self.ch2.freq = (self.ch2.freq & 0x700) | v as u16,
            0xFF19 => {
                self.ch2.freq = (self.ch2.freq & 0xFF) | ((v as u16 & 0x07) << 8);
                self.ch2.length_enable = v & 0x40 != 0;
                if v & 0x80 != 0 {
                    self.ch2.trigger();
                }
            }
            0xFF1A => {
                self.ch3.dac = v & 0x80 != 0;
                if !self.ch3.dac {
                    self.ch3.enabled = false;
                }
            }
            0xFF1B => self.ch3.length = 256 - v as u16,
            0xFF1C => self.ch3.volume_code = (v >> 5) & 0x03,
            0xFF1D => self.ch3.freq = (self.ch3.freq & 0x700) | v as u16,
            0xFF1E => {
                self.ch3.freq = (self.ch3.freq & 0xFF) | ((v as u16 & 0x07) << 8);
                self.ch3.length_enable = v & 0x40 != 0;
                if v & 0x80 != 0 {
                    self.ch3.trigger();
                }
            }
            0xFF20 => self.ch4.length = 64 - (v & 0x3F) as u16,
            0xFF21 => {
                self.ch4.env_start_vol = v >> 4;
                self.ch4.env_add = v & 0x08 != 0;
                self.ch4.env_period = v & 0x07;
                self.ch4.dac = v & 0xF8 != 0;
                if !self.ch4.dac {
                    self.ch4.enabled = false;
                }
            }
            0xFF22 => {
                self.ch4.clock_shift = v >> 4;
                self.ch4.width7 = v & 0x08 != 0;
                self.ch4.divisor_code = v & 0x07;
            }
            0xFF23 => {
                self.ch4.length_enable = v & 0x40 != 0;
                if v & 0x80 != 0 {
                    self.ch4.trigger();
                }
            }
            0xFF24 => self.nr50 = v,
            0xFF25 => self.nr51 = v,
            0xFF26 => {
                let on = v & 0x80 != 0;
                if self.power && !on {
                    let ram = self.ch3.ram;
                    self.ch1 = Square::default();
                    self.ch2 = Square::default();
                    self.ch3 = Wave::default();
                    self.ch4 = Noise::default();
                    self.ch3.ram = ram;
                    self.nr50 = 0;
                    self.nr51 = 0;
                    self.frame_seq = 0;
                }
                self.power = on;
            }
            0xFF30..=0xFF3F => self.ch3.ram[(addr - 0xFF30) as usize] = v,
            _ => {}
        }
    }
}
