//! ARM block data transfer (LDM/STM).

use super::Arm7;

pub(crate) fn arm_block_transfer(cpu: &mut Arm7, op: u32) {
    let pre = op & (1 << 24) != 0;
    let up = op & (1 << 23) != 0;
    let s = op & (1 << 22) != 0;
    let wb = op & (1 << 21) != 0;
    let load = op & (1 << 20) != 0;
    let rn = ((op >> 16) & 0xF) as usize;
    let rlist = op & 0xFFFF;

    // Empty rlist transfers r15 only and moves the base by 0x40 (n = 16).
    let list = if rlist == 0 { 0x8000 } else { rlist };
    let n = if rlist == 0 { 16 } else { rlist.count_ones() };
    let base = cpu.reg(rn);
    let end = if up { base.wrapping_add(4 * n) } else { base.wrapping_sub(4 * n) };
    // Lowest address of the window; registers transfer ascending from here.
    let mut addr = match (pre, up) {
        (false, true) => base,
        (true, true) => base.wrapping_add(4),
        (false, false) => end.wrapping_add(4),
        (true, false) => end,
    };
    // S bit without an r15 load transfers the user bank.
    let user = s && !(load && list & 0x8000 != 0);

    if load {
        cpu.bus.idle();
        let mut pc = None;
        for i in 0..16 {
            if list & (1 << i) == 0 {
                continue;
            }
            let v = cpu.bus.read32(addr);
            addr = addr.wrapping_add(4);
            if i == 15 {
                pc = Some(v);
            } else if user {
                cpu.set_user_reg(i, v);
            } else {
                cpu.r[i] = v;
            }
        }
        // A loaded base wins over writeback.
        if wb && rlist & (1 << rn) == 0 {
            cpu.set_reg(rn, end);
        }
        if let Some(v) = pc {
            if s {
                cpu.restore_cpsr_from_spsr();
            }
            cpu.branch(v);
        }
    } else {
        let first = list.trailing_zeros() as usize;
        for i in 0..16 {
            if list & (1 << i) == 0 {
                continue;
            }
            let v = if i == 15 {
                cpu.r[15].wrapping_add(4)
            } else if i == rn {
                // Base in list: old value if stored first, else the written-back value.
                if wb && i != first { end } else { base }
            } else if user {
                cpu.user_reg(i)
            } else {
                cpu.r[i]
            };
            cpu.bus.write32(addr, v);
            addr = addr.wrapping_add(4);
        }
        if wb {
            cpu.set_reg(rn, end);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::cpu::{test_arm, FLAG_C, FLAG_N, FLAG_T, MODE_FIQ, MODE_SVC, MODE_SYS};

    const BASE: u32 = 0x0300_0000;

    fn op(load: bool, pre: bool, up: bool, s: bool, w: bool, rn: u32, rlist: u32) -> u32 {
        0xE800_0000
            | (pre as u32) << 24
            | (up as u32) << 23
            | (s as u32) << 22
            | (w as u32) << 21
            | (load as u32) << 20
            | rn << 16
            | rlist
    }

    #[test]
    fn stmia_stores_ascending_with_writeback() {
        let mut cpu = test_arm(&[op(false, false, true, false, true, 0, 0b1110)]);
        cpu.r[0] = BASE;
        cpu.r[1] = 0x11;
        cpu.r[2] = 0x22;
        cpu.r[3] = 0x33;
        cpu.step();
        assert_eq!(cpu.bus.read32(BASE), 0x11);
        assert_eq!(cpu.bus.read32(BASE + 4), 0x22);
        assert_eq!(cpu.bus.read32(BASE + 8), 0x33);
        assert_eq!(cpu.r[0], BASE + 12);
    }

    #[test]
    fn stmib_window() {
        let mut cpu = test_arm(&[op(false, true, true, false, true, 0, 0b0110)]);
        cpu.r[0] = BASE;
        cpu.r[1] = 0x11;
        cpu.r[2] = 0x22;
        cpu.step();
        assert_eq!(cpu.bus.read32(BASE + 4), 0x11);
        assert_eq!(cpu.bus.read32(BASE + 8), 0x22);
        assert_eq!(cpu.r[0], BASE + 8);
    }

    #[test]
    fn stmda_window() {
        let mut cpu = test_arm(&[op(false, false, false, false, true, 0, 0b0110)]);
        cpu.r[0] = BASE;
        cpu.r[1] = 0x11;
        cpu.r[2] = 0x22;
        cpu.step();
        assert_eq!(cpu.bus.read32(BASE - 4), 0x11);
        assert_eq!(cpu.bus.read32(BASE), 0x22);
        assert_eq!(cpu.r[0], BASE - 8);
    }

    #[test]
    fn stmdb_window() {
        let mut cpu = test_arm(&[op(false, true, false, false, true, 0, 0b0110)]);
        cpu.r[0] = BASE;
        cpu.r[1] = 0x11;
        cpu.r[2] = 0x22;
        cpu.step();
        assert_eq!(cpu.bus.read32(BASE - 8), 0x11);
        assert_eq!(cpu.bus.read32(BASE - 4), 0x22);
        assert_eq!(cpu.r[0], BASE - 8);
    }

    #[test]
    fn ldmia_loads_with_writeback() {
        let mut cpu = test_arm(&[op(true, false, true, false, true, 0, 0b0110)]);
        cpu.r[0] = BASE;
        cpu.bus.write32(BASE, 0xAA);
        cpu.bus.write32(BASE + 4, 0xBB);
        cpu.step();
        assert_eq!(cpu.r[1], 0xAA);
        assert_eq!(cpu.r[2], 0xBB);
        assert_eq!(cpu.r[0], BASE + 8);
    }

    #[test]
    fn ldmdb_window() {
        let mut cpu = test_arm(&[op(true, true, false, false, true, 0, 0b0110)]);
        cpu.r[0] = BASE;
        cpu.bus.write32(BASE - 8, 0xAA);
        cpu.bus.write32(BASE - 4, 0xBB);
        cpu.step();
        assert_eq!(cpu.r[1], 0xAA);
        assert_eq!(cpu.r[2], 0xBB);
        assert_eq!(cpu.r[0], BASE - 8);
    }

    #[test]
    fn stm_base_first_stores_old_base() {
        let mut cpu = test_arm(&[op(false, false, true, false, true, 0, 0b0011)]);
        cpu.r[0] = BASE;
        cpu.r[1] = 0x11;
        cpu.step();
        assert_eq!(cpu.bus.read32(BASE), BASE);
        assert_eq!(cpu.bus.read32(BASE + 4), 0x11);
        assert_eq!(cpu.r[0], BASE + 8);
    }

    #[test]
    fn stm_base_not_first_stores_written_back() {
        let mut cpu = test_arm(&[op(false, false, true, false, true, 1, 0b0011)]);
        cpu.r[0] = 0x11;
        cpu.r[1] = BASE;
        cpu.step();
        assert_eq!(cpu.bus.read32(BASE), 0x11);
        assert_eq!(cpu.bus.read32(BASE + 4), BASE + 8);
        assert_eq!(cpu.r[1], BASE + 8);
    }

    #[test]
    fn ldm_base_in_list_skips_writeback() {
        let mut cpu = test_arm(&[op(true, false, true, false, true, 0, 0b0011)]);
        cpu.r[0] = BASE;
        cpu.bus.write32(BASE, 0xAA);
        cpu.bus.write32(BASE + 4, 0xBB);
        cpu.step();
        assert_eq!(cpu.r[0], 0xAA);
        assert_eq!(cpu.r[1], 0xBB);
    }

    #[test]
    fn stm_empty_rlist_ia() {
        let mut cpu = test_arm(&[op(false, false, true, false, true, 0, 0)]);
        cpu.r[0] = BASE;
        cpu.step();
        assert_eq!(cpu.bus.read32(BASE), 0x0800_00CC);
        assert_eq!(cpu.r[0], BASE + 0x40);
    }

    #[test]
    fn stm_empty_rlist_db() {
        let mut cpu = test_arm(&[op(false, true, false, false, true, 0, 0)]);
        cpu.r[0] = BASE;
        cpu.step();
        assert_eq!(cpu.bus.read32(BASE - 0x40), 0x0800_00CC);
        assert_eq!(cpu.r[0], BASE - 0x40);
    }

    #[test]
    fn stm_empty_rlist_da() {
        let mut cpu = test_arm(&[op(false, false, false, false, true, 0, 0)]);
        cpu.r[0] = BASE;
        cpu.step();
        assert_eq!(cpu.bus.read32(BASE - 0x3C), 0x0800_00CC);
        assert_eq!(cpu.r[0], BASE - 0x40);
    }

    #[test]
    fn ldm_empty_rlist_loads_pc() {
        let mut cpu = test_arm(&[op(true, false, true, false, true, 0, 0)]);
        cpu.r[0] = BASE;
        cpu.bus.write32(BASE, 0x0800_0100);
        cpu.step();
        assert_eq!(cpu.exec_pc(), 0x0800_0100);
        assert_eq!(cpu.r[0], BASE + 0x40);
    }

    #[test]
    fn stm_stores_pc_plus_12() {
        let mut cpu = test_arm(&[op(false, false, true, false, false, 0, 0x8002)]);
        cpu.r[0] = BASE;
        cpu.r[1] = 0x11;
        cpu.step();
        assert_eq!(cpu.bus.read32(BASE), 0x11);
        assert_eq!(cpu.bus.read32(BASE + 4), 0x0800_00CC);
    }

    #[test]
    fn ldm_pc_branches() {
        let mut cpu = test_arm(&[op(true, false, true, false, false, 0, 0x8000)]);
        cpu.r[0] = BASE;
        cpu.bus.write32(BASE, 0x0800_0100);
        cpu.step();
        assert_eq!(cpu.exec_pc(), 0x0800_0100);
        assert_eq!(cpu.r[15], 0x0800_0104);
    }

    #[test]
    fn ldm_s_pc_restores_cpsr() {
        let mut cpu = test_arm(&[op(true, false, true, true, false, 0, 0x8002)]);
        cpu.set_cpsr(MODE_SVC);
        cpu.set_spsr(MODE_SYS | FLAG_N | FLAG_C);
        cpu.r[0] = BASE;
        cpu.bus.write32(BASE, 0xAB);
        cpu.bus.write32(BASE + 4, 0x0800_0100);
        cpu.step();
        assert_eq!(cpu.cpsr, MODE_SYS | FLAG_N | FLAG_C);
        assert_eq!(cpu.r[1], 0xAB);
        assert_eq!(cpu.exec_pc(), 0x0800_0100);
    }

    #[test]
    fn ldm_s_pc_restores_thumb_before_branch() {
        let mut cpu = test_arm(&[op(true, false, true, true, false, 0, 0x8000)]);
        cpu.set_cpsr(MODE_SVC);
        cpu.set_spsr(MODE_SYS | FLAG_T);
        cpu.r[0] = BASE;
        cpu.bus.write32(BASE, 0x0800_0100);
        cpu.step();
        assert!(cpu.cpsr & FLAG_T != 0);
        assert_eq!(cpu.exec_pc(), 0x0800_0100);
    }

    #[test]
    fn ldm_s_loads_user_bank() {
        let mut cpu = test_arm(&[op(true, false, true, true, false, 0, 0x2100)]);
        cpu.set_cpsr(MODE_FIQ);
        cpu.r[0] = BASE;
        cpu.r[8] = 0xF8;
        cpu.r[13] = 0xFD;
        cpu.bus.write32(BASE, 0x0808);
        cpu.bus.write32(BASE + 4, 0x0D0D);
        cpu.step();
        assert_eq!(cpu.r[8], 0xF8);
        assert_eq!(cpu.r[13], 0xFD);
        assert_eq!(cpu.user_reg(8), 0x0808);
        assert_eq!(cpu.user_reg(13), 0x0D0D);
    }

    #[test]
    fn stm_s_stores_user_bank() {
        let mut cpu = test_arm(&[op(false, false, true, true, false, 0, 0x0100)]);
        cpu.set_cpsr(MODE_FIQ);
        cpu.set_user_reg(8, 0x1234);
        cpu.r[0] = BASE;
        cpu.r[8] = 0xBAD;
        cpu.step();
        assert_eq!(cpu.bus.read32(BASE), 0x1234);
    }

    #[test]
    fn ldm_s_writeback_hits_current_mode_base() {
        let mut cpu = test_arm(&[op(true, false, true, true, true, 13, 0x0100)]);
        cpu.set_cpsr(MODE_FIQ);
        cpu.r[13] = BASE;
        cpu.bus.write32(BASE, 0x0808);
        cpu.step();
        assert_eq!(cpu.r[13], BASE + 4);
        assert_eq!(cpu.user_reg(13), 0x0300_7F00);
        assert_eq!(cpu.user_reg(8), 0x0808);
    }
}
