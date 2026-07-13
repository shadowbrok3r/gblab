//! DIV/TIMA/TMA/TAC timer with falling-edge TIMA increments and delayed reload.

pub const INT_TIMER: u8 = 1 << 2;

pub struct Timer {
    div: u16,
    tima: u8,
    tma: u8,
    tac: u8,
    /// M-cycles remaining until the post-overflow TMA reload.
    reload_delay: u8,
}

impl Timer {
    pub fn new() -> Self {
        Timer { div: 0xAB00, tima: 0, tma: 0, tac: 0xF8, reload_delay: 0 }
    }

    fn selected_bit(&self) -> u16 {
        match self.tac & 0x03 {
            0 => 1 << 9,
            1 => 1 << 3,
            2 => 1 << 5,
            _ => 1 << 7,
        }
    }

    fn timer_enabled(&self) -> bool {
        self.tac & 0x04 != 0
    }

    fn increment_tima(&mut self) {
        let (v, overflow) = self.tima.overflowing_add(1);
        self.tima = v;
        if overflow {
            self.reload_delay = 2;
        }
    }

    /// Advance by CPU T-cycles (always a multiple of 4).
    pub fn tick(&mut self, t: u32, iflags: &mut u8) {
        for _ in 0..t / 4 {
            if self.reload_delay > 0 {
                self.reload_delay -= 1;
                if self.reload_delay == 0 {
                    self.tima = self.tma;
                    *iflags |= INT_TIMER;
                }
            }
            let before = self.div & self.selected_bit() != 0;
            self.div = self.div.wrapping_add(4);
            let after = self.div & self.selected_bit() != 0;
            if self.timer_enabled() && before && !after {
                self.increment_tima();
            }
        }
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF04 => (self.div >> 8) as u8,
            0xFF05 => self.tima,
            0xFF06 => self.tma,
            0xFF07 => self.tac | 0xF8,
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        match addr {
            0xFF04 => {
                // Resetting DIV while the selected bit is high clocks TIMA.
                if self.timer_enabled() && self.div & self.selected_bit() != 0 {
                    self.increment_tima();
                }
                self.div = 0;
            }
            0xFF05 => {
                if self.reload_delay != 1 {
                    self.tima = v;
                    self.reload_delay = 0;
                }
            }
            0xFF06 => self.tma = v,
            0xFF07 => self.tac = v & 0x07,
            _ => {}
        }
    }
}
