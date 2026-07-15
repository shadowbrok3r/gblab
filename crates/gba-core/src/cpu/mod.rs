//! ARM7TDMI interpreter: registers, mode banking, pipeline, and decode.
//!
//! Conventions used by all instruction modules:
//! - At the start of an `exec_*` call, `r[15]` = executing address + 8 (ARM)
//!   or + 4 (Thumb). Writing r15 goes through `set_reg`/`branch`, which
//!   reloads the pipeline.
//! - Memory reads are aligned by the Bus; rotation of unaligned loads is done
//!   in the instruction modules.

mod alu;
mod arm_block;
mod arm_mem;
mod arm_misc;
mod bios;
mod thumb_alu;
mod thumb_branch;
mod thumb_mem;

use crate::bus::Bus;

pub const MODE_USR: u32 = 0x10;
pub const MODE_FIQ: u32 = 0x11;
pub const MODE_IRQ: u32 = 0x12;
pub const MODE_SVC: u32 = 0x13;
pub const MODE_ABT: u32 = 0x17;
pub const MODE_UND: u32 = 0x1B;
pub const MODE_SYS: u32 = 0x1F;

pub const FLAG_N: u32 = 1 << 31;
pub const FLAG_Z: u32 = 1 << 30;
pub const FLAG_C: u32 = 1 << 29;
pub const FLAG_V: u32 = 1 << 28;
pub const FLAG_I: u32 = 1 << 7;
pub const FLAG_F: u32 = 1 << 6;
pub const FLAG_T: u32 = 1 << 5;

/// Banked r8-r14 + SPSR for the five exception modes (index by `bank_of`).
#[derive(Default)]
struct Banks {
    usr_r8_r12: [u32; 5],
    fiq_r8_r12: [u32; 5],
    r13: [u32; 6],
    r14: [u32; 6],
    spsr: [u32; 6],
}

/// usr/sys=0, fiq=1, irq=2, svc=3, abt=4, und=5.
fn bank_of(mode: u32) -> usize {
    match mode {
        MODE_FIQ => 1,
        MODE_IRQ => 2,
        MODE_SVC => 3,
        MODE_ABT => 4,
        MODE_UND => 5,
        _ => 0,
    }
}

pub struct Arm7 {
    pub r: [u32; 16],
    pub cpsr: u32,
    banks: Banks,
    pipe: [u32; 2],
    /// IntrWait HLE: sleeping until these BIOS_IF bits get set.
    swi_wait: Option<u16>,
    pub bus: Bus,
}

impl Arm7 {
    /// Post-BIOS boot state: SYS mode, ARM state, PC at ROM entry.
    pub fn new(bus: Bus) -> Self {
        let mut cpu = Arm7 {
            r: [0; 16],
            cpsr: MODE_SYS,
            banks: Banks::default(),
            pipe: [0; 2],
            swi_wait: None,
            bus,
        };
        cpu.r[13] = 0x0300_7F00;
        cpu.banks.r13[bank_of(MODE_IRQ)] = 0x0300_7FA0;
        cpu.banks.r13[bank_of(MODE_SVC)] = 0x0300_7FE0;
        cpu.branch(0x0800_0000);
        cpu
    }

    // ---- register / flag access ----------------------------------------

    pub fn reg(&self, i: usize) -> u32 {
        self.r[i]
    }

    /// Writing r15 branches (and stays in the current state).
    pub(crate) fn set_reg(&mut self, i: usize, v: u32) {
        if i == 15 {
            self.branch(v);
        } else {
            self.r[i] = v;
        }
    }

    pub(crate) fn thumb(&self) -> bool {
        self.cpsr & FLAG_T != 0
    }

    pub(crate) fn flag(&self, f: u32) -> bool {
        self.cpsr & f != 0
    }

    pub(crate) fn set_flag(&mut self, f: u32, on: bool) {
        if on {
            self.cpsr |= f;
        } else {
            self.cpsr &= !f;
        }
    }

    pub(crate) fn set_nz(&mut self, v: u32) {
        self.set_flag(FLAG_N, v & 0x8000_0000 != 0);
        self.set_flag(FLAG_Z, v == 0);
    }

    /// Full CPSR write (MSR, S-bit r15): swaps banks on mode change.
    pub(crate) fn set_cpsr(&mut self, v: u32) {
        self.swap_banks(v & 0x1F);
        self.cpsr = v;
    }

    /// Current mode's SPSR; usr/sys have none and read CPSR.
    pub(crate) fn spsr(&self) -> u32 {
        let b = bank_of(self.cpsr & 0x1F);
        if b == 0 { self.cpsr } else { self.banks.spsr[b] }
    }

    pub(crate) fn set_spsr(&mut self, v: u32) {
        let b = bank_of(self.cpsr & 0x1F);
        if b != 0 {
            self.banks.spsr[b] = v;
        }
    }

    pub(crate) fn restore_cpsr_from_spsr(&mut self) {
        let s = self.spsr();
        self.set_cpsr(s);
    }

    fn swap_banks(&mut self, new_mode: u32) {
        let old = bank_of(self.cpsr & 0x1F);
        let new = bank_of(new_mode);
        if old == new {
            return;
        }
        // r8-r12 are banked only for FIQ.
        if old == 1 {
            self.banks.fiq_r8_r12.copy_from_slice(&self.r[8..13]);
            self.r[8..13].copy_from_slice(&self.banks.usr_r8_r12);
        } else if new == 1 {
            self.banks.usr_r8_r12.copy_from_slice(&self.r[8..13]);
            self.r[8..13].copy_from_slice(&self.banks.fiq_r8_r12);
        }
        self.banks.r13[old] = self.r[13];
        self.banks.r14[old] = self.r[14];
        self.r[13] = self.banks.r13[new];
        self.r[14] = self.banks.r14[new];
    }

    /// User-bank register access for LDM/STM with S-bit.
    pub(crate) fn user_reg(&self, i: usize) -> u32 {
        let b = bank_of(self.cpsr & 0x1F);
        match i {
            8..=12 if b == 1 => self.banks.usr_r8_r12[i - 8],
            13 if b != 0 => self.banks.r13[0],
            14 if b != 0 => self.banks.r14[0],
            _ => self.r[i],
        }
    }

    pub(crate) fn set_user_reg(&mut self, i: usize, v: u32) {
        let b = bank_of(self.cpsr & 0x1F);
        match i {
            8..=12 if b == 1 => self.banks.usr_r8_r12[i - 8] = v,
            13 if b != 0 => self.banks.r13[0] = v,
            14 if b != 0 => self.banks.r14[0] = v,
            _ => self.r[i] = v,
        }
    }

    // ---- pipeline -------------------------------------------------------

    /// Branch to `target` in the current state, reloading the pipeline.
    pub(crate) fn branch(&mut self, target: u32) {
        if self.thumb() {
            let t = target & !1;
            self.pipe[0] = self.bus.read16(t) as u32;
            self.pipe[1] = self.bus.read16(t.wrapping_add(2)) as u32;
            self.r[15] = t.wrapping_add(2);
        } else {
            let t = target & !3;
            self.pipe[0] = self.bus.read32(t);
            self.pipe[1] = self.bus.read32(t.wrapping_add(4));
            self.r[15] = t.wrapping_add(4);
        }
    }

    /// Address of the instruction about to execute (r15 minus one fetch).
    pub fn exec_pc(&self) -> u32 {
        self.r[15].wrapping_sub(if self.thumb() { 2 } else { 4 })
    }

    /// Execute one instruction; returns elapsed bus cycles.
    pub fn step(&mut self) -> u32 {
        let start = self.bus.cycles;
        if self.bus.dma.pending() {
            crate::dma::run_pending(&mut self.bus);
        }
        // Halt wakes on any enabled interrupt, IME regardless.
        if self.bus.halted {
            if self.bus.ie_reg & self.bus.if_reg != 0 {
                self.bus.halted = false;
            } else {
                self.bus.idle();
                return (self.bus.cycles - start).max(1) as u32;
            }
        }
        // IntrWait HLE: only baseline code sleeps; IRQ handlers run through.
        if let Some(mask) = self.swi_wait
            && matches!(self.cpsr & 0x1F, MODE_USR | MODE_SYS)
        {
            let flags = self.bus.read16(bios::BIOS_IF);
            if flags & mask != 0 {
                self.bus.write16(bios::BIOS_IF, flags & !mask);
                self.swi_wait = None;
            } else if self.bus.ime
                && (self.bus.ie_reg & self.bus.if_reg) != 0
                && !self.flag(FLAG_I)
            {
                self.irq();
            } else {
                self.bus.idle();
                return (self.bus.cycles - start).max(1) as u32;
            }
        }
        if self.bus.ime
            && (self.bus.ie_reg & self.bus.if_reg) != 0
            && !self.flag(FLAG_I)
        {
            self.irq();
        }
        let op = self.pipe[0];
        self.pipe[0] = self.pipe[1];
        if self.thumb() {
            self.r[15] = self.r[15].wrapping_add(2);
            self.pipe[1] = self.bus.read16(self.r[15]) as u32;
            self.exec_thumb(op as u16);
        } else {
            self.r[15] = self.r[15].wrapping_add(4);
            self.pipe[1] = self.bus.read32(self.r[15]);
            self.exec_arm(op);
        }
        (self.bus.cycles - start).max(1) as u32
    }

    // ---- exceptions -----------------------------------------------------

    fn enter_exception(&mut self, vector: u32, mode: u32, lr: u32) {
        let old = self.cpsr;
        self.swap_banks(mode);
        self.cpsr = (old & !(0x1F | FLAG_T)) | mode | FLAG_I;
        self.banks.spsr[bank_of(mode)] = old;
        self.r[14] = lr;
        self.branch(vector);
    }

    /// SWI: HLE the common BIOS calls; unknown ones take the real exception.
    pub(crate) fn exception_swi(&mut self) {
        let op = if self.thumb() {
            self.bus.read16(self.exec_pc()) as u32 & 0xFF
        } else {
            (self.bus.read32(self.exec_pc()) >> 16) & 0xFF
        };
        if !bios::hle(self, op) {
            let lr = self.r[15].wrapping_sub(if self.thumb() { 2 } else { 4 });
            self.enter_exception(0x08, MODE_SVC, lr);
        }
    }

    pub(crate) fn exception_undefined(&mut self) {
        let lr = self.r[15].wrapping_sub(if self.thumb() { 2 } else { 4 });
        self.enter_exception(0x04, MODE_UND, lr);
    }

    /// LR = about-to-execute + 4, so `subs pc, lr, #4` resumes correctly.
    fn irq(&mut self) {
        let lr = self.exec_pc().wrapping_add(4);
        self.enter_exception(0x18, MODE_IRQ, lr);
    }

    // ---- decode ---------------------------------------------------------

    pub(crate) fn condition(&self, cond: u32) -> bool {
        let n = self.flag(FLAG_N);
        let z = self.flag(FLAG_Z);
        let c = self.flag(FLAG_C);
        let v = self.flag(FLAG_V);
        match cond {
            0x0 => z,
            0x1 => !z,
            0x2 => c,
            0x3 => !c,
            0x4 => n,
            0x5 => !n,
            0x6 => v,
            0x7 => !v,
            0x8 => c && !z,
            0x9 => !c || z,
            0xA => n == v,
            0xB => n != v,
            0xC => !z && n == v,
            0xD => z || n != v,
            0xE => true,
            _ => false, // NV: never on ARMv4
        }
    }

    fn exec_arm(&mut self, op: u32) {
        if !self.condition(op >> 28) {
            return;
        }
        match (op >> 25) & 7 {
            0b000 => {
                if op & 0x0FFF_FFF0 == 0x012F_FF10 {
                    arm_misc::arm_bx(self, op);
                } else if op & 0x0FC0_00F0 == 0x0000_0090 {
                    arm_misc::arm_multiply(self, op);
                } else if op & 0x0F80_00F0 == 0x0080_0090 {
                    arm_misc::arm_multiply_long(self, op);
                } else if op & 0x0FB0_0FF0 == 0x0100_0090 {
                    arm_mem::arm_swap(self, op);
                } else if op & 0x0000_0090 == 0x0000_0090 {
                    arm_mem::arm_halfword_transfer(self, op);
                } else if op & 0x0190_0000 == 0x0100_0000 {
                    // TST/TEQ/CMP/CMN without S: MRS/MSR.
                    arm_misc::arm_psr(self, op);
                } else {
                    alu::arm_data_processing(self, op);
                }
            }
            0b001 => {
                if op & 0x01B0_0000 == 0x0120_0000 {
                    arm_misc::arm_psr(self, op); // MSR immediate
                } else if op & 0x0190_0000 == 0x0100_0000 {
                    self.exception_undefined(); // MRS immediate doesn't exist
                } else {
                    alu::arm_data_processing(self, op);
                }
            }
            0b010 => arm_mem::arm_single_transfer(self, op),
            0b011 => {
                if op & 0x10 != 0 {
                    self.exception_undefined();
                } else {
                    arm_mem::arm_single_transfer(self, op);
                }
            }
            0b100 => arm_block::arm_block_transfer(self, op),
            0b101 => arm_misc::arm_branch(self, op),
            0b110 => self.exception_undefined(), // no coprocessors on GBA
            _ => {
                if op & 0x0100_0000 != 0 {
                    arm_misc::arm_swi(self, op);
                } else {
                    self.exception_undefined();
                }
            }
        }
    }

    fn exec_thumb(&mut self, op: u16) {
        match op >> 13 {
            0b000 => {
                if (op >> 11) & 3 == 3 {
                    thumb_alu::thumb_add_sub(self, op);
                } else {
                    thumb_alu::thumb_shift_imm(self, op);
                }
            }
            0b001 => thumb_alu::thumb_imm_ops(self, op),
            0b010 => match (op >> 10) & 7 {
                0b000 => thumb_alu::thumb_alu_ops(self, op),
                0b001 => thumb_alu::thumb_hi_reg_bx(self, op),
                0b010 | 0b011 => thumb_mem::thumb_ldr_pc(self, op),
                _ => {
                    if op & 0x0200 != 0 {
                        thumb_mem::thumb_ldst_sign(self, op);
                    } else {
                        thumb_mem::thumb_ldst_reg(self, op);
                    }
                }
            },
            0b011 => thumb_mem::thumb_ldst_imm(self, op),
            0b100 => {
                if op & 0x1000 != 0 {
                    thumb_mem::thumb_ldst_sp(self, op);
                } else {
                    thumb_mem::thumb_ldst_half(self, op);
                }
            }
            0b101 => {
                if op & 0x1000 == 0 {
                    thumb_alu::thumb_addr_calc(self, op);
                } else if (op >> 8) & 0xF == 0 {
                    thumb_alu::thumb_sp_adjust(self, op);
                } else if (op >> 9) & 3 == 0b10 {
                    thumb_mem::thumb_push_pop(self, op);
                } else {
                    self.exception_undefined();
                }
            }
            0b110 => {
                if op & 0x1000 == 0 {
                    thumb_mem::thumb_ldm_stm(self, op);
                } else if (op >> 8) & 0xF == 0xF {
                    thumb_branch::thumb_swi(self, op);
                } else {
                    thumb_branch::thumb_cond_branch(self, op);
                }
            }
            _ => {
                if op & 0x1000 == 0 {
                    thumb_branch::thumb_branch(self, op);
                } else {
                    thumb_branch::thumb_bl(self, op);
                }
            }
        }
    }
}

// ---- test helpers -------------------------------------------------------

#[cfg(test)]
pub(crate) fn test_arm(code: &[u32]) -> Arm7 {
    let mut rom = vec![0u8; 0xC0];
    for w in code {
        rom.extend_from_slice(&w.to_le_bytes());
    }
    rom.resize(rom.len().max(0x200), 0);
    let mut cpu = Arm7::new(Bus::new_test(rom));
    cpu.branch(0x0800_00C0);
    cpu
}

#[cfg(test)]
pub(crate) fn test_thumb(code: &[u16]) -> Arm7 {
    let mut rom = vec![0u8; 0xC0];
    for h in code {
        rom.extend_from_slice(&h.to_le_bytes());
    }
    rom.resize(rom.len().max(0x200), 0);
    let mut cpu = Arm7::new(Bus::new_test(rom));
    cpu.cpsr |= FLAG_T;
    cpu.branch(0x0800_00C0);
    cpu
}
