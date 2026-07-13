//! Headless runs of Blargg's test ROMs, checking serial output.
//! ROMs live in <workspace>/test-roms/blargg (gitignored; see README).

use gb_core::GameBoy;

fn rom_path(rel: &str) -> Option<std::path::PathBuf> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-roms/blargg").join(rel);
    p.exists().then_some(p)
}

/// Run until serial output contains "Passed" or "Failed" (or frame budget out).
fn run_serial_test(rel: &str) {
    let Some(path) = rom_path(rel) else {
        eprintln!("skipping {rel}: ROM not present");
        return;
    };
    let rom = std::fs::read(&path).unwrap();
    let mut gb = GameBoy::new(rom).unwrap();
    let mut output = String::new();
    for _ in 0..4000 {
        gb.run_frame();
        output.push_str(&String::from_utf8_lossy(&gb.take_serial()));
        if output.contains("Passed") {
            return;
        }
        assert!(!output.contains("Failed"), "{rel} failed:\n{output}");
    }
    panic!("{rel} did not finish; output so far:\n{output}");
}

#[test]
fn cpu_instrs() {
    run_serial_test("cpu_instrs/cpu_instrs.gb");
}

#[test]
fn instr_timing() {
    run_serial_test("instr_timing/instr_timing.gb");
}

#[test]
fn mem_timing() {
    run_serial_test("mem_timing/mem_timing.gb");
}
