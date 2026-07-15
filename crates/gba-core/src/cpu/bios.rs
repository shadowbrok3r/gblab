//! HLE of the BIOS SWI calls the stub BIOS doesn't implement natively.

use super::Arm7;

/// User IRQ handlers acknowledge waits by setting bits here (BIOS_IF).
pub(crate) const BIOS_IF: u32 = 0x0300_7FF8;

/// Returns false when the call is unknown and the real vector should run.
pub(crate) fn hle(cpu: &mut Arm7, num: u32) -> bool {
    match num {
        0x01 => {} // RegisterRamReset: memory is already zeroed
        0x02 => cpu.bus.halted = true,
        0x04 => {
            let (discard, mask) = (cpu.r[0], cpu.r[1] as u16);
            intr_wait(cpu, discard != 0, mask);
        }
        0x05 => intr_wait(cpu, true, 1),
        0x06 => div(cpu, 0, 1),
        0x07 => div(cpu, 1, 0),
        0x08 => cpu.r[0] = (cpu.r[0] as f64).sqrt() as u32,
        0x0B => cpu_set(cpu),
        0x0C => cpu_fast_set(cpu),
        0x11 | 0x12 => lz77(cpu),
        _ => return false,
    }
    true
}

/// BIOS Div/DivArm: quotient r0, remainder r1, |quotient| r3.
fn div(cpu: &mut Arm7, num: usize, den: usize) {
    let (n, d) = (cpu.r[num] as i32, cpu.r[den] as i32);
    if d == 0 {
        return;
    }
    let q = n.wrapping_div(d);
    cpu.r[1] = n.wrapping_rem(d) as u32;
    cpu.r[0] = q as u32;
    cpu.r[3] = q.unsigned_abs();
}

/// IntrWait: sleep until a user IRQ handler sets `mask` bits in BIOS_IF.
fn intr_wait(cpu: &mut Arm7, discard: bool, mask: u16) {
    if discard {
        let cur = cpu.bus.read16(BIOS_IF);
        cpu.bus.write16(BIOS_IF, cur & !mask);
    }
    cpu.bus.ime = true;
    cpu.swi_wait = Some(mask);
}

/// CpuSet: r0 src, r1 dst, r2 = count | fill<<24 | word<<26.
fn cpu_set(cpu: &mut Arm7) {
    let (mut src, mut dst, ctl) = (cpu.r[0], cpu.r[1], cpu.r[2]);
    let count = ctl & 0x1F_FFFF;
    let fill = ctl & 1 << 24 != 0;
    if ctl & 1 << 26 != 0 {
        let (mut s, mut d) = (src & !3, dst & !3);
        let v0 = if fill { cpu.bus.read32(s) } else { 0 };
        for _ in 0..count {
            let v = if fill { v0 } else { let v = cpu.bus.read32(s); s += 4; v };
            cpu.bus.write32(d, v);
            d += 4;
        }
    } else {
        src &= !1;
        dst &= !1;
        let v0 = if fill { cpu.bus.read16(src) } else { 0 };
        for _ in 0..count {
            let v = if fill { v0 } else { let v = cpu.bus.read16(src); src += 2; v };
            cpu.bus.write16(dst, v);
            dst += 2;
        }
    }
}

/// CpuFastSet: word copy/fill, count rounded up to multiples of 8 words.
fn cpu_fast_set(cpu: &mut Arm7) {
    let (mut src, mut dst, ctl) = (cpu.r[0] & !3, cpu.r[1] & !3, cpu.r[2]);
    let count = (ctl & 0x1F_FFFF).next_multiple_of(8);
    let fill = ctl & 1 << 24 != 0;
    let v0 = if fill { cpu.bus.read32(src) } else { 0 };
    for _ in 0..count {
        let v = if fill { v0 } else { let v = cpu.bus.read32(src); src += 4; v };
        cpu.bus.write32(dst, v);
        dst += 4;
    }
}

/// LZ77UnComp (WRAM and VRAM): decompress then store as halfwords.
fn lz77(cpu: &mut Arm7) {
    let (mut src, dst) = (cpu.r[0], cpu.r[1]);
    let size = (cpu.bus.read32(src & !3) >> 8) as usize;
    src += 4;
    let mut out = Vec::with_capacity(size + 1);
    while out.len() < size {
        let flags = cpu.bus.read8(src);
        src += 1;
        for bit in (0..8).rev() {
            if out.len() >= size {
                break;
            }
            if flags & (1 << bit) != 0 {
                let b0 = cpu.bus.read8(src) as usize;
                let b1 = cpu.bus.read8(src + 1) as usize;
                src += 2;
                let len = (b0 >> 4) + 3;
                let disp = ((b0 & 0xF) << 8 | b1) + 1;
                for _ in 0..len {
                    let v = out[out.len() - disp];
                    out.push(v);
                }
            } else {
                out.push(cpu.bus.read8(src));
                src += 1;
            }
        }
    }
    out.truncate(size);
    if out.len() % 2 != 0 {
        out.push(0);
    }
    for (i, pair) in out.chunks(2).enumerate() {
        cpu.bus.write16(dst + i as u32 * 2, u16::from_le_bytes([pair[0], pair[1]]));
    }
}
