# Maple EXP Overlay

A tiny EXP/hr overlay for MapleStory Classic World. Rust, no OCR engine, no runtime files. It only looks at your screen: it never touches the game's files or memory and never sends input.

> **Unofficial.** This is a fan-made tool. It is not made, endorsed or supported by Nexon and is not affiliated with Nexon. MapleStory is a trademark of Nexon. Using any third-party tool with an online game is at your own risk: read Nexon's Terms of Service, and if you are unsure whether a screen-reading tool is allowed, ask Nexon support. Nexon can act on an account at its own discretion.

## Install

1. Download the latest `maple-exp-overlay-<version>-windows-x64.zip` from the repository's Releases page.
2. Unzip it anywhere and run `maple-exp-overlay.exe`. Nothing is installed; to uninstall, quit it from the tray, delete the folder and `%APPDATA%\MapleExpOverlay`.

The executable is not code-signed, so Windows SmartScreen may show "Windows protected your PC". Click "More info", then "Run anyway". To check the download, compare its SHA-256 with the `.sha256` file on the release page:

```
Get-FileHash .\maple-exp-overlay-<version>-windows-x64.zip -Algorithm SHA256
```

## Use it

1. Start the game (windowed or borderless fullscreen) with the EXP bar visible, then run `maple-exp-overlay.exe`. The settings window opens on the **Capture** tab the first time.
2. Click **Select area...** and drag a box around the EXP text, for example `EXP. 24,855 [20.25%]`. Include the `EXP.` label and a little margin, and nothing else. Esc cancels.
3. Click **Test in game**, then switch to the game within 5 seconds and leave it in front. It says "Works" when the bar can be read, or tells you what to change. Test with the game in front: with the settings window in front, Windows can show the taskbar over the bar, which it normally hides while you play. If the taskbar or another window still covers the bar in game, set **Capture method** to **Game window** and test again.
4. Right-click the overlay (or use the tray icon) for **Settings...**, **Reset session**, **Hide overlay** and **Quit**. Drag the overlay to move it.

The overlay reads nothing until an area is selected, and it never searches for the bar by itself. If you move or resize the game, or change its UI scale, select the area again.

Launching the exe again while it runs opens the settings.

## Settings

| Tab | What's there |
|---|---|
| Layout (top) | Drag the pieces of data around to arrange the overlay. Click one to show or hide it, right-click to change its colour. Presets: Compact, One line, Vertical list. |
| Overlay | Colour theme and colours, font, size, bold, opacity, position, click-through, always on top, taskbar button, hide when idle. |
| Data | Short numbers, the idle pause time, the "recent rate" length. |
| Capture | Step-by-step setup, the EXP bar area (Select area, Test), the capture method (**Screen**, the default, reads what is on screen like any screen recorder; **Game window** reads the game's own picture, so the taskbar or a window on top can't hide the bar), and how often it looks. |
| General | Start with Windows, tray icon, start hidden, open the settings folder, reset. |

Settings live in `%APPDATA%\MapleExpOverlay\settings.json` (the settings window and the overlay are separate processes; the overlay reloads the file when it changes).

## How the numbers are worked out

The bar shows `EXP. 24,855 [20.25%]`. Every second the digits are matched against saved shapes (`src/digits.bin`, from the original project). A changed reading only counts when two looks in a row agree. EXP gained is the change between readings (a level-up counts the rest of the old bar plus the start of the new one). EXP/h = gained / time farmed x 3600, where time farmed leaves out stretches without EXP longer than the pause time. Time to level = percent left / percent per hour. When the game shows only the percentage, it works from that alone.

## Not working?

1. Use **Settings > Capture > Test**: it says what it sees in your area and what to change.
2. "Text cut off" or "no brackets seen": the box is too small or something covers the bar. Select the area again, or switch **Capture method** to **Game window**.
3. A black capture: the game is in fullscreen-exclusive mode, which no tool can capture. Use borderless or windowed fullscreen.
4. Still stuck: run `maple-exp-overlay.exe --dump` and look at `dump.txt`, `dump-screen.bmp` and `dump-window.bmp` in `%APPDATA%\MapleExpOverlay`.

## Build

```
cargo build --release
cargo test --release
```

Needs Rust 1.92 or newer and the Windows SDK (for the icon resource). If the linker can't open files, set `CARGO_TARGET_DIR` to a short path. CI (`.github/workflows/ci.yml`) enforces `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, a release build and the tests.

### Releasing

Bump `version` in `Cargo.toml`, commit, then tag and push:

```
git tag v0.2.1
git push origin v0.2.1
```

The release workflow checks the tag matches `Cargo.toml`, runs the tests, builds the exe with build paths remapped (so no local paths end up in the binary), and publishes a GitHub release with the zip and its SHA-256.

The overlay is plain Win32 + GDI. The settings window is Slint (software renderer) and runs as its own short-lived process (`--settings`), so it uses no memory while you play. Window capture uses Windows Graphics Capture, the same API OBS and Discord use.

## Licences

See `NOTICE`: the digit templates come from the original project (MIT), and the settings window uses Slint.

## Credits

The digit templates and the idea of reading the bar by shape come from [Maple-Classic-Exp-Overlay](https://github.com/beanguri/Maple-Classic-Exp-Overlay).
