//! Barrel shifter and ARM data-processing instructions.

use super::{Arm7, FLAG_C, FLAG_V};

/// Shift by immediate amount for addressing modes; no carry-out.
pub(crate) fn shift_by_imm(cpu: &Arm7, rm_val: u32, shift_type: u32, amount: u32) -> u32 {
    shift_imm(rm_val, shift_type, amount, cpu.flag(FLAG_C)).0
}

/// Shift-by-immediate with amount==0 special forms; returns (value, carry).
fn shift_imm(v: u32, ty: u32, amount: u32, c: bool) -> (u32, bool) {
    match ty {
        0 => {
            if amount == 0 {
                (v, c)
            } else {
                (v << amount, v & (1 << (32 - amount)) != 0)
            }
        }
        1 => {
            // LSR #0 encodes LSR #32.
            if amount == 0 {
                (0, v >> 31 != 0)
            } else {
                (v >> amount, v & (1 << (amount - 1)) != 0)
            }
        }
        2 => {
            // ASR #0 encodes ASR #32.
            if amount == 0 {
                (((v as i32) >> 31) as u32, v >> 31 != 0)
            } else {
                (((v as i32) >> amount) as u32, v & (1 << (amount - 1)) != 0)
            }
        }
        _ => {
            // ROR #0 encodes RRX.
            if amount == 0 {
                (((c as u32) << 31) | (v >> 1), v & 1 != 0)
            } else {
                let r = v.rotate_right(amount);
                (r, r >> 31 != 0)
            }
        }
    }
}

/// Shift by register amount (low byte of Rs); returns (value, carry).
fn shift_reg(v: u32, ty: u32, n: u32, c: bool) -> (u32, bool) {
    if n == 0 {
        return (v, c);
    }
    match ty {
        0 => match n {
            1..=31 => (v << n, v & (1 << (32 - n)) != 0),
            32 => (0, v & 1 != 0),
            _ => (0, false),
        },
        1 => match n {
            1..=31 => (v >> n, v & (1 << (n - 1)) != 0),
            32 => (0, v >> 31 != 0),
            _ => (0, false),
        },
        2 => {
            if n < 32 {
                (((v as i32) >> n) as u32, v & (1 << (n - 1)) != 0)
            } else {
                (((v as i32) >> 31) as u32, v >> 31 != 0)
            }
        }
        _ => {
            // ROR by a multiple of 32 leaves the value, carry = bit31.
            let n = n & 31;
            if n == 0 {
                (v, v >> 31 != 0)
            } else {
                let r = v.rotate_right(n);
                (r, r >> 31 != 0)
            }
        }
    }
}

/// a + b + cin; sets NZCV when `set`. Subtraction passes !b.
fn adc(cpu: &mut Arm7, a: u32, b: u32, cin: u32, set: bool) -> u32 {
    let r64 = a as u64 + b as u64 + cin as u64;
    let r = r64 as u32;
    if set {
        cpu.set_nz(r);
        cpu.set_flag(FLAG_C, r64 >> 32 != 0);
        cpu.set_flag(FLAG_V, (!(a ^ b) & (a ^ r)) >> 31 != 0);
    }
    r
}

pub(crate) fn arm_data_processing(cpu: &mut Arm7, op: u32) {
    let opcode = (op >> 21) & 0xF;
    let s = op & (1 << 20) != 0;
    let rn = ((op >> 16) & 0xF) as usize;
    let rd = ((op >> 12) & 0xF) as usize;

    let mut rn_val = cpu.reg(rn);
    let mut shifter_c = cpu.flag(FLAG_C);

    let op2 = if op & (1 << 25) != 0 {
        // Immediate: imm8 ror 2*rot; rot!=0 sets carry from result bit31.
        let rot = (op >> 8) & 0xF;
        let v = (op & 0xFF).rotate_right(2 * rot);
        if rot != 0 {
            shifter_c = v >> 31 != 0;
        }
        v
    } else {
        let ty = (op >> 5) & 3;
        let mut rm_val = cpu.reg((op & 0xF) as usize);
        if op & 0x10 != 0 {
            // Shift by register: r15 reads as +12, one internal cycle.
            let n = cpu.reg(((op >> 8) & 0xF) as usize) & 0xFF;
            if op & 0xF == 15 {
                rm_val = rm_val.wrapping_add(4);
            }
            if rn == 15 {
                rn_val = rn_val.wrapping_add(4);
            }
            cpu.bus.idle();
            let (v, c) = shift_reg(rm_val, ty, n, shifter_c);
            shifter_c = c;
            v
        } else {
            let (v, c) = shift_imm(rm_val, ty, (op >> 7) & 0x1F, shifter_c);
            shifter_c = c;
            v
        }
    };

    let test_op = (0x8..=0xB).contains(&opcode);
    // Rd==15 forms take flags wholesale from SPSR instead (ARM26 P-forms).
    let set = s && rd != 15;
    let cin = cpu.flag(FLAG_C) as u32;

    let result = match opcode {
        0x0 | 0x8 => rn_val & op2,                    // AND/TST
        0x1 | 0x9 => rn_val ^ op2,                    // EOR/TEQ
        0x2 | 0xA => adc(cpu, rn_val, !op2, 1, set),  // SUB/CMP
        0x3 => adc(cpu, op2, !rn_val, 1, set),        // RSB
        0x4 | 0xB => adc(cpu, rn_val, op2, 0, set),   // ADD/CMN
        0x5 => adc(cpu, rn_val, op2, cin, set),       // ADC
        0x6 => adc(cpu, rn_val, !op2, cin, set),      // SBC
        0x7 => adc(cpu, op2, !rn_val, cin, set),      // RSC
        0xC => rn_val | op2,                          // ORR
        0xD => op2,                                   // MOV
        0xE => rn_val & !op2,                         // BIC
        _ => !op2,                                    // MVN
    };

    // Logical ops: C = shifter carry, V unchanged.
    let logical = matches!(opcode, 0x0 | 0x1 | 0x8 | 0x9 | 0xC | 0xD | 0xE | 0xF);
    if set && logical {
        cpu.set_nz(result);
        cpu.set_flag(FLAG_C, shifter_c);
    }

    if test_op {
        if rd == 15 {
            cpu.restore_cpsr_from_spsr();
        }
    } else {
        // Rd==15 with S restores SPSR before the branch so T takes effect.
        if rd == 15 && s {
            cpu.restore_cpsr_from_spsr();
        }
        cpu.set_reg(rd, result);
    }
}

#[cfg(test)]
mod tests {
    use crate::cpu::{test_arm, FLAG_C, FLAG_N, FLAG_T, FLAG_V, FLAG_Z, MODE_SVC, MODE_SYS};

    fn dp(i: u32, opcode: u32, s: u32, rn: u32, rd: u32, op2: u32) -> u32 {
        0xE000_0000 | i << 25 | opcode << 21 | s << 20 | rn << 16 | rd << 12 | op2
    }

    #[test]
    fn imm_rotate_sets_carry() {
        // MOVS r0, #0x80000000 (imm 2 ror 2)
        let mut cpu = test_arm(&[dp(1, 0xD, 1, 0, 0, 0x102)]);
        cpu.step();
        assert_eq!(cpu.r[0], 0x8000_0000);
        assert!(cpu.flag(FLAG_N));
        assert!(cpu.flag(FLAG_C));
        assert!(!cpu.flag(FLAG_Z));
    }

    #[test]
    fn imm_no_rotate_keeps_carry() {
        // MOVS r0, #0 with C set: rot==0 leaves C alone.
        let mut cpu = test_arm(&[dp(1, 0xD, 1, 0, 0, 0x000)]);
        cpu.set_flag(FLAG_C, true);
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn lsl0_passes_value_and_carry() {
        // MOVS r0, r1
        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 0, 0x001)]);
        cpu.r[1] = 0x1234;
        cpu.set_flag(FLAG_C, true);
        cpu.step();
        assert_eq!(cpu.r[0], 0x1234);
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn lsl_imm_carry_out() {
        // MOVS r0, r1, LSL #1
        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 0, 0x081)]);
        cpu.r[1] = 0x8000_0001;
        cpu.step();
        assert_eq!(cpu.r[0], 2);
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn lsr_imm_0_is_lsr32() {
        // MOVS r0, r1, LSR #0
        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 0, 0x021)]);
        cpu.r[1] = 0x8000_0000;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn asr_imm_0_sign_fills() {
        // MOVS r0, r1, ASR #0
        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 0, 0x041)]);
        cpu.r[1] = 0x8000_0000;
        cpu.step();
        assert_eq!(cpu.r[0], 0xFFFF_FFFF);
        assert!(cpu.flag(FLAG_C));
        assert!(cpu.flag(FLAG_N));

        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 0, 0x041)]);
        cpu.r[1] = 0x7FFF_FFFF;
        cpu.set_flag(FLAG_C, true);
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(!cpu.flag(FLAG_C));
    }

    #[test]
    fn ror_imm_0_is_rrx() {
        // MOVS r0, r1, ROR #0
        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 0, 0x061)]);
        cpu.r[1] = 1;
        cpu.set_flag(FLAG_C, true);
        cpu.step();
        assert_eq!(cpu.r[0], 0x8000_0000);
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn shift_reg_zero_amount_unchanged() {
        // MOVS r0, r1, LSR r2 with r2 low byte 0 (0x100 masks to 0).
        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 0, 0x231)]);
        cpu.r[1] = 0xFFFF_FFFF;
        cpu.r[2] = 0x100;
        cpu.set_flag(FLAG_C, true);
        cpu.step();
        assert_eq!(cpu.r[0], 0xFFFF_FFFF);
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn lsl_reg_32_and_over() {
        // MOVS r0, r1, LSL r2
        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 0, 0x211)]);
        cpu.r[1] = 0x8000_0001;
        cpu.r[2] = 32;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_C));

        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 0, 0x211)]);
        cpu.r[1] = 0x8000_0001;
        cpu.r[2] = 33;
        cpu.set_flag(FLAG_C, true);
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(!cpu.flag(FLAG_C));
    }

    #[test]
    fn lsr_reg_32_and_over() {
        // MOVS r0, r1, LSR r2
        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 0, 0x231)]);
        cpu.r[1] = 0x8000_0000;
        cpu.r[2] = 32;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_C));

        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 0, 0x231)]);
        cpu.r[1] = 0x8000_0000;
        cpu.r[2] = 33;
        cpu.set_flag(FLAG_C, true);
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(!cpu.flag(FLAG_C));
    }

    #[test]
    fn asr_reg_ge32_sign_fills() {
        // MOVS r0, r1, ASR r2
        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 0, 0x251)]);
        cpu.r[1] = 0x8000_0000;
        cpu.r[2] = 100;
        cpu.step();
        assert_eq!(cpu.r[0], 0xFFFF_FFFF);
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn ror_reg_32_and_over() {
        // MOVS r0, r1, ROR r2: exactly 32 leaves value, C = bit31.
        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 0, 0x271)]);
        cpu.r[1] = 0x8000_0001;
        cpu.r[2] = 32;
        cpu.step();
        assert_eq!(cpu.r[0], 0x8000_0001);
        assert!(cpu.flag(FLAG_C));

        // 33 rotates by 1.
        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 0, 0x271)]);
        cpu.r[1] = 3;
        cpu.r[2] = 33;
        cpu.step();
        assert_eq!(cpu.r[0], 0x8000_0001);
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn reg_shift_reads_r15_plus_12() {
        // ADD r0, r15, r15, LSL r2 with r2=0: both operands read exec+12.
        let mut cpu = test_arm(&[dp(0, 0x4, 0, 15, 0, 0x21F)]);
        cpu.r[2] = 0;
        cpu.step();
        assert_eq!(cpu.r[0], 2 * 0x0800_00CC);
    }

    #[test]
    fn imm_shift_reads_r15_plus_8() {
        // MOV r0, r15
        let mut cpu = test_arm(&[dp(0, 0xD, 0, 0, 0, 0x00F)]);
        cpu.step();
        assert_eq!(cpu.r[0], 0x0800_00C8);
    }

    #[test]
    fn reg_shift_takes_extra_cycle() {
        let mut plain = test_arm(&[dp(0, 0xD, 0, 0, 0, 0x001)]);
        let mut shifted = test_arm(&[dp(0, 0xD, 0, 0, 0, 0x211)]);
        let base = plain.step();
        assert_eq!(shifted.step(), base + 1);
    }

    #[test]
    fn sub_flags() {
        // SUBS r0, r1, r2
        let op = dp(0, 0x2, 1, 1, 0, 0x002);
        let mut cpu = test_arm(&[op]);
        cpu.r[1] = 5;
        cpu.r[2] = 5;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
        assert!(!cpu.flag(FLAG_V));

        let mut cpu = test_arm(&[op]);
        cpu.r[1] = 0;
        cpu.r[2] = 1;
        cpu.step();
        assert_eq!(cpu.r[0], 0xFFFF_FFFF);
        assert!(!cpu.flag(FLAG_C));
        assert!(cpu.flag(FLAG_N));

        let mut cpu = test_arm(&[op]);
        cpu.r[1] = 0x8000_0000;
        cpu.r[2] = 1;
        cpu.step();
        assert_eq!(cpu.r[0], 0x7FFF_FFFF);
        assert!(cpu.flag(FLAG_V));
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn add_flags() {
        // ADDS r0, r1, r2
        let op = dp(0, 0x4, 1, 1, 0, 0x002);
        let mut cpu = test_arm(&[op]);
        cpu.r[1] = 0x7FFF_FFFF;
        cpu.r[2] = 1;
        cpu.step();
        assert_eq!(cpu.r[0], 0x8000_0000);
        assert!(cpu.flag(FLAG_V));
        assert!(!cpu.flag(FLAG_C));
        assert!(cpu.flag(FLAG_N));

        let mut cpu = test_arm(&[op]);
        cpu.r[1] = 0xFFFF_FFFF;
        cpu.r[2] = 1;
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_C));
        assert!(cpu.flag(FLAG_Z));
        assert!(!cpu.flag(FLAG_V));
    }

    #[test]
    fn adc_uses_c_flag_not_shifter_carry() {
        // ADCS r0, r1, r2, LSR #1 with C=0: shifter carry-out must not feed the add.
        let mut cpu = test_arm(&[dp(0, 0x5, 1, 1, 0, 0x0A2)]);
        cpu.r[1] = 1;
        cpu.r[2] = 3;
        cpu.step();
        assert_eq!(cpu.r[0], 2);
        assert!(!cpu.flag(FLAG_C));

        // ADCS r0, r1, r2 with C=1 adds the carry.
        let mut cpu = test_arm(&[dp(0, 0x5, 1, 1, 0, 0x002)]);
        cpu.r[1] = 1;
        cpu.r[2] = 2;
        cpu.set_flag(FLAG_C, true);
        cpu.step();
        assert_eq!(cpu.r[0], 4);
    }

    #[test]
    fn sbc_subtracts_not_c() {
        // SBCS r0, r1, r2
        let op = dp(0, 0x6, 1, 1, 0, 0x002);
        let mut cpu = test_arm(&[op]);
        cpu.r[1] = 5;
        cpu.r[2] = 3;
        cpu.step();
        assert_eq!(cpu.r[0], 1);
        assert!(cpu.flag(FLAG_C));

        let mut cpu = test_arm(&[op]);
        cpu.r[1] = 5;
        cpu.r[2] = 3;
        cpu.set_flag(FLAG_C, true);
        cpu.step();
        assert_eq!(cpu.r[0], 2);
    }

    #[test]
    fn rsb_rsc_reverse_operands() {
        // RSB r0, r1, r2
        let mut cpu = test_arm(&[dp(0, 0x3, 0, 1, 0, 0x002)]);
        cpu.r[1] = 3;
        cpu.r[2] = 10;
        cpu.step();
        assert_eq!(cpu.r[0], 7);

        // RSC r0, r1, r2 with C=0: r2 - r1 - 1.
        let mut cpu = test_arm(&[dp(0, 0x7, 0, 1, 0, 0x002)]);
        cpu.r[1] = 3;
        cpu.r[2] = 10;
        cpu.step();
        assert_eq!(cpu.r[0], 6);
    }

    #[test]
    fn logical_s_sets_shifter_carry_keeps_v() {
        // ANDS r0, r1, r2, LSL #1 with V preset.
        let mut cpu = test_arm(&[dp(0, 0x0, 1, 1, 0, 0x082)]);
        cpu.r[1] = 0xFFFF_FFFF;
        cpu.r[2] = 0x8000_0000;
        cpu.set_flag(FLAG_V, true);
        cpu.step();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
        assert!(cpu.flag(FLAG_V));
    }

    #[test]
    fn test_ops_never_write_rd() {
        // TST r1, #1 with Rd=2; CMP r1, #5 with Rd=3.
        let mut cpu = test_arm(&[dp(1, 0x8, 1, 1, 2, 0x001), dp(1, 0xA, 1, 1, 3, 0x005)]);
        cpu.r[1] = 5;
        cpu.r[2] = 0xDEAD_BEEF;
        cpu.r[3] = 0xCAFE_F00D;
        cpu.step();
        assert_eq!(cpu.r[2], 0xDEAD_BEEF);
        assert!(!cpu.flag(FLAG_Z));
        cpu.step();
        assert_eq!(cpu.r[3], 0xCAFE_F00D);
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn teq_cmn_set_flags() {
        // TEQ r1, r1; CMN r1, r2.
        let mut cpu = test_arm(&[dp(0, 0x9, 1, 1, 0, 0x001), dp(0, 0xB, 1, 1, 0, 0x002)]);
        cpu.r[1] = 0xAAAA_5555;
        cpu.r[2] = 0x5555_AAAB;
        cpu.step();
        assert!(cpu.flag(FLAG_Z));
        cpu.step();
        assert!(cpu.flag(FLAG_Z));
        assert!(cpu.flag(FLAG_C));
    }

    #[test]
    fn orr_eor_bic_mvn() {
        let mut cpu = test_arm(&[
            dp(0, 0xC, 0, 1, 0, 0x002), // ORR r0, r1, r2
            dp(0, 0x1, 0, 1, 3, 0x002), // EOR r3, r1, r2
            dp(0, 0xE, 0, 1, 4, 0x002), // BIC r4, r1, r2
            dp(0, 0xF, 0, 0, 5, 0x002), // MVN r5, r2
        ]);
        cpu.r[1] = 0xFF00_FF00;
        cpu.r[2] = 0x0F0F_0F0F;
        cpu.step();
        cpu.step();
        cpu.step();
        cpu.step();
        assert_eq!(cpu.r[0], 0xFF0F_FF0F);
        assert_eq!(cpu.r[3], 0xF00F_F00F);
        assert_eq!(cpu.r[4], 0xF000_F000);
        assert_eq!(cpu.r[5], 0xF0F0_F0F0);
    }

    #[test]
    fn mov_pc_branches() {
        // MOV pc, #0x08000000
        let mut cpu = test_arm(&[dp(1, 0xD, 0, 0, 15, 0x408)]);
        cpu.step();
        assert_eq!(cpu.exec_pc(), 0x0800_0000);
    }

    #[test]
    fn movs_pc_restores_spsr_then_branches() {
        // MOVS pc, lr from SVC with SPSR.T set: lands in Thumb at lr&!1.
        let mut cpu = test_arm(&[dp(0, 0xD, 1, 0, 15, 0x00E)]);
        cpu.set_cpsr((cpu.cpsr & !0x1F) | MODE_SVC);
        cpu.set_spsr(MODE_SYS | FLAG_T | FLAG_N);
        cpu.r[14] = 0x0800_0101;
        cpu.step();
        assert_eq!(cpu.cpsr, MODE_SYS | FLAG_T | FLAG_N);
        assert!(cpu.thumb());
        assert_eq!(cpu.exec_pc(), 0x0800_0100);
    }

    #[test]
    fn subs_pc_returns_to_arm() {
        // SUBS pc, lr, #4 from SVC with SPSR = SYS (ARM state).
        let mut cpu = test_arm(&[dp(1, 0x2, 1, 14, 15, 0x004)]);
        cpu.set_cpsr((cpu.cpsr & !0x1F) | MODE_SVC);
        cpu.set_spsr(MODE_SYS | FLAG_Z);
        cpu.r[14] = 0x0800_0104;
        cpu.step();
        assert_eq!(cpu.cpsr, MODE_SYS | FLAG_Z);
        assert_eq!(cpu.exec_pc(), 0x0800_0100);
    }

    #[test]
    fn shift_by_imm_helper() {
        let mut cpu = test_arm(&[0]);
        cpu.set_flag(FLAG_C, true);
        assert_eq!(super::shift_by_imm(&cpu, 0x10, 0, 4), 0x100);
        assert_eq!(super::shift_by_imm(&cpu, 0x8000_0000, 1, 0), 0);
        assert_eq!(super::shift_by_imm(&cpu, 0x8000_0000, 2, 0), 0xFFFF_FFFF);
        assert_eq!(super::shift_by_imm(&cpu, 2, 3, 0), 0x8000_0001);
        cpu.set_flag(FLAG_C, false);
        assert_eq!(super::shift_by_imm(&cpu, 2, 3, 0), 0x0000_0001);
        assert_eq!(super::shift_by_imm(&cpu, 0xF0, 3, 4), 0x0000_000F);
    }
}
