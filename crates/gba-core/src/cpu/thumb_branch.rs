//! Thumb formats 16-19: branches and SWI.

use super::Arm7;

pub(crate) fn thumb_cond_branch(cpu: &mut Arm7, op: u16) {
    let cond = ((op >> 8) & 0xF) as u32;
    // cond 0xE is undefined in Thumb; 0xF (SWI) never reaches here.
    if cond == 0xE {
        cpu.exception_undefined();
        return;
    }
    if cpu.condition(cond) {
        let off = ((op & 0xFF) as i8 as i32) << 1;
        cpu.branch(cpu.r[15].wrapping_add(off as u32));
    }
}

pub(crate) fn thumb_swi(cpu: &mut Arm7, op: u16) {
    let _ = op;
    cpu.exception_swi();
}

pub(crate) fn thumb_branch(cpu: &mut Arm7, op: u16) {
    // Bit 11 set (0xE800-0xEFFF) is the BLX suffix, undefined on ARMv4.
    if op & 0x0800 != 0 {
        cpu.exception_undefined();
        return;
    }
    let off = (((op & 0x7FF) as i32) << 21) >> 20;
    cpu.branch(cpu.r[15].wrapping_add(off as u32));
}

pub(crate) fn thumb_bl(cpu: &mut Arm7, op: u16) {
    let imm = (op & 0x7FF) as u32;
    if op & 0x0800 == 0 {
        // First half: LR = PC + signext(imm11) << 12.
        let off = ((imm as i32) << 21) >> 9;
        cpu.r[14] = cpu.r[15].wrapping_add(off as u32);
    } else {
        // Second half: branch to LR + imm << 1, LR = return address | 1.
        let target = cpu.r[14].wrapping_add(imm << 1);
        cpu.r[14] = cpu.r[15].wrapping_sub(2) | 1;
        cpu.branch(target);
    }
}

#[cfg(test)]
mod tests {
    use crate::cpu::{test_thumb, FLAG_T, FLAG_Z, MODE_UND};

    const BASE: u32 = 0x0800_00C0;

    #[test]
    fn cond_branch_forward_taken() {
        // beq +4 (imm8 = 2): target = base + 4 + 4.
        let mut cpu = test_thumb(&[0xD002]);
        cpu.cpsr |= FLAG_Z;
        cpu.step();
        assert_eq!(cpu.exec_pc(), BASE + 8);
    }

    #[test]
    fn cond_branch_not_taken() {
        let mut cpu = test_thumb(&[0xD002]);
        cpu.cpsr &= !FLAG_Z;
        cpu.step();
        assert_eq!(cpu.exec_pc(), BASE + 2);
    }

    #[test]
    fn cond_branch_backward_taken() {
        // bne -4 (imm8 = 0xFE): branch to self.
        let mut cpu = test_thumb(&[0xD1FE]);
        cpu.cpsr &= !FLAG_Z;
        cpu.step();
        assert_eq!(cpu.exec_pc(), BASE);
    }

    #[test]
    fn cond_branch_cond_e_is_undefined() {
        let mut cpu = test_thumb(&[0xDE00]);
        cpu.step();
        assert_eq!(cpu.cpsr & 0x1F, MODE_UND);
        assert_eq!(cpu.cpsr & FLAG_T, 0);
        assert_eq!(cpu.r[14], BASE + 2);
        assert_eq!(cpu.exec_pc(), 0x04);
    }

    #[test]
    fn branch_forward() {
        // b +4 (imm11 = 2): target = base + 4 + 4.
        let mut cpu = test_thumb(&[0xE002]);
        cpu.step();
        assert_eq!(cpu.exec_pc(), BASE + 8);
    }

    #[test]
    fn branch_negative_offset() {
        // b -4 (imm11 = 0x7FE): branch to self.
        let mut cpu = test_thumb(&[0xE7FE]);
        cpu.step();
        assert_eq!(cpu.exec_pc(), BASE);
    }

    #[test]
    fn branch_blx_suffix_is_undefined() {
        let mut cpu = test_thumb(&[0xE800]);
        cpu.step();
        assert_eq!(cpu.cpsr & 0x1F, MODE_UND);
        assert_eq!(cpu.r[14], BASE + 2);
        assert_eq!(cpu.exec_pc(), 0x04);
    }

    #[test]
    fn bl_forward_round_trip() {
        // bl base+6: hi imm 0, lo imm 1.
        let mut cpu = test_thumb(&[0xF000, 0xF801]);
        cpu.step();
        assert_eq!(cpu.r[14], BASE + 4);
        cpu.step();
        assert_eq!(cpu.exec_pc(), BASE + 6);
        // Return address = second half + 2 with Thumb bit set.
        assert_eq!(cpu.r[14], (BASE + 4) | 1);
    }

    #[test]
    fn bl_backward_offset() {
        // bl base-16: offset -20 from PC+4 = hi 0x7FF, lo 0x7F6.
        let mut cpu = test_thumb(&[0xF7FF, 0xFFF6]);
        cpu.step();
        assert_eq!(cpu.r[14], (BASE + 4).wrapping_sub(0x1000));
        cpu.step();
        assert_eq!(cpu.exec_pc(), BASE - 16);
        assert_eq!(cpu.r[14], (BASE + 4) | 1);
    }
}
