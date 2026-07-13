//! Sharp SM83 CPU. Every memory access ticks the bus 4 T-cycles, so
//! instruction timing falls out of the access pattern.

use crate::bus::Bus;

const FZ: u8 = 0x80;
const FN: u8 = 0x40;
const FH: u8 = 0x20;
const FC: u8 = 0x10;

pub struct Cpu {
    pub bus: Bus,
    a: u8,
    f: u8,
    b: u8,
    c: u8,
    d: u8,
    e: u8,
    h: u8,
    l: u8,
    sp: u16,
    pc: u16,
    ime: bool,
    ei_pending: bool,
    halted: bool,
    halt_bug: bool,
}

impl Cpu {
    pub fn new(bus: Bus) -> Self {
        let cgb = bus.cgb;
        let mut cpu = Cpu {
            bus,
            a: 0x01,
            f: 0xB0,
            b: 0x00,
            c: 0x13,
            d: 0x00,
            e: 0xD8,
            h: 0x01,
            l: 0x4D,
            sp: 0xFFFE,
            pc: 0x0100,
            ime: false,
            ei_pending: false,
            halted: false,
            halt_bug: false,
        };
        if cgb {
            cpu.a = 0x11;
            cpu.f = 0x80;
            cpu.b = 0x00;
            cpu.c = 0x00;
            cpu.d = 0xFF;
            cpu.e = 0x56;
            cpu.h = 0x00;
            cpu.l = 0x0D;
        }
        cpu
    }

    // -- 16-bit register pairs ------------------------------------------------

    fn af(&self) -> u16 {
        ((self.a as u16) << 8) | self.f as u16
    }
    fn bc(&self) -> u16 {
        ((self.b as u16) << 8) | self.c as u16
    }
    fn de(&self) -> u16 {
        ((self.d as u16) << 8) | self.e as u16
    }
    fn hl(&self) -> u16 {
        ((self.h as u16) << 8) | self.l as u16
    }
    fn set_af(&mut self, v: u16) {
        self.a = (v >> 8) as u8;
        self.f = v as u8 & 0xF0;
    }
    fn set_bc(&mut self, v: u16) {
        self.b = (v >> 8) as u8;
        self.c = v as u8;
    }
    fn set_de(&mut self, v: u16) {
        self.d = (v >> 8) as u8;
        self.e = v as u8;
    }
    fn set_hl(&mut self, v: u16) {
        self.h = (v >> 8) as u8;
        self.l = v as u8;
    }

    fn flag(&self, m: u8) -> bool {
        self.f & m != 0
    }
    fn set_flag(&mut self, m: u8, on: bool) {
        if on {
            self.f |= m;
        } else {
            self.f &= !m;
        }
    }

    // -- Cycle-counted memory access ------------------------------------------

    fn rd(&mut self, addr: u16) -> u8 {
        self.bus.tick(4);
        self.bus.read(addr)
    }

    fn wr(&mut self, addr: u16, v: u8) {
        self.bus.tick(4);
        self.bus.write(addr, v);
    }

    fn internal(&mut self) {
        self.bus.tick(4);
    }

    fn fetch(&mut self) -> u8 {
        let v = self.rd(self.pc);
        if self.halt_bug {
            self.halt_bug = false;
        } else {
            self.pc = self.pc.wrapping_add(1);
        }
        v
    }

    fn fetch16(&mut self) -> u16 {
        let lo = self.fetch() as u16;
        let hi = self.fetch() as u16;
        (hi << 8) | lo
    }

    fn push16(&mut self, v: u16) {
        self.sp = self.sp.wrapping_sub(1);
        self.wr(self.sp, (v >> 8) as u8);
        self.sp = self.sp.wrapping_sub(1);
        self.wr(self.sp, v as u8);
    }

    fn pop16(&mut self) -> u16 {
        let lo = self.rd(self.sp) as u16;
        self.sp = self.sp.wrapping_add(1);
        let hi = self.rd(self.sp) as u16;
        self.sp = self.sp.wrapping_add(1);
        (hi << 8) | lo
    }

    // -- Execution loop --------------------------------------------------------

    pub fn step(&mut self) {
        let pending = self.bus.ie & self.bus.iflags & 0x1F;
        if self.halted && pending != 0 {
            self.halted = false;
        }
        if self.ime && pending != 0 {
            self.dispatch_interrupt();
            return;
        }
        if self.halted {
            self.internal();
            return;
        }
        let ei_was_pending = self.ei_pending;
        let op = self.fetch();
        self.exec(op);
        if ei_was_pending && self.ei_pending {
            self.ime = true;
            self.ei_pending = false;
        }
    }

    fn dispatch_interrupt(&mut self) {
        self.ime = false;
        self.halted = false;
        self.internal();
        self.internal();
        self.sp = self.sp.wrapping_sub(1);
        self.wr(self.sp, (self.pc >> 8) as u8);
        // Re-evaluate after the high push: writing the stack may hit IE.
        let pending = self.bus.ie & self.bus.iflags & 0x1F;
        self.sp = self.sp.wrapping_sub(1);
        self.wr(self.sp, self.pc as u8);
        if pending == 0 {
            self.pc = 0x0000;
        } else {
            let bit = pending.trailing_zeros() as u16;
            self.bus.iflags &= !(1 << bit);
            self.pc = 0x0040 + 8 * bit;
        }
        self.internal();
    }

    fn reg8(&mut self, idx: u8) -> u8 {
        match idx {
            0 => self.b,
            1 => self.c,
            2 => self.d,
            3 => self.e,
            4 => self.h,
            5 => self.l,
            6 => {
                let hl = self.hl();
                self.rd(hl)
            }
            _ => self.a,
        }
    }

    fn set_reg8(&mut self, idx: u8, v: u8) {
        match idx {
            0 => self.b = v,
            1 => self.c = v,
            2 => self.d = v,
            3 => self.e = v,
            4 => self.h = v,
            5 => self.l = v,
            6 => {
                let hl = self.hl();
                self.wr(hl, v);
            }
            _ => self.a = v,
        }
    }

    fn cond(&self, idx: u8) -> bool {
        match idx {
            0 => !self.flag(FZ),
            1 => self.flag(FZ),
            2 => !self.flag(FC),
            _ => self.flag(FC),
        }
    }

    // -- ALU --------------------------------------------------------------------

    fn alu_add(&mut self, v: u8, carry: bool) {
        let c = (carry && self.flag(FC)) as u8;
        let r = self.a.wrapping_add(v).wrapping_add(c);
        self.set_flag(FZ, r == 0);
        self.set_flag(FN, false);
        self.set_flag(FH, (self.a & 0x0F) + (v & 0x0F) + c > 0x0F);
        self.set_flag(FC, (self.a as u16) + (v as u16) + (c as u16) > 0xFF);
        self.a = r;
    }

    fn alu_sub(&mut self, v: u8, carry: bool, keep: bool) {
        let c = (carry && self.flag(FC)) as u8;
        let r = self.a.wrapping_sub(v).wrapping_sub(c);
        self.set_flag(FZ, r == 0);
        self.set_flag(FN, true);
        self.set_flag(FH, (self.a & 0x0F) < (v & 0x0F) + c);
        self.set_flag(FC, (self.a as u16) < (v as u16) + (c as u16));
        if !keep {
            self.a = r;
        }
    }

    fn alu(&mut self, op: u8, v: u8) {
        match op {
            0 => self.alu_add(v, false),
            1 => self.alu_add(v, true),
            2 => self.alu_sub(v, false, false),
            3 => self.alu_sub(v, true, false),
            4 => {
                self.a &= v;
                self.f = if self.a == 0 { FZ | FH } else { FH };
            }
            5 => {
                self.a ^= v;
                self.f = if self.a == 0 { FZ } else { 0 };
            }
            6 => {
                self.a |= v;
                self.f = if self.a == 0 { FZ } else { 0 };
            }
            _ => self.alu_sub(v, false, true),
        }
    }

    fn inc8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_add(1);
        self.set_flag(FZ, r == 0);
        self.set_flag(FN, false);
        self.set_flag(FH, v & 0x0F == 0x0F);
        r
    }

    fn dec8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_sub(1);
        self.set_flag(FZ, r == 0);
        self.set_flag(FN, true);
        self.set_flag(FH, v & 0x0F == 0x00);
        r
    }

    fn add_hl(&mut self, v: u16) {
        let hl = self.hl();
        let r = hl.wrapping_add(v);
        self.set_flag(FN, false);
        self.set_flag(FH, (hl & 0x0FFF) + (v & 0x0FFF) > 0x0FFF);
        self.set_flag(FC, (hl as u32) + (v as u32) > 0xFFFF);
        self.set_hl(r);
        self.internal();
    }

    fn sp_plus_imm(&mut self) -> u16 {
        let d = self.fetch() as i8 as i16 as u16;
        let r = self.sp.wrapping_add(d);
        self.set_flag(FZ, false);
        self.set_flag(FN, false);
        self.set_flag(FH, (self.sp & 0x0F) + (d & 0x0F) > 0x0F);
        self.set_flag(FC, (self.sp & 0xFF) + (d & 0xFF) > 0xFF);
        r
    }

    fn daa(&mut self) {
        let mut adjust = 0u8;
        let mut carry = self.flag(FC);
        if !self.flag(FN) {
            if self.flag(FH) || self.a & 0x0F > 0x09 {
                adjust |= 0x06;
            }
            if carry || self.a > 0x99 {
                adjust |= 0x60;
                carry = true;
            }
            self.a = self.a.wrapping_add(adjust);
        } else {
            if self.flag(FH) {
                adjust |= 0x06;
            }
            if carry {
                adjust |= 0x60;
            }
            self.a = self.a.wrapping_sub(adjust);
        }
        self.set_flag(FZ, self.a == 0);
        self.set_flag(FH, false);
        self.set_flag(FC, carry);
    }

    // -- Rotates / shifts (CB and the A-register short forms) --------------------

    fn rlc(&mut self, v: u8) -> u8 {
        let r = v.rotate_left(1);
        self.f = if v & 0x80 != 0 { FC } else { 0 } | if r == 0 { FZ } else { 0 };
        r
    }
    fn rrc(&mut self, v: u8) -> u8 {
        let r = v.rotate_right(1);
        self.f = if v & 0x01 != 0 { FC } else { 0 } | if r == 0 { FZ } else { 0 };
        r
    }
    fn rl(&mut self, v: u8) -> u8 {
        let r = (v << 1) | self.flag(FC) as u8;
        self.f = if v & 0x80 != 0 { FC } else { 0 } | if r == 0 { FZ } else { 0 };
        r
    }
    fn rr(&mut self, v: u8) -> u8 {
        let r = (v >> 1) | ((self.flag(FC) as u8) << 7);
        self.f = if v & 0x01 != 0 { FC } else { 0 } | if r == 0 { FZ } else { 0 };
        r
    }
    fn sla(&mut self, v: u8) -> u8 {
        let r = v << 1;
        self.f = if v & 0x80 != 0 { FC } else { 0 } | if r == 0 { FZ } else { 0 };
        r
    }
    fn sra(&mut self, v: u8) -> u8 {
        let r = (v >> 1) | (v & 0x80);
        self.f = if v & 0x01 != 0 { FC } else { 0 } | if r == 0 { FZ } else { 0 };
        r
    }
    fn swap(&mut self, v: u8) -> u8 {
        let r = v.rotate_left(4);
        self.f = if r == 0 { FZ } else { 0 };
        r
    }
    fn srl(&mut self, v: u8) -> u8 {
        let r = v >> 1;
        self.f = if v & 0x01 != 0 { FC } else { 0 } | if r == 0 { FZ } else { 0 };
        r
    }

    fn exec_cb(&mut self) {
        let op = self.fetch();
        let idx = op & 0x07;
        let bit = (op >> 3) & 0x07;
        match op >> 6 {
            0 => {
                let v = self.reg8(idx);
                let r = match bit {
                    0 => self.rlc(v),
                    1 => self.rrc(v),
                    2 => self.rl(v),
                    3 => self.rr(v),
                    4 => self.sla(v),
                    5 => self.sra(v),
                    6 => self.swap(v),
                    _ => self.srl(v),
                };
                self.set_reg8(idx, r);
            }
            1 => {
                let v = self.reg8(idx);
                self.set_flag(FZ, v & (1 << bit) == 0);
                self.set_flag(FN, false);
                self.set_flag(FH, true);
            }
            2 => {
                let v = self.reg8(idx) & !(1 << bit);
                self.set_reg8(idx, v);
            }
            _ => {
                let v = self.reg8(idx) | (1 << bit);
                self.set_reg8(idx, v);
            }
        }
    }

    fn exec(&mut self, op: u8) {
        match op {
            0x00 => {}
            0x01 => {
                let v = self.fetch16();
                self.set_bc(v);
            }
            0x02 => {
                let a = self.bc();
                let v = self.a;
                self.wr(a, v);
            }
            0x03 => {
                let v = self.bc().wrapping_add(1);
                self.set_bc(v);
                self.internal();
            }
            0x04 => self.b = { let v = self.b; self.inc8(v) },
            0x05 => self.b = { let v = self.b; self.dec8(v) },
            0x06 => self.b = self.fetch(),
            0x07 => {
                let v = self.a;
                self.a = self.rlc(v);
                self.set_flag(FZ, false);
            }
            0x08 => {
                let a = self.fetch16();
                let sp = self.sp;
                self.wr(a, sp as u8);
                self.wr(a.wrapping_add(1), (sp >> 8) as u8);
            }
            0x09 => {
                let v = self.bc();
                self.add_hl(v);
            }
            0x0A => {
                let a = self.bc();
                self.a = self.rd(a);
            }
            0x0B => {
                let v = self.bc().wrapping_sub(1);
                self.set_bc(v);
                self.internal();
            }
            0x0C => self.c = { let v = self.c; self.inc8(v) },
            0x0D => self.c = { let v = self.c; self.dec8(v) },
            0x0E => self.c = self.fetch(),
            0x0F => {
                let v = self.a;
                self.a = self.rrc(v);
                self.set_flag(FZ, false);
            }
            0x10 => {
                self.fetch();
                self.bus.perform_speed_switch();
            }
            0x11 => {
                let v = self.fetch16();
                self.set_de(v);
            }
            0x12 => {
                let a = self.de();
                let v = self.a;
                self.wr(a, v);
            }
            0x13 => {
                let v = self.de().wrapping_add(1);
                self.set_de(v);
                self.internal();
            }
            0x14 => self.d = { let v = self.d; self.inc8(v) },
            0x15 => self.d = { let v = self.d; self.dec8(v) },
            0x16 => self.d = self.fetch(),
            0x17 => {
                let v = self.a;
                self.a = self.rl(v);
                self.set_flag(FZ, false);
            }
            0x18 => {
                let d = self.fetch() as i8;
                self.internal();
                self.pc = self.pc.wrapping_add(d as u16);
            }
            0x19 => {
                let v = self.de();
                self.add_hl(v);
            }
            0x1A => {
                let a = self.de();
                self.a = self.rd(a);
            }
            0x1B => {
                let v = self.de().wrapping_sub(1);
                self.set_de(v);
                self.internal();
            }
            0x1C => self.e = { let v = self.e; self.inc8(v) },
            0x1D => self.e = { let v = self.e; self.dec8(v) },
            0x1E => self.e = self.fetch(),
            0x1F => {
                let v = self.a;
                self.a = self.rr(v);
                self.set_flag(FZ, false);
            }
            0x20 | 0x28 | 0x30 | 0x38 => {
                let d = self.fetch() as i8;
                if self.cond((op >> 3) & 0x03) {
                    self.internal();
                    self.pc = self.pc.wrapping_add(d as u16);
                }
            }
            0x21 => {
                let v = self.fetch16();
                self.set_hl(v);
            }
            0x22 => {
                let a = self.hl();
                let v = self.a;
                self.wr(a, v);
                self.set_hl(a.wrapping_add(1));
            }
            0x23 => {
                let v = self.hl().wrapping_add(1);
                self.set_hl(v);
                self.internal();
            }
            0x24 => self.h = { let v = self.h; self.inc8(v) },
            0x25 => self.h = { let v = self.h; self.dec8(v) },
            0x26 => self.h = self.fetch(),
            0x27 => self.daa(),
            0x29 => {
                let v = self.hl();
                self.add_hl(v);
            }
            0x2A => {
                let a = self.hl();
                self.a = self.rd(a);
                self.set_hl(a.wrapping_add(1));
            }
            0x2B => {
                let v = self.hl().wrapping_sub(1);
                self.set_hl(v);
                self.internal();
            }
            0x2C => self.l = { let v = self.l; self.inc8(v) },
            0x2D => self.l = { let v = self.l; self.dec8(v) },
            0x2E => self.l = self.fetch(),
            0x2F => {
                self.a = !self.a;
                self.set_flag(FN, true);
                self.set_flag(FH, true);
            }
            0x31 => self.sp = self.fetch16(),
            0x32 => {
                let a = self.hl();
                let v = self.a;
                self.wr(a, v);
                self.set_hl(a.wrapping_sub(1));
            }
            0x33 => {
                self.sp = self.sp.wrapping_add(1);
                self.internal();
            }
            0x34 => {
                let a = self.hl();
                let v = self.rd(a);
                let r = self.inc8(v);
                self.wr(a, r);
            }
            0x35 => {
                let a = self.hl();
                let v = self.rd(a);
                let r = self.dec8(v);
                self.wr(a, r);
            }
            0x36 => {
                let v = self.fetch();
                let a = self.hl();
                self.wr(a, v);
            }
            0x37 => {
                self.set_flag(FN, false);
                self.set_flag(FH, false);
                self.set_flag(FC, true);
            }
            0x39 => {
                let v = self.sp;
                self.add_hl(v);
            }
            0x3A => {
                let a = self.hl();
                self.a = self.rd(a);
                self.set_hl(a.wrapping_sub(1));
            }
            0x3B => {
                self.sp = self.sp.wrapping_sub(1);
                self.internal();
            }
            0x3C => self.a = { let v = self.a; self.inc8(v) },
            0x3D => self.a = { let v = self.a; self.dec8(v) },
            0x3E => self.a = self.fetch(),
            0x3F => {
                let c = self.flag(FC);
                self.set_flag(FN, false);
                self.set_flag(FH, false);
                self.set_flag(FC, !c);
            }
            0x76 => {
                let pending = self.bus.ie & self.bus.iflags & 0x1F;
                if !self.ime && pending != 0 {
                    self.halt_bug = true;
                } else {
                    self.halted = true;
                }
            }
            0x40..=0x7F => {
                let v = self.reg8(op & 0x07);
                self.set_reg8((op >> 3) & 0x07, v);
            }
            0x80..=0xBF => {
                let v = self.reg8(op & 0x07);
                self.alu((op >> 3) & 0x07, v);
            }
            0xC0 | 0xC8 | 0xD0 | 0xD8 => {
                self.internal();
                if self.cond((op >> 3) & 0x03) {
                    self.pc = self.pop16();
                    self.internal();
                }
            }
            0xC1 => {
                let v = self.pop16();
                self.set_bc(v);
            }
            0xC2 | 0xCA | 0xD2 | 0xDA => {
                let a = self.fetch16();
                if self.cond((op >> 3) & 0x03) {
                    self.internal();
                    self.pc = a;
                }
            }
            0xC3 => {
                let a = self.fetch16();
                self.internal();
                self.pc = a;
            }
            0xC4 | 0xCC | 0xD4 | 0xDC => {
                let a = self.fetch16();
                if self.cond((op >> 3) & 0x03) {
                    self.internal();
                    let pc = self.pc;
                    self.push16(pc);
                    self.pc = a;
                }
            }
            0xC5 => {
                self.internal();
                let v = self.bc();
                self.push16(v);
            }
            0xC6 | 0xCE | 0xD6 | 0xDE | 0xE6 | 0xEE | 0xF6 | 0xFE => {
                let v = self.fetch();
                self.alu((op >> 3) & 0x07, v);
            }
            0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => {
                self.internal();
                let pc = self.pc;
                self.push16(pc);
                self.pc = (op & 0x38) as u16;
            }
            0xC9 => {
                self.pc = self.pop16();
                self.internal();
            }
            0xCB => self.exec_cb(),
            0xCD => {
                let a = self.fetch16();
                self.internal();
                let pc = self.pc;
                self.push16(pc);
                self.pc = a;
            }
            0xD1 => {
                let v = self.pop16();
                self.set_de(v);
            }
            0xD5 => {
                self.internal();
                let v = self.de();
                self.push16(v);
            }
            0xD9 => {
                self.pc = self.pop16();
                self.internal();
                self.ime = true;
            }
            0xE0 => {
                let a = 0xFF00 | self.fetch() as u16;
                let v = self.a;
                self.wr(a, v);
            }
            0xE1 => {
                let v = self.pop16();
                self.set_hl(v);
            }
            0xE2 => {
                let a = 0xFF00 | self.c as u16;
                let v = self.a;
                self.wr(a, v);
            }
            0xE5 => {
                self.internal();
                let v = self.hl();
                self.push16(v);
            }
            0xE8 => {
                let r = self.sp_plus_imm();
                self.internal();
                self.internal();
                self.sp = r;
            }
            0xE9 => self.pc = self.hl(),
            0xEA => {
                let a = self.fetch16();
                let v = self.a;
                self.wr(a, v);
            }
            0xF0 => {
                let a = 0xFF00 | self.fetch() as u16;
                self.a = self.rd(a);
            }
            0xF1 => {
                let v = self.pop16();
                self.set_af(v);
            }
            0xF2 => {
                let a = 0xFF00 | self.c as u16;
                self.a = self.rd(a);
            }
            0xF3 => {
                self.ime = false;
                self.ei_pending = false;
            }
            0xF5 => {
                self.internal();
                let v = self.af();
                self.push16(v);
            }
            0xF8 => {
                let r = self.sp_plus_imm();
                self.internal();
                self.set_hl(r);
            }
            0xF9 => {
                self.sp = self.hl();
                self.internal();
            }
            0xFA => {
                let a = self.fetch16();
                self.a = self.rd(a);
            }
            0xFB => self.ei_pending = true,
            // Unused opcodes lock the CPU on hardware; treat as NOP.
            0xD3 | 0xDB | 0xDD | 0xE3 | 0xE4 | 0xEB | 0xEC | 0xED | 0xF4 | 0xFC | 0xFD => {}
        }
    }
}
