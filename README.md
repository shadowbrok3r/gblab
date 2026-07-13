# GBLab

Game Boy / Game Boy Color emulator in Rust (edition 2024) for desktop and
Android, built as the screen half of an ESP32-driven handheld: the phone runs
the emulator, a custom ESP32-H2 controller feeds it buttons over BLE (custom
GATT), and the controller wiring gets designed in WireLab.

## Layout

| Crate                  | What it is                                                        |
| ---------------------- | ----------------------------------------------------------------- |
| `crates/gb-core`       | Pure emulator core (SM83 CPU, PPU, APU, MBC1/2/3/5, DMG + CGB)    |
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
```

Current status: all four Blargg suites pass; dmg-acid2 renders pixel-perfect
against the reference; cgb-acid2 matches structurally (1:1 color map).
Render any ROM headless with:

```sh
cargo run -p gb-core --example screenshot -- rom.gb out.ppm [frames]
```

## Controller roadmap

- `gblab::input::ControllerLink` is the seam: BLE GATT client (Android, via
  JNI) implements it next to the existing keyboard/touch sources.
- Firmware: ESP32-H2-DEV-KIT-N4 (BLE 5, no classic BT/Wi-Fi) advertising a
  custom GATT service with a button-state characteristic (notify).
- Wiring design for the button matrix happens in WireLab.
