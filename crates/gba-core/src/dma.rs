//! The four DMA channels (0x040000B0-0x040000DE).

use crate::bus::Bus;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DmaTiming {
    VBlank,
    HBlank,
}

#[derive(Clone, Copy, Default)]
struct Channel {
    sad: u32,
    dad: u32,
    cnt_l: u16,
    ctrl: u16,
    /// Internal registers latched on the enable rising edge.
    src: u32,
    dst: u32,
    count: u32,
    pending: bool,
}

pub struct Dma {
    ch: [Channel; 4],
}

/// Count 0 means the channel maximum (0x4000; 0x10000 on channel 3).
fn count_latch(i: usize, cnt_l: u16) -> u32 {
    let (mask, max) = if i == 3 { (0xFFFF, 0x1_0000) } else { (0x3FFF, 0x4000) };
    let c = cnt_l as u32 & mask;
    if c == 0 { max } else { c }
}

fn src_mask(i: usize) -> u32 {
    if i == 0 { 0x07FF_FFFF } else { 0x0FFF_FFFF }
}

fn dst_mask(i: usize) -> u32 {
    if i == 3 { 0x0FFF_FFFF } else { 0x07FF_FFFF }
}

impl Dma {
    pub fn new() -> Self {
        Dma { ch: [Channel::default(); 4] }
    }

    pub fn read16(&self, addr: u32) -> u16 {
        let off = addr as usize - 0xB0;
        match off % 12 {
            10 => self.ch[off / 12].ctrl,
            _ => 0, // SAD/DAD/count are write-only
        }
    }

    pub fn write16(&mut self, addr: u32, v: u16) {
        let off = addr as usize - 0xB0;
        let i = off / 12;
        let c = &mut self.ch[i];
        match off % 12 {
            0 => c.sad = (c.sad & 0xFFFF_0000) | v as u32,
            2 => c.sad = (c.sad & 0xFFFF) | (v as u32) << 16,
            4 => c.dad = (c.dad & 0xFFFF_0000) | v as u32,
            6 => c.dad = (c.dad & 0xFFFF) | (v as u32) << 16,
            8 => c.cnt_l = v,
            _ => {
                let was = c.ctrl;
                // Bit 11 (game pak DRQ) exists only on channel 3.
                c.ctrl = v & if i == 3 { 0xFFE0 } else { 0xF7E0 };
                if c.ctrl & 0x8000 == 0 {
                    c.pending = false;
                } else if was & 0x8000 == 0 {
                    c.src = c.sad & src_mask(i);
                    c.dst = c.dad & dst_mask(i);
                    c.count = count_latch(i, c.cnt_l);
                    if (c.ctrl >> 12) & 3 == 0 {
                        c.pending = true;
                    }
                }
            }
        }
    }

    /// Marks enabled channels with matching start timing as pending.
    pub fn request(&mut self, timing: DmaTiming) {
        let code = match timing {
            DmaTiming::VBlank => 1,
            DmaTiming::HBlank => 2,
        };
        for c in &mut self.ch {
            if c.ctrl & 0x8000 != 0 && (c.ctrl >> 12) & 3 == code {
                c.pending = true;
            }
        }
    }

    /// Marks special-timing channels 1/2 whose destination is `fifo_addr`.
    pub fn request_fifo(&mut self, fifo_addr: u32) {
        for c in &mut self.ch[1..3] {
            if c.ctrl & 0x8000 != 0 && (c.ctrl >> 12) & 3 == 3 && c.dst == fifo_addr {
                c.pending = true;
            }
        }
    }

    pub fn pending(&self) -> bool {
        self.ch.iter().any(|c| c.pending)
    }
}

/// Executes all pending transfers (called between CPU instructions).
pub(crate) fn run_pending(bus: &mut Bus) {
    // Requests raised mid-transfer re-mark channels; cap guards a runaway loop.
    for _ in 0..256 {
        let Some(i) = (0..4).find(|&i| bus.dma.ch[i].pending) else { return };
        bus.dma.ch[i].pending = false;
        run_channel(bus, i);
    }
}

fn run_channel(bus: &mut Bus, i: usize) {
    let c = bus.dma.ch[i];
    // FIFO mode: 4 words, 32-bit, destination fixed, count untouched.
    let fifo = (i == 1 || i == 2) && (c.ctrl >> 12) & 3 == 3;
    let word = fifo || c.ctrl & 0x0400 != 0;
    let unit: u32 = if word { 4 } else { 2 };
    let count = if fifo { 4 } else { c.count };
    let src_adj = (c.ctrl >> 7) & 3;
    let dst_adj = if fifo { 2 } else { (c.ctrl >> 5) & 3 };
    let mut src = c.src & !(unit - 1);
    let mut dst = c.dst & !(unit - 1);
    for _ in 0..count {
        if word {
            let v = bus.read32(src);
            bus.write32(dst, v);
        } else {
            let v = bus.read16(src);
            bus.write16(dst, v);
        }
        src = step(src, src_adj, unit);
        dst = step(dst, dst_adj, unit);
    }
    if c.ctrl & 0x4000 != 0 {
        bus.if_reg |= 1 << (8 + i);
    }
    let ch = &mut bus.dma.ch[i];
    ch.src = src;
    if !fifo {
        ch.dst = dst;
        if c.ctrl & 0x0200 != 0 {
            // Repeat re-latches count, and destination when adjust is inc-reload.
            ch.count = count_latch(i, ch.cnt_l);
            if dst_adj == 3 {
                ch.dst = ch.dad & dst_mask(i);
            }
        }
    }
    if c.ctrl & 0x0200 == 0 {
        ch.ctrl &= !0x8000;
    }
}

/// Address adjustment: 0 and 3 increment, 1 decrements, 2 fixed.
fn step(addr: u32, adj: u16, unit: u32) -> u32 {
    match adj {
        1 => addr.wrapping_sub(unit),
        2 => addr,
        _ => addr.wrapping_add(unit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EW: u32 = 0x0200_0000;

    fn bus() -> Bus {
        Bus::new(vec![0; 0x200])
    }

    fn setup(bus: &mut Bus, ch: u32, sad: u32, dad: u32, count: u16, ctrl: u16) {
        let base = 0x0400_00B0 + ch * 12;
        bus.write32(base, sad);
        bus.write32(base + 4, dad);
        bus.write16(base + 8, count);
        bus.write16(base + 10, ctrl);
    }

    fn ctrl_of(bus: &mut Bus, ch: u32) -> u16 {
        bus.read16(0x0400_00BA + ch * 12)
    }

    #[test]
    fn immediate_16bit_copy() {
        let mut b = bus();
        b.write16(EW, 0x1111);
        b.write16(EW + 2, 0x2222);
        b.write16(EW + 4, 0x3333);
        setup(&mut b, 0, EW, EW + 0x100, 3, 0x8000);
        assert!(b.dma.pending());
        run_pending(&mut b);
        assert!(!b.dma.pending());
        assert_eq!(b.read16(EW + 0x100), 0x1111);
        assert_eq!(b.read16(EW + 0x102), 0x2222);
        assert_eq!(b.read16(EW + 0x104), 0x3333);
        assert_eq!(ctrl_of(&mut b, 0) & 0x8000, 0); // non-repeat clears enable
    }

    #[test]
    fn immediate_32bit_copy() {
        let mut b = bus();
        b.write32(EW, 0xDEAD_BEEF);
        b.write32(EW + 4, 0xCAFE_F00D);
        setup(&mut b, 3, EW, EW + 0x100, 2, 0x8400);
        run_pending(&mut b);
        assert_eq!(b.read32(EW + 0x100), 0xDEAD_BEEF);
        assert_eq!(b.read32(EW + 0x104), 0xCAFE_F00D);
    }

    #[test]
    fn dest_fixed_writes_one_location() {
        let mut b = bus();
        b.write16(EW, 1);
        b.write16(EW + 2, 2);
        b.write16(EW + 4, 3);
        setup(&mut b, 0, EW, EW + 0x100, 3, 0x8040);
        run_pending(&mut b);
        assert_eq!(b.read16(EW + 0x100), 3);
        assert_eq!(b.read16(EW + 0x102), 0);
    }

    #[test]
    fn decrement_adjust_copies_backwards() {
        let mut b = bus();
        b.write16(EW, 0xAAAA);
        b.write16(EW + 2, 0xBBBB);
        b.write16(EW + 4, 0xCCCC);
        setup(&mut b, 0, EW + 4, EW + 0x104, 3, 0x80A0); // src dec, dst dec
        run_pending(&mut b);
        assert_eq!(b.read16(EW + 0x104), 0xCCCC);
        assert_eq!(b.read16(EW + 0x102), 0xBBBB);
        assert_eq!(b.read16(EW + 0x100), 0xAAAA);
    }

    #[test]
    fn count_zero_latches_channel_maximum() {
        let mut b = bus();
        setup(&mut b, 0, EW, EW, 0, 0x9000); // vblank timing: latch without running
        setup(&mut b, 3, EW, EW, 0, 0x9000);
        assert_eq!(b.dma.ch[0].count, 0x4000);
        assert_eq!(b.dma.ch[3].count, 0x1_0000);
    }

    #[test]
    fn count_masks_to_14_bits_below_channel_3() {
        let mut b = bus();
        setup(&mut b, 1, EW, EW, 0x4001, 0x9000);
        assert_eq!(b.dma.ch[1].count, 1);
    }

    #[test]
    fn addresses_force_align_to_unit() {
        let mut b = bus();
        b.write16(EW, 0x5150);
        setup(&mut b, 0, EW + 1, EW + 0x103, 1, 0x8000);
        run_pending(&mut b);
        assert_eq!(b.read16(EW + 0x102), 0x5150);
    }

    #[test]
    fn irq_sets_if_bit_8_plus_channel() {
        let mut b = bus();
        setup(&mut b, 2, EW, EW + 0x100, 1, 0xC000);
        run_pending(&mut b);
        assert_eq!(b.if_reg & (1 << 10), 1 << 10);
    }

    #[test]
    fn repeat_keeps_enable_and_relatches_count() {
        let mut b = bus();
        for n in 0..4u32 {
            b.write16(EW + 2 * n, 0x0A00 + n as u16);
        }
        setup(&mut b, 0, EW, EW + 0x100, 2, 0xA200); // hblank + repeat
        assert!(!b.dma.pending());
        b.dma.request(DmaTiming::HBlank);
        run_pending(&mut b);
        assert_ne!(ctrl_of(&mut b, 0) & 0x8000, 0);
        assert_eq!(b.dma.ch[0].count, 2);
        b.dma.request(DmaTiming::HBlank);
        run_pending(&mut b); // src/dst persisted: continues where it stopped
        for n in 0..4u32 {
            assert_eq!(b.read16(EW + 0x100 + 2 * n), 0x0A00 + n as u16);
        }
    }

    #[test]
    fn repeat_inc_reload_resets_destination() {
        let mut b = bus();
        b.write16(EW, 1);
        b.write16(EW + 2, 2);
        b.write16(EW + 4, 3);
        b.write16(EW + 6, 4);
        setup(&mut b, 0, EW, EW + 0x100, 2, 0xA260); // dst inc-reload
        b.dma.request(DmaTiming::HBlank);
        run_pending(&mut b);
        b.dma.request(DmaTiming::HBlank);
        run_pending(&mut b);
        assert_eq!(b.read16(EW + 0x100), 3);
        assert_eq!(b.read16(EW + 0x102), 4);
        assert_eq!(b.read16(EW + 0x104), 0);
    }

    #[test]
    fn vblank_request_marks_only_vblank_channels() {
        let mut b = bus();
        setup(&mut b, 0, EW, EW, 1, 0x9000); // vblank, enabled
        setup(&mut b, 1, EW, EW, 1, 0xA000); // hblank, enabled
        setup(&mut b, 2, EW, EW, 1, 0x1000); // vblank, disabled
        b.dma.request(DmaTiming::VBlank);
        assert!(b.dma.ch[0].pending);
        assert!(!b.dma.ch[1].pending);
        assert!(!b.dma.ch[2].pending);
    }

    #[test]
    fn disabling_a_channel_cancels_pending() {
        let mut b = bus();
        setup(&mut b, 0, EW, EW, 1, 0x9000);
        b.dma.request(DmaTiming::VBlank);
        assert!(b.dma.pending());
        b.write16(0x0400_00BA, 0x1000);
        assert!(!b.dma.pending());
    }

    #[test]
    fn fifo_request_matches_dad_and_moves_four_fixed_words() {
        let mut b = bus();
        for n in 0..4u32 {
            b.write32(EW + 4 * n, 0xF1F0_0000 + n);
        }
        setup(&mut b, 1, EW, EW + 0x100, 42, 0xB600); // special + repeat + 32-bit
        b.dma.request_fifo(EW + 0x104);
        assert!(!b.dma.pending()); // DAD mismatch
        b.dma.request_fifo(EW + 0x100);
        assert!(b.dma.pending());
        run_pending(&mut b);
        assert_eq!(b.read32(EW + 0x100), 0xF1F0_0003); // fixed dest keeps last word
        assert_eq!(b.read32(EW + 0x104), 0);
        assert_eq!(b.dma.ch[1].src, EW + 16);
        assert_eq!(b.dma.ch[1].count, 42); // count untouched
        assert_ne!(ctrl_of(&mut b, 1) & 0x8000, 0);
    }

    #[test]
    fn fifo_only_matches_channels_1_and_2() {
        let mut b = bus();
        setup(&mut b, 0, EW, EW + 0x100, 1, 0xB600);
        setup(&mut b, 3, EW, EW + 0x100, 1, 0xB600);
        b.dma.request_fifo(EW + 0x100);
        assert!(!b.dma.pending());
    }

    #[test]
    fn lowest_channel_runs_first() {
        let mut b = bus();
        b.write16(EW, 0x5555);
        b.write16(EW + 2, 0xAAAA);
        setup(&mut b, 1, EW + 2, EW + 0x100, 1, 0x9000);
        setup(&mut b, 0, EW, EW + 0x100, 1, 0x9000);
        b.dma.request(DmaTiming::VBlank);
        run_pending(&mut b); // ch0 writes first, ch1 overwrites
        assert_eq!(b.read16(EW + 0x100), 0xAAAA);
    }

    #[test]
    fn enable_edge_latches_once() {
        let mut b = bus();
        setup(&mut b, 0, EW, EW + 0x100, 5, 0x9000);
        b.write32(0x0400_00B0, EW + 0x40); // SAD rewrite without enable edge
        b.write16(0x0400_00B8, 9);
        b.write16(0x0400_00BA, 0x9000); // still enabled: no re-latch
        assert_eq!(b.dma.ch[0].src, EW);
        assert_eq!(b.dma.ch[0].count, 5);
    }
}
