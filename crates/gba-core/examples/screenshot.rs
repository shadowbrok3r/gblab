//! Run a ROM headless for N frames and write the screen as a PPM.
//! Usage: screenshot <rom> <out.ppm> [frames]

use gba_core::{GameBoyAdvance, SCREEN_H, SCREEN_W};

fn main() {
    let mut args = std::env::args().skip(1);
    let rom_path = args.next().expect("usage: screenshot <rom> <out.ppm> [frames]");
    let out_path = args.next().expect("usage: screenshot <rom> <out.ppm> [frames]");
    let frames: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(120);

    let rom = std::fs::read(&rom_path).expect("read rom");
    let mut gba = GameBoyAdvance::new(rom).expect("load rom");
    for _ in 0..frames {
        gba.run_frame();
    }
    let fb = gba.framebuffer();
    let mut ppm = format!("P6\n{SCREEN_W} {SCREEN_H}\n255\n").into_bytes();
    for px in fb.chunks(4) {
        ppm.extend_from_slice(&px[..3]);
    }
    std::fs::write(&out_path, ppm).expect("write ppm");
    println!("wrote {out_path}; pc={:08X} r12={}", gba.debug_pc(), gba.debug_reg(12));
}
