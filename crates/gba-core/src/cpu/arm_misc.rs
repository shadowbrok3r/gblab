//! ARM branches, PSR transfer, multiply, and SWI.

use super::{Arm7, FLAG_N, FLAG_T, FLAG_Z, MODE_USR};

pub(crate) fn arm_branch(cpu: &mut Arm7, op: u32) {
    // Sign-extended 24-bit offset, shifted left 2.
    let offset = ((op << 8) as i32 >> 6) as u32;
    if op & 0x0100_0000 != 0 {
        cpu.r[14] = cpu.r[15].wrapping_sub(4);
    }
    cpu.branch(cpu.r[15].wrapping_add(offset));
}

pub(crate) fn arm_bx(cpu: &mut Arm7, op: u32) {
    let v = cpu.reg((op & 0xF) as usize);
    cpu.set_flag(FLAG_T, v & 1 != 0);
    cpu.branch(v);
}

pub(crate) fn arm_psr(cpu: &mut Arm7, op: u32) {
    let spsr_sel = op & 0x0040_0000 != 0;
    if op & 0x0020_0000 == 0 {
        // MRS
        let v = if spsr_sel { cpu.spsr() } else { cpu.cpsr };
        cpu.set_reg(((op >> 12) & 0xF) as usize, v);
        return;
    }
    // MSR: immediate-with-rotate or register operand.
    let v = if op & 0x0200_0000 != 0 {
        (op & 0xFF).rotate_right(((op >> 8) & 0xF) * 2)
    } else {
        cpu.reg((op & 0xF) as usize)
    };
    // Field bits 19-16 select flags/status/extension/control bytes.
    let mut mask = 0u32;
    for (bit, m) in [(19, 0xFF00_0000u32), (18, 0x00FF_0000), (17, 0x0000_FF00), (16, 0x0000_00FF)] {
        if op & (1 << bit) != 0 {
            mask |= m;
        }
    }
    if spsr_sel {
        let s = cpu.spsr();
        cpu.set_spsr((s & !mask) | (v & mask));
    } else {
        if cpu.cpsr & 0x1F == MODE_USR {
            mask &= 0xFF00_0000;
        }
        // MSR never changes the CPSR T bit.
        let merged = (cpu.cpsr & !mask) | (v & mask);
        cpu.set_cpsr((merged & !FLAG_T) | (cpu.cpsr & FLAG_T));
    }
}

pub(crate) fn arm_multiply(cpu: &mut Arm7, op: u32) {
    let rm = cpu.reg((op & 0xF) as usize);
    let rs = cpu.reg(((op >> 8) & 0xF) as usize);
    let mut v = rm.wrapping_mul(rs);
    if op & 0x0020_0000 != 0 {
        v = v.wrapping_add(cpu.reg(((op >> 12) & 0xF) as usize));
    }
    cpu.set_reg(((op >> 16) & 0xF) as usize, v);
    if op & 0x0010_0000 != 0 {
        cpu.set_nz(v);
    }
    cpu.bus.idle();
}

pub(crate) fn arm_multiply_long(cpu: &mut Arm7, op: u32) {
    let rm = cpu.reg((op & 0xF) as usize);
    let rs = cpu.reg(((op >> 8) & 0xF) as usize);
    let rd_lo = ((op >> 12) & 0xF) as usize;
    let rd_hi = ((op >> 16) & 0xF) as usize;
    let mut v = if op & 0x0040_0000 != 0 {
        (rm as i32 as i64).wrapping_mul(rs as i32 as i64) as u64
    } else {
        (rm as u64) * (rs as u64)
    };
    if op & 0x0020_0000 != 0 {
        v = v.wrapping_add(((cpu.reg(rd_hi) as u64) << 32) | cpu.reg(rd_lo) as u64);
    }
    cpu.set_reg(rd_lo, v as u32);
    cpu.set_reg(rd_hi, (v >> 32) as u32);
    if op & 0x0010_0000 != 0 {
        cpu.set_flag(FLAG_N, v & (1 << 63) != 0);
        cpu.set_flag(FLAG_Z, v == 0);
    }
    cpu.bus.idle();
    cpu.bus.idle();
}

pub(crate) fn arm_swi(cpu: &mut Arm7, op: u32) {
    let _ = op;
    cpu.exception_swi();
}

#[cfg(test)]
mod tests {
    use crate::cpu::{
        test_arm, FLAG_C, FLAG_N, FLAG_T, FLAG_V, FLAG_Z, MODE_IRQ, MODE_SVC, MODE_USR,
    };

    #[test]
    fn b_forward() {
        let mut cpu = test_arm(&[0xEA00_0002]);
        cpu.step();
        assert_eq!(cpu.exec_pc(), 0x0800_00D0);
    }

    #[test]
    fn b_backward() {
        let mut cpu = test_arm(&[0xEAFF_FFFC]);
        cpu.step();
        assert_eq!(cpu.exec_pc(), 0x0800_00B8);
    }

    #[test]
    fn bl_sets_lr() {
        let mut cpu = test_arm(&[0xEB00_0002]);
        cpu.step();
        assert_eq!(cpu.r[14], 0x0800_00C4);
        assert_eq!(cpu.exec_pc(), 0x0800_00D0);
    }

    #[test]
    fn bx_to_thumb_halfword_aligns() {
        let mut cpu = test_arm(&[0xE12F_FF10]);
        cpu.r[0] = 0x0800_0103;
        cpu.step();
        assert!(cpu.cpsr & FLAG_T != 0);
        assert_eq!(cpu.exec_pc(), 0x0800_0102);
    }

    #[test]
    fn bx_arm_word_aligns() {
        let mut cpu = test_arm(&[0xE12F_FF10]);
        cpu.r[0] = 0x0800_0102;
        cpu.step();
        assert!(cpu.cpsr & FLAG_T == 0);
        assert_eq!(cpu.exec_pc(), 0x0800_0100);
    }

    #[test]
    fn mrs_cpsr() {
        let mut cpu = test_arm(&[0xE10F_2000]);
        cpu.cpsr |= FLAG_C;
        cpu.step();
        assert_eq!(cpu.r[2], FLAG_C | 0x1F);
    }

    #[test]
    fn mrs_spsr() {
        let mut cpu = test_arm(&[0xE14F_2000]);
        cpu.set_cpsr((cpu.cpsr & !0x1F) | MODE_SVC);
        cpu.set_spsr(0xF000_0011);
        cpu.step();
        assert_eq!(cpu.r[2], 0xF000_0011);
    }

    #[test]
    fn msr_cpsr_flags_immediate() {
        // MSR CPSR_f, #0xF0000000
        let mut cpu = test_arm(&[0xE328_F4F0]);
        cpu.step();
        assert_eq!(cpu.cpsr & 0xF000_0000, 0xF000_0000);
        assert_eq!(cpu.cpsr & 0x1F, 0x1F);
    }

    #[test]
    fn msr_cpsr_t_protected_and_banks_swap() {
        // MSR CPSR_c, r0
        let mut cpu = test_arm(&[0xE121_F000]);
        cpu.r[0] = FLAG_T | MODE_IRQ;
        cpu.step();
        assert_eq!(cpu.cpsr & 0x1F, MODE_IRQ);
        assert_eq!(cpu.cpsr & FLAG_T, 0);
        assert_eq!(cpu.r[13], 0x0300_7FA0);
    }

    #[test]
    fn msr_cpsr_usr_flags_only() {
        // MSR CPSR_fc, r0 in user mode: control byte ignored.
        let mut cpu = test_arm(&[0xE129_F000]);
        cpu.set_cpsr((cpu.cpsr & !0x1F) | MODE_USR);
        cpu.r[0] = 0xF000_0000 | MODE_SVC;
        cpu.step();
        assert_eq!(cpu.cpsr & 0xF000_0000, 0xF000_0000);
        assert_eq!(cpu.cpsr & 0x1F, MODE_USR);
    }

    #[test]
    fn msr_spsr_full_keeps_t() {
        // MSR SPSR_fsxc, r0: SPSR may hold T.
        let mut cpu = test_arm(&[0xE16F_F000]);
        cpu.set_cpsr((cpu.cpsr & !0x1F) | MODE_SVC);
        cpu.r[0] = FLAG_C | FLAG_T | MODE_IRQ;
        cpu.step();
        assert_eq!(cpu.spsr(), FLAG_C | FLAG_T | MODE_IRQ);
        assert_eq!(cpu.cpsr & 0x1F, MODE_SVC);
    }

    #[test]
    fn msr_spsr_sys_ignored() {
        let mut cpu = test_arm(&[0xE16F_F000]);
        cpu.r[0] = 0xDEAD_BEEF;
        let before = cpu.cpsr;
        cpu.step();
        assert_eq!(cpu.cpsr, before);
        assert_eq!(cpu.spsr(), before);
    }

    #[test]
    fn mul_wraps() {
        let mut cpu = test_arm(&[0xE002_0190]);
        cpu.r[0] = 0x1234_5678;
        cpu.r[1] = 0x100;
        cpu.step();
        assert_eq!(cpu.r[2], 0x3456_7800);
    }

    #[test]
    fn muls_nz_leaves_cv() {
        let mut cpu = test_arm(&[0xE012_0190]);
        cpu.cpsr |= FLAG_C | FLAG_V;
        cpu.r[0] = 0xFFFF_FFFF;
        cpu.r[1] = 1;
        cpu.step();
        assert!(cpu.cpsr & FLAG_N != 0);
        assert!(cpu.cpsr & FLAG_Z == 0);
        assert!(cpu.cpsr & FLAG_C != 0);
        assert!(cpu.cpsr & FLAG_V != 0);
    }

    #[test]
    fn muls_zero_sets_z() {
        let mut cpu = test_arm(&[0xE012_0190]);
        cpu.r[0] = 0;
        cpu.r[1] = 123;
        cpu.step();
        assert!(cpu.cpsr & FLAG_Z != 0);
        assert!(cpu.cpsr & FLAG_N == 0);
    }

    #[test]
    fn mla_accumulates() {
        let mut cpu = test_arm(&[0xE023_2190]);
        cpu.r[0] = 5;
        cpu.r[1] = 7;
        cpu.r[2] = 100;
        cpu.step();
        assert_eq!(cpu.r[3], 135);
    }

    #[test]
    fn umull() {
        let mut cpu = test_arm(&[0xE083_2190]);
        cpu.r[0] = 0xFFFF_FFFF;
        cpu.r[1] = 0xFFFF_FFFF;
        cpu.step();
        assert_eq!(cpu.r[2], 1);
        assert_eq!(cpu.r[3], 0xFFFF_FFFE);
    }

    #[test]
    fn smull_negative() {
        let mut cpu = test_arm(&[0xE0C3_2190]);
        cpu.r[0] = (-2i32) as u32;
        cpu.r[1] = 3;
        cpu.step();
        assert_eq!(cpu.r[2], 0xFFFF_FFFA);
        assert_eq!(cpu.r[3], 0xFFFF_FFFF);
    }

    #[test]
    fn umlal_accumulates() {
        let mut cpu = test_arm(&[0xE0A3_2190]);
        cpu.r[2] = 0xFFFF_FFFF;
        cpu.r[3] = 1;
        cpu.r[0] = 2;
        cpu.r[1] = 3;
        cpu.step();
        assert_eq!(cpu.r[2], 5);
        assert_eq!(cpu.r[3], 2);
    }

    #[test]
    fn smlal_accumulates() {
        let mut cpu = test_arm(&[0xE0E3_2190]);
        cpu.r[2] = 0xFFFF_FFF6;
        cpu.r[3] = 0xFFFF_FFFF;
        cpu.r[0] = (-1i32) as u32;
        cpu.r[1] = 4;
        cpu.step();
        assert_eq!(cpu.r[2], 0xFFFF_FFF2);
        assert_eq!(cpu.r[3], 0xFFFF_FFFF);
    }

    #[test]
    fn smulls_sets_n() {
        let mut cpu = test_arm(&[0xE0D3_2190]);
        cpu.r[0] = (-1i32) as u32;
        cpu.r[1] = 1;
        cpu.step();
        assert!(cpu.cpsr & FLAG_N != 0);
        assert!(cpu.cpsr & FLAG_Z == 0);
    }

    #[test]
    fn umulls_zero_sets_z() {
        let mut cpu = test_arm(&[0xE093_2190]);
        cpu.r[0] = 0;
        cpu.r[1] = 5;
        cpu.step();
        assert!(cpu.cpsr & FLAG_Z != 0);
    }

    #[test]
    fn umulls_z_needs_all_64_bits_zero() {
        let mut cpu = test_arm(&[0xE093_2190]);
        cpu.r[0] = 0x1_0000;
        cpu.r[1] = 0x1_0000;
        cpu.step();
        assert_eq!(cpu.r[2], 0);
        assert_eq!(cpu.r[3], 1);
        assert!(cpu.cpsr & FLAG_Z == 0);
    }
}
