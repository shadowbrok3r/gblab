//! Thumb formats 6-11, 14, 15: loads, stores, push/pop, ldm/stm.

use super::Arm7;

/// Word load with unaligned rotation.
fn load_word(cpu: &mut Arm7, addr: u32) -> u32 {
    cpu.bus.read32(addr).rotate_right(8 * (addr & 3))
}

/// Halfword load; odd address rotates by 8.
fn load_half(cpu: &mut Arm7, addr: u32) -> u32 {
    (cpu.bus.read16(addr) as u32).rotate_right(8 * (addr & 1))
}

/// fmt6: LDR rd, [pc, #imm8*4] with pc forced word-aligned.
pub(crate) fn thumb_ldr_pc(cpu: &mut Arm7, op: u16) {
    let rd = ((op >> 8) & 7) as usize;
    let addr = (cpu.r[15] & !2).wrapping_add(((op & 0xFF) as u32) << 2);
    let v = load_word(cpu, addr);
    cpu.bus.idle();
    cpu.r[rd] = v;
}

/// fmt7: LDR/STR/LDRB/STRB rd, [rb, ro].
pub(crate) fn thumb_ldst_reg(cpu: &mut Arm7, op: u16) {
    let ro = ((op >> 6) & 7) as usize;
    let rb = ((op >> 3) & 7) as usize;
    let rd = (op & 7) as usize;
    let addr = cpu.r[rb].wrapping_add(cpu.r[ro]);
    match (op >> 10) & 3 {
        0b00 => cpu.bus.write32(addr, cpu.r[rd]),
        0b01 => cpu.bus.write8(addr, cpu.r[rd] as u8),
        0b10 => {
            let v = load_word(cpu, addr);
            cpu.bus.idle();
            cpu.r[rd] = v;
        }
        _ => {
            let v = cpu.bus.read8(addr) as u32;
            cpu.bus.idle();
            cpu.r[rd] = v;
        }
    }
}

/// fmt8: STRH/LDRH/LDRSB/LDRSH rd, [rb, ro].
pub(crate) fn thumb_ldst_sign(cpu: &mut Arm7, op: u16) {
    let ro = ((op >> 6) & 7) as usize;
    let rb = ((op >> 3) & 7) as usize;
    let rd = (op & 7) as usize;
    let addr = cpu.r[rb].wrapping_add(cpu.r[ro]);
    match (op >> 10) & 3 {
        0b00 => {
            cpu.bus.write16(addr, cpu.r[rd] as u16);
            return;
        }
        0b10 => cpu.r[rd] = load_half(cpu, addr),
        0b01 => cpu.r[rd] = cpu.bus.read8(addr) as i8 as u32,
        _ => {
            // LDRSH from an odd address acts as LDRSB.
            cpu.r[rd] = if addr & 1 != 0 {
                cpu.bus.read8(addr) as i8 as u32
            } else {
                cpu.bus.read16(addr) as i16 as u32
            };
        }
    }
    cpu.bus.idle();
}

/// fmt9: LDR/STR/LDRB/STRB rd, [rb, #imm5] (word offset scaled x4).
pub(crate) fn thumb_ldst_imm(cpu: &mut Arm7, op: u16) {
    let imm = ((op >> 6) & 0x1F) as u32;
    let rb = ((op >> 3) & 7) as usize;
    let rd = (op & 7) as usize;
    match (op >> 11) & 3 {
        0b00 => cpu.bus.write32(cpu.r[rb].wrapping_add(imm << 2), cpu.r[rd]),
        0b01 => {
            let v = load_word(cpu, cpu.r[rb].wrapping_add(imm << 2));
            cpu.bus.idle();
            cpu.r[rd] = v;
        }
        0b10 => cpu.bus.write8(cpu.r[rb].wrapping_add(imm), cpu.r[rd] as u8),
        _ => {
            let v = cpu.bus.read8(cpu.r[rb].wrapping_add(imm)) as u32;
            cpu.bus.idle();
            cpu.r[rd] = v;
        }
    }
}

/// fmt10: STRH/LDRH rd, [rb, #imm5*2].
pub(crate) fn thumb_ldst_half(cpu: &mut Arm7, op: u16) {
    let addr = cpu.r[((op >> 3) & 7) as usize].wrapping_add((((op >> 6) & 0x1F) as u32) << 1);
    let rd = (op & 7) as usize;
    if op & 0x0800 != 0 {
        let v = load_half(cpu, addr);
        cpu.bus.idle();
        cpu.r[rd] = v;
    } else {
        cpu.bus.write16(addr, cpu.r[rd] as u16);
    }
}

/// fmt11: LDR/STR rd, [sp, #imm8*4].
pub(crate) fn thumb_ldst_sp(cpu: &mut Arm7, op: u16) {
    let rd = ((op >> 8) & 7) as usize;
    let addr = cpu.r[13].wrapping_add(((op & 0xFF) as u32) << 2);
    if op & 0x0800 != 0 {
        let v = load_word(cpu, addr);
        cpu.bus.idle();
        cpu.r[rd] = v;
    } else {
        cpu.bus.write32(addr, cpu.r[rd]);
    }
}

/// fmt14: PUSH {rlist[,lr]} / POP {rlist[,pc]} on the full-descending stack.
pub(crate) fn thumb_push_pop(cpu: &mut Arm7, op: u16) {
    let load = op & 0x0800 != 0;
    let r_bit = op & 0x0100 != 0;
    let rlist = op & 0xFF;
    // Empty list transfers r15 and moves sp by 0x40.
    if rlist == 0 && !r_bit {
        if load {
            let v = cpu.bus.read32(cpu.r[13]);
            cpu.r[13] = cpu.r[13].wrapping_add(0x40);
            cpu.bus.idle();
            cpu.set_reg(15, v);
        } else {
            cpu.r[13] = cpu.r[13].wrapping_sub(0x40);
            cpu.bus.write32(cpu.r[13], cpu.r[15]);
        }
        return;
    }
    let count = rlist.count_ones() + r_bit as u32;
    if load {
        let mut addr = cpu.r[13];
        cpu.r[13] = cpu.r[13].wrapping_add(4 * count);
        cpu.bus.idle();
        for i in 0..8 {
            if rlist & (1 << i) != 0 {
                cpu.r[i] = cpu.bus.read32(addr);
                addr = addr.wrapping_add(4);
            }
        }
        if r_bit {
            let v = cpu.bus.read32(addr);
            cpu.set_reg(15, v);
        }
    } else {
        let mut addr = cpu.r[13].wrapping_sub(4 * count);
        cpu.r[13] = addr;
        for i in 0..8 {
            if rlist & (1 << i) != 0 {
                cpu.bus.write32(addr, cpu.r[i]);
                addr = addr.wrapping_add(4);
            }
        }
        if r_bit {
            cpu.bus.write32(addr, cpu.r[14]);
        }
    }
}

/// fmt15: LDMIA/STMIA rb!, {rlist}.
pub(crate) fn thumb_ldm_stm(cpu: &mut Arm7, op: u16) {
    let load = op & 0x0800 != 0;
    let rb = ((op >> 8) & 7) as usize;
    let rlist = op & 0xFF;
    let base = cpu.r[rb];
    // Empty list transfers r15 and advances the base by 0x40.
    if rlist == 0 {
        if load {
            let v = cpu.bus.read32(base);
            cpu.r[rb] = base.wrapping_add(0x40);
            cpu.bus.idle();
            cpu.set_reg(15, v);
        } else {
            // Hardware stores r15 + 2 (exec + 6).
            cpu.bus.write32(base, cpu.r[15].wrapping_add(2));
            cpu.r[rb] = base.wrapping_add(0x40);
        }
        return;
    }
    let wb = base.wrapping_add(4 * rlist.count_ones());
    if load {
        // Writeback first; a loaded rb overwrites it.
        cpu.r[rb] = wb;
        cpu.bus.idle();
        let mut addr = base;
        for i in 0..8 {
            if rlist & (1 << i) != 0 {
                cpu.r[i] = cpu.bus.read32(addr);
                addr = addr.wrapping_add(4);
            }
        }
    } else {
        // Writeback after the first store: old base stored iff rb is lowest listed.
        let mut addr = base;
        for i in 0..8 {
            if rlist & (1 << i) != 0 {
                cpu.bus.write32(addr, cpu.r[i]);
                if addr == base {
                    cpu.r[rb] = wb;
                }
                addr = addr.wrapping_add(4);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::cpu::{test_thumb, FLAG_T};

    const IWRAM: u32 = 0x0300_0000;

    #[test]
    fn ldr_pc_ignores_bit1_of_pc() {
        // 0xC0: LDR r0,[pc,#4] (pc=0xC4); 0xC2: LDR r1,[pc,#4] (pc=0xC6 &!2 = 0xC4).
        let mut cpu = test_thumb(&[0x4801, 0x4901, 0, 0, 0xBEEF, 0xDEAD]);
        cpu.step();
        cpu.step();
        assert_eq!(cpu.r[0], 0xDEAD_BEEF);
        assert_eq!(cpu.r[1], 0xDEAD_BEEF);
    }

    #[test]
    fn ldst_reg_word_unaligned_rotation() {
        // STR r0,[r1,r2]; LDR r3,[r1,r4] with r4=1.
        let mut cpu = test_thumb(&[0x5088, 0x590B]);
        cpu.r[0] = 0x1122_3344;
        cpu.r[1] = IWRAM;
        cpu.r[2] = 0;
        cpu.r[4] = 1;
        cpu.step();
        assert_eq!(cpu.bus.read32(IWRAM), 0x1122_3344);
        cpu.step();
        assert_eq!(cpu.r[3], 0x4411_2233);
    }

    #[test]
    fn ldst_reg_byte() {
        // STRB r0,[r1,r2]; LDRB r3,[r1,r4].
        let mut cpu = test_thumb(&[0x5488, 0x5D0B]);
        cpu.r[0] = 0xFFEE_AACC;
        cpu.r[1] = IWRAM;
        cpu.step();
        assert_eq!(cpu.bus.read32(IWRAM), 0x0000_00CC);
        cpu.step();
        assert_eq!(cpu.r[3], 0xCC);
    }

    #[test]
    fn strh_ldrh_reg_offset() {
        // STRH r0,[r1,r2]; LDRH r3,[r1,r4].
        let mut cpu = test_thumb(&[0x5288, 0x5B0B]);
        cpu.r[0] = 0x1234_ABCD;
        cpu.r[1] = IWRAM;
        cpu.step();
        assert_eq!(cpu.bus.read32(IWRAM) & 0xFFFF, 0xABCD);
        cpu.step();
        assert_eq!(cpu.r[3], 0x0000_ABCD);
    }

    #[test]
    fn ldrh_odd_address_rotates() {
        // LDRH r3,[r1,r4] with r4=1.
        let mut cpu = test_thumb(&[0x5B0B]);
        cpu.bus.write16(IWRAM, 0xABCD);
        cpu.r[1] = IWRAM;
        cpu.r[4] = 1;
        cpu.step();
        assert_eq!(cpu.r[3], 0xCD00_00AB);
    }

    #[test]
    fn ldrsb_sign_extends() {
        // LDRSB r3,[r1,r2].
        let mut cpu = test_thumb(&[0x568B]);
        cpu.bus.write8(IWRAM, 0x80);
        cpu.r[1] = IWRAM;
        cpu.step();
        assert_eq!(cpu.r[3], 0xFFFF_FF80);
    }

    #[test]
    fn ldrsh_even_sign_extends() {
        // LDRSH r3,[r1,r2].
        let mut cpu = test_thumb(&[0x5E8B]);
        cpu.bus.write16(IWRAM, 0x8001);
        cpu.r[1] = IWRAM;
        cpu.step();
        assert_eq!(cpu.r[3], 0xFFFF_8001);
    }

    #[test]
    fn ldrsh_odd_acts_as_ldrsb() {
        // LDRSH r3,[r1,r4] with r4=1; bytes at IWRAM are [FF, 80].
        let mut cpu = test_thumb(&[0x5F0B]);
        cpu.bus.write16(IWRAM, 0x80FF);
        cpu.r[1] = IWRAM;
        cpu.r[4] = 1;
        cpu.step();
        assert_eq!(cpu.r[3], 0xFFFF_FF80);
    }

    #[test]
    fn ldst_imm_word_offset_scaled_x4() {
        // STR r0,[r1,#4]; LDR r2,[r1,#4].
        let mut cpu = test_thumb(&[0x6048, 0x684A]);
        cpu.r[0] = 0xCAFE_F00D;
        cpu.r[1] = IWRAM;
        cpu.step();
        assert_eq!(cpu.bus.read32(IWRAM + 4), 0xCAFE_F00D);
        cpu.step();
        assert_eq!(cpu.r[2], 0xCAFE_F00D);
    }

    #[test]
    fn ldst_imm_byte_offset_unscaled() {
        // STRB r0,[r1,#3]; LDRB r2,[r1,#3].
        let mut cpu = test_thumb(&[0x70C8, 0x78CA]);
        cpu.r[0] = 0xAB;
        cpu.r[1] = IWRAM;
        cpu.step();
        assert_eq!(cpu.bus.read8(IWRAM + 3), 0xAB);
        cpu.step();
        assert_eq!(cpu.r[2], 0xAB);
    }

    #[test]
    fn ldst_half_imm_offset_scaled_x2() {
        // STRH r0,[r1,#2]; LDRH r2,[r1,#2].
        let mut cpu = test_thumb(&[0x8048, 0x884A]);
        cpu.r[0] = 0xBEEF;
        cpu.r[1] = IWRAM;
        cpu.step();
        assert_eq!(cpu.bus.read16(IWRAM + 2), 0xBEEF);
        cpu.step();
        assert_eq!(cpu.r[2], 0xBEEF);
    }

    #[test]
    fn ldst_sp_relative() {
        // STR r0,[sp,#8]; LDR r1,[sp,#8].
        let mut cpu = test_thumb(&[0x9002, 0x9902]);
        cpu.r[0] = 0x1357_9BDF;
        let sp = cpu.r[13];
        cpu.step();
        assert_eq!(cpu.bus.read32(sp + 8), 0x1357_9BDF);
        cpu.step();
        assert_eq!(cpu.r[1], 0x1357_9BDF);
    }

    #[test]
    fn push_pop_round_trip_with_pc() {
        // PUSH {r0,r1,lr}; POP {r2,r3,pc}.
        let mut cpu = test_thumb(&[0xB503, 0xBD0C]);
        cpu.r[0] = 0x1111_1111;
        cpu.r[1] = 0x2222_2222;
        cpu.r[14] = 0x0800_0101;
        let sp0 = cpu.r[13];
        cpu.step();
        assert_eq!(cpu.r[13], sp0 - 12);
        assert_eq!(cpu.bus.read32(sp0 - 12), 0x1111_1111);
        assert_eq!(cpu.bus.read32(sp0 - 8), 0x2222_2222);
        assert_eq!(cpu.bus.read32(sp0 - 4), 0x0800_0101);
        cpu.step();
        assert_eq!(cpu.r[2], 0x1111_1111);
        assert_eq!(cpu.r[3], 0x2222_2222);
        assert_eq!(cpu.r[13], sp0);
        assert_eq!(cpu.exec_pc(), 0x0800_0100);
        assert!(cpu.cpsr & FLAG_T != 0);
    }

    #[test]
    fn push_lr_only() {
        // PUSH {lr} with empty low list.
        let mut cpu = test_thumb(&[0xB500]);
        cpu.r[14] = 0xDEAD_BEEF;
        let sp0 = cpu.r[13];
        cpu.step();
        assert_eq!(cpu.r[13], sp0 - 4);
        assert_eq!(cpu.bus.read32(sp0 - 4), 0xDEAD_BEEF);
    }

    #[test]
    fn stm_base_lowest_stores_old_base() {
        // STMIA r0!, {r0,r1}.
        let mut cpu = test_thumb(&[0xC003]);
        cpu.r[0] = IWRAM + 0x10;
        cpu.r[1] = 0xAABB_CCDD;
        cpu.step();
        assert_eq!(cpu.bus.read32(IWRAM + 0x10), IWRAM + 0x10);
        assert_eq!(cpu.bus.read32(IWRAM + 0x14), 0xAABB_CCDD);
        assert_eq!(cpu.r[0], IWRAM + 0x18);
    }

    #[test]
    fn stm_base_not_lowest_stores_written_back() {
        // STMIA r1!, {r0,r1}.
        let mut cpu = test_thumb(&[0xC103]);
        cpu.r[0] = 0x1234_5678;
        cpu.r[1] = IWRAM + 0x20;
        cpu.step();
        assert_eq!(cpu.bus.read32(IWRAM + 0x20), 0x1234_5678);
        assert_eq!(cpu.bus.read32(IWRAM + 0x24), IWRAM + 0x28);
        assert_eq!(cpu.r[1], IWRAM + 0x28);
    }

    #[test]
    fn ldm_base_in_list_loaded_value_wins() {
        // LDMIA r0!, {r0,r1}.
        let mut cpu = test_thumb(&[0xC803]);
        cpu.bus.write32(IWRAM + 0x30, 0xCAFE_BABE);
        cpu.bus.write32(IWRAM + 0x34, 0x0BAD_F00D);
        cpu.r[0] = IWRAM + 0x30;
        cpu.step();
        assert_eq!(cpu.r[0], 0xCAFE_BABE);
        assert_eq!(cpu.r[1], 0x0BAD_F00D);
    }

    #[test]
    fn ldm_writeback() {
        // LDMIA r0!, {r1,r2}.
        let mut cpu = test_thumb(&[0xC806]);
        cpu.bus.write32(IWRAM + 0x40, 0x1111_0000);
        cpu.bus.write32(IWRAM + 0x44, 0x2222_0000);
        cpu.r[0] = IWRAM + 0x40;
        cpu.step();
        assert_eq!(cpu.r[1], 0x1111_0000);
        assert_eq!(cpu.r[2], 0x2222_0000);
        assert_eq!(cpu.r[0], IWRAM + 0x48);
    }

    #[test]
    fn stm_empty_rlist_stores_pc_and_advances_0x40() {
        // STMIA r0!, {}: stores exec + 6.
        let mut cpu = test_thumb(&[0xC000]);
        cpu.r[0] = IWRAM + 0x100;
        cpu.step();
        assert_eq!(cpu.bus.read32(IWRAM + 0x100), 0x0800_00C6);
        assert_eq!(cpu.r[0], IWRAM + 0x140);
    }

    #[test]
    fn ldm_empty_rlist_loads_pc_and_advances_0x40() {
        // LDMIA r0!, {}.
        let mut cpu = test_thumb(&[0xC800]);
        cpu.bus.write32(IWRAM + 0x200, 0x0800_0200);
        cpu.r[0] = IWRAM + 0x200;
        cpu.step();
        assert_eq!(cpu.exec_pc(), 0x0800_0200);
        assert_eq!(cpu.r[0], IWRAM + 0x240);
        assert!(cpu.cpsr & FLAG_T != 0);
    }
}
