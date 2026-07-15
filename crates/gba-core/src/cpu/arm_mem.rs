//! ARM single/halfword/signed data transfers and SWP.

use super::{alu, Arm7};

pub(crate) fn arm_single_transfer(cpu: &mut Arm7, op: u32) {
    let pre = op & (1 << 24) != 0;
    let up = op & (1 << 23) != 0;
    let byte = op & (1 << 22) != 0;
    let wb = op & (1 << 21) != 0;
    let load = op & (1 << 20) != 0;
    let rn = ((op >> 16) & 0xF) as usize;
    let rd = ((op >> 12) & 0xF) as usize;

    let offset = if op & (1 << 25) != 0 {
        let rm = cpu.reg((op & 0xF) as usize);
        alu::shift_by_imm(cpu, rm, (op >> 5) & 3, (op >> 7) & 0x1F)
    } else {
        op & 0xFFF
    };

    let base = cpu.reg(rn);
    let offset_addr = if up { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
    let addr = if pre { offset_addr } else { base };

    if load {
        let value = if byte {
            cpu.bus.read8(addr) as u32
        } else {
            // Unaligned word loads rotate the aligned word into place.
            cpu.bus.read32(addr).rotate_right(8 * (addr & 3))
        };
        cpu.bus.idle();
        // Post-index always writes back; loaded value wins when Rd == Rn.
        if (!pre || wb) && rn != rd {
            cpu.set_reg(rn, offset_addr);
        }
        cpu.set_reg(rd, value);
    } else {
        // STR of r15 stores exec + 12.
        let value = if rd == 15 { cpu.r[15].wrapping_add(4) } else { cpu.reg(rd) };
        if byte {
            cpu.bus.write8(addr, value as u8);
        } else {
            cpu.bus.write32(addr, value);
        }
        if !pre || wb {
            cpu.set_reg(rn, offset_addr);
        }
    }
}

pub(crate) fn arm_halfword_transfer(cpu: &mut Arm7, op: u32) {
    let pre = op & (1 << 24) != 0;
    let up = op & (1 << 23) != 0;
    let imm = op & (1 << 22) != 0;
    let wb = op & (1 << 21) != 0;
    let load = op & (1 << 20) != 0;
    let rn = ((op >> 16) & 0xF) as usize;
    let rd = ((op >> 12) & 0xF) as usize;
    let sh = (op >> 5) & 3;

    // Immediate offset splits across bits 11-8 and 3-0.
    let offset = if imm { ((op >> 4) & 0xF0) | (op & 0xF) } else { cpu.reg((op & 0xF) as usize) };
    let base = cpu.reg(rn);
    let offset_addr = if up { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
    let addr = if pre { offset_addr } else { base };

    if load {
        let value = match sh {
            // LDRH: odd address rotates the halfword.
            1 => (cpu.bus.read16(addr) as u32).rotate_right(8 * (addr & 1)),
            // LDRSB.
            2 => cpu.bus.read8(addr) as i8 as u32,
            // LDRSH: odd address acts as LDRSB.
            _ => {
                if addr & 1 != 0 {
                    cpu.bus.read8(addr) as i8 as u32
                } else {
                    cpu.bus.read16(addr) as i16 as u32
                }
            }
        };
        cpu.bus.idle();
        if (!pre || wb) && rn != rd {
            cpu.set_reg(rn, offset_addr);
        }
        cpu.set_reg(rd, value);
    } else {
        let value = if rd == 15 { cpu.r[15].wrapping_add(4) } else { cpu.reg(rd) };
        cpu.bus.write16(addr, value as u16);
        if !pre || wb {
            cpu.set_reg(rn, offset_addr);
        }
    }
}

pub(crate) fn arm_swap(cpu: &mut Arm7, op: u32) {
    let byte = op & (1 << 22) != 0;
    let rn = ((op >> 16) & 0xF) as usize;
    let rd = ((op >> 12) & 0xF) as usize;
    let rm = (op & 0xF) as usize;

    let addr = cpu.reg(rn);
    let src = cpu.reg(rm);
    let loaded = if byte {
        let v = cpu.bus.read8(addr) as u32;
        cpu.bus.write8(addr, src as u8);
        v
    } else {
        let v = cpu.bus.read32(addr).rotate_right(8 * (addr & 3));
        cpu.bus.write32(addr, src);
        v
    };
    cpu.bus.idle();
    cpu.set_reg(rd, loaded);
}

#[cfg(test)]
mod tests {
    use crate::cpu::test_arm;

    const RAM: u32 = 0x0300_0000;

    fn sdt(i: u32, p: u32, u: u32, b: u32, w: u32, l: u32, rn: u32, rd: u32, off: u32) -> u32 {
        0xE400_0000
            | i << 25
            | p << 24
            | u << 23
            | b << 22
            | w << 21
            | l << 20
            | rn << 16
            | rd << 12
            | off
    }

    fn hdt(p: u32, u: u32, i: u32, w: u32, l: u32, rn: u32, rd: u32, sh: u32, off: u32) -> u32 {
        0xE000_0090
            | p << 24
            | u << 23
            | i << 22
            | w << 21
            | l << 20
            | rn << 16
            | rd << 12
            | (off & 0xF0) << 4
            | sh << 5
            | (off & 0xF)
    }

    fn swp(b: u32, rn: u32, rd: u32, rm: u32) -> u32 {
        0xE100_0090 | b << 22 | rn << 16 | rd << 12 | rm
    }

    #[test]
    fn ldr_word_imm_pre() {
        let mut cpu = test_arm(&[sdt(0, 1, 1, 0, 0, 1, 1, 0, 4)]); // ldr r0, [r1, #4]
        cpu.r[1] = RAM;
        cpu.bus.write32(RAM + 4, 0xCAFE_BABE);
        cpu.step();
        assert_eq!(cpu.r[0], 0xCAFE_BABE);
        assert_eq!(cpu.r[1], RAM);
    }

    #[test]
    fn ldr_unaligned_rotates() {
        let mut cpu = test_arm(&[sdt(0, 1, 1, 0, 0, 1, 1, 0, 0)]); // ldr r0, [r1]
        cpu.r[1] = RAM + 1;
        cpu.bus.write32(RAM, 0x1122_3344);
        cpu.step();
        assert_eq!(cpu.r[0], 0x4411_2233);
    }

    #[test]
    fn ldrb_zero_extends() {
        let mut cpu = test_arm(&[sdt(0, 1, 1, 1, 0, 1, 1, 0, 3)]); // ldrb r0, [r1, #3]
        cpu.r[1] = RAM;
        cpu.bus.write32(RAM, 0x8899_AABB);
        cpu.step();
        assert_eq!(cpu.r[0], 0x88);
    }

    #[test]
    fn str_word() {
        let mut cpu = test_arm(&[sdt(0, 1, 1, 0, 0, 0, 1, 0, 0)]); // str r0, [r1]
        cpu.r[0] = 0xDEAD_BEEF;
        cpu.r[1] = RAM;
        cpu.step();
        assert_eq!(cpu.bus.read32(RAM), 0xDEAD_BEEF);
    }

    #[test]
    fn strb_writes_single_byte() {
        let mut cpu = test_arm(&[sdt(0, 1, 1, 1, 0, 0, 1, 0, 1)]); // strb r0, [r1, #1]
        cpu.bus.write32(RAM, 0x1122_3344);
        cpu.r[0] = 0xFFFF_FFAB;
        cpu.r[1] = RAM;
        cpu.step();
        assert_eq!(cpu.bus.read32(RAM), 0x1122_AB44);
    }

    #[test]
    fn str_down_offset() {
        let mut cpu = test_arm(&[sdt(0, 1, 0, 0, 0, 0, 1, 0, 4)]); // str r0, [r1, #-4]
        cpu.r[0] = 0x5555_AAAA;
        cpu.r[1] = RAM + 8;
        cpu.step();
        assert_eq!(cpu.bus.read32(RAM + 4), 0x5555_AAAA);
    }

    #[test]
    fn ldr_post_index_writes_back() {
        let mut cpu = test_arm(&[sdt(0, 0, 1, 0, 0, 1, 1, 0, 4)]); // ldr r0, [r1], #4
        cpu.r[1] = RAM;
        cpu.bus.write32(RAM, 0x1234_5678);
        cpu.step();
        assert_eq!(cpu.r[0], 0x1234_5678);
        assert_eq!(cpu.r[1], RAM + 4);
    }

    #[test]
    fn ldr_pre_index_writeback() {
        let mut cpu = test_arm(&[sdt(0, 1, 1, 0, 1, 1, 1, 0, 4)]); // ldr r0, [r1, #4]!
        cpu.r[1] = RAM;
        cpu.bus.write32(RAM + 4, 0x0BAD_F00D);
        cpu.step();
        assert_eq!(cpu.r[0], 0x0BAD_F00D);
        assert_eq!(cpu.r[1], RAM + 4);
    }

    #[test]
    fn ldr_rd_eq_rn_loaded_value_wins() {
        let mut cpu = test_arm(&[sdt(0, 0, 1, 0, 0, 1, 1, 1, 4)]); // ldr r1, [r1], #4
        cpu.r[1] = RAM;
        cpu.bus.write32(RAM, 0x1234_5678);
        cpu.step();
        assert_eq!(cpu.r[1], 0x1234_5678);
    }

    #[test]
    fn str_rd_eq_rn_stores_old_base() {
        let mut cpu = test_arm(&[sdt(0, 0, 1, 0, 0, 0, 1, 1, 4)]); // str r1, [r1], #4
        cpu.r[1] = RAM;
        cpu.step();
        assert_eq!(cpu.bus.read32(RAM), RAM);
        assert_eq!(cpu.r[1], RAM + 4);
    }

    #[test]
    fn ldr_pc_branches() {
        let mut cpu = test_arm(&[sdt(0, 1, 1, 0, 0, 1, 1, 15, 0)]); // ldr pc, [r1]
        cpu.r[1] = RAM;
        cpu.bus.write32(RAM, 0x0800_0102); // low bits masked by branch
        cpu.step();
        assert_eq!(cpu.exec_pc(), 0x0800_0100);
        assert_eq!(cpu.r[15], 0x0800_0104);
    }

    #[test]
    fn str_pc_stores_exec_plus_12() {
        let mut cpu = test_arm(&[sdt(0, 1, 1, 0, 0, 0, 1, 15, 0)]); // str pc, [r1]
        cpu.r[1] = RAM;
        cpu.step();
        assert_eq!(cpu.bus.read32(RAM), 0x0800_00CC);
    }

    #[test]
    fn ldr_scaled_register_offset() {
        let off = 2 << 7 | 2; // r2, lsl #2
        let mut cpu = test_arm(&[sdt(1, 1, 1, 0, 0, 1, 1, 0, off)]); // ldr r0, [r1, r2, lsl #2]
        cpu.r[1] = RAM;
        cpu.r[2] = 3;
        cpu.bus.write32(RAM + 12, 0xFEED_FACE);
        cpu.step();
        assert_eq!(cpu.r[0], 0xFEED_FACE);
    }

    #[test]
    fn ldr_takes_internal_cycle() {
        let mut cpu = test_arm(&[sdt(0, 1, 1, 0, 0, 1, 1, 0, 0), sdt(0, 1, 1, 0, 0, 0, 1, 0, 0)]);
        cpu.r[1] = RAM;
        assert_eq!(cpu.step(), 3); // fetch + data + idle
        assert_eq!(cpu.step(), 2); // fetch + data
    }

    #[test]
    fn ldrh_aligned() {
        let mut cpu = test_arm(&[hdt(1, 1, 1, 0, 1, 1, 0, 1, 2)]); // ldrh r0, [r1, #2]
        cpu.r[1] = RAM;
        cpu.bus.write32(RAM, 0xAABB_CCDD);
        cpu.step();
        assert_eq!(cpu.r[0], 0xAABB);
    }

    #[test]
    fn ldrh_odd_rotates() {
        let mut cpu = test_arm(&[hdt(1, 1, 1, 0, 1, 1, 0, 1, 0)]); // ldrh r0, [r1]
        cpu.r[1] = RAM + 1;
        cpu.bus.write32(RAM, 0x0000_AABB);
        cpu.step();
        assert_eq!(cpu.r[0], 0xBB00_00AA);
    }

    #[test]
    fn ldrsb_sign_extends() {
        let mut cpu = test_arm(&[hdt(1, 1, 1, 0, 1, 1, 0, 2, 0)]); // ldrsb r0, [r1]
        cpu.r[1] = RAM;
        cpu.bus.write32(RAM, 0x0000_0080);
        cpu.step();
        assert_eq!(cpu.r[0], 0xFFFF_FF80);
    }

    #[test]
    fn ldrsh_even_sign_extends() {
        let mut cpu = test_arm(&[hdt(1, 1, 1, 0, 1, 1, 0, 3, 0)]); // ldrsh r0, [r1]
        cpu.r[1] = RAM;
        cpu.bus.write32(RAM, 0x0000_8000);
        cpu.step();
        assert_eq!(cpu.r[0], 0xFFFF_8000);
    }

    #[test]
    fn ldrsh_odd_acts_as_ldrsb() {
        let mut cpu = test_arm(&[hdt(1, 1, 1, 0, 1, 1, 0, 3, 0)]); // ldrsh r0, [r1]
        cpu.r[1] = RAM + 1;
        cpu.bus.write32(RAM, 0x0000_FF00);
        cpu.step();
        assert_eq!(cpu.r[0], 0xFFFF_FFFF);
    }

    #[test]
    fn strh_writes_halfword() {
        let mut cpu = test_arm(&[hdt(1, 1, 1, 0, 0, 1, 0, 1, 2)]); // strh r0, [r1, #2]
        cpu.bus.write32(RAM, 0x1111_2222);
        cpu.r[0] = 0xFFFF_ABCD;
        cpu.r[1] = RAM;
        cpu.step();
        assert_eq!(cpu.bus.read32(RAM), 0xABCD_2222);
    }

    #[test]
    fn ldrh_imm_offset_split_nibbles() {
        let mut cpu = test_arm(&[hdt(1, 1, 1, 0, 1, 1, 0, 1, 0x14)]); // ldrh r0, [r1, #0x14]
        cpu.r[1] = RAM;
        cpu.bus.write32(RAM + 0x14, 0x0000_BEEF);
        cpu.step();
        assert_eq!(cpu.r[0], 0xBEEF);
    }

    #[test]
    fn ldrh_register_offset() {
        let mut cpu = test_arm(&[hdt(1, 1, 0, 0, 1, 1, 0, 1, 2)]); // ldrh r0, [r1, r2]
        cpu.r[1] = RAM;
        cpu.r[2] = 6;
        cpu.bus.write32(RAM + 4, 0x1234_0000);
        cpu.step();
        assert_eq!(cpu.r[0], 0x1234);
    }

    #[test]
    fn ldrh_post_index_writes_back() {
        let mut cpu = test_arm(&[hdt(0, 1, 1, 0, 1, 1, 0, 1, 2)]); // ldrh r0, [r1], #2
        cpu.r[1] = RAM;
        cpu.bus.write32(RAM, 0x0000_4321);
        cpu.step();
        assert_eq!(cpu.r[0], 0x4321);
        assert_eq!(cpu.r[1], RAM + 2);
    }

    #[test]
    fn strh_down_pre_writeback() {
        let mut cpu = test_arm(&[hdt(1, 0, 1, 1, 0, 1, 0, 1, 4)]); // strh r0, [r1, #-4]!
        cpu.r[0] = 0x7777;
        cpu.r[1] = RAM + 8;
        cpu.step();
        assert_eq!(cpu.bus.read32(RAM + 4) & 0xFFFF, 0x7777);
        assert_eq!(cpu.r[1], RAM + 4);
    }

    #[test]
    fn swp_word_rotates_load() {
        let mut cpu = test_arm(&[swp(0, 1, 0, 2)]); // swp r0, r2, [r1]
        cpu.r[1] = RAM + 1;
        cpu.r[2] = 0xDEAD_BEEF;
        cpu.bus.write32(RAM, 0x1122_3344);
        cpu.step();
        assert_eq!(cpu.r[0], 0x4411_2233);
        assert_eq!(cpu.bus.read32(RAM), 0xDEAD_BEEF);
    }

    #[test]
    fn swpb_byte() {
        let mut cpu = test_arm(&[swp(1, 1, 0, 2)]); // swpb r0, r2, [r1]
        cpu.r[1] = RAM + 2;
        cpu.r[2] = 0xFFFF_FF55;
        cpu.bus.write32(RAM, 0x11AA_2233);
        cpu.step();
        assert_eq!(cpu.r[0], 0xAA);
        assert_eq!(cpu.bus.read32(RAM), 0x1155_2233);
    }

    #[test]
    fn swp_rd_eq_rm() {
        let mut cpu = test_arm(&[swp(0, 1, 0, 0)]); // swp r0, r0, [r1]
        cpu.r[0] = 0xAAAA_AAAA;
        cpu.r[1] = RAM;
        cpu.bus.write32(RAM, 0x5555_5555);
        cpu.step();
        assert_eq!(cpu.r[0], 0x5555_5555);
        assert_eq!(cpu.bus.read32(RAM), 0xAAAA_AAAA);
    }
}
