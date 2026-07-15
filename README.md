# GBLab

Game Boy / Game Boy Color emulator in Rust (edition 2024) for desktop and
Android, built as the screen half of an ESP32-driven handheld: the phone runs
the emulator, a custom ESP32-H2 controller feeds it buttons over BLE (custom
GATT), and the controller wiring gets designed in WireLab.

## Layout

| Crate                  | What it is                                                        |
| ---------------------- | ----------------------------------------------------------------- |
| `crates/gb-core`       | Pure emulator core (SM83 CPU, PPU, APU, MBC1/2/3/5, DMG + CGB)    |
| `crates/gba-core`      | GBA core (ARM7TDMI, scanline PPU modes 0-5, DMA, timers, APU)     |
| `crates/gblab`         | egui app, cdylib for Android (NativeActivity via `android_main`)  |
| `crates/gblab-desktop` | Desktop launcher binary                                           |

## Desktop

```sh
cargo run -p gblab-desktop --release [rom.gb]
```

Keys: arrows = D-pad, Z = B, X = A, Enter = Start, Backspace = Select.
"Pad" checkbox shows the on-screen touch gamepad (same widget Android uses).

## Android (Samsung S26)

One-time: `rustup target add aarch64-linux-android`, `cargo install cargo-apk2`.
Signing uses the keystore at `~/.android/gblab-release.keystore` (configured in
`crates/gblab/Cargo.toml`).

```sh
export ANDROID_HOME=$HOME/Android/Sdk
export ANDROID_NDK_HOME=$HOME/Android/Sdk/ndk/28.0.12674087
export JAVA_HOME=$HOME/jdk17
export PATH="$JAVA_HOME/bin:$ANDROID_HOME/platform-tools:$PATH"

cd crates/gblab
cargo apk2 build --target aarch64-linux-android --release
adb install -r ../../target/release/apk/gblab.apk
adb shell monkey -p com.kingsofalchemy.gblab 1
```

(`cargo apk2 run --target aarch64-linux-android` builds a debug APK,
installs, and launches in one step.)

ROMs on the phone go in either folder shown by the in-app "ROMs" browser:

```sh
adb push game.gb /storage/emulated/0/Android/data/com.kingsofalchemy.gblab/files/
```

Battery saves are written as `<rom>.sav` next to the ROM.

## Tests

`./scripts/fetch-test-roms.sh` downloads the suites into `test-roms/`
(gitignored), then:

```sh
cargo test -p gb-core            # Blargg: cpu_instrs, instr_timing, mem_timing, halt_bug
cargo test -p gba-core           # unit tests + jsmolka arm/thumb/memory ROM suites
```

Current status: all four Blargg suites pass; dmg-acid2 renders pixel-perfect
against the reference; cgb-acid2 matches structurally (1:1 color map).
gba-core passes jsmolka's arm, thumb, memory, and nes suites (they park in an
idle loop with the failed test number in r12/r7; 0 means pass) and the ppu
demos are pinned by framebuffer hash. The app plays .gb/.gbc/.gba by
extension; GBA L/R map to Q/W on desktop and shoulder pills on the touch pad.
Render any ROM headless with:

```sh
cargo run -p gb-core --example screenshot -- rom.gb out.ppm [frames]
cargo run -p gba-core --example screenshot -- rom.gba out.ppm [frames]
```

## Controller (ESP32-H2 BLE pad)

The controller is an ESP32-H2-DEV-KIT-N4 (Waveshare) running
`firmware/gblab-pad-fw`: a trouble-host GATT peripheral at fixed static
random address `FF:62:4C:42:47:FF`, service
`8f7a2d43-1e5b-4c9a-9d0e-5c33a1b0f001`, with a 1-byte buttons characteristic
(`...f002`, read+notify). Bit order matches the joypad: Right, Left, Up,
Down, A, B, Select, Start. The BOOT button doubles as A for bench tests.

```sh
cd firmware/gblab-pad-fw
cargo run --release        # espflash flash --monitor
```

Button wiring (active-low, one side to GPIO, other to GND; internal pull-ups):

| Button | GPIO | | Button | GPIO |
| ------ | ---- |-| ------ | ---- |
| Up     | 0    | | A      | 10   |
| Down   | 1    | | B      | 11   |
| Left   | 5    | | Select | 12   |
| Right  | 4    | | Start  | 22   |

GPIO13/14 are the 32K-crystal pins and GPIO8/9/25 are strapping pins on this
board; avoid them for buttons. The wiring diagram lives in WireLab
("GBLab Controller" in the Examples menu, board profile
`esp32-h2-devkit-n4`).

On the phone, tap "Connect pad" in the top bar. The app connects directly to
the fixed address (no scanning, no location permission), needs the Bluetooth
permission once, and auto-reconnects every few seconds while enabled. The
Android side is `java/.../BleController.java` (compiled into the APK by
cargo-apk2's `java_sources`) polled from Rust via JNI in `src/ble.rs`.
