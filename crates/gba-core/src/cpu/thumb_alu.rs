//! Thumb formats 1-5, 12, 13: shifts, ALU, hi-register ops, address calc.

use super::{Arm7, FLAG_C, FLAG_T, FLAG_V};

// ---- flag-setting arithmetic ---------------------------------------------

fn add_flags(cpu: &mut Arm7, a: u32, b: u32, c: u32) -> u32 {
    let wide = a as u64 + b as u64 + c as u64;
    let r = wide as u32;
    cpu.set_nz(r);
    cpu.set_flag(FLAG_C, wide > 0xFFFF_FFFF);
    cpu.set_flag(FLAG_V, (!(a ^ b) & (a ^ r)) & 0x8000_0000 != 0);
    r
}

/// `c` = 1 for SUB/CMP/NEG, current carry for SBC.
fn sub_flags(cpu: &mut Arm7, a: u32, b: u32, c: u32) -> u32 {
    let r = a.wrapping_sub(b).wrapping_sub(1 - c);
    cpu.set_nz(r);
    cpu.set_flag(FLAG_C, a as u64 >= b as u64 + (1 - c) as u64);
    cpu.set_flag(FLAG_V, ((a ^ b) & (a ^ r)) & 0x8000_0000 != 0);
    r
}

// ---- shifts (amount already resolved; 0 leaves C unchanged) ---------------

fn lsl_by(cpu: &mut Arm7, v: u32, amt: u32) -> u32 {
    match amt {
        0 => v,
        1..=31 => {
            cpu.set_flag(FLAG_C, (v >> (32 - amt)) & 1 != 0);
            v << amt
        }
        32 => {
            cpu.set_flag(FLAG_C, v & 1 != 0);
            0
        }
        _ => {
            cpu.set_flag(FLAG_C, false);
            0
        }
    }
}

fn lsr_by(cpu: &mut Arm7, v: u32, amt: u32) -> u32 {
    match amt {
        0 => v,
        1..=31 => {
            cpu.set_flag(FLAG_C, (v >> (amt - 1)) & 1 != 0);
            v >> amt
        }
        32 => {
            cpu.set_flag(FLAG_C, v & 0x8000_0000 != 0);
            0
        }
        _ => {
            cpu.set_flag(FLAG_C, false);
            0
        }
    }
}

fn asr_by(cpu: &mut Arm7, v: u32, amt: u32) -> u32 {
    match amt {
        0 => v,
        1..=31 => {
            cpu.set_flag(FLAG_C, (v >> (amt - 1)) & 1 != 0);
            ((v as i32) >> amt) as u32
        }
        _ => {
            cpu.set_flag(FLAG_C, v & 0x8000_0000 != 0);
            ((v as i32) >> 31) as u32
        }
    }
}

fn ror_by(cpu: &mut Arm7, v: u32, amt: u32) -> u32 {
    if amt == 0 {
        return v;
    }
    let r = amt & 31;
    if r == 0 {
        cpu.set_flag(FLAG_C, v & 0x8000_0000 != 0);
        v
    } else {
        cpu.set_flag(FLAG_C, (v >> (r - 1)) & 1 != 0);
        v.rotate_right(r)
    }
}

// ---- format 1: shift by immediate -----------------------------------------

pub(crate) fn thumb_shift_imm(cpu: &mut Arm7, op: u16) {
    let imm = ((op >> 6) & 0x1F) as u32;
    let rs = ((op >> 3) & 7) as usize;
    let rd = (op & 7) as usize;
    let v = cpu.r[rs];
    let r = match (op >> 11) & 3 {
        // LSL #0 passes the value with C unchanged.
        0 => lsl_by(cpu, v, imm),
        // Immediate 0 encodes a shift of 32.
        1 => lsr_by(cpu, v, if imm == 0 { 32 } else { imm }),
        _ => asr_by(cpu, v, if imm == 0 { 32 } else { imm }),
    };
    cpu.r[rd] = r;
    cpu.set_nz(r);
}

// ---- format 2: add/sub register or 3-bit immediate ------------------------

pub(crate) fn thumb_add_sub(cpu: &mut Arm7, op: u16) {
    let rd = (op & 7) as usize;
    let a = cpu.r[((op >> 3) & 7) as usize];
    let field = ((op >> 6) & 7) as u32;
    let b = if op & 0x0400 != 0 { field } else { cpu.r[field as usize] };
    cpu.r[rd] = if op & 0x0200 != 0 {
        sub_flags(cpu, a, b, 1)
    } else {
        add_flags(cpu, a, b, 0)
    };
}

// ---- format 3: MOV/CMP/ADD/SUB 8-bit immediate -----------------------------

pub(crate) fn thumb_imm_ops(cpu: &mut Arm7, op: u16) {
    let rd = ((op >> 8) & 7) as usize;
    let imm = (op & 0xFF) as u32;
    match (op >> 11) & 3 {
        0 => {
            cpu.r[rd] = imm;
            cpu.set_nz(imm);
        }
        1 => {
            sub_flags(cpu, cpu.r[rd], imm, 1);
        }
        2 => cpu.r[rd] = add_flags(cpu, cpu.r[rd], imm, 0),
        _ => cpu.r[rd] = sub_flags(cpu, cpu.r[rd], imm, 1),
    }
}

// ---- format 4: register ALU ops -------------------------------------------

pub(crate) fn thumb_alu_ops(cpu: &mut Arm7, op: u16) {
    let rd = (op & 7) as usize;
    let s = cpu.r[((op >> 3) & 7) as usize];
    let d = cpu.r[rd];
    let c = cpu.flag(FLAG_C) as u32;
    let logical = |cpu: &mut Arm7, r: u32| {
        cpu.r[rd] = r;
        cpu.set_nz(r);
    };
    match (op >> 6) & 0xF {
        0x0 => logical(cpu, d & s),
        0x1 => logical(cpu, d ^ s),
        0x2 => {
            cpu.bus.idle();
            let r = lsl_by(cpu, d, s & 0xFF);
            logical(cpu, r);
        }
        0x3 => {
            cpu.bus.idle();
            let r = lsr_by(cpu, d, s & 0xFF);
            logical(cpu, r);
        }
        0x4 => {
            cpu.bus.idle();
            let r = asr_by(cpu, d, s & 0xFF);
            logical(cpu, r);
        }
        0x5 => cpu.r[rd] = add_flags(cpu, d, s, c),
        0x6 => cpu.r[rd] = sub_flags(cpu, d, s, c),
        0x7 => {
            cpu.bus.idle();
            let r = ror_by(cpu, d, s & 0xFF);
            logical(cpu, r);
        }
        0x8 => cpu.set_nz(d & s),
        0x9 => cpu.r[rd] = sub_flags(cpu, 0, s, 1),
        0xA => {
            sub_flags(cpu, d, s, 1);
        }
        0xB => {
            add_flags(cpu, d, s, 0);
        }
        0xC => logical(cpu, d | s),
        0xD => {
            cpu.bus.idle();
            logical(cpu, d.wrapping_mul(s));
        }
        0xE => logical(cpu, d & !s),
        _ => logical(cpu, !s),
    }
}

// ---- format 5: hi-register ADD/CMP/MOV/BX ----------------------------------

pub(crate) fn thumb_hi_reg_bx(cpu: &mut Arm7, op: u16) {
    let rd = ((op & 7) | ((op >> 4) & 8)) as usize;
    let rs = ((op >> 3) & 0xF) as usize;
    match (op >> 8) & 3 {
        0 => {
            let v = cpu.reg(rd).wrapping_add(cpu.reg(rs));
            cpu.set_reg(rd, v);
        }
        1 => {
            sub_flags(cpu, cpu.reg(rd), cpu.reg(rs), 1);
        }
        2 => {
            let v = cpu.reg(rs);
            cpu.set_reg(rd, v);
        }
        _ => {
            let t = cpu.reg(rs);
            cpu.set_flag(FLAG_T, t & 1 != 0);
            cpu.branch(t);
        }
    }
}

// ---- format 12: address calculation ----------------------------------------

pub(crate) fn thumb_addr_calc(cpu: &mut Arm7, op: u16) {
    let rd = ((op >> 8) & 7) as usize;
    let imm = ((op & 0xFF) as u32) << 2;
    let base = if op & 0x0800 != 0 { cpu.r[13] } else { cpu.r[15] & !2 };
    cpu.r[rd] = base.wrapping_add(imm);
}

// ---- format 13: SP adjust ---------------------------------------------------

pub(crate) fn thumb_sp_adjust(cpu: &mut Arm7, op: u16) {
    let imm = ((op & 0x7F) as u32) << 2;
    cpu.r[13] = if op & 0x80 != 0 {
        cpu.r[13].wrapping_sub(imm)
    } else {
        cpu.r[13].wrapping_add(imm)
    };
}

#[cfg(test)]
mod tests {
    use crate::cpu::{test_thumb, FLAG_C, FLAG_N, FLAG_T, FLAG_V, FLAG_Z};

    // ---- format 1 ----------------------------------------------------------

    #[test]
    fn fmt1_lsl_zero_keeps_carry() {
        // lsl r0, r1, #0
        let mut cpu = test_thumb(&[0x0008]);
        cpu.cpsr |= FLAG_C;
        cpu.r[1] = 0x8000_0000;
        cpu.step();
        assert_eq!(cpu.r[0], 0x8000_0000);
        assert!(cpu.flag(FLAG_C));
        assert!(cpu.flag(FLAG_N));
        assert!(!cpu.flag(FLAG_Z));
    }

    #[test]
    fn fmt1_lsl_shifts_carry_out() {
        // lsl r0, r1, #1
        let mut cpu = test_thumb(&[0x0048]);
        cpu.r[1] = 0x8000_0001;
        cpu.step();
        assert_eq!(cpu.r[0], 2);
        assert!(cpu.flag(FLAG_C));
        assert!(!cpu.flag(FLAG_N));
    }

    #[test]
    fn fmt1_lsr_zero_means_lsr32() {
        // lsr r0, r1, #0
        let mut cpu = test_thumb(&[0x0808]);
        cpu.r[1] = 0x8000_0000;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn fmt1_asr_zero_means_asr32() {
        // asr r0, r1, #0
        let mut cpu = test_thumb(&[0x1008]);
        cpu.r[1] = 0x8000_0000;
        cpu.step();
        assert_eq!(cpu.r[0], 0xFFFF_FFFF);
        assert!(cpu.flag(FLAG_N));
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn fmt1_asr_zero_positive_input() {
        // asr r0, r1, #0
        let mut cpu = test_thumb(&[0x1008]);
        cpu.cpsr |= FLAG_C;
        cpu.r[1] = 0x7FFF_FFFF;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_Z));
        assert!(!cpu.flag(FLAG_C));
    }

    // ---- format 2 ----------------------------------------------------------

    #[test]
    fn fmt2_add_reg_overflow() {
        // add r0, r1, r2
        let mut cpu = test_thumb(&[0x1888]);
        cpu.r[1] = 0x7FFF_FFFF;
        cpu.r[2] = 1;
        cpu.step();
        assert_eq!(cpu.r[0], 0x8000_0000);
        assert!(cpu.flag(FLAG_N));
        assert!(cpu.flag(FLAG_V));
        assert!(!cpu.flag(FLAG_C));
    }

    #[test]
    fn fmt2_sub_imm_borrow() {
        // sub r0, r1, #1
        let mut cpu = test_thumb(&[0x1E48]);
        cpu.r[1] = 0;
        cpu.step();
        assert_eq!(cpu.r[0], 0xFFFF_FFFF);
        assert!(!cpu.flag(FLAG_C));
        assert!(cpu.flag(FLAG_N));
        assert!(!cpu.flag(FLAG_V));
    }

    #[test]
    fn fmt2_sub_reg_equal_sets_cz() {
        // sub r0, r1, r2
        let mut cpu = test_thumb(&[0x1A88]);
        cpu.r[1] = 7;
        cpu.r[2] = 7;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
    }

    // ---- format 3 ----------------------------------------------------------

    #[test]
    fn fmt3_mov_keeps_cv() {
        // mov r0, #0
        let mut cpu = test_thumb(&[0x2000]);
        cpu.cpsr |= FLAG_C | FLAG_V;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
        assert!(cpu.flag(FLAG_V));
    }

    #[test]
    fn fmt3_cmp_flags() {
        // cmp r0, #5 twice with different r0
        let mut cpu = test_thumb(&[0x2805, 0x2805]);
        cpu.r[0] = 5;
        cpu.step();
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
        cpu.r[0] = 4;
        cpu.step();
        assert!(!cpu.flag(FLAG_C));
        assert!(cpu.flag(FLAG_N));
        assert_eq!(cpu.r[0], 4);
    }

    #[test]
    fn fmt3_add_sub_imm8() {
        // add r0, #0xFF ; sub r0, #1
        let mut cpu = test_thumb(&[0x30FF, 0x3801]);
        cpu.r[0] = 0xFFFF_FF01;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
        cpu.step();
        assert_eq!(cpu.r[0], 0xFFFF_FFFF);
        assert!(!cpu.flag(FLAG_C));
    }

    // ---- format 4 shifts by register ---------------------------------------

    #[test]
    fn fmt4_lsl_amount_low_byte_only() {
        // lsl r0, r1 with r1 = 0x100: low byte 0 shifts nothing, C kept.
        let mut cpu = test_thumb(&[0x4088]);
        cpu.cpsr |= FLAG_C;
        cpu.r[0] = 0x1234;
        cpu.r[1] = 0x100;
        cpu.step();
        assert_eq!(cpu.r[0], 0x1234);
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn fmt4_lsl_by_32_and_more() {
        // lsl r0, r1
        let mut cpu = test_thumb(&[0x4088, 0x4098]);
        cpu.r[0] = 1;
        cpu.r[1] = 32;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
        // lsl r0, r3 with amount 33 clears C.
        cpu.r[0] = 0xFFFF_FFFF;
        cpu.r[3] = 33;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(!cpu.flag(FLAG_C));
    }

    #[test]
    fn fmt4_lsr_by_32() {
        // lsr r0, r1
        let mut cpu = test_thumb(&[0x40C8]);
        cpu.r[0] = 0x8000_0000;
        cpu.r[1] = 32;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_C));
        assert!(cpu.flag(FLAG_Z));
    }

    #[test]
    fn fmt4_asr_over_32_sign_fills() {
        // asr r0, r1
        let mut cpu = test_thumb(&[0x4108]);
        cpu.r[0] = 0x8000_0000;
        cpu.r[1] = 200;
        cpu.step();
        assert_eq!(cpu.r[0], 0xFFFF_FFFF);
        assert!(cpu.flag(FLAG_C));
        assert!(cpu.flag(FLAG_N));
    }

    #[test]
    fn fmt4_ror_specials() {
        // ror r0, r1
        let mut cpu = test_thumb(&[0x41C8, 0x41C8, 0x41C8]);
        // Amount 0: value and C unchanged.
        cpu.cpsr |= FLAG_C;
        cpu.r[0] = 0x1234_5678;
        cpu.r[1] = 0;
        cpu.step();
        assert_eq!(cpu.r[0], 0x1234_5678);
        assert!(cpu.flag(FLAG_C));
        // Amount 32: value unchanged, C = bit31.
        cpu.r[0] = 0x7FFF_FFFF;
        cpu.r[1] = 32;
        cpu.step();
        assert_eq!(cpu.r[0], 0x7FFF_FFFF);
        assert!(!cpu.flag(FLAG_C));
        // Amount 33 rotates by 1.
        cpu.r[0] = 1;
        cpu.r[1] = 33;
        cpu.step();
        assert_eq!(cpu.r[0], 0x8000_0000);
        assert!(cpu.flag(FLAG_C));
    }

    // ---- format 4 arithmetic/logic ------------------------------------------

    #[test]
    fn fmt4_adc_uses_carry() {
        // adc r0, r1
        let mut cpu = test_thumb(&[0x4148]);
        cpu.cpsr |= FLAG_C;
        cpu.r[0] = 0xFFFF_FFFF;
        cpu.r[1] = 0;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
        assert!(!cpu.flag(FLAG_V));
    }

    #[test]
    fn fmt4_sbc_borrow_when_carry_clear() {
        // sbc r0, r1
        let mut cpu = test_thumb(&[0x4188]);
        cpu.r[0] = 5;
        cpu.r[1] = 3;
        cpu.step();
        assert_eq!(cpu.r[0], 1);
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn fmt4_neg_edge_cases() {
        // neg r0, r1 three times
        let mut cpu = test_thumb(&[0x4248, 0x4248, 0x4248]);
        cpu.r[1] = 1;
        cpu.step();
        assert_eq!(cpu.r[0], 0xFFFF_FFFF);
        assert!(!cpu.flag(FLAG_C));
        cpu.r[1] = 0;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_C));
        assert!(cpu.flag(FLAG_Z));
        cpu.r[1] = 0x8000_0000;
        cpu.step();
        assert_eq!(cpu.r[0], 0x8000_0000);
        assert!(cpu.flag(FLAG_V));
        assert!(!cpu.flag(FLAG_C));
    }

    #[test]
    fn fmt4_mul_sets_nz_only() {
        // mul r0, r1
        let mut cpu = test_thumb(&[0x4348]);
        cpu.cpsr |= FLAG_C | FLAG_V;
        cpu.r[0] = 0x8000_0000;
        cpu.r[1] = 1;
        cpu.step();
        assert_eq!(cpu.r[0], 0x8000_0000);
        assert!(cpu.flag(FLAG_N));
        assert!(cpu.flag(FLAG_C));
        assert!(cpu.flag(FLAG_V));
    }

    #[test]
    fn fmt4_tst_does_not_write() {
        // tst r0, r1
        let mut cpu = test_thumb(&[0x4208]);
        cpu.r[0] = 0xF0;
        cpu.r[1] = 0x0F;
        cpu.step();
        assert_eq!(cpu.r[0], 0xF0);
        assert!(cpu.flag(FLAG_Z));
    }

    #[test]
    fn fmt4_cmn_and_bic_mvn() {
        // cmn r0, r1 ; bic r2, r3 ; mvn r4, r5
        let mut cpu = test_thumb(&[0x42C8, 0x439A, 0x43EC]);
        cpu.r[0] = 1;
        cpu.r[1] = 0xFFFF_FFFF;
        cpu.step();
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
        assert_eq!(cpu.r[0], 1);
        cpu.r[2] = 0xFF;
        cpu.r[3] = 0x0F;
        cpu.step();
        assert_eq!(cpu.r[2], 0xF0);
        cpu.r[5] = 0;
        cpu.step();
        assert_eq!(cpu.r[4], 0xFFFF_FFFF);
        assert!(cpu.flag(FLAG_N));
    }

    // ---- format 5 ------------------------------------------------------------

    #[test]
    fn fmt5_add_hi_no_flags() {
        // add r0, r8
        let mut cpu = test_thumb(&[0x4440]);
        cpu.r[0] = 0x7FFF_FFFF;
        cpu.r[8] = 1;
        cpu.step();
        assert_eq!(cpu.r[0], 0x8000_0000);
        assert!(!cpu.flag(FLAG_N));
        assert!(!cpu.flag(FLAG_V));
    }

    #[test]
    fn fmt5_cmp_hi_sets_flags() {
        // cmp r8, r9
        let mut cpu = test_thumb(&[0x45C8]);
        cpu.r[8] = 5;
        cpu.r[9] = 5;
        cpu.step();
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn fmt5_mov_from_pc_reads_exec_plus_4() {
        // mov r0, r15
        let mut cpu = test_thumb(&[0x4678]);
        cpu.step();
        assert_eq!(cpu.r[0], 0x0800_00C4);
    }

    #[test]
    fn fmt5_add_pc_branches() {
        // add r15, r0
        let mut cpu = test_thumb(&[0x4487]);
        cpu.r[0] = 4;
        cpu.step();
        assert_eq!(cpu.exec_pc(), 0x0800_00C8);
        assert!(cpu.thumb());
    }

    #[test]
    fn fmt5_mov_pc_ignores_bit0_keeps_thumb() {
        // mov r15, r0
        let mut cpu = test_thumb(&[0x4687]);
        cpu.r[0] = 0x0800_00D1;
        cpu.step();
        assert_eq!(cpu.exec_pc(), 0x0800_00D0);
        assert!(cpu.thumb());
    }

    #[test]
    fn fmt5_bx_to_arm() {
        // bx r0
        let mut cpu = test_thumb(&[0x4700]);
        cpu.r[0] = 0x0800_0100;
        cpu.step();
        assert!(!cpu.thumb());
        assert!(!cpu.flag(FLAG_T));
        assert_eq!(cpu.exec_pc(), 0x0800_0100);
    }

    #[test]
    fn fmt5_bx_bit0_stays_thumb() {
        // bx r0
        let mut cpu = test_thumb(&[0x4700]);
        cpu.r[0] = 0x0800_0101;
        cpu.step();
        assert!(cpu.thumb());
        assert_eq!(cpu.exec_pc(), 0x0800_0100);
    }

    // ---- formats 12 and 13 -----------------------------------------------------

    #[test]
    fn fmt12_add_pc_masks_bit1() {
        // mov r1, #0 ; add r0, pc, #0 (second slot: pc = 0xC6, masked to 0xC4)
        let mut cpu = test_thumb(&[0x2100, 0xA000]);
        cpu.step();
        cpu.step();
        assert_eq!(cpu.r[0], 0x0800_00C4);
    }

    #[test]
    fn fmt12_add_pc_imm() {
        // add r0, pc, #16
        let mut cpu = test_thumb(&[0xA004]);
        cpu.step();
        assert_eq!(cpu.r[0], 0x0800_00D4);
    }

    #[test]
    fn fmt12_add_sp() {
        // add r0, sp, #8
        let mut cpu = test_thumb(&[0xA802]);
        cpu.r[13] = 0x0300_7F00;
        cpu.cpsr |= FLAG_C;
        cpu.step();
        assert_eq!(cpu.r[0], 0x0300_7F08);
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn fmt13_sp_adjust() {
        // add sp, #4 ; sub sp, #8
        let mut cpu = test_thumb(&[0xB001, 0xB082]);
        cpu.r[13] = 0x0300_7F00;
        cpu.step();
        assert_eq!(cpu.r[13], 0x0300_7F04);
        cpu.step();
        assert_eq!(cpu.r[13], 0x0300_7EFC);
        assert!(!cpu.flag(FLAG_Z));
    }
}
