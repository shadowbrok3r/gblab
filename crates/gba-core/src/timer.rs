//! The four cascade-capable timers (0x04000100-0x0400010E).

const PRESCALE: [u64; 4] = [1, 64, 256, 1024];

#[derive(Clone, Copy, Default)]
struct Timer {
    reload: u16,
    counter: u16,
    ctrl: u16,
    /// Cycles accumulated toward the next prescaled increment.
    phase: u64,
}

pub struct Timers {
    t: [Timer; 4],
}

impl Timers {
    pub fn new() -> Self {
        Timers { t: [Timer::default(); 4] }
    }

    pub fn read16(&self, addr: u32) -> u16 {
        let t = &self.t[((addr >> 2) & 3) as usize];
        if addr & 2 == 0 { t.counter } else { t.ctrl }
    }

    pub fn write16(&mut self, addr: u32, v: u16) {
        let t = &mut self.t[((addr >> 2) & 3) as usize];
        if addr & 2 == 0 {
            t.reload = v;
        } else {
            let rising = v & 0x80 != 0 && t.ctrl & 0x80 == 0;
            t.ctrl = v & 0x00C7;
            if rising {
                t.counter = t.reload;
                t.phase = 0;
            }
        }
    }

    /// Advances all timers; returns a bitmask of timers that overflowed.
    pub fn tick(&mut self, cycles: u64, if_reg: &mut u16) -> u8 {
        let mut mask = 0u8;
        let mut prev_overflows = 0u64;
        for n in 0..4 {
            let t = &mut self.t[n];
            let mut overflows = 0u64;
            if t.ctrl & 0x80 != 0 {
                // Cascade counts the previous timer's overflows; timer 0 counts normally.
                let increments = if n > 0 && t.ctrl & 0x04 != 0 {
                    prev_overflows
                } else {
                    let total = t.phase + cycles;
                    let pre = PRESCALE[(t.ctrl & 3) as usize];
                    t.phase = total % pre;
                    total / pre
                };
                let space = 0x1_0000 - t.counter as u64;
                if increments < space {
                    t.counter += increments as u16;
                } else {
                    let period = 0x1_0000 - t.reload as u64;
                    let rem = increments - space;
                    overflows = 1 + rem / period;
                    t.counter = t.reload + (rem % period) as u16;
                    mask |= 1 << n;
                    if t.ctrl & 0x40 != 0 {
                        *if_reg |= 1 << (3 + n);
                    }
                }
            }
            prev_overflows = overflows;
        }
        mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CNT: u32 = 0x100;
    const CTL: u32 = 0x102;

    fn tick(t: &mut Timers, cycles: u64) -> (u8, u16) {
        let mut if_reg = 0;
        let mask = t.tick(cycles, &mut if_reg);
        (mask, if_reg)
    }

    #[test]
    fn prescaler_64_counts_once_per_64_cycles() {
        let mut t = Timers::new();
        t.write16(CTL, 0x81);
        tick(&mut t, 63);
        assert_eq!(t.read16(CNT), 0);
        tick(&mut t, 1);
        assert_eq!(t.read16(CNT), 1);
        tick(&mut t, 128);
        assert_eq!(t.read16(CNT), 3);
    }

    #[test]
    fn big_tick_catches_up_across_prescaler() {
        let mut t = Timers::new();
        t.write16(CTL, 0x81);
        tick(&mut t, 6400);
        assert_eq!(t.read16(CNT), 100);
    }

    #[test]
    fn big_tick_produces_multiple_overflows() {
        let mut t = Timers::new();
        t.write16(CNT, 0xFFFE); // period 2
        t.write16(CTL, 0x80);
        t.write16(CTL + 4, 0x84); // timer 1 cascade counts the overflows
        let (mask, _) = tick(&mut t, 10);
        assert_eq!(mask, 0b01);
        assert_eq!(t.read16(CNT), 0xFFFE);
        assert_eq!(t.read16(CNT + 4), 5);
    }

    #[test]
    fn cascade_chain_0_to_1() {
        let mut t = Timers::new();
        t.write16(CNT, 0xFFFF);
        t.write16(CTL, 0x80);
        t.write16(CTL + 4, 0x84);
        let (mask, _) = tick(&mut t, 5);
        assert_eq!(t.read16(CNT + 4), 5);
        assert_eq!(mask, 0b01);
    }

    #[test]
    fn cascade_overflow_propagates_in_one_tick() {
        let mut t = Timers::new();
        t.write16(CNT, 0xFFFF);
        t.write16(CTL, 0x80);
        t.write16(CNT + 4, 0xFFFF);
        t.write16(CTL + 4, 0xC4); // cascade + IRQ
        let (mask, if_reg) = tick(&mut t, 1);
        assert_eq!(mask, 0b11);
        assert_eq!(if_reg, 1 << 4);
    }

    #[test]
    fn cascade_bit_on_timer_0_counts_normally() {
        let mut t = Timers::new();
        t.write16(CTL, 0x84);
        tick(&mut t, 3);
        assert_eq!(t.read16(CNT), 3);
    }

    #[test]
    fn cascade_ignores_disabled_previous_timer() {
        let mut t = Timers::new();
        t.write16(CTL + 4, 0x84);
        tick(&mut t, 100);
        assert_eq!(t.read16(CNT + 4), 0);
    }

    #[test]
    fn enable_rising_edge_reloads_counter() {
        let mut t = Timers::new();
        t.write16(CNT, 0x1234);
        t.write16(CTL, 0x80);
        assert_eq!(t.read16(CNT), 0x1234);
        tick(&mut t, 4);
        assert_eq!(t.read16(CNT), 0x1238);
        t.write16(CTL, 0x80); // no edge: counter untouched
        assert_eq!(t.read16(CNT), 0x1238);
        t.write16(CTL, 0x00);
        t.write16(CTL, 0x80);
        assert_eq!(t.read16(CNT), 0x1234);
    }

    #[test]
    fn enable_edge_resets_prescaler_phase() {
        let mut t = Timers::new();
        t.write16(CTL, 0x81);
        tick(&mut t, 32);
        t.write16(CTL, 0x01);
        t.write16(CTL, 0x81);
        tick(&mut t, 63);
        assert_eq!(t.read16(CNT), 0);
        tick(&mut t, 1);
        assert_eq!(t.read16(CNT), 1);
    }

    #[test]
    fn reload_write_takes_effect_on_next_overflow() {
        let mut t = Timers::new();
        t.write16(CNT, 0xFFFF);
        t.write16(CTL, 0x80);
        t.write16(CNT, 0x1234); // counter unaffected until overflow
        assert_eq!(t.read16(CNT), 0xFFFF);
        tick(&mut t, 1);
        assert_eq!(t.read16(CNT), 0x1234);
    }

    #[test]
    fn overflow_sets_if_bit_when_irq_enabled() {
        let mut t = Timers::new();
        t.write16(CNT + 8, 0xFFFF);
        t.write16(CTL + 8, 0xC0); // timer 2, IRQ
        let (mask, if_reg) = tick(&mut t, 1);
        assert_eq!(mask, 0b100);
        assert_eq!(if_reg, 1 << 5);
    }

    #[test]
    fn overflow_without_irq_leaves_if_clear() {
        let mut t = Timers::new();
        t.write16(CNT, 0xFFFF);
        t.write16(CTL, 0x80);
        let (mask, if_reg) = tick(&mut t, 1);
        assert_eq!(mask, 0b1);
        assert_eq!(if_reg, 0);
    }

    #[test]
    fn control_reads_back_masked() {
        let mut t = Timers::new();
        t.write16(CTL + 12, 0xFFFF);
        assert_eq!(t.read16(CTL + 12), 0x00C7);
    }

    #[test]
    fn disabled_timer_does_not_count() {
        let mut t = Timers::new();
        tick(&mut t, 1000);
        assert_eq!(t.read16(CNT), 0);
    }
}
