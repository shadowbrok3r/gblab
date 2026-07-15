//! Headless runs of jsmolka's gba-tests CPU suites.
//! ROMs live in <workspace>/test-roms/jsmolka (gitignored; see README).
//!
//! The suites park in an infinite loop when done and leave the failed test
//! number (0 = all passed) in a per-suite register.

use gba_core::GameBoyAdvance;

fn rom_path(rel: &str) -> Option<std::path::PathBuf> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-roms/jsmolka")
        .join(rel);
    p.exists().then_some(p)
}

fn run_parked_test(rel: &str, test_reg: usize) {
    let Some(path) = rom_path(rel) else {
        eprintln!("skipping {rel}: ROM not present");
        return;
    };
    let rom = std::fs::read(&path).unwrap();
    let mut gba = GameBoyAdvance::new(rom).unwrap();
    let mut last_pc = u32::MAX;
    let mut parked = 0;
    for _ in 0..600 {
        gba.run_frame();
        let pc = gba.debug_pc();
        parked = if pc == last_pc { parked + 1 } else { 0 };
        last_pc = pc;
        if parked >= 3 {
            let n = gba.debug_reg(test_reg);
            assert!(n == 0, "{rel}: test {n} failed (parked at {pc:08X})");
            return;
        }
    }
    panic!("{rel} never parked; pc={last_pc:08X} r{test_reg}={}", gba.debug_reg(test_reg));
}

#[test]
fn arm() {
    run_parked_test("arm/arm.gba", 12);
}

#[test]
fn thumb() {
    run_parked_test("thumb/thumb.gba", 7);
}

#[test]
fn memory() {
    run_parked_test("memory/memory.gba", 12);
}

#[test]
fn nes() {
    run_parked_test("nes/nes.gba", 12);
}

/// FNV-1a over the framebuffer after `frames` frames.
fn screen_hash(rel: &str, frames: u32) -> Option<u64> {
    let path = rom_path(rel)?;
    let rom = std::fs::read(&path).unwrap();
    let mut gba = GameBoyAdvance::new(rom).unwrap();
    for _ in 0..frames {
        gba.run_frame();
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in gba.framebuffer() {
        h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3);
    }
    Some(h)
}

/// PPU demo ROMs render forever; pin the visually-verified output.
#[test]
fn ppu_demos() {
    for (rom, expect) in [
        ("ppu/hello.gba", 0x62B7_6C0E_0223_A81C),
        ("ppu/shades.gba", 0x19E7_C5AF_1FB0_BF25),
        ("ppu/stripes.gba", 0x2F1E_64B4_8356_B525),
    ] {
        match screen_hash(rom, 60) {
            Some(h) => assert!(h == expect, "{rom}: framebuffer hash {h:#018X} != {expect:#018X}"),
            None => eprintln!("skipping {rom}: ROM not present"),
        }
    }
}
