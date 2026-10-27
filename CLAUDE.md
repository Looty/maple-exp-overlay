# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Maple EXP Overlay is a tiny native Windows EXP/hr overlay for MapleStory Classic World: Rust, plain Win32 + GDI for the overlay, Slint (software renderer) only for the settings window. Goals are minimal memory and near-zero idle CPU, so avoid per-frame work and keep Slint out of the overlay process.

## Commands

```
cargo build --release          # target/release/maple-exp-overlay.exe (size-optimised, LTO, panic=abort, stripped)
cargo test                     # reader (sample bar screenshots in tests/frames), tracker, config, editor, level table
cargo fmt --check              # CI enforces rustfmt (max_width 140, see rustfmt.toml)
cargo clippy --all-targets -- -D warnings   # CI enforces zero clippy warnings
```

- Release builds use `windows_subsystem = "windows"` and `panic = "abort"`: a panic shows up only as exit code `0xc0000409`. Use a debug build (`cargo run`) to see messages.
- A running overlay locks its exe; quit it from the tray before rebuilding. If the linker can't open files, use a short `CARGO_TARGET_DIR`.
- `.cargo/config.toml` is local-only (gitignored) and remaps build paths so no local paths end up in the binary; CI sets the same through `RUSTFLAGS`. Never commit it.
- Releases: bump `version` in `Cargo.toml`, tag `vX.Y.Z`, push the tag (see `.github/workflows/release.yml`). CI signs the bare exe with minisign for the in-app updater, so the `MINISIGN_SECRET_KEY` repository secret must exist or the release fails; its public half is `update::PUBLIC_KEY`.
- `cargo test -- --ignored` also runs a live download from GitHub through the updater's WinHTTP code.
- Modes of the one exe: no argument = overlay, `--settings`, `--select-area`, `--dump` (writes `dump.txt` and pictures to `%APPDATA%\MapleExpOverlay`), `--screen` (ignore the game window).

## Architecture

- `src/lib.rs` (platform-neutral, unit tested): `reader` (finds the `EXP. 24,855 [20.25%]` text in a screenshot by colour and matches digits against `digits.bin` templates, no OCR), `tracker` (readings to EXP/h, sessions, idle, level-up), `exp_table` (the game's levelling table; gives the level and the exact EXP left), `config` (`Settings` as JSON in `%APPDATA%\MapleExpOverlay\settings.json`, `#[serde(default)]` everywhere, `clamp()` after load), `editor` (geometry and moves of the layout editor), `text`.
- `src/main.rs` + `overlay.rs`: the overlay window (one timer, GDI drawing, tray icon). It never searches for the bar: it reads only the box in `Settings::region`. While MapleStory isn't running it idles (slower looks, nothing kept in memory).
- `src/settings_ui.rs` + `ui/settings.slint`: the settings window, a separate short-lived process (`--settings`) that edits the settings file; the overlay re-reads it when it changes. `selector.rs` is the area picker (`--select-area`), also its own process.
- Updates: `update.rs` in the lib (version compare, release JSON, minisign check of the exe; the trusted comment must be `maple-exp-overlay vX.Y.Z`) and `updater.rs` (WinHTTP download, rename-swap of the running exe to `.old`, restart of the overlay). Only the settings window's About tab uses it.
- `src/capture.rs`: screen capture (GDI BitBlt) and `wgc.rs` (Windows Graphics Capture of the game window, opt-in). `system.rs`: autostart and single instance.

### Gotchas

- Windows hides the taskbar over a borderless game only while the game is in front, so a screen-capture test taken from the settings window can fail where the overlay works in game (that's what "Test in game" is for).
- The white-digit threshold is tried at several levels (`WHITE_TRIES` in `reader.rs`): a light panel behind the digits can merge with them at a low threshold.
- `Item::tone` 0-2 are theme colours, 3+ index `config::PALETTE`.
- The digit templates and the detection approach come from beanguri's Maple-Classic-Exp-Overlay (MIT); keep `NOTICE` shipped with any release.
